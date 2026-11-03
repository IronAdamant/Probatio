//! `ledger build` reverts each fix commit's src diff. `ledger check` runs those bugs again.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::envelope::Envelope;
use crate::json::{self, Value};
use crate::memory::golden_changes;

pub fn build(
    package: &Path,
    out: &Path,
    commit: &str,
    src: &str,
    tests_dir: &str,
    max_lines: usize,
    max_commits: Option<u64>,
    repo_flag: Option<&Path>,
) -> Envelope {
    let repo = if let Some(path) = repo_flag {
        if !path.is_dir() || git(path, &["rev-parse", "--is-inside-work-tree"]).is_err() {
            return Envelope::new(false, "ledger.build", "repo is not a git repository", "Pass --repo as the git root.");
        }
        path.to_path_buf()
    } else if let Some(root) = git_root(package) {
        root
    } else {
        return Envelope::new(false, "ledger.build", "package is not inside a git repository. Pass --repo.", "Fix the command and run it again.");
    };
    let resolved = git(&repo, &["rev-parse", "--verify", &format!("{commit}^{{commit}}")]);
    let Ok(sha) = resolved else {
        return Envelope::new(false, "ledger.build", "that commit does not resolve", "Pass a commit that exists.");
    };
    let sha = sha.trim().to_string();
    let mut args = vec!["rev-list".to_string(), "--no-merges".into(), sha.clone()];
    if let Some(max) = max_commits {
        args.insert(1, format!("--max-count={max}"));
    }
    let list = git(&repo, &args.iter().map(String::as_str).collect::<Vec<_>>()).unwrap_or_default();
    let mut entries = Vec::new();
    let mut clean = 0i64;
    let mut hand = 0i64;
    let mut skipped = 0i64;
    let mut scanned = 0i64;
    fs::create_dir_all(out).ok();
    for commit_id in list.lines() {
        if commit_id.is_empty() {
            continue;
        }
        scanned += 1;
        let stat = git(&repo, &["diff", "--numstat", &format!("{commit_id}^"), commit_id, "--", src, tests_dir]).unwrap_or_default();
        let lines = stat.lines().filter_map(|line| line.split('\t').next().and_then(|item| item.parse::<usize>().ok())).sum::<usize>();
        let subject = git(&repo, &["log", "-1", "--format=%s", commit_id]).unwrap_or_default();
        let date = git(&repo, &["log", "-1", "--format=%aI", commit_id]).unwrap_or_default();
        let id = commit_id.chars().take(7).collect::<String>();
        if lines > max_lines {
            skipped += 1;
            entries.push(entry(&id, commit_id, date.trim(), subject.trim(), "skipped", lines, &format!("src diff is {lines} lines, over the {max_lines} line cap")));
            continue;
        }
        if lines == 0 {
            continue;
        }
        let bug = git(&repo, &["diff", "-R", &format!("{commit_id}^"), commit_id, "--", src]).unwrap_or_default();
        if bug.trim().is_empty() {
            continue;
        }
        if apply_check(&repo, &bug) {
            clean += 1;
            let patch = format!("{id}.patch");
            let body = format!("# probatio-mutant direction=forward meaning=apply-to-introduce-the-bug\n{bug}");
            let _ = fs::write(out.join(&patch), body);
            entries.push(entry_patch(&id, commit_id, date.trim(), subject.trim(), "clean", lines, &patch));
        } else {
            hand += 1;
            entries.push(entry(&id, commit_id, date.trim(), subject.trim(), "hand", lines, "patch does not apply"));
        }
    }
    let value = Value::object(vec![
        ("commit", Value::String(sha.clone())),
        ("package", Value::String(package.display().to_string())),
        ("testsDir", Value::String(tests_dir.to_string())),
        ("entries", Value::Array(entries)),
    ]);
    let _ = fs::write(out.join("ledger.json"), json::stringify(&value));
    Envelope::new(
        true,
        "ledger.build",
        format!("{clean} fixes reverse cleanly."),
        "A hand row needs a mutant written by hand. Nothing in the checkout was rewritten.",
    )
    .with_extra("clean", Value::Int(clean))
    .with_extra("hand", Value::Int(hand))
    .with_extra("handmade", Value::Int(0))
    .with_extra("skipped", Value::Int(skipped))
    .with_extra("commitsScanned", Value::Int(scanned))
    .with_extra("commit", Value::String(sha))
    .with_extra("full", Value::String(out.join("ledger.json").display().to_string()))
}

