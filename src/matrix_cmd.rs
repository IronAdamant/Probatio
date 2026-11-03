//! `matrix report` reads the recorded score tables and writes matrix.json. It deletes nothing.

use std::fs;
use std::path::Path;

use crate::envelope::Envelope;
use crate::json::{self, Value};

pub fn report(scores: &str, direct: &str, out: &Path, patch_dirs: &[String]) -> Result<Envelope, String> {
    let mut rows = parse_union(scores)?;
    rows.extend(parse_direct(direct)?);
    if rows.is_empty() {
        return Ok(Envelope::new(
            false,
            "matrix.report",
            "The score files have no suite rows.",
            "Pass --scores and --direct that contain the recorded table.",
        ));
    }
    let reference = rows.iter().find(|row| row.source == "union" && row.holdout.is_some()).cloned();
    let directs: Vec<Row> = rows.iter().filter(|row| row.source == "direct").cloned().collect();
    let gap_row = weakest(&directs);
    let missed = gap_row.as_ref().map(|row| row.missed.clone()).unwrap_or_default();
    let gaps = resolve_gaps(&missed, patch_dirs);
    let batch2: Vec<Value> = rows
        .iter()
        .filter(|row| row.source == "union" && lower(row.synthetic2, row.synthetic1))
        .map(|row| {
            Value::object(vec![
                ("set", Value::String(row.set.clone())),
                ("synthetic1", count_value(row.synthetic1)),
                ("synthetic2", count_value(row.synthetic2)),
            ])
        })
        .collect();
    let below = if let Some(reference) = &reference {
        rows.iter()
            .filter(|row| row.set != reference.set && lower(row.holdout, reference.holdout))
            .map(|row| {
                Value::object(vec![
                    ("set", Value::String(row.set.clone())),
                    ("holdout", count_value(row.holdout)),
                    ("reference", count_value(reference.holdout)),
                ])
            })
            .collect()
    } else {
        Vec::new()
    };
    let summary = summarize(gap_row.as_ref(), &batch2, gaps.len());
    let next = next_sentence(&gaps);
    let full = out.join("matrix.json");
    let shown = gaps.iter().take(10).cloned().collect::<Vec<_>>();
    let rest = gaps.len().saturating_sub(shown.len());
    let mut envelope = Envelope::new(true, "matrix.report", summary, next)
        .with_extra("rows", Value::Array(rows.iter().map(row_value).collect()))
        .with_extra(
            "collapse",
            Value::object(vec![("batch2", Value::Array(batch2)), ("holdoutBelowReference", Value::Array(below))]),
        )
        .with_extra("gaps", Value::Array(shown))
        .with_extra("rest", Value::Int(rest as i64))
        .with_extra(
            "pruning",
            Value::object(vec![
                ("mode", Value::String("advisory".into())),
                ("deletedTests", Value::Int(0)),
                ("advice", Value::Array(Vec::new())),
            ]),
        )
        .with_extra("full", Value::String(full.display().to_string()));
    fs::create_dir_all(out).map_err(|error| error.to_string())?;
    fs::write(&full, envelope.render("")).map_err(|error| error.to_string())?;
    let _ = &mut envelope;
    Ok(envelope)
}

fn summarize(gap: Option<&Row>, batch2: &[Value], gap_count: usize) -> String {
    let mut parts = Vec::new();
    if let Some(row) = gap {
        if let Some((caught, total)) = row.holdout {
            let train = row.real.map(|(caught, total)| format!(" Train real is {caught}/{total}.")).unwrap_or_default();
            parts.push(format!("{} holdout is {caught}/{total}.{train}", row.set));
        }
    }
    if !batch2.is_empty() {
        parts.push(format!("Cover collapse on batch 2: {} rows.", batch2.len()));
    }
    parts.push(format!("{gap_count} gaps. Pruning is advisory and deleted 0 tests."));
    parts.join(" ")
}

fn next_sentence(gaps: &[Value]) -> String {
    let Some(first) = gaps.first() else {
        return "No gaps.".into();
    };
    let id = first.get("id").and_then(Value::as_str).unwrap_or("");
    let file = first.get("file").and_then(Value::as_str);
    let line = first.get("line").and_then(Value::as_int);
    match (file, line) {
        (Some(file), Some(line)) if !file.is_empty() => format!("{id} at {file}:{line}"),
        _ => format!("{id} has no patch, so there is no file:line."),
    }
}

#[derive(Clone)]
struct Row {
    set: String,
    name: String,
    source: &'static str,
    lines: Option<i64>,
    holdout: Option<(i64, i64)>,
    real: Option<(i64, i64)>,
    synthetic1: Option<(i64, i64)>,
    synthetic2: Option<(i64, i64)>,
    missed: Vec<String>,
}

fn empty(source: &'static str) -> Row {
    Row {
        set: String::new(),
        name: String::new(),
        source,
        lines: None,
        holdout: None,
        real: None,
        synthetic1: None,
        synthetic2: None,
        missed: Vec::new(),
    }
}

