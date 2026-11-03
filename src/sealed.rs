//! Sealed runs and check-kill. A hidden label must stay out of the report, and a kill is a real rerun.

use std::fs;
use std::path::{Path, PathBuf};

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
    if !flags.bool("confirm", true) {
        args.push("--no-confirm".into());
    }
    if let Some(suite) = flags.get("suite-command") {
        args.push("--suite-command".into());
        args.push(suite.to_string());
    }
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

fn missing(command: &str, message: &str) -> Envelope {
    Envelope::new(false, command, message.to_string(), "Pass the missing flag and run the command again.")
        .with_extra("done", Value::Bool(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_label_without_the_three_fields_is_not_a_success() {
        let dir = std::env::temp_dir().join(format!("probatio-sealed-{}", std::process::id()));
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
        let dir = std::env::temp_dir().join(format!("probatio-ck-{}", std::process::id()));
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
}
