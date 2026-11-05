//! Read a finished run. Name tests that killed, tests that saw no kill, and one gap per survivor.

use std::fs;
use std::path::{Path, PathBuf};

use crate::envelope::Envelope;
use crate::json::{self, Value};

pub fn tally(out: &Path) -> Envelope {
    let command = "mutate.tally";
    let results_dir = out.join("results");
    let full = out.join("tally.json");
    let results = read_results(&results_dir);
    if results.is_empty() {
        return Envelope::new(false, command, "No mutant results in that directory.", "Run mutate run, then tally the same --out directory.")
            .with_extra("keep", Value::Array(Vec::new()))
            .with_extra("noKillsYet", Value::Array(Vec::new()))
            .with_extra("gaps", Value::Array(Vec::new()))
            .with_extra("rest", Value::Int(0))
            .with_extra("deletedTests", Value::Int(0))
            .with_extra("pruning", pruning())
            .with_extra("full", Value::String(full.display().to_string()));
    }
    let mut kill_counts: Vec<(String, usize)> = Vec::new();
    for result in &results {
        if result.outcome != "killed" {
            continue;
        }
        for name in &result.killed_by {
            if !is_test_name(name) {
                continue;
            }
            if let Some(row) = kill_counts.iter_mut().find(|(known, _)| known == name) {
                row.1 += 1;
            } else {
                kill_counts.push((name.clone(), 1));
            }
        }
    }
    let mut seen = Vec::new();
    let mut see = |name: &str| {
        if !is_test_name(name) {
            return;
        }
        if seen.iter().any(|known: &String| names_match(known, name)) {
            return;
        }
        seen.push(name.to_string());
    };
    for (name, _) in &kill_counts {
        see(name);
    }
    for name in coverage_names(&out.join("coverage-map.json")) {
        see(&name);
    }
    for result in &results {
        for name in result.selected.iter().chain(result.tests.iter()) {
            see(name);
        }
    }
    let count_for = |name: &str| {
        kill_counts.iter().filter(|(killed, _)| names_match(name, killed)).map(|(_, count)| *count).sum::<usize>()
    };
    let mut keep: Vec<String> = seen.iter().filter(|name| count_for(name) > 0).cloned().collect();
    keep.sort();
    let mut no_kills: Vec<String> = seen.iter().filter(|name| count_for(name) == 0).cloned().collect();
    no_kills.sort();
    let no_coverage = results.iter().filter(|item| item.outcome == "no coverage").count();
    let survived = results.iter().filter(|item| item.outcome == "survived").count();
    let timeouts = results.iter().filter(|item| item.outcome == "timeout").count();
    let mut gaps: Vec<Value> = results
        .iter()
        .filter(|item| item.outcome == "survived")
        .map(|item| {
            Value::object(vec![
                ("id", Value::String(item.id.clone())),
                ("file", Value::String(item.file.clone())),
                ("line", Value::Int(item.line)),
                ("outcome", Value::String("survived".into())),
            ])
        })
        .collect();
    gaps.sort_by(|left, right| {
        let file = left.get("file").and_then(Value::as_str).unwrap_or("").cmp(right.get("file").and_then(Value::as_str).unwrap_or(""));
        file.then(left.get("line").and_then(Value::as_int).unwrap_or(0).cmp(&right.get("line").and_then(Value::as_int).unwrap_or(0)))
            .then(left.get("id").and_then(Value::as_str).unwrap_or("").cmp(right.get("id").and_then(Value::as_str).unwrap_or("")))
    });
    let killed = results.iter().filter(|item| item.outcome == "killed").count();
    let unnamed = results.iter().filter(|item| item.outcome == "killed" && item.killed_by.iter().all(|name| !is_test_name(name))).count();
    let mut summary = format!(
        "{no_coverage} no coverage, {survived} survived, {killed} killed, {} tests killed a mutant, {} tests saw no kill in this batch, {} gaps.",
        keep.len(),
        no_kills.len(),
        gaps.len()
    );
    if unnamed > 0 {
        summary.push_str(&format!(" {unnamed} kills name no test, because the suite output does not."));
    }
    summary.push_str(" No test file was deleted.");
    let mut notes = Vec::new();
    if no_coverage > 0 {
        notes.push(format!("{no_coverage} no coverage. That line was not seen. It is not a pass and not a gap."));
    }
    if let Some(first) = gaps.first() {
        let id = first.get("id").and_then(Value::as_str).unwrap_or("");
        let file = first.get("file").and_then(Value::as_str).unwrap_or("");
        let line = first.get("line").and_then(Value::as_int).unwrap_or(0);
        notes.push(format!("First gap is {id} at {file}:{line}. A survivor is not a pass. Add one test that fails on that mutant and passes on the code, commit it, then run nextCall."));
    }
    if timeouts > 0 {
        notes.push("Timed out. The mutant ran longer than the baseline allows.".into());
    }
    if notes.is_empty() {
        notes.push("No survivor in this run. No test file was deleted.".into());
    }
    let shown = gaps.iter().take(10).cloned().collect::<Vec<_>>();
    let rest = gaps.len().saturating_sub(shown.len());
    let next_call = next_argv(out, &keep, !gaps.is_empty());
    let mut envelope = Envelope::new(true, command, summary, notes.join(" "))
        .with_extra("keep", Value::Array(keep.iter().cloned().map(Value::String).collect()))
        .with_extra("noKillsYet", Value::Array(no_kills.into_iter().map(Value::String).collect()))
        .with_extra("gaps", Value::Array(shown))
        .with_extra("rest", Value::Int(rest as i64))
        .with_extra("deletedTests", Value::Int(0))
        .with_extra("pruning", pruning())
        .with_extra("full", Value::String(full.display().to_string()));
    envelope.next_call = next_call;
    let _ = fs::create_dir_all(out);
    let _ = fs::write(&full, envelope.render(""));
    envelope
}

