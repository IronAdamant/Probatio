//! Gaps, guards, findings, and the status page. A seal hides an id from that page.

use std::fs;
use std::path::{Path, PathBuf};

use crate::envelope::Envelope;
use crate::hash;
use crate::json::{self, Value};

pub const YARDSTICK: &str = "these tests fit the yardstick, not the code.";

#[derive(Clone, Debug)]
pub struct Gap {
    pub id: String,
    pub status: String,
    pub file: String,
    pub line: i64,
    pub guard: Option<String>,
    pub fixed_in: Option<String>,
    pub guard_file: Option<String>,
    pub guard_line_no: Option<i64>,
    pub guard_line: Option<String>,
    pub guard_hash: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Decision {
    pub id: String,
    pub file: String,
    pub line: i64,
    pub hash: String,
    pub status: String,
    pub reason: String,
}

#[derive(Clone, Debug)]
pub struct Finding {
    pub id: String,
    pub status: String,
    pub reason: String,
    pub file: Option<String>,
    pub line: Option<i64>,
    pub hash: Option<String>,
    pub agent: Option<String>,
}

#[derive(Clone, Debug)]
pub struct MemoryState {
    pub gaps: Vec<Gap>,
    pub decisions: Vec<Decision>,
}

pub fn line_hash(line: &str) -> String {
    hash::hex_prefix(line.as_bytes(), 16)
}

pub fn read_source_line(file: &Path, line: i64) -> Option<String> {
    let text = fs::read_to_string(file).ok()?;
    let lines: Vec<&str> = text.split('\n').collect();
    if line < 1 || line as usize > lines.len() {
        return None;
    }
    let raw = lines[(line as usize) - 1];
    Some(raw.trim_end_matches('\r').to_string())
}

pub fn guard_changes(message: &str) -> Vec<(String, String)> {
    let mut named = Vec::new();
    for line in message.lines() {
        let Some(rest) = line.strip_prefix("Guard-Change:") else { continue };
        let rest = rest.trim_start();
        let Some((guard, why)) = split_reason(rest) else { continue };
        named.push((guard, why));
    }
    named
}

pub fn golden_changes(message: &str) -> Vec<(String, String)> {
    let mut named = Vec::new();
    for line in message.lines() {
        let Some(rest) = line.strip_prefix("Golden-Change:") else { continue };
        let rest = rest.trim_start();
        let Some((row, why)) = split_reason(rest) else { continue };
        named.push((row, why));
    }
    named
}

fn split_reason(rest: &str) -> Option<(String, String)> {
    let bytes = rest.as_bytes();
    let mut index = 0;
    while index + 1 < bytes.len() {
        if bytes[index] == b':' && bytes[index + 1].is_ascii_whitespace() {
            let left = rest[..index].trim().to_string();
            let why = rest[index + 1..].trim().to_string();
            if left.is_empty() || why.is_empty() {
                return None;
            }
            return Some((left, why));
        }
        index += 1;
    }
    None
}

pub fn empty_state() -> MemoryState {
    MemoryState { gaps: Vec::new(), decisions: Vec::new() }
}

pub fn load_state(dir: &Path) -> MemoryState {
    let file = dir.join("state.json");
    let Ok(text) = fs::read_to_string(file) else {
        return empty_state();
    };
    let Ok(value) = json::parse(&text) else {
        return empty_state();
    };
    let gaps = match value.get("gaps") {
        Some(Value::Array(items)) => items.iter().filter_map(gap_from).collect(),
        _ => Vec::new(),
    };
    let decisions = match value.get("decisions") {
        Some(Value::Array(items)) => items.iter().filter_map(decision_from).collect(),
        _ => Vec::new(),
    };
    MemoryState { gaps, decisions }
}

pub fn save_state(dir: &Path, state: &MemoryState) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    let mut gaps = state.gaps.clone();
    gaps.sort_by(|a, b| a.id.cmp(&b.id));
    let mut decisions = state.decisions.clone();
    decisions.sort_by(|a, b| a.id.cmp(&b.id));
    let value = Value::object(vec![
        ("schemaVersion", Value::Int(2)),
        ("gaps", Value::Array(gaps.iter().map(gap_value).collect())),
        ("decisions", Value::Array(decisions.iter().map(decision_value).collect())),
    ]);
    fs::write(dir.join("state.json"), json::stringify(&value)).map_err(|error| error.to_string())
}

