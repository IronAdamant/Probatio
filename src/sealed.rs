//! Sealed runs and check-kill. A hidden label must stay out of the report, and a kill is a real rerun.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::args::Flags;
use crate::envelope::Envelope;
use crate::json::{self, Value};

pub fn sealed(flags: &Flags) -> Envelope {
    let command = "mutate.sealed";
    let label_path = match flags.need("label") {
        Ok(value) => PathBuf::from(value),
        Err(message) => return Envelope::new(false, command, message, "Pass --label. The hidden test stays hidden."),
    };
    let label = match read_label(&label_path) {
        Ok(value) => value,
        Err(message) => return Envelope::new(false, command, message, "Keep the label in its own file. Run the same command again."),
    };
    for rel in flags.texts("hide") {
        if rel.split(['/', '\\']).any(|part| part == "..") {
            return Envelope::new(false, command, format!("--hide stays inside the package. Refused {rel}."), "Pass a relative path with no .. segment.");
        }
    }
    let report = crate::runner::run(flags);
    let body = report.render("");
    let out = flags.get("out").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    if leaked(&body, &label) || tree_leaked(&out, &label) {
        return Envelope::new(false, command, "The label was copied into the report.", "Keep the label in its own file. Run the same command again.");
    }
    if !report.ok {
        return report;
    }
    if let Some(fix) = flags.get("fix") {
        let repo = flags.get("repo").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(flags.get("package").unwrap_or(".")));
        if crate::worktree::resolve_commit(&repo, fix).is_err() {
            return Envelope::new(false, command, "that commit does not resolve", "Pass --fix as a commit that exists. The hidden test stays hidden.");
        }
        let kills = killed_names(&out);
        let edited = fix_edited_killers(&repo, fix, &flags.texts("hide"), &kills);
        let not_started = fs::read_to_string(out.join("not-started.txt")).unwrap_or_default();
        let waiting = not_started.lines().any(|line| !line.trim().is_empty());
        if !waiting && !kills.is_empty() && !edited.older {
            let summary = format!(
                "{} Every kill came from a test edited by the fix commit ({}). That is the fix's own test, so no older test caught the bug.",
                report.summary, edited.files.join(", ")
            );
            return Envelope::new(false, command, summary, "Record this as a miss. Do not restore the hidden test.")
                .with_extra("killed", Value::Int(0))
                .with_extra("miss", Value::Bool(true))
                .with_extra("fixEdited", Value::Array(edited.files.into_iter().map(Value::String).collect()));
        }
    }
    let killed = report.extra.iter().find(|(key, _)| key == "killed").and_then(|(_, value)| value.as_int()).unwrap_or(0);
    if killed == 0 {
        let mut kept = report;
        kept.command = command.into();
        if kept.summary.is_empty() {
            kept.summary = "The hidden test stayed hidden. No catch was recorded.".into();
        }
        return kept;
    }
    report
}

