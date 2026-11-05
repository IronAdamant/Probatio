//! `mutate diff`: mutate only the lines changed since a ref, run the suite on each, and name the
//! survivors. This is the call an agent makes right after editing code: were these edits tested?

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::args::Flags;
use crate::envelope::Envelope;
use crate::json::{self, Value};
use crate::operators::{self, Mutant};

const COMMAND: &str = "mutate.diff";
const DEFAULT_MAX: usize = 30;

pub fn diff(flags: &Flags) -> Envelope {
    let Some(package) = flags.get("package").map(PathBuf::from) else {
        return Envelope::new(false, COMMAND, "missing --package", "Pass --package, the project directory you edited.");
    };
    if !package.is_dir() {
        return Envelope::new(false, COMMAND, format!("--package {} is not a directory", package.display()), "Pass the project directory you edited.");
    }
    let since = flags.get("since").unwrap_or("HEAD").to_string();
    let out = flags.get("out").map(PathBuf::from).unwrap_or_else(|| package.join(".probatio").join("diff"));
    let max = match flags.parse_int("max-mutants") {
        Ok(value) => value.map(|value| value as usize).unwrap_or(DEFAULT_MAX).max(1),
        Err(message) => return Envelope::new(false, COMMAND, message, "Pass --max-mutants as a whole number."),
    };
    let src = flags.get("src").map(|dir| dir.trim_matches(['/', '\\']).replace('\\', "/"));

    let changed = match changed_lines(&package, &since) {
        Ok(changed) => changed,
        Err(message) => return Envelope::new(false, COMMAND, message, "Pass --since as a ref git can diff against (HEAD, main, origin/main), from inside a git checkout."),
    };
    let changed: Vec<(String, Vec<u32>)> = changed
        .into_iter()
        .filter(|(file, _)| operators::kind_of(file) && !file.ends_with(".d.ts"))
        .filter(|(file, _)| src.as_deref().is_none_or(|dir| dir.is_empty() || file.starts_with(&format!("{dir}/"))))
        .collect();
    let line_count: usize = changed.iter().map(|(_, lines)| lines.len()).sum();
    let mut points: Vec<(Mutant, String)> = Vec::new();
    for (file, lines) in &changed {
        let Ok(text) = fs::read_to_string(package.join(file)) else { continue };
        for point in operators::scan(file, &text, flags.get("operators") == Some("wide")) {
            if lines.contains(&point.line) {
                points.push((point, text.clone()));
            }
        }
    }
    points.sort_by(|a, b| a.0.file.cmp(&b.0.file).then(a.0.line.cmp(&b.0.line)).then(a.0.start.cmp(&b.0.start)));
    let scope = format!("{} in {} since {since}", count(line_count, "changed line"), count(changed.len(), "file"));
    if points.is_empty() {
        let summary = if line_count == 0 {
            format!("No changed source line since {since}. Nothing was mutated.")
        } else {
            format!("{scope}, and none has a condition, comparison, or boolean to mutate.")
        };
        return Envelope::new(true, COMMAND, summary, "Nothing to score. Edit code, or pass --since with an older ref.")
            .with_extra("changedLines", Value::Int(line_count as i64))
            .with_extra("mutantCount", Value::Int(0));
    }
    let not_run = points.len().saturating_sub(max);
    points.truncate(max);

    // A fresh directory each call: the tests may have changed since the last one, so an old
    // result or baseline must not be reused.
    let generate_out = out.join("generate");
    let run_out = out.join("run");
    for dir in [&generate_out, &run_out] {
        if dir.exists() {
            if let Err(error) = fs::remove_dir_all(dir) {
                return Envelope::new(false, COMMAND, format!("{} could not be cleared: {error}", dir.display()), "Close what holds that directory, or pass another --out.");
            }
        }
    }
    if let Err(message) = crate::generate::write_mutants(&generate_out, &points, if flags.get("operators") == Some("wide") { "wide" } else { "core" }) {
        return Envelope::new(false, COMMAND, message, "The out directory could not be written. Pass another --out.");
    }

    let mut argv: Vec<String> = vec![
        "--package".into(),
        package.display().to_string(),
        "--patches".into(),
        generate_out.display().to_string(),
        "--out".into(),
        run_out.display().to_string(),
    ];
    for key in ["suite-command", "timeout-ms", "workers"] {
        if let Some(value) = flags.get(key) {
            argv.push(format!("--{key}"));
            argv.push(value.to_string());
        }
    }
    if !flags.bool("confirm", true) {
        argv.push("--no-confirm".into());
    }
    let mut envelope = crate::runner::run(&Flags::parse(&argv));
    envelope.command = COMMAND.into();
    envelope.summary = format!("{scope}: {}. {}", count(points.len(), "mutant"), envelope.summary);
    if !envelope.ok {
        return envelope.with_extra("changedLines", Value::Int(line_count as i64));
    }
    let survivors = survivors(&run_out, &points);
    let mut next = if let Some(first) = survivors.first() {
        let file = first.get("file").and_then(Value::as_str).unwrap_or("");
        let line = first.get("line").and_then(Value::as_int).unwrap_or(0);
        let lines = untested_lines(&survivors);
        let verb = if lines == 1 { "is" } else { "are" };
        format!(
            "{} {verb} not tested: {} survived there. First: {file}:{line}. Add a test that fails on it, then call mutate diff again.",
            count(lines, "changed line"),
            count(survivors.len(), "mutant")
        )
    } else if envelope_count(&envelope, "killed") > 0 {
        "No survivor: every mutant on the changed lines that ran was caught (see counts for no coverage and did not build).".to_string()
    } else {
        "No survivor and no kill: no test judged these lines (see counts). This is not a pass.".to_string()
    };
    if not_run > 0 {
        next.push_str(&format!(" {not_run} more mutants on these lines were not run; pass maxMutants to score them."));
    }
    envelope.next = next;
    envelope.next_call = None;
    envelope
        .with_extra("changedLines", Value::Int(line_count as i64))
        .with_extra("mutantCount", Value::Int(points.len() as i64))
        .with_extra("notRun", Value::Int(not_run as i64))
        .with_extra("survivors", Value::Array(survivors.into_iter().take(10).collect()))
        .with_extra("results", Value::String(run_out.join("results").display().to_string()))
}