pub fn load_sealed(dir: &Path) -> Vec<String> {
    let Ok(text) = fs::read_to_string(dir.join("sealed.json")) else {
        return Vec::new();
    };
    let Ok(value) = json::parse(&text) else {
        return Vec::new();
    };
    let mut ids = match value.get("ids") {
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    ids.sort();
    ids.dedup();
    ids
}

pub fn seal_ids(dir: &Path, ids: &[String]) -> Result<usize, String> {
    let mut merged = load_sealed(dir);
    merged.extend(ids.iter().cloned());
    merged.sort();
    merged.dedup();
    fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    let value = Value::object(vec![("ids", Value::Array(merged.iter().cloned().map(Value::String).collect()))]);
    fs::write(dir.join("sealed.json"), json::stringify(&value)).map_err(|error| error.to_string())?;
    Ok(merged.len())
}

pub fn choose_sealed(ids: &[String], fraction: f64, seed: u32) -> Vec<String> {
    if ids.is_empty() || fraction <= 0.0 {
        return Vec::new();
    }
    let count = ((ids.len() as f64) * fraction).round() as usize;
    let count = count.clamp(1, ids.len());
    let mut order = ids.to_vec();
    let mut rng = Rng { state: seed };
    let mut index = order.len();
    while index > 1 {
        index -= 1;
        let swap = (rng.next() * (index as f64 + 1.0)).floor() as usize;
        order.swap(index, swap.min(index));
    }
    let mut chosen = order.into_iter().take(count).collect::<Vec<_>>();
    chosen.sort();
    chosen
}

struct Rng {
    state: u32,
}

impl Rng {
    fn next(&mut self) -> f64 {
        let product = (self.state as i32).wrapping_mul(1_103_515_245) as i64 + 12_345;
        self.state = product as u32;
        self.state as f64 / 4_294_967_296.0
    }
}

pub fn parse_guard(guard: &str) -> Result<(String, i64), String> {
    let Some((file, line)) = guard.rsplit_once(':') else {
        return Err("guard must be file:line".into());
    };
    let line = line.parse::<i64>().map_err(|_| "guard must be file:line".to_string())?;
    if file.is_empty() || line < 1 {
        return Err("guard must be file:line".into());
    }
    Ok((file.to_string(), line))
}

pub fn fix_gap(state: &MemoryState, id: &str, file: &str, line: i64, guard: &str, commit: &str, root: &Path) -> Result<MemoryState, String> {
    let (guard_file, guard_line_no) = parse_guard(guard)?;
    let absolute = root.join(&guard_file);
    let text = read_source_line(&absolute, guard_line_no).ok_or_else(|| "guard line does not exist".to_string())?;
    let hash = line_hash(&text);
    let mut gaps: Vec<Gap> = state.gaps.iter().filter(|gap| gap.id != id).cloned().collect();
    gaps.push(Gap {
        id: id.to_string(),
        status: "fixed".into(),
        file: file.to_string(),
        line,
        guard: Some(guard.to_string()),
        fixed_in: Some(commit.to_string()),
        guard_file: Some(guard_file.clone()),
        guard_line_no: Some(guard_line_no),
        guard_line: Some(text.clone()),
        guard_hash: Some(hash.clone()),
    });
    let mut decisions: Vec<Decision> = state.decisions.iter().filter(|item| item.id != id).cloned().collect();
    decisions.push(Decision {
        id: id.to_string(),
        file: guard_file,
        line: guard_line_no,
        hash,
        status: "fixed".into(),
        reason: format!("guarded by {guard}"),
    });
    Ok(MemoryState { gaps, decisions })
}

pub fn revert_gap(state: &MemoryState, id: &str) -> Result<(MemoryState, String), String> {
    let gap = state.gaps.iter().find(|item| item.id == id).ok_or_else(|| "that id is not a fixed gap".to_string())?;
    if gap.fixed_in.is_none() || gap.guard.is_none() {
        return Err("that id is not a fixed gap".into());
    }
    let text = format!(
        "fixed in {}, guarded by {}; the guard broke",
        gap.fixed_in.as_deref().unwrap_or(""),
        gap.guard.as_deref().unwrap_or("")
    );
    let gaps = state
        .gaps
        .iter()
        .map(|item| {
            let mut copy = item.clone();
            if copy.id == id {
                copy.status = "regression".into();
            }
            copy
        })
        .collect();
    Ok((MemoryState { gaps, decisions: state.decisions.clone() }, text))
}

pub fn guard_allows(state: &MemoryState, guard: &str, message: &str, root: &Path) -> (bool, String) {
    let owners: Vec<&Gap> = state
        .gaps
        .iter()
        .filter(|gap| gap.guard.as_deref() == Some(guard) && (gap.status == "fixed" || gap.status == "regression"))
        .collect();
    if owners.is_empty() {
        return (true, format!("{guard} guards nothing."));
    }
    let owner = owners[0];
    if guard_changes(message).iter().any(|(name, _)| name == guard) {
        return (true, format!("{guard} changed with a Guard-Change trailer."));
    }
    let file = owner.guard_file.as_ref().map(|rel| root.join(rel)).unwrap_or_else(|| PathBuf::from(""));
    let line = owner.guard_line_no.and_then(|number| read_source_line(&file, number));
    let weakened = line.is_none() || owner.guard_line.as_ref().is_some_and(|saved| line.as_ref() != Some(saved));
    if !weakened {
        return (true, format!("{guard} is intact."));
    }
    (false, format!("Refusing to change {guard}. It is the guard of {}.", owner.id))
}

pub fn append_finding(dir: &Path, finding: &Finding) -> Result<(), String> {
    if !matches!(finding.status.as_str(), "open" | "fixed" | "equivalent" | "wont-fix") {
        return Err("status must be open, fixed, equivalent, or wont-fix".into());
    }
    if finding.reason.trim().is_empty() {
        return Err("a finding needs a reason".into());
    }
    fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    let mut line = json::stringify_compact(&finding_value(finding));
    line.push('\n');
    use std::io::Write;
    let mut file = fs::OpenOptions::new().create(true).append(true).open(dir.join("findings.jsonl")).map_err(|error| error.to_string())?;
    file.write_all(line.as_bytes()).map_err(|error| error.to_string())
}

pub fn load_findings(dir: &Path) -> Vec<Finding> {
    let Ok(text) = fs::read_to_string(dir.join("findings.jsonl")) else {
        return Vec::new();
    };
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| json::parse(line).ok())
        .filter_map(|value| finding_from(&value))
        .collect()
}