pub fn check_kill(flags: &Flags) -> Envelope {
    let command = "check-kill";
    let id = flags.positionals.first().map(String::as_str).or_else(|| flags.get("id")).unwrap_or("").to_string();
    if id.is_empty() {
        return Envelope::new(false, command, "check-kill needs a mutant id", "Pass the id. The task is not done.");
    }
    let package = match flags.need("package") {
        Ok(value) => PathBuf::from(value),
        Err(message) => return missing(command, &message),
    };
    let out = match flags.need("out") {
        Ok(value) => PathBuf::from(value),
        Err(message) => return missing(command, &message),
    };
    let patch_dirs = flags.texts("patches");
    if patch_dirs.is_empty() {
        return missing(command, "--patches is required");
    }
    let found = patch_dirs.iter().find_map(|dir| find_mutant(Path::new(dir), &id));
    let Some(mutant) = found else {
        return Envelope::new(false, command, format!("{id} is not done. The patch is missing."), "The task is not done.")
            .with_extra("done", Value::Bool(false))
            .with_extra("outcome", Value::Null)
            .with_extra("suiteGreen", Value::Bool(false))
            .with_extra("killedBy", Value::Array(Vec::new()));
    };
    let one = out.join("one");
    let _ = fs::create_dir_all(&one);
    let _ = fs::write(one.join("mutants.json"), json::stringify(&Value::object(vec![("mutants", Value::Array(vec![mutant]))])));
    let mut args = vec![
        "--package".into(),
        package.display().to_string(),
        "--patches".into(),
        one.display().to_string(),
        "--out".into(),
        out.display().to_string(),
        "--timeout-ms".into(),
        flags.get("timeout-ms").or_else(|| flags.get("suite-timeout-ms")).unwrap_or("60000").to_string(),
    ];
    if let Some(suite) = flags.get("suite-command") {
        args.push("--suite-command".into());
        args.push(suite.to_string());
    }
    crate::runner::forward_run_flags(&mut args, flags);
    let report = crate::runner::run(&Flags::parse(&args));
    let suite_green = fs::read_to_string(out.join("baseline.json"))
        .ok()
        .and_then(|text| json::parse(&text).ok())
        .and_then(|value| value.get("judgement").and_then(Value::as_str).map(|item| item == "green"))
        .unwrap_or(false);
    if !report.ok {
        return Envelope::new(false, command, format!("{id} is not done. {}", report.summary), "The task is not done.")
            .with_extra("done", Value::Bool(false))
            .with_extra("outcome", Value::Null)
            .with_extra("suiteGreen", Value::Bool(suite_green))
            .with_extra("killedBy", Value::Array(Vec::new()));
    }
    let result = fs::read_to_string(out.join("results").join(format!("{id}.json"))).ok().and_then(|text| json::parse(&text).ok());
    let outcome = result.as_ref().and_then(|value| value.get("outcome").and_then(Value::as_str)).unwrap_or("");
    let killed_by = match result.as_ref().and_then(|value| value.get("killedBy")) {
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    let done = suite_green && outcome == "killed";
    let head = report.summary.clone();
    Envelope::new(
        done,
        command,
        if done { format!("{head} {id} is dead and the suite is green.") } else { format!("{head} {id} is not done.") },
        if done { "The gap-fixing task is done." } else { "The task is not done." },
    )
    .with_extra("done", Value::Bool(done))
    .with_extra("outcome", if outcome.is_empty() { Value::Null } else { Value::String(outcome.into()) })
    .with_extra("cause", Value::String(if done { "test" } else { "" }.into()))
    .with_extra("killedBy", Value::Array(killed_by.into_iter().map(Value::String).collect()))
    .with_extra("suiteGreen", Value::Bool(suite_green))
}

struct Label {
    bug_id: String,
    issue_id: String,
    expected_fail: String,
}

fn read_label(path: &Path) -> Result<Label, String> {
    let text = fs::read_to_string(path).map_err(|_| "label file could not be read".to_string())?;
    let bug_id = field(&text, "bug_id");
    let issue_id = field(&text, "issue_id");
    let expected_fail = field(&text, "expected_fail");
    if bug_id.is_empty() || issue_id.is_empty() || expected_fail.is_empty() {
        return Err("label file needs bug_id, issue_id, and expected_fail".into());
    }
    Ok(Label { bug_id, issue_id, expected_fail })
}

fn field(text: &str, key: &str) -> String {
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix(&format!("{key}=")) {
            return rest.trim().to_string();
        }
    }
    String::new()
}

fn leaked(body: &str, label: &Label) -> bool {
    body.contains(&label.bug_id) || body.contains(&label.issue_id) || body.contains(&label.expected_fail)
}

fn tree_leaked(dir: &Path, label: &Label) -> bool {
    let Ok(entries) = fs::read_dir(dir) else { return false };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if tree_leaked(&path, label) {
                return true;
            }
            continue;
        }
        let Ok(meta) = path.metadata() else { continue };
        if meta.len() > 2_000_000 {
            continue;
        }
        if let Ok(text) = fs::read_to_string(&path) {
            if leaked(&text, label) {
                return true;
            }
        }
    }
    false
}

fn find_mutant(dir: &Path, id: &str) -> Option<Value> {
    let file = if dir.is_file() { dir.to_path_buf() } else { dir.join("mutants.json") };
    let text = fs::read_to_string(file).ok()?;
    let value = json::parse(&text).ok()?;
    let items = value.get("mutants")?;
    let Value::Array(items) = items else { return None };
    items.iter().find(|item| item.get("id").and_then(Value::as_str) == Some(id)).cloned()
}

struct Edited {
    files: Vec<String>,
    older: bool,
}

fn killed_names(out: &Path) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let Ok(read) = fs::read_dir(out.join("results")) else { return rows };
    for entry in read.flatten() {
        let Ok(text) = fs::read_to_string(entry.path()) else { continue };
        let Ok(value) = json::parse(&text) else { continue };
        if value.get("outcome").and_then(Value::as_str) != Some("killed") {
            continue;
        }
        let names = match value.get("killedBy") {
            Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).map(str::to_string).collect(),
            _ => Vec::new(),
        };
        rows.push(names);
    }
    rows
}