/// Changed lines per file, relative to the package: tracked edits from `git diff <since>` and
/// every line of an untracked file.
fn changed_lines(package: &Path, since: &str) -> Result<Vec<(String, Vec<u32>)>, String> {
    let diff = git(package, &["diff", "-U0", "--no-renames", "--no-color", "--relative", since, "--"])?;
    let mut changed = parse_ranges(&diff);
    let untracked = git(package, &["ls-files", "--others", "--exclude-standard"])?;
    for file in untracked.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if file.starts_with(".probatio/") || !operators::kind_of(file) {
            continue;
        }
        let Ok(text) = fs::read_to_string(package.join(file)) else { continue };
        let count = text.lines().count() as u32;
        changed.push((file.to_string(), (1..=count.max(1)).collect()));
    }
    Ok(changed)
}

/// `+start,count` hunk headers of a `-U0` diff. A pure deletion marks the line before it.
pub fn parse_ranges(diff: &str) -> Vec<(String, Vec<u32>)> {
    let mut rows: Vec<(String, Vec<u32>)> = Vec::new();
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("+++ ") {
            let name = rest.split('\t').next().unwrap_or(rest).trim();
            // A deleted file's hunks belong to no new file. The empty name is dropped below.
            let name = if name == "/dev/null" { String::new() } else { name.strip_prefix("b/").unwrap_or(name).to_string() };
            rows.push((name, Vec::new()));
        } else if let Some(rest) = line.strip_prefix("@@ ") {
            let Some((_, plus)) = rest.split_once('+') else { continue };
            let range = plus.split_whitespace().next().unwrap_or("");
            let (start, count) = match range.split_once(',') {
                Some((start, count)) => (start.parse::<u32>().unwrap_or(0), count.parse::<u32>().unwrap_or(1)),
                None => (range.parse::<u32>().unwrap_or(0), 1),
            };
            if let Some(row) = rows.last_mut() {
                if count == 0 {
                    row.1.push(start.max(1));
                } else {
                    row.1.extend(start..start.saturating_add(count));
                }
            }
        }
    }
    rows.retain(|(name, lines)| !name.is_empty() && !lines.is_empty());
    rows
}

fn survivors(run_out: &Path, points: &[(Mutant, String)]) -> Vec<Value> {
    let mut found = Vec::new();
    for (mutant, text) in points {
        let path = run_out.join("results").join(format!("{}.json", mutant.id));
        let Ok(body) = fs::read_to_string(&path) else { continue };
        let Ok(value) = json::parse(&body) else { continue };
        if value.get("outcome").and_then(Value::as_str) != Some("survived") {
            continue;
        }
        found.push(Value::object(vec![
            ("file", Value::String(mutant.file.clone())),
            ("line", Value::Int(mutant.line as i64)),
            ("column", Value::Int(column(text, mutant.start) as i64)),
            ("original", Value::String(mutant.original.clone())),
            ("replacement", Value::String(mutant.replacement.clone())),
        ]));
    }
    found
}