pub fn status_command(dir: &Path, root: &Path, score: Option<(i64, i64, i64, i64)>, agent: Option<&str>, page_path: &Path) -> Result<Envelope, String> {
    let state = load_state(dir);
    let sealed = load_sealed(dir);
    let hidden = |id: &str| sealed.iter().any(|item| item == id);
    let open: Vec<&Gap> = state.gaps.iter().filter(|gap| gap.status == "open" && !hidden(&gap.id)).collect();
    let shown = open.iter().take(10).copied().collect::<Vec<_>>();
    let rest = open.len().saturating_sub(shown.len());
    let fixes: Vec<&Gap> = state.gaps.iter().filter(|gap| gap.status == "fixed" && !hidden(&gap.id)).collect();
    let mut guards: Vec<String> = state.gaps.iter().filter(|gap| gap.guard.is_some() && !hidden(&gap.id)).filter_map(|gap| gap.guard.clone()).collect();
    guards.sort();
    guards.dedup();
    let mut decisions = Vec::new();
    for item in state.decisions.iter().filter(|item| !hidden(&item.id)) {
        let current = read_source_line(&root.join(&item.file), item.line);
        let review = if current.as_deref().is_some_and(|line| line_hash(line) == item.hash) { "current" } else { "stale" };
        decisions.push((item, review));
    }
    let regressions: Vec<&Gap> = state
        .gaps
        .iter()
        .filter(|gap| gap.status == "regression" && !hidden(&gap.id) && gap.fixed_in.is_some() && gap.guard.is_some())
        .collect();
    let findings = load_findings(dir);
    let findings: Vec<&Finding> = findings.iter().filter(|item| !hidden(&item.id)).collect();
    let findings_tail: Vec<&Finding> = findings.iter().rev().take(10).rev().copied().collect();
    let yard = score.and_then(|(visible_before, visible_after, sealed_before, sealed_after)| {
        if visible_after > visible_before && sealed_after <= sealed_before {
            Some(YARDSTICK.to_string())
        } else {
            None
        }
    });
    let open_count = shown.len() + rest;
    let summary = yard.unwrap_or_else(|| format!("{open_count} open gaps, {} fixes, {} decisions.", fixes.len(), decisions.len()));
    let next = if let Some(item) = regressions.first() {
        format!(
            "fixed in {}, guarded by {}; the guard broke",
            item.fixed_in.as_deref().unwrap_or(""),
            item.guard.as_deref().unwrap_or("")
        )
    } else if let Some(gap) = shown.first() {
        format!("{} at {}:{}", gap.id, gap.file, gap.line)
    } else {
        "Nothing waiting.".into()
    };
    let page = render_page(&summary, &shown, rest, &fixes, &guards, &decisions, &regressions);
    if let Some(parent) = page_path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(page_path, page).map_err(|error| error.to_string())?;
    let mut envelope = Envelope::new(true, "status", summary, next)
        .with_extra("agent", Value::String(agent.unwrap_or("unknown").into()))
        .with_extra("gaps", Value::Array(shown.iter().map(|gap| gap_value(gap)).collect()))
        .with_extra("rest", Value::Int(rest as i64))
        .with_extra(
            "recentFixes",
            Value::Array(
                fixes
                    .iter()
                    .map(|gap| {
                        Value::object(vec![
                            ("id", Value::String(gap.id.clone())),
                            ("fixedIn", opt_string(&gap.fixed_in)),
                            ("guard", opt_string(&gap.guard)),
                        ])
                    })
                    .collect(),
            ),
        )
        .with_extra("protectedGuards", Value::Array(guards.into_iter().map(Value::String).collect()))
        .with_extra(
            "decisions",
            Value::Array(
                decisions
                    .iter()
                    .map(|(item, review)| {
                        let mut value = decision_value(item);
                        if let Value::Object(entries) = &mut value {
                            entries.push(("review".into(), Value::String((*review).into())));
                        }
                        value
                    })
                    .collect(),
            ),
        )
        .with_extra(
            "regressions",
            Value::Array(
                regressions
                    .iter()
                    .map(|gap| {
                        Value::object(vec![
                            ("id", Value::String(gap.id.clone())),
                            (
                                "text",
                                Value::String(format!(
                                    "fixed in {}, guarded by {}; the guard broke",
                                    gap.fixed_in.as_deref().unwrap_or(""),
                                    gap.guard.as_deref().unwrap_or("")
                                )),
                            ),
                        ])
                    })
                    .collect(),
            ),
        )
        .with_extra("findings", Value::Array(findings_tail.iter().map(|item| finding_value(item)).collect()))
        .with_extra("sealedCount", Value::Int(sealed.len() as i64));
    let _ = &mut envelope;
    Ok(envelope)
}