pub fn check(package: &Path, ledger_dir: &Path, out: &Path, update: bool, message: &str, timeout_ms: u64, extra: &[String]) -> Envelope {
    let ledger_file = if ledger_dir.is_file() { ledger_dir.to_path_buf() } else { ledger_dir.join("ledger.json") };
    let Ok(text) = fs::read_to_string(&ledger_file) else {
        return Envelope::new(false, "ledger.check", format!("No ledger.json in {}.", ledger_dir.display()), "Run probatio ledger build first.");
    };
    if text.lines().any(|line| line.trim() == "red") || text.contains("\"status\": \"red\"") || text.contains("\"status\":\"red\"") {
        return Envelope::new(
            false,
            "ledger.check",
            "A fixed bug is back in reach of the suite.",
            "Do not re-record it away. Find the test that stopped guarding it, or give the reason in a Golden-Change: line.",
        );
    }
    let Ok(value) = json::parse(&text) else {
        return Envelope::new(false, "ledger.check", "ledger.json is not JSON.", "Rebuild the ledger.");
    };
    let Some(Value::Array(entries)) = value.get("entries") else {
        return Envelope::new(false, "ledger.check", "The ledger has no bug that applies.", "Rebuild the ledger, or write a hand-made mutant for a fix.");
    };
    let parent = ledger_file.parent().unwrap_or(ledger_dir);
    let guarded: Vec<&Value> = entries
        .iter()
        .filter(|item| {
            let status = item.get("status").and_then(Value::as_str).unwrap_or("");
            (status == "clean" || status == "handmade") && item.get("patch").and_then(Value::as_str).is_some()
        })
        .collect();
    if guarded.is_empty() {
        return Envelope::new(false, "ledger.check", "The ledger has no bug that applies.", "Rebuild the ledger, or write a hand-made mutant for a fix.");
    }
    let patch_dir = out.join("patches");
    let _ = fs::remove_dir_all(out);
    let _ = fs::create_dir_all(&patch_dir);
    let mut rows = Vec::new();
    for item in &guarded {
        let Some(patch) = item.get("patch").and_then(Value::as_str) else { continue };
        let source = parent.join(patch);
        if !source.is_file() {
            return Envelope::new(false, "ledger.check", format!("{patch} is listed in ledger.json and is not in {}.", parent.display()), "Restore the patch or rebuild the ledger.");
        }
        let _ = fs::copy(&source, patch_dir.join(patch));
        rows.push(mutant_from_patch(&source, item));
    }
    let _ = fs::write(
        patch_dir.join("mutants.json"),
        json::stringify(&Value::object(vec![("mutants", Value::Array(rows))])),
    );
    let run_out = out.join("run");
    let mut args = vec![
        "--package".into(),
        package.display().to_string(),
        "--patches".into(),
        patch_dir.display().to_string(),
        "--out".into(),
        run_out.display().to_string(),
        "--timeout-ms".into(),
        timeout_ms.to_string(),
        "--no-confirm".into(),
    ];
    args.extend_from_slice(extra);
    let report = crate::runner::run(&crate::args::Flags::parse(&args));
    if !report.ok {
        return Envelope::new(false, "ledger.check", format!("The ledger run did not score: {}", report.summary), report.next);
    }
    let allowed = golden_changes(message);
    let mut regressed = Vec::new();
    let mut changed = Vec::new();
    let mut unrecorded = Vec::new();
    let mut unauthorized = Vec::new();
    let mut killed = 0i64;
    for item in &guarded {
        let id = item.get("id").and_then(Value::as_str).unwrap_or("");
        let patch = item.get("patch").and_then(Value::as_str).unwrap_or("");
        let patch_id = Path::new(patch).file_stem().and_then(|item| item.to_str()).unwrap_or(id);
        let result_path = run_out.join("results").join(format!("{patch_id}.json"));
        let outcome = fs::read_to_string(&result_path)
            .ok()
            .and_then(|text| json::parse(&text).ok())
            .and_then(|value| value.get("outcome").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_else(|| "missing".into());
        if outcome == "killed" {
            killed += 1;
        }
        let golden_file = parent.join(format!("{id}.golden.json"));
        let recorded = fs::read_to_string(&golden_file)
            .ok()
            .and_then(|text| json::parse(&text).ok())
            .and_then(|value| value.get("rows").and_then(|rows| rows.get(id)).and_then(|row| row.get("outcome")).and_then(Value::as_str).map(str::to_string));
        let regression = recorded.as_deref() == Some("killed") && outcome != "killed";
        if recorded.is_none() {
            unrecorded.push(id.to_string());
        } else if regression {
            regressed.push(id.to_string());
        } else if recorded.as_deref() != Some(outcome.as_str()) {
            changed.push(id.to_string());
        }
        let differs = recorded.as_deref() != Some(outcome.as_str());
        if differs && recorded.is_some() && !regression && !allowed.iter().any(|(name, _)| name == id) {
            unauthorized.push(id.to_string());
        }
        let refresh = update && !regression && (recorded.is_none() || !differs || allowed.iter().any(|(name, _)| name == id));
        if refresh {
            let body = Value::object(vec![(
                "rows",
                Value::object(vec![(
                    id,
                    Value::object(vec![
                        ("ok", Value::Bool(true)),
                        ("outcome", Value::String(outcome.clone())),
                        ("reason", Value::String(reason(&outcome).into())),
                        ("next", Value::String(reason(&outcome).into())),
                    ]),
                )]),
            )]);
            let _ = fs::write(&golden_file, json::stringify(&body));
        }
    }
    let ok = regressed.is_empty() && if update { unauthorized.is_empty() } else { changed.is_empty() && unrecorded.is_empty() };
    let mut parts = vec![format!("{killed} of {} ledger bugs caught.", guarded.len())];
    if !regressed.is_empty() {
        parts.push(format!("Regressed: {}.", regressed.join(", ")));
    }
    if !changed.is_empty() {
        parts.push(format!("Changed: {}.", changed.join(", ")));
    }
    if !unrecorded.is_empty() {
        parts.push(format!("Not recorded yet: {}.", unrecorded.join(", ")));
    }
    let next = if !regressed.is_empty() {
        format!("A fixed bug is back in reach of the suite: {} was caught and is not now. Find the test that stopped guarding it.", regressed[0])
    } else if !unauthorized.is_empty() && update {
        format!("Not written: {} changed outcome. Add Golden-Change: <id>: <why> to --message for each.", unauthorized.join(", "))
    } else if update {
        "Re-recorded. Commit the goldens with the reason for each changed outcome.".into()
    } else if !changed.is_empty() || !unrecorded.is_empty() {
        "Review the changed outcomes, then re-record with --update and a Golden-Change line for each changed id.".into()
    } else {
        "Every ledger bug has the outcome recorded for it.".into()
    };
    Envelope::new(ok, "ledger.check", parts.join(" "), next)
        .with_extra("caught", Value::Int(killed))
        .with_extra("total", Value::Int(guarded.len() as i64))
        .with_extra("regressed", Value::Array(regressed.into_iter().map(Value::String).collect()))
        .with_extra("changed", Value::Array(changed.into_iter().map(Value::String).collect()))
        .with_extra("unrecorded", Value::Array(unrecorded.into_iter().map(Value::String).collect()))
}

fn reason(outcome: &str) -> &'static str {
    match outcome {
        "killed" => "A test the suite already had caught this ledger bug.",
        "survived" => "The suite ran the line and did not catch this ledger bug.",
        "no coverage" => "No test runs the line this ledger bug changes.",
        "unviable" => "This ledger bug does not build as it stands. A hand-made mutant that keeps the API would guard it.",
        "flaky" => "A test failed once and passed on the rerun. That is not a catch.",
        "timeout" => "The run passed the baseline clock. That is not a catch.",
        "error" => "The ledger patch did not run. It may no longer apply: rebuild the ledger or refresh the hand-made mutant.",
        "missing" => "No result was written for this ledger bug.",
        _ => "The ledger patch did not run.",
    }
}

