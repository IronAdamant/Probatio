//! `verify-change` mutates the lines a commit changed, then scores those mutants.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::args::Flags;
use crate::envelope::Envelope;
use crate::json::{self, Value};
use crate::operators::{self, Mutant};

pub fn verify(flags: &Flags) -> Envelope {
    let command = "verify-change";
    let package = match flags.need("package") {
        Ok(value) => PathBuf::from(value),
        Err(message) => return Envelope::new(false, command, message, "Pass the missing flag and run the command again."),
    };
    let out = match flags.need("out") {
        Ok(value) => PathBuf::from(value),
        Err(message) => return Envelope::new(false, command, message, "Pass the missing flag and run the command again."),
    };
    let base = flags.get("base").unwrap_or("HEAD").to_string();
    let commit = flags.get("commit").unwrap_or("HEAD").to_string();
    let max_mutants = flags.int("max-mutants", 4).unwrap_or(4) as usize;
    let max_tests = flags.parse_int("max-tests").unwrap_or(None).map(|value| value as usize);
    let wide = flags.get("operators") == Some("wide");
    let repo = if let Some(path) = flags.get("repo") {
        let path = PathBuf::from(path);
        if !path.is_dir() || git(&path, &["rev-parse", "--is-inside-work-tree"]).is_err() {
            return Envelope::new(false, command, "repo is not a git repository", "Pass --repo as the git root.");
        }
        Some(path)
    } else {
        git_root(&package)
    };
    let Some(repo) = repo else {
        return baseline_only(flags, &package, &out, "package is not inside a git repository. Pass --repo.");
    };
    let diff = git(&repo, &["diff", "-U0", "--no-renames", &base, &commit]);
    let names = git(&repo, &["diff", "--name-only", "--no-renames", &base, &commit]).unwrap_or_default();
    let changed: Vec<String> = names.lines().filter(|line| !line.trim().is_empty()).map(|line| line.trim().to_string()).collect();
    let status = git(&repo, &["status", "--porcelain"]).unwrap_or_default();
    if !status.trim().is_empty() && (base == commit || changed.is_empty()) {
        return Envelope::new(
            false,
            command,
            "The working tree has uncommitted changes. verify-change did not score them. Commit the edit, or pass --base and --commit that name different commits with a diff.",
            "Commit the edit, then run the same command.",
        );
    }
    let diff = match diff {
        Ok(text) => text,
        Err(message) => return Envelope::new(false, command, message, "Fix the git diff and run again."),
    };
    let ranges = parse_ranges(&diff);
    let mut chosen: Vec<(Mutant, String)> = Vec::new();
    let mut package_files = Vec::new();
    for (file, lines) in &ranges {
        let rel = file.strip_prefix("a/").or_else(|| file.strip_prefix("b/")).unwrap_or(file);
        if !operators::kind_of(rel) {
            continue;
        }
        let text = git(&repo, &["show", &format!("{commit}:{rel}")]).unwrap_or_default();
        if text.is_empty() {
            continue;
        }
        package_files.push(rel.to_string());
        for point in operators::scan(rel, &text, wide) {
            if lines.contains(&point.line) {
                chosen.push((point, text.clone()));
            }
        }
    }
    if let Some(max_tests) = max_tests {
        package_files.truncate(max_tests);
    }
    chosen.sort_by(|a, b| a.0.file.cmp(&b.0.file).then(a.0.line.cmp(&b.0.line)).then(a.0.op.cmp(&b.0.op)));
    let not_run = chosen.len().saturating_sub(max_mutants);
    chosen.truncate(max_mutants);
    let patches = out.join("mutants");
    let _ = fs::create_dir_all(&patches);
    let mut written = Vec::new();
    for (mutant, _text) in &chosen {
        written.push(Value::object(vec![
            ("id", Value::String(mutant.id.clone())),
            ("file", Value::String(mutant.file.clone())),
            ("line", Value::Int(mutant.line as i64)),
            ("start", Value::Int(mutant.start as i64)),
            ("end", Value::Int(mutant.end as i64)),
            ("op", Value::String(mutant.op.clone())),
            ("original", Value::String(mutant.original.clone())),
            ("replacement", Value::String(mutant.replacement.clone())),
        ]));
    }
    let _ = fs::write(patches.join("mutants.json"), json::stringify(&Value::object(vec![("mutants", Value::Array(written.clone()))])));
    if chosen.is_empty() {
        return Envelope::new(true, command, "0 mutants on the changed lines. No mutant was introduced.", "Nothing was mutated.")
            .with_extra("affected", Value::Array(Vec::new()))
            .with_extra("ran", Value::Int(0))
            .with_extra("caught", Value::Int(0))
            .with_extra("missed", Value::Int(0))
            .with_extra("goldenContract", Value::Array(Vec::new()));
    }
    let mut run_flags = vec![
        "--package".into(),
        package.display().to_string(),
        "--patches".into(),
        patches.display().to_string(),
        "--out".into(),
        out.join("run").display().to_string(),
        "--no-confirm".into(),
        "--timeout-ms".into(),
        flags.get("timeout-ms").unwrap_or("120000").to_string(),
    ];
    if flags.get("timeout-ms").is_none() {
        if let Some(minutes) = flags.get("max-minutes") {
            run_flags.push("--max-minutes".into());
            run_flags.push(minutes.to_string());
        }
    }
    if let Some(suite) = flags.get("suite-command") {
        run_flags.push("--suite-command".into());
        run_flags.push(suite.to_string());
    }
    let report = crate::runner::run(&Flags::parse(&run_flags));
    if !report.ok {
        return Envelope::new(false, command, report.summary, report.next);
    }
    let (caught, missed) = count_run(&out.join("run"));
    Envelope::new(true, command, format!("{} ran, {caught} caught, {missed} missed.", chosen.len()), "A kill is an assertion the suite already had.")
        .with_extra("affected", Value::Array(package_files.into_iter().map(Value::String).collect()))
        .with_extra("ran", Value::Int(chosen.len() as i64))
        .with_extra("caught", Value::Int(caught))
        .with_extra("missed", Value::Int(missed))
        .with_extra("notRun", Value::Int(not_run as i64))
        .with_extra("goldenContract", Value::Array(Vec::new()))
        .with_extra("operators", Value::String(if wide { "wide" } else { "core" }.into()))
}