fn pruning() -> Value {
    Value::object(vec![
        ("mode", Value::String("advisory".into())),
        ("deletedTests", Value::Int(0)),
        ("advice", Value::Array(Vec::new())),
    ])
}

fn next_argv(out: &Path, keep: &[String], has_gaps: bool) -> Option<Vec<String>> {
    if !has_gaps && keep.is_empty() {
        return None;
    }
    let stored = fs::read_to_string(out.join("run.json")).ok().and_then(|text| json::parse(&text).ok());
    let base = stored.as_ref().and_then(|value| value.get("out")).and_then(Value::as_str).map(PathBuf::from).unwrap_or_else(|| out.to_path_buf());
    let label = if has_gaps { "after-gaps" } else { "rescore" };
    let mut argv = vec!["mutate".into(), "run".into()];
    if let Some(package) = stored.as_ref().and_then(|value| value.get("package")).and_then(Value::as_str) {
        argv.push("--package".into());
        argv.push(package.to_string());
    }
    if let Some(Value::Array(dirs)) = stored.as_ref().and_then(|value| value.get("patches")) {
        for dir in dirs {
            if let Some(dir) = dir.as_str() {
                argv.push("--patches".into());
                argv.push(dir.to_string());
            }
        }
    }
    argv.push("--out".into());
    argv.push(fresh_dir(&base, label).display().to_string());
    if let Some(repo) = stored.as_ref().and_then(|value| value.get("repo")).and_then(Value::as_str) {
        argv.push("--repo".into());
        argv.push(repo.to_string());
    }
    if !has_gaps {
        if let Some(commit) = stored.as_ref().and_then(|value| value.get("commit")).and_then(Value::as_str) {
            argv.push("--commit".into());
            argv.push(commit.to_string());
        }
    }
    if stored.as_ref().and_then(|value| value.get("confirm")).and_then(Value::as_bool) == Some(false) {
        argv.push("--no-confirm".into());
    }
    if let Some(command) = stored.as_ref().and_then(|value| value.get("suiteCommand")).and_then(Value::as_str) {
        if has_gaps && !command.is_empty() {
            argv.push("--suite-command".into());
            argv.push(command.to_string());
        }
    }
    if !has_gaps {
        for id in keep {
            argv.push("--only-test".into());
            argv.push(id.clone());
        }
    }
    Some(argv)
}