fn render_page(summary: &str, gaps: &[&Gap], rest: usize, fixes: &[&Gap], guards: &[String], decisions: &[(&Decision, &str)], regressions: &[&Gap]) -> String {
    let mut lines = vec!["# PROBATIO".to_string(), String::new(), summary.to_string(), String::new(), "## Gaps".into()];
    if gaps.is_empty() {
        lines.push("None.".into());
    }
    for gap in gaps {
        lines.push(format!("- {} at {}:{}", gap.id, gap.file, gap.line));
    }
    if rest > 0 {
        lines.push(format!("- {rest} more"));
    }
    lines.push(String::new());
    lines.push("## Recent fixes".into());
    if fixes.is_empty() {
        lines.push("None.".into());
    }
    for fix in fixes {
        lines.push(format!("- {} fixed in {}, guard {}", fix.id, fix.fixed_in.as_deref().unwrap_or(""), fix.guard.as_deref().unwrap_or("")));
    }
    lines.push(String::new());
    lines.push("## Protected guards".into());
    if guards.is_empty() {
        lines.push("None.".into());
    }
    for guard in guards {
        lines.push(format!("- {guard}"));
    }
    lines.push(String::new());
    lines.push("## Decisions".into());
    if decisions.is_empty() {
        lines.push("None.".into());
    }
    for (decision, review) in decisions {
        lines.push(format!("- {} {} {review} {}", decision.id, decision.status, decision.reason));
    }
    lines.push(String::new());
    lines.push("## Regressions".into());
    if regressions.is_empty() {
        lines.push("None.".into());
    }
    for item in regressions {
        lines.push(format!(
            "- {}: fixed in {}, guarded by {}; the guard broke",
            item.id,
            item.fixed_in.as_deref().unwrap_or(""),
            item.guard.as_deref().unwrap_or("")
        ));
    }
    lines.push(String::new());
    lines.join("\n")
}

fn opt_string(value: &Option<String>) -> Value {
    match value {
        Some(text) => Value::String(text.clone()),
        None => Value::Null,
    }
}

fn gap_value(gap: &Gap) -> Value {
    Value::object(vec![
        ("id", Value::String(gap.id.clone())),
        ("status", Value::String(gap.status.clone())),
        ("file", Value::String(gap.file.clone())),
        ("line", Value::Int(gap.line)),
        ("guard", opt_string(&gap.guard)),
        ("fixedIn", opt_string(&gap.fixed_in)),
        ("guardFile", opt_string(&gap.guard_file)),
        ("guardLineNo", gap.guard_line_no.map(Value::Int).unwrap_or(Value::Null)),
        ("guardLine", opt_string(&gap.guard_line)),
        ("guardHash", opt_string(&gap.guard_hash)),
    ])
}