fn baseline_only(flags: &Flags, package: &Path, out: &Path, git_note: &str) -> Envelope {
    let Some(suite) = flags.get("suite-command") else {
        return Envelope::new(false, "verify-change", "No suite was run.", git_note);
    };
    let timeout = flags.int("timeout-ms", 120_000).unwrap_or(120_000).max(1);
    let timed = crate::process::run_shell(suite, Some(package), std::time::Duration::from_millis(timeout));
    if !timed.spawned || timed.timed_out || timed.status != Some(0) {
        return Envelope::new(false, "verify-change", "The baseline suite failed. No mutant was introduced.", "Start from a green suite.");
    }
    let _ = fs::create_dir_all(out);
    Envelope::new(true, "verify-change", "The baseline suite passed. No mutant was introduced.", "Pass --base and --commit with a diff to score mutants.")
}

fn count_run(out: &Path) -> (i64, i64) {
    let Ok(entries) = fs::read_dir(out.join("results")) else { return (0, 0) };
    let mut caught = 0i64;
    let mut missed = 0i64;
    for entry in entries.flatten() {
        let Ok(text) = fs::read_to_string(entry.path()) else { continue };
        let Ok(value) = json::parse(&text) else { continue };
        match value.get("outcome").and_then(Value::as_str) {
            Some("killed") => caught += 1,
            Some("survived") => missed += 1,
            _ => {}
        }
    }
    (caught, missed)
}

fn parse_ranges(diff: &str) -> Vec<(String, Vec<u32>)> {
    let mut rows: Vec<(String, Vec<u32>)> = Vec::new();
    let mut file = String::new();
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("+++ ") {
            let mut name = rest.split('\t').next().unwrap_or(rest).trim().to_string();
            if let Some(stripped) = name.strip_prefix("b/").or_else(|| name.strip_prefix("a/")) {
                name = stripped.to_string();
            }
            file = name;
            if !rows.iter().any(|(known, _)| known == &file) {
                rows.push((file.clone(), Vec::new()));
            }
        } else if let Some(rest) = line.strip_prefix("@@ ") {
            if let Some(plus) = rest.split_once('+') {
                let digits: String = plus.1.chars().take_while(|ch| ch.is_ascii_digit()).collect();
                if let Ok(start) = digits.parse::<u32>() {
                    let count = plus.1.split(',').nth(1).and_then(|item| item.split_whitespace().next()).and_then(|item| item.parse::<u32>().ok()).unwrap_or(1);
                    if let Some(row) = rows.iter_mut().find(|(known, _)| known == &file) {
                        for line_no in start..start.saturating_add(count.max(1)) {
                            row.1.push(line_no);
                        }
                    }
                }
            }
        }
    }
    rows
}

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git").arg("-C").arg(repo).args(args).output().map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

fn git_root(path: &Path) -> Option<PathBuf> {
    git(path, &["rev-parse", "--show-toplevel"]).ok().map(|text| PathBuf::from(text.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_uncommitted_edit_is_not_scored() {
        let root = std::env::temp_dir().join(format!("probatio-verify-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        let git = |args: &[&str]| { let _ = Command::new("git").arg("-C").arg(&root).args(args).status(); };
        git(&["init"]);
        git(&["config", "user.email", "a@b.c"]);
        git(&["config", "user.name", "A"]);
        fs::write(root.join("src/a.js"), "export function f(a, b) { return a && b; }\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "base"]);
        fs::write(root.join("src/a.js"), "export function f(a, b) { return a || b; }\n").unwrap();
        let envelope = verify(&Flags::parse(&[
            "--package".into(),
            root.display().to_string(),
            "--out".into(),
            root.join("out").display().to_string(),
        ]));
        assert!(!envelope.ok, "{}", envelope.summary);
        assert!(envelope.summary.contains("uncommitted"), "{}", envelope.summary);
        let _ = fs::remove_dir_all(&root);
    }
}