fn mutant_from_patch(path: &Path, item: &Value) -> Value {
    let id = path.file_stem().and_then(|name| name.to_str()).unwrap_or("m").to_string();
    let file = item
        .get("files")
        .and_then(|value| match value {
            Value::Array(items) => items.first(),
            _ => None,
        })
        .and_then(|row| row.get("file"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Value::object(vec![
        ("id", Value::String(id)),
        ("file", Value::String(file)),
        ("line", Value::Int(1)),
        ("start", Value::Int(0)),
        ("end", Value::Int(0)),
        ("op", Value::String("ledger".into())),
        ("original", Value::String(String::new())),
        ("replacement", Value::String(String::new())),
        ("patch", Value::String(path.file_name().and_then(|name| name.to_str()).unwrap_or("").into())),
    ])
}

fn entry(id: &str, commit: &str, date: &str, subject: &str, status: &str, lines: usize, reason: &str) -> Value {
    Value::object(vec![
        ("id", Value::String(id.into())),
        ("commit", Value::String(commit.into())),
        ("date", Value::String(date.into())),
        ("subject", Value::String(subject.into())),
        ("fixer", Value::String("human".into())),
        ("status", Value::String(status.into())),
        ("lines", Value::Int(lines as i64)),
        ("files", Value::Array(Vec::new())),
        ("reason", Value::String(reason.into())),
    ])
}

fn entry_patch(id: &str, commit: &str, date: &str, subject: &str, status: &str, lines: usize, patch: &str) -> Value {
    Value::object(vec![
        ("id", Value::String(id.into())),
        ("commit", Value::String(commit.into())),
        ("date", Value::String(date.into())),
        ("subject", Value::String(subject.into())),
        ("fixer", Value::String("human".into())),
        ("status", Value::String(status.into())),
        ("lines", Value::Int(lines as i64)),
        ("files", Value::Array(Vec::new())),
        ("patch", Value::String(patch.into())),
    ])
}

fn apply_check(repo: &Path, diff: &str) -> bool {
    let mut child = Command::new("git");
    child.arg("-C").arg(repo).args(["apply", "--check", "--whitespace=nowarn", "-"]);
    child.stdin(std::process::Stdio::piped());
    let Ok(mut child) = child.spawn() else { return false };
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        let _ = stdin.write_all(diff.as_bytes());
    }
    child.wait().map(|status| status.success()).unwrap_or(false)
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
    use std::process::Command;

    #[test]
    fn build_records_a_clean_revert_and_a_red_file_is_not_rewritten() {
        let root = std::env::temp_dir().join(format!("probatio-ledger-build-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        let git = |args: &[&str]| {
            Command::new("git").arg("-C").arg(&root).args(args).status().unwrap();
        };
        git(&["init"]);
        git(&["config", "user.email", "a@b.c"]);
        git(&["config", "user.name", "A"]);
        fs::write(root.join("src/a.js"), "export function f(a, b) { return a && b; }\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "base"]);
        fs::write(root.join("src/a.js"), "export function f(a, b) { return a || b; }\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "swap"]);
        let out = root.join("ledger");
        let envelope = build(&root, &out, "HEAD", "src", "tests", 300, None, None);
        assert!(envelope.ok, "{}", envelope.summary);
        assert!(envelope.summary.contains("reverse cleanly"), "{}", envelope.summary);
        assert!(out.join("ledger.json").is_file());
        let red = root.join("red.json");
        let body = "{\n  \"status\": \"red\"\n}\n";
        fs::write(&red, body).unwrap();
        let refused = check(&root, &red, &root.join("check"), false, "", 5_000, &[]);
        assert!(!refused.ok);
        assert!(refused.summary.contains("back in reach"), "{}", refused.summary);
        assert_eq!(fs::read_to_string(&red).unwrap(), body);
        let _ = fs::remove_dir_all(&root);
    }
}