fn parse_union(text: &str) -> Result<Vec<Row>, String> {
    let lines: Vec<&str> = text.lines().filter(|line| !line.trim().is_empty()).collect();
    if lines.is_empty() {
        return Ok(Vec::new());
    }
    let headers: Vec<(String, Option<i64>)> = lines[0].split('\t').map(parse_header).collect();
    let mut rows = Vec::new();
    for line in lines.iter().skip(1) {
        let cells: Vec<&str> = line.split('\t').collect();
        let mut row = empty("union");
        for (index, header) in headers.iter().enumerate() {
            let cell = cells.get(index).copied().unwrap_or("").trim();
            if header.0 == "set" || header.0.is_empty() && index == 0 {
                row.set = cell.to_string();
                row.name = cell.to_string();
            } else if header.0 == "lines" {
                row.lines = Some(cell.parse::<i64>().map_err(|_| format!("lines is not an integer: {cell}"))?);
            } else if let Some(total) = header.1 {
                assign(&mut row, &header.0, parse_caught(cell, total)?)?;
            }
        }
        rows.push(row);
    }
    Ok(rows)
}

fn parse_direct(text: &str) -> Result<Vec<Row>, String> {
    let mut rows = Vec::new();
    let mut current: Option<Row> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(caps) = metric(line) {
            let row = current.as_mut().ok_or_else(|| format!("metric before a suite: {line}"))?;
            assign(row, &caps.0, (caps.1, caps.2))?;
            if !caps.3.is_empty() {
                row.missed.extend(caps.3.split_whitespace().map(str::to_string));
            }
            continue;
        }
        let mut row = empty("direct");
        row.name = line.to_string();
        row.set = line.split('(').next().unwrap_or(line).split_whitespace().collect::<String>();
        if let Some(done) = current.replace(row) {
            rows.push(done);
        }
    }
    if let Some(done) = current {
        rows.push(done);
    }
    Ok(rows)
}

fn metric(line: &str) -> Option<(String, i64, i64, String)> {
    let rest = line.strip_prefix("holdout:")
        .map(|rest| ("holdout", rest))
        .or_else(|| line.strip_prefix("real (train):").map(|rest| ("real", rest)))
        .or_else(|| line.strip_prefix("synthetic-1:").map(|rest| ("synthetic1", rest)))
        .or_else(|| line.strip_prefix("synthetic-2:").map(|rest| ("synthetic2", rest)))?;
    let body = rest.1.trim();
    let (nums, missed) = body.split_once("missed").map(|(left, right)| (left.trim(), right.trim().to_string())).unwrap_or((body, String::new()));
    let nums = nums.trim().trim_end_matches(|ch: char| ch == ')' || ch.is_ascii_whitespace());
    let (caught, total) = nums.split_once('/')?;
    let caught = caught.split_whitespace().next()?.parse().ok()?;
    let total = total.split_whitespace().next()?.trim_end_matches(')').parse().ok()?;
    Some((rest.0.to_string(), caught, total, missed))
}

fn parse_header(cell: &str) -> (String, Option<i64>) {
    let cell = cell.trim();
    if let Some(open) = cell.rfind('(') {
        if cell.ends_with(')') {
            let name = cell[..open].trim();
            if let Ok(total) = cell[open + 1..cell.len() - 1].parse::<i64>() {
                return (metric_kind(name), Some(total));
            }
        }
    }
    (if cell.is_empty() { "set".into() } else { cell.to_string() }, None)
}

fn metric_kind(name: &str) -> String {
    match name {
        "synthetic-1" => "synthetic1".into(),
        "synthetic-2" => "synthetic2".into(),
        "real (train)" | "real" => "real".into(),
        other => other.to_string(),
    }
}

fn parse_caught(cell: &str, total: i64) -> Result<(i64, i64), String> {
    let digits: String = cell.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    let caught = digits.parse::<i64>().map_err(|_| format!("cannot read a count from \"{cell}\""))?;
    Ok((caught, total))
}

fn assign(row: &mut Row, kind: &str, count: (i64, i64)) -> Result<(), String> {
    match kind {
        "holdout" => row.holdout = Some(count),
        "real" => row.real = Some(count),
        "synthetic1" => row.synthetic1 = Some(count),
        "synthetic2" => row.synthetic2 = Some(count),
        "set" | "lines" => {}
        other => return Err(format!("unknown score column {other}")),
    }
    Ok(())
}

fn weakest(rows: &[Row]) -> Option<Row> {
    let mut best: Option<&Row> = None;
    let mut best_rate = f64::MAX;
    for row in rows {
        let Some(rate) = rate(row.holdout) else { continue };
        if rate < best_rate {
            best = Some(row);
            best_rate = rate;
        }
    }
    best.cloned()
}