/// `1 mutant`, `2 mutants`.
fn count(n: usize, noun: &str) -> String {
    if n == 1 { format!("1 {noun}") } else { format!("{n} {noun}s") }
}

/// Distinct `file:line` pairs among the survivors: two mutants on one line are one untested line.
fn untested_lines(survivors: &[Value]) -> usize {
    let mut seen: Vec<(String, i64)> = Vec::new();
    for item in survivors {
        let key = (item.get("file").and_then(Value::as_str).unwrap_or("").to_string(), item.get("line").and_then(Value::as_int).unwrap_or(0));
        if !seen.contains(&key) {
            seen.push(key);
        }
    }
    seen.len()
}

fn envelope_count(envelope: &Envelope, key: &str) -> i64 {
    envelope.extra.iter().find(|(name, _)| name == key).and_then(|(_, value)| value.as_int()).unwrap_or(0)
}

/// 1-based character column of a byte offset, so two operators on one line can be told apart.
fn column(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    let line_start = text[..offset].rfind('\n').map(|at| at + 1).unwrap_or(0);
    text[line_start..offset].chars().count() + 1
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git").arg("-C").arg(dir).args(args).output().map_err(|error| format!("git did not start: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let text = String::from_utf8_lossy(&output.stderr);
        Err(format!("git {} failed: {}", args.first().unwrap_or(&""), text.lines().next().unwrap_or("").trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_survivors_on_one_line_are_one_untested_line() {
        let survivor = |line: i64| Value::object(vec![("file", Value::String("src/a.rs".into())), ("line", Value::Int(line))]);
        assert_eq!(untested_lines(&[survivor(34), survivor(34)]), 1);
        assert_eq!(untested_lines(&[survivor(34), survivor(35)]), 2);
    }

    #[test]
    fn hunk_headers_name_the_new_lines() {
        let diff = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -3 +3 @@\n-x\n+y\n@@ -9,0 +10,2 @@\n+p\n+q\n@@ -20,2 +21,0 @@\n-r\n-s\n--- a/gone.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-z\n";
        assert_eq!(parse_ranges(diff), vec![("src/a.rs".to_string(), vec![3, 10, 11, 21])]);
    }

    fn git_ok(dir: &Path, args: &[&str]) {
        let status = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
        assert!(status.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&status.stderr));
    }

    /// An uncommitted edit is scored, the unchanged line beside it is not, and the survivor is named.
    #[test]
    fn only_the_edited_lines_are_mutated_and_a_survivor_is_named() {
        let dir = std::env::temp_dir().join(format!("probatio-diff-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        git_ok(&dir, &["init", "-q"]);
        git_ok(&dir, &["config", "user.email", "a@b.c"]);
        git_ok(&dir, &["config", "user.name", "a"]);
        git_ok(&dir, &["config", "core.autocrlf", "false"]);
        fs::write(dir.join("src/A.java"), "class A {\n  boolean f(boolean a, boolean b) { return a && b; }\n  boolean g(int n) { return n > 0; }\n}\n").unwrap();
        git_ok(&dir, &["add", "."]);
        git_ok(&dir, &["commit", "-q", "-m", "init"]);
        fs::write(dir.join("src/A.java"), "class A {\n  boolean f(boolean a, boolean b) { return a && b; }\n  boolean g(int n) { return n >= 0; }\n}\n").unwrap();
        let out = dir.join("out");
        let argv: Vec<String> = ["--package", &dir.display().to_string(), "--out", &out.display().to_string(), "--suite-command", "echo ok", "--no-confirm"]
            .iter()
            .map(|item| item.to_string())
            .collect();
        let envelope = diff(&Flags::parse(&argv));
        assert!(envelope.ok, "{} {}", envelope.summary, envelope.next);
        assert!(envelope.summary.starts_with("1 changed line in 1 file since HEAD: 1 mutant."), "{}", envelope.summary);
        let value = envelope.to_value("");
        let survivors = json::stringify_compact(value.get("survivors").unwrap());
        assert!(survivors.contains(r#""file":"src/A.java","line":3,"column":31,"original":">=""#), "{survivors}");
        assert!(envelope.next.contains("src/A.java:3"), "{}", envelope.next);

        git_ok(&dir, &["checkout", "-q", "--", "src/A.java"]);
        let clean = diff(&Flags::parse(&argv));
        assert!(clean.ok);
        assert!(clean.summary.starts_with("No changed source line since HEAD."), "{}", clean.summary);
        let _ = fs::remove_dir_all(&dir);
    }
}