fn fix_edited_killers(repo: &Path, commit: &str, hide: &[String], kills: &[Vec<String>]) -> Edited {
    if kills.is_empty() {
        return Edited { files: Vec::new(), older: false };
    }
    let diff = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["diff", "--name-only", &format!("{commit}^"), commit])
        .output();
    let Ok(diff) = diff else {
        return Edited { files: Vec::new(), older: true };
    };
    if !diff.status.success() {
        return Edited { files: Vec::new(), older: true };
    }
    let hidden: Vec<String> = hide.iter().map(|rel| rel.replace('\\', "/")).collect();
    let edited: Vec<String> = String::from_utf8_lossy(&diff.stdout)
        .lines()
        .map(|line| line.trim().replace('\\', "/"))
        .filter(|line| !line.is_empty() && !hidden.iter().any(|item| item == line))
        .collect();
    let mut files = Vec::new();
    let mut older = false;
    for kill in kills {
        if kill.is_empty() {
            older = true;
            continue;
        }
        for id in kill {
            if let Some(file) = edited.iter().find(|file| killer_in_file(id, file, repo, commit)) {
                if !files.contains(file) {
                    files.push(file.clone());
                }
            } else {
                older = true;
            }
        }
    }
    files.sort();
    Edited { files, older }
}

fn killer_in_file(id: &str, rel: &str, repo: &Path, commit: &str) -> bool {
    if id.contains(rel) {
        return true;
    }
    let base = Path::new(rel).file_name().and_then(|name| name.to_str()).unwrap_or(rel);
    if id.starts_with(&format!("{base}::")) {
        return true;
    }
    let stem = base.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(base);
    if !stem.is_empty() && id.split(['.', '#', ':', '/']).any(|part| part == stem) {
        return true;
    }
    let leaf = id.rsplit("::").next().unwrap_or(id).rsplit('#').next().unwrap_or(id);
    let leaf = leaf.trim();
    if leaf.len() < 4 {
        return false;
    }
    let shown = Command::new("git").arg("-C").arg(repo).args(["show", &format!("{commit}:{rel}")]).output();
    let Ok(shown) = shown else { return false };
    if !shown.status.success() {
        return false;
    }
    let body = String::from_utf8_lossy(&shown.stdout);
    body.contains(&format!("\"{leaf}\""))
        || body.contains(&format!("'{leaf}'"))
        || body.contains(&format!("`{leaf}`"))
        || body.contains(&format!("{leaf}("))
        || body.contains(&format!("def {leaf}"))
        || body.contains(&format!("fn {leaf}"))
}

fn missing(command: &str, message: &str) -> Envelope {
    Envelope::new(false, command, message.to_string(), "Pass the missing flag and run the command again.")
        .with_extra("done", Value::Bool(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_label_without_the_three_fields_is_not_a_success() {
        let dir = crate::testdir::TempDir::new("sealed");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("label.txt"), "bug_id=only\n").unwrap();
        let envelope = sealed(&Flags::parse(&[
            "--package".into(),
            dir.display().to_string(),
            "--out".into(),
            dir.join("out").display().to_string(),
            "--label".into(),
            dir.join("label.txt").display().to_string(),
        ]));
        assert!(!envelope.ok, "{}", envelope.summary);
        assert!(envelope.summary.contains("bug_id"), "{}", envelope.summary);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn check_kill_without_a_patch_is_not_done() {
        let dir = crate::testdir::TempDir::new("ck");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::create_dir_all(dir.join("patches")).unwrap();
        fs::write(dir.join("patches/mutants.json"), "{\"mutants\":[]}\n").unwrap();
        let envelope = check_kill(&Flags::parse(&[
            "m-missing".into(),
            "--package".into(),
            dir.display().to_string(),
            "--patches".into(),
            dir.join("patches").display().to_string(),
            "--out".into(),
            dir.join("out").display().to_string(),
        ]));
        assert!(!envelope.ok, "{}", envelope.summary);
        assert!(envelope.summary.contains("patch is missing"), "{}", envelope.summary);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_fix_that_edited_the_killing_test_is_a_miss() {
        let root = crate::testdir::TempDir::new("fix");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("tests")).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git").arg("-C").arg(&root).args(args).status().unwrap();
        };
        git(&["init"]);
        git(&["config", "user.email", "a@b.c"]);
        git(&["config", "user.name", "A"]);
        fs::write(root.join("tests/old.test.js"), "test('keeps the old gate', () => {});\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "base"]);
        fs::write(root.join("tests/old.test.js"), "test('keeps the new gate', () => {});\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "fix"]);
        let edited = fix_edited_killers(&root, "HEAD", &[], &[vec!["tests/old.test.js::keeps the new gate".into()]]);
        assert!(!edited.older, "{:?}", edited.files);
        assert_eq!(edited.files, ["tests/old.test.js"]);
        let kept = fix_edited_killers(&root, "HEAD", &[], &[vec!["tests/untouched.test.js::other".into()]]);
        assert!(kept.older);
        let _ = fs::remove_dir_all(&root);
    }
}