fn lower(left: Option<(i64, i64)>, right: Option<(i64, i64)>) -> bool {
    match (rate(left), rate(right)) {
        (Some(left), Some(right)) => left < right,
        _ => false,
    }
}

fn rate(count: Option<(i64, i64)>) -> Option<f64> {
    count.and_then(|(caught, total)| if total == 0 { None } else { Some(caught as f64 / total as f64) })
}

fn resolve_gaps(ids: &[String], patch_dirs: &[String]) -> Vec<Value> {
    let mut unique = ids.to_vec();
    unique.sort();
    unique.dedup();
    unique.into_iter().map(|id| locate(&id, patch_dirs)).collect()
}

fn locate(id: &str, patch_dirs: &[String]) -> Value {
    let bare = id.trim_start_matches("ho-").trim_start_matches("h-").trim_start_matches("r-").trim_start_matches("v-");
    let names = [format!("{id}.patch"), format!("r-{bare}.patch"), format!("h-{bare}.patch"), format!("{bare}.patch"), format!("ho-{bare}.patch")];
    for dir in patch_dirs {
        for name in &names {
            let path = Path::new(dir).join(name);
            let Ok(raw) = fs::read_to_string(&path) else { continue };
            let (file, line) = first_hunk(&raw);
            return Value::object(vec![
                ("id", Value::String(id.into())),
                ("file", file.map(Value::String).unwrap_or(Value::Null)),
                ("line", line.map(Value::Int).unwrap_or(Value::Null)),
                ("locations", Value::Array(Vec::new())),
            ]);
        }
    }
    Value::object(vec![
        ("id", Value::String(id.into())),
        ("file", Value::Null),
        ("line", Value::Null),
        ("locations", Value::Array(Vec::new())),
    ])
}

fn first_hunk(raw: &str) -> (Option<String>, Option<i64>) {
    let mut file = None;
    let mut line = None;
    for row in raw.lines() {
        if let Some(rest) = row.strip_prefix("+++ ") {
            let mut name = rest.split('\t').next().unwrap_or(rest).trim();
            if name == "/dev/null" {
                continue;
            }
            if let Some(stripped) = name.strip_prefix("a/").or_else(|| name.strip_prefix("b/")) {
                name = stripped;
            }
            file = Some(name.to_string());
        }
        if line.is_none() {
            if let Some(rest) = row.strip_prefix("@@ -") {
                if let Some(plus) = rest.split_once(" +") {
                    let digits: String = plus.1.chars().take_while(|ch| ch.is_ascii_digit()).collect();
                    line = digits.parse().ok();
                }
            }
        }
    }
    (file, line)
}

fn count_value(count: Option<(i64, i64)>) -> Value {
    match count {
        Some((caught, total)) => Value::object(vec![("caught", Value::Int(caught)), ("total", Value::Int(total))]),
        None => Value::Null,
    }
}

fn row_value(row: &Row) -> Value {
    Value::object(vec![
        ("set", Value::String(row.set.clone())),
        ("name", Value::String(row.name.clone())),
        ("source", Value::String(row.source.into())),
        ("lines", row.lines.map(Value::Int).unwrap_or(Value::Null)),
        ("holdout", count_value(row.holdout)),
        ("real", count_value(row.real)),
        ("synthetic1", count_value(row.synthetic1)),
        ("synthetic2", count_value(row.synthetic2)),
        ("missed", Value::Array(row.missed.iter().cloned().map(Value::String).collect())),
    ])
}

pub fn read_kills(path: &Path) -> Result<Vec<(String, i64)>, String> {
    let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let value = json::parse(&text).map_err(|error| error.to_string())?;
    let Value::Object(entries) = value else {
        return Err("--kills must be a JSON object of test name to kill count".into());
    };
    let mut kills = Vec::new();
    for (name, value) in entries {
        let Some(count) = value.as_int() else {
            return Err(format!("kill count for {name} must be a non-negative integer"));
        };
        if count < 0 {
            return Err(format!("kill count for {name} must be a non-negative integer"));
        }
        kills.push((name, count));
    }
    Ok(kills)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_direct_table_names_the_weakest_holdout_and_writes_matrix_json() {
        let dir = std::env::temp_dir().join(format!("probatio-matrix-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("patches")).unwrap();
        fs::write(dir.join("patches/m1.patch"), "--- a/src/a.py\n+++ b/src/a.py\n@@ -3,1 +3,1 @@\n-a and b\n+a or b\n").unwrap();
        let scores = "set\tholdout (4)\nfull\t3\n";
        let direct = "small suite\nholdout: 1/4 missed m1\n";
        let envelope = report(scores, direct, &dir.join("out"), &[dir.join("patches").display().to_string()]).unwrap();
        assert!(envelope.ok, "{}", envelope.summary);
        assert!(envelope.summary.contains("1 gaps"), "{}", envelope.summary);
        assert!(dir.join("out/matrix.json").is_file());
        let _ = fs::remove_dir_all(&dir);
    }
}