fn fresh_dir(base: &Path, label: &str) -> PathBuf {
    let mut index = 1;
    loop {
        let candidate = if index == 1 {
            PathBuf::from(format!("{}.{label}", base.display()))
        } else {
            PathBuf::from(format!("{}.{label}-{index}", base.display()))
        };
        if !candidate.exists() {
            return candidate;
        }
        index += 1;
    }
}

struct Row {
    id: String,
    outcome: String,
    killed_by: Vec<String>,
    selected: Vec<String>,
    tests: Vec<String>,
    file: String,
    line: i64,
}

fn read_results(dir: &Path) -> Vec<Row> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut files = entries.filter_map(|item| item.ok()).map(|item| item.path()).collect::<Vec<_>>();
    files.sort();
    let mut rows = Vec::new();
    for path in files {
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else { continue };
        let Ok(value) = json::parse(&text) else { continue };
        let Some(outcome) = value.get("outcome").and_then(Value::as_str) else { continue };
        let outcome = if outcome == "killed" && value.get("cause").and_then(Value::as_str) == Some("build") {
            "unviable"
        } else {
            outcome
        };
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| path.file_stem().and_then(|item| item.to_str()).unwrap_or("").to_string());
        let file = value
            .get("files")
            .and_then(|item| match item {
                Value::Array(items) => items.first(),
                _ => None,
            })
            .and_then(|item| item.get("file"))
            .or_else(|| value.get("file"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let line = value
            .get("files")
            .and_then(|item| match item {
                Value::Array(items) => items.first(),
                _ => None,
            })
            .and_then(|item| item.get("line"))
            .and_then(Value::as_int)
            .or_else(|| value.get("line").and_then(Value::as_int))
            .unwrap_or(0);
        rows.push(Row {
            id,
            outcome: outcome.to_string(),
            killed_by: strings(&value, "killedBy"),
            selected: strings(&value, "selectedTests"),
            tests: strings(&value, "tests"),
            file,
            line,
        });
    }
    rows
}

fn strings(value: &Value, key: &str) -> Vec<String> {
    match value.get(key) {
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        _ => Vec::new(),
    }
}

fn coverage_names(file: &Path) -> Vec<String> {
    let Ok(text) = fs::read_to_string(file) else { return Vec::new() };
    let Ok(value) = json::parse(&text) else { return Vec::new() };
    let Some(Value::Object(files)) = value.get("files") else { return Vec::new() };
    let mut names = Vec::new();
    for (_, lines) in files {
        let Value::Object(lines) = lines else { continue };
        for (_, tests) in lines {
            let Value::Array(tests) = tests else { continue };
            for name in tests.iter().filter_map(Value::as_str) {
                if !names.iter().any(|known: &String| known == name) {
                    names.push(name.to_string());
                }
            }
        }
    }
    names
}

fn names_match(a: &str, b: &str) -> bool {
    a == b || a.ends_with(&format!("::{b}")) || b.ends_with(&format!("::{a}"))
}

fn is_test_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let leaf = name.rsplit("::").next().unwrap_or(name);
    !matches!(leaf, "suite" | "pytest" | "unittest" | "command" | "build" | "javac" | "compiler" | "cargo" | "tsc" | "dotnet" | "swiftc" | "cobc" | "nasm" | "clang")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gap_names_its_file_and_line_and_an_unnamed_kill_is_counted() {
        let out = std::env::temp_dir().join(format!("probatio-tally-{}", std::process::id()));
        let _ = fs::remove_dir_all(&out);
        fs::create_dir_all(out.join("results")).unwrap();
        fs::write(out.join("results/m1.json"), r#"{"id":"m1","file":"src/cli.rs","line":161,"outcome":"survived","tests":[]}"#).unwrap();
        fs::write(out.join("results/m2.json"), r#"{"id":"m2","file":"src/cli.rs","line":110,"outcome":"killed","tests":[]}"#).unwrap();
        let envelope = tally(&out);
        assert!(envelope.ok);
        assert!(envelope.next.contains("First gap is m1 at src/cli.rs:161."), "{}", envelope.next);
        assert!(envelope.summary.contains("1 killed, 0 tests killed a mutant"), "{}", envelope.summary);
        assert!(envelope.summary.contains("1 kills name no test"), "{}", envelope.summary);
        let _ = fs::remove_dir_all(&out);
    }
}