fn decision_value(item: &Decision) -> Value {
    Value::object(vec![
        ("id", Value::String(item.id.clone())),
        ("file", Value::String(item.file.clone())),
        ("line", Value::Int(item.line)),
        ("hash", Value::String(item.hash.clone())),
        ("status", Value::String(item.status.clone())),
        ("reason", Value::String(item.reason.clone())),
    ])
}

fn finding_value(item: &Finding) -> Value {
    let mut entries = vec![
        ("id".into(), Value::String(item.id.clone())),
        ("status".into(), Value::String(item.status.clone())),
        ("reason".into(), Value::String(item.reason.clone())),
    ];
    if let Some(file) = &item.file {
        entries.push(("file".into(), Value::String(file.clone())));
    }
    if let Some(line) = item.line {
        entries.push(("line".into(), Value::Int(line)));
    }
    if let Some(hash) = &item.hash {
        entries.push(("hash".into(), Value::String(hash.clone())));
    }
    if let Some(agent) = &item.agent {
        entries.push(("agent".into(), Value::String(agent.clone())));
    }
    Value::Object(entries)
}

fn gap_from(value: &Value) -> Option<Gap> {
    Some(Gap {
        id: value.get("id").and_then(Value::as_str)?.to_string(),
        status: value.get("status").and_then(Value::as_str).unwrap_or("open").to_string(),
        file: value.get("file").and_then(Value::as_str).unwrap_or("").to_string(),
        line: value.get("line").and_then(Value::as_int).unwrap_or(0),
        guard: value.get("guard").and_then(Value::as_str).map(str::to_string),
        fixed_in: value.get("fixedIn").and_then(Value::as_str).map(str::to_string),
        guard_file: value.get("guardFile").and_then(Value::as_str).map(str::to_string),
        guard_line_no: value.get("guardLineNo").and_then(Value::as_int),
        guard_line: value.get("guardLine").and_then(Value::as_str).map(str::to_string),
        guard_hash: value.get("guardHash").and_then(Value::as_str).map(str::to_string),
    })
}

fn decision_from(value: &Value) -> Option<Decision> {
    Some(Decision {
        id: value.get("id").and_then(Value::as_str)?.to_string(),
        file: value.get("file").and_then(Value::as_str).unwrap_or("").to_string(),
        line: value.get("line").and_then(Value::as_int).unwrap_or(0),
        hash: value.get("hash").and_then(Value::as_str).unwrap_or("").to_string(),
        status: value.get("status").and_then(Value::as_str).unwrap_or("open").to_string(),
        reason: value.get("reason").and_then(Value::as_str).unwrap_or("").to_string(),
    })
}

fn finding_from(value: &Value) -> Option<Finding> {
    Some(Finding {
        id: value.get("id").and_then(Value::as_str)?.to_string(),
        status: value.get("status").and_then(Value::as_str)?.to_string(),
        reason: value.get("reason").and_then(Value::as_str).unwrap_or("").to_string(),
        file: value.get("file").and_then(Value::as_str).map(str::to_string),
        line: value.get("line").and_then(Value::as_int),
        hash: value.get("hash").and_then(Value::as_str).map(str::to_string),
        agent: value.get("agent").and_then(Value::as_str).map(str::to_string),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gap_fix_requires_a_real_guard_line_and_status_hides_a_seal() {
        let root = std::env::temp_dir().join(format!("probatio-mem-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/guard.txt"), "line one\nkeep this\n").unwrap();
        let state = fix_gap(&empty_state(), "g1", "src/a.py", 4, "src/guard.txt:2", "abc", &root).unwrap();
        save_state(&root, &state).unwrap();
        let page = root.join("PROBATIO.md");
        let open = status_command(&root, &root, None, None, &page).unwrap();
        assert!(open.summary.contains("0 open gaps, 1 fixes"), "{}", open.summary);
        assert!(page.is_file());
        seal_ids(&root, &["g1".into()]).unwrap();
        let hidden = status_command(&root, &root, None, Some("ada"), &page).unwrap();
        assert!(hidden.summary.contains("0 open gaps, 0 fixes"), "{}", hidden.summary);
        let broken = guard_allows(&load_state(&root), "src/guard.txt:2", "", &root);
        fs::write(root.join("src/guard.txt"), "line one\nchanged\n").unwrap();
        let refused = guard_allows(&load_state(&root), "src/guard.txt:2", "", &root);
        assert!(!refused.0, "{}", refused.1);
        let allowed = guard_allows(&load_state(&root), "src/guard.txt:2", "Guard-Change: src/guard.txt:2: the assertion moved", &root);
        assert!(allowed.0, "{}", allowed.1);
        let _ = broken;
        let _ = fs::remove_dir_all(&root);
    }
}
