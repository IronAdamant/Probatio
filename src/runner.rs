//! `mutate run`. A red baseline, a missing report, a timeout, and a mutant that did not build
//! never become a kill count. A large batch with no line map stops before the first mutant.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::args::Flags;
use crate::decide::{self, Baseline, Counts, Selection, Verdict};
use crate::envelope::Envelope;
use crate::json::{self, Value};
use crate::operators;
use crate::process::{self, TimedRun};
use crate::worktree;

const DEFAULT_TIMEOUT_MS: u64 = 120_000;

#[derive(Clone, Debug)]
struct LoadedMutant {
    id: String,
    file: String,
    start: usize,
    end: usize,
    original: String,
    replacement: String,
    tests: Vec<String>,
}

#[derive(Clone, Debug)]
struct Scored {
    kind: Verdict,
    duration_ms: u64,
    timed_out: bool,
}

#[derive(Clone, Debug)]
struct BaselineRun {
    kind: Baseline,
    duration_ms: Option<u64>,
}

pub fn run(flags: &Flags) -> Envelope {
    let command = "mutate.run";
    let package = match flags.need("package") {
        Ok(value) => PathBuf::from(value),
        Err(message) => return missing(command, &message),
    };
    let patches = match flags.need("patches") {
        Ok(value) => PathBuf::from(value),
        Err(message) => return missing(command, &message),
    };
    let out = match flags.need("out") {
        Ok(value) => PathBuf::from(value),
        Err(message) => return missing(command, &message),
    };
    if !package.is_dir() {
        return missing(command, "package is not a directory");
    }
    let suite = match suite_command(flags, &package) {
        Ok(value) => value,
        Err(message) => {
            return Envelope::new(
                false,
                command,
                message,
                "Pass --suite-command for a layout this binary does not discover.",
            )
        }
    };
    let mutants = match load_mutants(&patches) {
        Ok(value) => value,
        Err(message) => {
            return Envelope::new(
                false,
                command,
                message,
                "generate writes mutants.json. Point --patches at that directory or file.",
            )
        }
    };
    if mutants.is_empty() {
        return Envelope::new(false, command, "No mutants in that patch set.", "Nothing was mutated.");
    }
    if let Err(error) = fs::create_dir_all(&out) {
        return Envelope::new(false, command, error.to_string(), "The out directory could not be created.");
    }

    let timeout = Duration::from_millis(flags.int("timeout-ms", DEFAULT_TIMEOUT_MS).max(1));
    let max_selected = selection_cap(flags);
    let has_line_map = flags.get("line-map").is_some_and(usable_line_map);
    let shared_tests = flags.get("tests").and_then(|path| read_test_list(Path::new(path)));

    let baseline = match prepared_baseline(&package, &out, &suite, timeout) {
        Ok(value) => value,
        Err(envelope) => return envelope,
    };
    if let Err(error) = write_baseline(&out, &baseline) {
        return Envelope::new(false, command, error, "The baseline record could not be written.");
    }
    if let Some((summary, next)) = decide::baseline_refusal(baseline.kind) {
        return Envelope::new(false, command, summary, next);
    }

    let pending: Vec<&LoadedMutant> = mutants.iter().filter(|mutant| !result_path(&out, &mutant.id).exists()).collect();
    if let Some(message) = decide::scale_stop(has_line_map, baseline.duration_ms, pending.len()) {
        let ids = pending.iter().map(|mutant| mutant.id.as_str()).collect::<Vec<_>>().join("\n");
        let _ = fs::write(out.join("not-started.txt"), format!("{ids}\n"));
        return Envelope::new(false, command, message, "The ids are in not-started.txt. Nothing was started.");
    }

    let git = worktree::is_git_repo(&package);
    let run_id = worktree::run_id_for(&out);
    if git {
        let _ = worktree::remove_leftovers(&package, &run_id);
    }
    if let Err(error) = fs::create_dir_all(out.join("results")) {
        return Envelope::new(false, command, error.to_string(), "The results directory could not be created.");
    }

    let mut verdicts = Vec::new();
    let mut any_capped = false;
    for mutant in &mutants {
        let path = result_path(&out, &mutant.id);
        if path.exists() {
            if let Some(kind) = verdict_of_file(&path) {
                verdicts.push(kind);
            }
            continue;
        }
        let tests = if mutant.tests.is_empty() {
            shared_tests.clone().unwrap_or_default()
        } else {
            mutant.tests.clone()
        };
        let selection = decide::cap_tests(&tests, max_selected);
        if selection.capped {
            any_capped = true;
        }
        let scored = if git {
            score_git(&package, &run_id, mutant, &suite, timeout)
        } else {
            score_copy(&package, mutant, &suite, timeout)
        };
        if let Err(error) = write_result(&path, mutant, &scored, &selection, max_selected) {
            return Envelope::new(false, command, error, "A mutant result could not be written.");
        }
        verdicts.push(scored.kind);
    }

    let counts = decide::counts_of(&verdicts);
    let summary = decide::campaign_summary(&counts);
    let mut next = if any_capped {
        decide::cap_note(max_selected)
    } else {
        "Each mutant is a file under results/. A kill is an assertion failure.".into()
    };
    if flags.int("workers", 1) > 1 {
        next.push_str(" Workers stay at 1.");
    }
    Envelope::new(true, command, summary, next)
        .with_extra("counts", counts_value(&counts))
        .with_extra("finished", Value::Int(counts.finished as i64))
}

fn missing(command: &str, message: &str) -> Envelope {
    Envelope::new(false, command, message.to_string(), "Pass the missing flag and run the command again.")
}

fn selection_cap(flags: &Flags) -> usize {
    let raw = flags.int("max-selected", decide::DEFAULT_MAX_SELECTED as u64);
    if raw == 0 { decide::DEFAULT_MAX_SELECTED } else { raw as usize }
}

pub fn suite_command(flags: &Flags, package: &Path) -> Result<String, String> {
    if let Some(command) = flags.get("suite-command") {
        if command.trim().is_empty() {
            return Err("missing --suite-command".into());
        }
        return Ok(command.to_string());
    }
    discover(package)
}

/// Name the suite command from marker files. This does not launch the tool.
pub fn discover(package: &Path) -> Result<String, String> {
    if package.join("go.mod").is_file() {
        Ok("go test ./...".into())
    } else if package.join("Cargo.toml").is_file() {
        Ok("cargo test".into())
    } else if package.join("pom.xml").is_file() {
        Ok("mvn -B test".into())
    } else if package.join("package.json").is_file() {
        Ok("node --test".into())
    } else if package.join("pytest.ini").is_file() || package.join("pyproject.toml").is_file() {
        Ok("python3 -m pytest".into())
    } else {
        Err("This layout has no suite Probatio can see.".into())
    }
}

fn usable_line_map(path: &str) -> bool {
    fs::read_to_string(path).ok().is_some_and(|text| !text.trim().is_empty())
}

fn read_test_list(path: &Path) -> Option<Vec<String>> {
    let text = fs::read_to_string(path).ok()?;
    let names = text.lines().map(str::trim).filter(|line| !line.is_empty()).map(str::to_string).collect::<Vec<_>>();
    Some(names)
}

fn prepared_baseline(package: &Path, out: &Path, suite: &str, timeout: Duration) -> Result<BaselineRun, Envelope> {
    let path = out.join("baseline.json");
    if let Some(saved) = read_baseline(&path) {
        if saved.kind == Baseline::Green {
            return Ok(saved);
        }
    }
    let timed = process::run_shell(suite, Some(package), timeout);
    Ok(baseline_from(&timed))
}

fn baseline_from(timed: &TimedRun) -> BaselineRun {
    BaselineRun {
        kind: decide::judge_baseline(timed.spawned, timed.timed_out, timed.status, &timed.stdout, &timed.stderr),
        duration_ms: Some(duration_ms(timed.duration)),
    }
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn write_baseline(out: &Path, baseline: &BaselineRun) -> Result<(), String> {
    let judgement = match baseline.kind {
        Baseline::Green => "green",
        Baseline::Red => "red",
        Baseline::NoReport => "no-report",
        Baseline::TimedOut => "timed-out",
        Baseline::MissingTool => "missing-tool",
    };
    let duration = match baseline.duration_ms {
        Some(ms) => Value::Int(ms as i64),
        None => Value::Null,
    };
    let value = Value::object(vec![
        ("judgement", Value::String(judgement.into())),
        ("durationMs", duration),
    ]);
    fs::write(out.join("baseline.json"), json::stringify(&value)).map_err(|error| error.to_string())
}

fn read_baseline(path: &Path) -> Option<BaselineRun> {
    let text = fs::read_to_string(path).ok()?;
    let value = json::parse(&text).ok()?;
    let judgement = value.get("judgement").and_then(Value::as_str)?;
    let kind = match judgement {
        "green" => Baseline::Green,
        "red" => Baseline::Red,
        "no-report" => Baseline::NoReport,
        "timed-out" => Baseline::TimedOut,
        "missing-tool" => Baseline::MissingTool,
        _ => return None,
    };
    let duration_ms = value.get("durationMs").and_then(Value::as_int).map(|ms| ms as u64);
    Some(BaselineRun { kind, duration_ms })
}

fn result_path(out: &Path, id: &str) -> PathBuf {
    out.join("results").join(format!("{id}.json"))
}

fn score_git(package: &Path, run_id: &str, mutant: &LoadedMutant, suite: &str, timeout: Duration) -> Scored {
    let dest = worktree::worktree_dest(run_id);
    if let Err(message) = worktree::add_worktree(package, &dest, run_id) {
        return error_scored(message);
    }
    let scored = apply_and_run(&dest, mutant, suite, timeout);
    worktree::remove_worktree(package, &dest);
    scored
}

fn score_copy(package: &Path, mutant: &LoadedMutant, suite: &str, timeout: Duration) -> Scored {
    let dest = std::env::temp_dir().join(format!("probatio-copy-{}-{}", worktree::run_id_for(package), mutant.id));
    let _ = fs::remove_dir_all(&dest);
    if let Err(message) = copy_tree(package, &dest) {
        return error_scored(message);
    }
    let scored = apply_and_run(&dest, mutant, suite, timeout);
    let _ = fs::remove_dir_all(&dest);
    scored
}

fn apply_and_run(root: &Path, mutant: &LoadedMutant, suite: &str, timeout: Duration) -> Scored {
    let file = match safe_join(root, &mutant.file) {
        Ok(path) => path,
        Err(message) => return error_scored(message),
    };
    let text = match fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) => return error_scored(error.to_string()),
    };
    if mutant.start > mutant.end || mutant.end > text.len() || !text.is_char_boundary(mutant.start) || !text.is_char_boundary(mutant.end) {
        return error_scored("mutant range does not fit the file".into());
    }
    if !mutant.original.is_empty() && text[mutant.start..mutant.end] != mutant.original {
        return error_scored("mutant original text does not match".into());
    }
    let updated = operators::apply(&text, mutant.start, mutant.end, &mutant.replacement);
    if let Err(error) = fs::write(&file, updated) {
        return error_scored(error.to_string());
    }
    let timed = process::run_shell(suite, Some(root), timeout);
    let report = format!("{}\n{}", timed.stdout, timed.stderr);
    Scored {
        kind: decide::classify(timed.timed_out, timed.status, &report),
        duration_ms: duration_ms(timed.duration),
        timed_out: timed.timed_out,
    }
}

fn error_scored(message: String) -> Scored {
    let _ = message;
    Scored { kind: Verdict::Error, duration_ms: 0, timed_out: false }
}

fn safe_join(root: &Path, file: &str) -> Result<PathBuf, String> {
    let path = Path::new(file);
    if file.contains("..") || path.is_absolute() {
        return Err("mutant path leaves the package".into());
    }
    Ok(root.join(path))
}

fn copy_tree(src: &Path, dest: &Path) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|error| error.to_string())?;
    for entry in fs::read_dir(src).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == ".git" || name == "target" || name == "node_modules" || name == "dist" {
            continue;
        }
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        if file_type.is_symlink() {
            continue;
        }
        let to = dest.join(entry.file_name());
        if file_type.is_dir() {
            copy_tree(&entry.path(), &to)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), &to).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn write_result(path: &Path, mutant: &LoadedMutant, scored: &Scored, selection: &Selection, max_selected: usize) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut entries = vec![
        ("id", Value::String(mutant.id.clone())),
        ("outcome", Value::String(scored.kind.as_str().into())),
        ("selectedCount", Value::Int(selection.selected_count as i64)),
        ("selectionCapped", Value::Bool(selection.capped)),
        ("durationMs", Value::Int(scored.duration_ms as i64)),
        ("timedOut", Value::Bool(scored.timed_out)),
        ("tests", Value::Array(selection.tests.iter().cloned().map(Value::String).collect())),
    ];
    if selection.capped {
        entries.push(("capNote", Value::String(decide::cap_note(max_selected))));
    }
    fs::write(path, json::stringify(&Value::Object(entries.into_iter().map(|(k, v)| (k.to_string(), v)).collect())))
        .map_err(|error| error.to_string())
}

fn verdict_of_file(path: &Path) -> Option<Verdict> {
    let text = fs::read_to_string(path).ok()?;
    let value = json::parse(&text).ok()?;
    match value.get("outcome").and_then(Value::as_str)? {
        "killed" => Some(Verdict::Killed),
        "survived" => Some(Verdict::Survived),
        "no coverage" => Some(Verdict::NoCoverage),
        "unviable" => Some(Verdict::Unviable),
        "timeout" => Some(Verdict::Timeout),
        "flaky" => Some(Verdict::Flaky),
        "error" => Some(Verdict::Error),
        _ => None,
    }
}

pub fn verdicts_in(out: &Path) -> Vec<(Verdict, Vec<String>)> {
    let dir = out.join("results");
    let Ok(entries) = fs::read_dir(&dir) else { return Vec::new() };
    let mut files = entries.filter_map(|entry| entry.ok()).map(|entry| entry.path()).collect::<Vec<_>>();
    files.sort();
    let mut found = Vec::new();
    for path in files {
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else { continue };
        let Ok(value) = json::parse(&text) else { continue };
        let Some(kind) = value.get("outcome").and_then(Value::as_str).and_then(parse_verdict) else { continue };
        let tests = match value.get("tests") {
            Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).map(str::to_string).collect(),
            _ => Vec::new(),
        };
        found.push((kind, tests));
    }
    found
}

fn parse_verdict(text: &str) -> Option<Verdict> {
    match text {
        "killed" => Some(Verdict::Killed),
        "survived" => Some(Verdict::Survived),
        "no coverage" => Some(Verdict::NoCoverage),
        "unviable" => Some(Verdict::Unviable),
        "timeout" => Some(Verdict::Timeout),
        "flaky" => Some(Verdict::Flaky),
        "error" => Some(Verdict::Error),
        _ => None,
    }
}

fn counts_value(counts: &Counts) -> Value {
    Value::object(vec![
        ("noCoverage", Value::Int(counts.no_coverage as i64)),
        ("survived", Value::Int(counts.survived as i64)),
        ("killed", Value::Int(counts.killed as i64)),
        ("unviable", Value::Int(counts.unviable as i64)),
        ("flaky", Value::Int(counts.flaky as i64)),
        ("timedOut", Value::Int(counts.timeouts as i64)),
        ("errored", Value::Int(counts.errors as i64)),
        ("finished", Value::Int(counts.finished as i64)),
    ])
}

fn load_mutants(patches: &Path) -> Result<Vec<LoadedMutant>, String> {
    if patches.is_file() {
        return load_file(patches);
    }
    if !patches.is_dir() {
        return Err("patches is not a file or directory".into());
    }
    let combined = patches.join("mutants.json");
    if combined.is_file() {
        return load_file(&combined);
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(patches).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if entry.path().extension().and_then(|ext| ext.to_str()) == Some("json") {
            files.push(entry.path());
        }
    }
    files.sort();
    let mut mutants = Vec::new();
    for file in files {
        mutants.extend(load_file(&file)?);
    }
    Ok(mutants)
}

fn load_file(path: &Path) -> Result<Vec<LoadedMutant>, String> {
    let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let value = json::parse(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    let items = match &value {
        Value::Array(_) => &value,
        Value::Object(_) => value.get("mutants").ok_or_else(|| format!("{} has no mutants", path.display()))?,
        _ => return Err(format!("{} is not a mutant list", path.display())),
    };
    let Value::Array(items) = items else {
        return Err(format!("{} mutants is not a list", path.display()));
    };
    items.iter().map(load_one).collect()
}

fn load_one(value: &Value) -> Result<LoadedMutant, String> {
    let field = |key: &str| value.get(key).and_then(Value::as_str).ok_or_else(|| format!("mutant is missing {key}"));
    let number = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_int)
            .ok_or_else(|| format!("mutant is missing {key}"))
            .and_then(|raw| usize::try_from(raw).map_err(|_| format!("mutant {key} is negative")))
    };
    let tests = match value.get("tests") {
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        _ => Vec::new(),
    };
    Ok(LoadedMutant {
        id: field("id")?.to_string(),
        file: field("file")?.to_string(),
        start: number("start")?,
        end: number("end")?,
        original: value.get("original").and_then(Value::as_str).unwrap_or("").to_string(),
        replacement: field("replacement")?.to_string(),
        tests,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generate;
    use std::process::Command;

    fn temp(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("probatio-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(path.join("src")).unwrap();
        path
    }

    fn write_source(package: &Path, body: &str) {
        fs::write(package.join("src/A.java"), body).unwrap();
    }

    fn one_and() -> &'static str {
        "class A {\n  boolean f(boolean a, boolean b) { return a && b; }\n}\n"
    }

    fn many_ands(count: usize) -> String {
        let mut body = String::from("class A {\n  boolean f(boolean a, boolean b) {\n");
        for index in 0..count {
            body.push_str(&format!("    boolean x{index} = a && b;\n"));
        }
        body.push_str("    return a;\n  }\n}\n");
        body
    }

    fn generate_into(package: &Path) -> PathBuf {
        let patches = package.join("patches");
        let args = vec!["--package".into(), package.display().to_string(), "--out".into(), patches.display().to_string()];
        let envelope = generate::generate(&Flags::parse(&args));
        assert!(envelope.ok, "{}", envelope.summary);
        patches
    }

    fn run_with(package: &Path, patches: &Path, out: &Path, extra: &[&str]) -> Envelope {
        let mut args = vec![
            "--package".into(),
            package.display().to_string(),
            "--patches".into(),
            patches.display().to_string(),
            "--out".into(),
            out.display().to_string(),
            "--timeout-ms".into(),
            "10000".into(),
        ];
        args.extend(extra.iter().map(|arg| (*arg).to_string()));
        run(&Flags::parse(&args))
    }

    fn parsed(envelope: &Envelope) -> Value {
        json::parse(&envelope.render("")).unwrap()
    }

    fn assert_no_kill_count(envelope: &Envelope) {
        assert!(!envelope.ok);
        assert!(!envelope.summary.contains("killed"), "{}", envelope.summary);
        assert!(!envelope.next.contains("killed"), "{}", envelope.next);
        let value = parsed(envelope);
        assert!(value.get("counts").is_none());
        assert!(value.get("killed").is_none());
        assert!(value.get("kills").is_none());
    }

    #[test]
    fn discovery_names_a_suite_without_launching_it() {
        let package = temp("discover");
        fs::write(package.join("pom.xml"), "<project/>").unwrap();
        assert_eq!(discover(&package).unwrap(), "mvn -B test");
        fs::write(package.join("go.mod"), "module example\n").unwrap();
        assert_eq!(discover(&package).unwrap(), "go test ./...");
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn run_refuses_a_red_baseline_with_no_kill_count() {
        let package = temp("red");
        write_source(&package, one_and());
        let patches = generate_into(&package);
        let out = package.join("out");
        let envelope = run_with(&package, &patches, &out, &["--suite-command", "echo fail; exit 1"]);
        assert_no_kill_count(&envelope);
        assert!(envelope.summary.contains("already failing"));
        assert!(fs::read_dir(out.join("results")).is_err() || fs::read_dir(out.join("results")).unwrap().next().is_none());
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn run_refuses_a_missing_test_report() {
        let package = temp("noreport");
        write_source(&package, one_and());
        let patches = generate_into(&package);
        let out = package.join("out");
        let envelope = run_with(&package, &patches, &out, &["--suite-command", "exit 0"]);
        assert_no_kill_count(&envelope);
        assert!(envelope.summary.contains("test report"), "{}", envelope.summary);
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn run_compile_failure_is_not_a_kill() {
        let package = temp("unviable");
        write_source(&package, one_and());
        let patches = generate_into(&package);
        let out = package.join("out");
        let script = "if grep -q '||' src/A.java; then echo 'error: cannot find symbol Widget' >&2; exit 1; else echo ok; fi";
        let envelope = run_with(&package, &patches, &out, &["--suite-command", script]);
        assert!(envelope.ok, "{}", envelope.summary);
        let value = parsed(&envelope);
        assert_eq!(value.get("counts").and_then(|counts| counts.get("killed")).and_then(Value::as_int), Some(0));
        assert_eq!(value.get("counts").and_then(|counts| counts.get("unviable")).and_then(Value::as_int), Some(1));
        let result = fs::read_dir(out.join("results")).unwrap().next().unwrap().unwrap().path();
        let body = json::parse(&fs::read_to_string(result).unwrap()).unwrap();
        assert_eq!(body.get("outcome").and_then(Value::as_str), Some("unviable"));
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn run_go_build_failure_is_not_a_kill() {
        let package = temp("gobuild");
        write_source(&package, one_and());
        let patches = generate_into(&package);
        let out = package.join("out");
        let script = "if grep -q '||' src/A.java; then printf '%s\\n' './main.go:3:6: undefined: nope' 'FAIL demo [build failed]' >&2; exit 1; else echo ok; fi";
        let envelope = run_with(&package, &patches, &out, &["--suite-command", script]);
        assert!(envelope.ok, "{}", envelope.summary);
        let value = parsed(&envelope);
        assert_eq!(value.get("counts").and_then(|counts| counts.get("killed")).and_then(Value::as_int), Some(0));
        assert_eq!(value.get("counts").and_then(|counts| counts.get("unviable")).and_then(Value::as_int), Some(1));
        let result = fs::read_dir(out.join("results")).unwrap().next().unwrap().unwrap().path();
        let body = json::parse(&fs::read_to_string(result).unwrap()).unwrap();
        assert_eq!(body.get("outcome").and_then(Value::as_str), Some("unviable"));
        assert_ne!(body.get("outcome").and_then(Value::as_str), Some("killed"));
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn run_timeout_is_not_a_kill() {
        let package = temp("timeout");
        write_source(&package, one_and());
        let patches = generate_into(&package);
        let out = package.join("out");
        let script = "if grep -q '||' src/A.java; then sleep 30; else echo ok; fi";
        let args = vec![
            "--package".into(),
            package.display().to_string(),
            "--patches".into(),
            patches.display().to_string(),
            "--out".into(),
            out.display().to_string(),
            "--timeout-ms".into(),
            "400".into(),
            "--suite-command".into(),
            script.into(),
        ];
        let envelope = run(&Flags::parse(&args));
        assert!(envelope.ok, "{}", envelope.summary);
        let value = parsed(&envelope);
        assert_eq!(value.get("counts").and_then(|counts| counts.get("killed")).and_then(Value::as_int), Some(0));
        assert_eq!(value.get("counts").and_then(|counts| counts.get("timedOut")).and_then(Value::as_int), Some(1));
        let result = fs::read_dir(out.join("results")).unwrap().next().unwrap().unwrap().path();
        let body = json::parse(&fs::read_to_string(&result).unwrap()).unwrap();
        assert_eq!(body.get("outcome").and_then(Value::as_str), Some("timeout"));
        let duration = body.get("durationMs").and_then(Value::as_int).unwrap() as u64;
        assert!(process::within_grace(Duration::from_millis(duration), Duration::from_millis(400)), "{duration}");
        drop(args);
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn run_caps_a_selection_over_thirty() {
        let package = temp("cap");
        write_source(&package, one_and());
        let patches = generate_into(&package);
        let tests = package.join("tests.txt");
        let names = (0..40).map(|index| format!("test{index}")).collect::<Vec<_>>().join("\n");
        fs::write(&tests, names).unwrap();
        let out = package.join("out");
        let envelope = run_with(
            &package,
            &patches,
            &out,
            &["--suite-command", "echo ok", "--tests", &tests.display().to_string()],
        );
        assert!(envelope.ok, "{}", envelope.summary);
        assert!(envelope.next.contains("capped"), "{}", envelope.next);
        let result = fs::read_dir(out.join("results")).unwrap().next().unwrap().unwrap().path();
        let body = json::parse(&fs::read_to_string(result).unwrap()).unwrap();
        assert_eq!(body.get("selectionCapped").and_then(Value::as_bool), Some(true));
        assert_eq!(body.get("selectedCount").and_then(Value::as_int), Some(30));
        assert!(body.get("capNote").and_then(Value::as_str).unwrap().contains("30"));
        let rendered = envelope.render("");
        assert!(!rendered.contains("test39"));
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn run_stops_an_unmapped_batch_before_the_first_mutant() {
        let package = temp("unmap");
        write_source(&package, &many_ands(31));
        let patches = generate_into(&package);
        let log = package.join("runs.log");
        let script = format!("echo ran >> '{}'; echo ok", log.display());
        let out = package.join("out");
        let started = std::time::Instant::now();
        let envelope = run_with(&package, &patches, &out, &["--suite-command", &script]);
        assert!(started.elapsed() < Duration::from_secs(5), "baseline took {:?}", started.elapsed());
        assert_no_kill_count(&envelope);
        assert!(envelope.summary.contains("No line map"), "{}", envelope.summary);
        assert!(envelope.summary.contains("31"), "{}", envelope.summary);
        assert!(envelope.next.contains("not-started.txt"));
        assert!(!envelope.render("").contains("\"notStarted\""));
        let ids = fs::read_to_string(out.join("not-started.txt")).unwrap();
        assert_eq!(ids.lines().count(), 31);
        assert!(fs::read_dir(out.join("results")).is_err());
        let ran = fs::read_to_string(&log).unwrap_or_default();
        assert_eq!(ran.lines().count(), 1, "a mutant ran: {ran}");
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn a_line_map_lets_a_large_batch_start() {
        let package = temp("mapped");
        write_source(&package, &many_ands(31));
        let patches = generate_into(&package);
        let map = package.join("lines.json");
        fs::write(&map, "{\"lines\":[1]}\n").unwrap();
        let out = package.join("out");
        let envelope = run_with(
            &package,
            &patches,
            &out,
            &["--suite-command", "echo ok", "--line-map", &map.display().to_string()],
        );
        assert!(envelope.ok, "{}", envelope.summary);
        assert_eq!(fs::read_dir(out.join("results")).unwrap().count(), 31);
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn resume_skips_a_finished_mutant_and_a_green_baseline() {
        let package = temp("resume");
        write_source(&package, one_and());
        let patches = generate_into(&package);
        let out = package.join("out");
        let log = package.join("runs.log");
        let script = format!("echo ran >> '{}'; echo ok", log.display());
        let first = run_with(&package, &patches, &out, &["--suite-command", &script]);
        assert!(first.ok, "{}", first.summary);
        assert_eq!(fs::read_to_string(&log).unwrap().lines().count(), 2);
        let second = run_with(&package, &patches, &out, &["--suite-command", &script]);
        assert!(second.ok, "{}", second.summary);
        assert_eq!(fs::read_to_string(&log).unwrap().lines().count(), 2, "resume ran the suite again");
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn run_removes_a_leftover_worktree_before_the_mutant() {
        let package = temp("wt");
        write_source(&package, one_and());
        git(&package, &["init"]);
        git(&package, &["add", "src/A.java"]);
        git_commit(&package, &["commit", "-m", "init"]);
        let patches = generate_into(&package);
        let out = package.join("out");
        fs::create_dir_all(&out).unwrap();
        let run_id = worktree::run_id_for(&out);
        let dest = worktree::worktree_dest(&run_id);
        let _ = fs::remove_dir_all(&dest);
        git(&package, &["worktree", "add", "--detach", &dest.display().to_string(), "HEAD"]);
        fs::write(dest.join("LEFTOVER"), "stale\n").unwrap();
        assert!(dest.join("LEFTOVER").exists());
        let envelope = run_with(&package, &patches, &out, &["--suite-command", "echo ok"]);
        assert!(envelope.ok, "{}", envelope.summary);
        assert!(!dest.join("LEFTOVER").exists());
        assert!(!dest.exists());
        let source = fs::read_to_string(package.join("src/A.java")).unwrap();
        assert!(source.contains("&&"), "{source}");
        assert!(!source.contains("||"));
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn run_scores_the_uncommitted_line() {
        let package = temp("dirty");
        write_source(&package, one_and());
        git(&package, &["init"]);
        git(&package, &["add", "src/A.java"]);
        git_commit(&package, &["commit", "-m", "init"]);
        let dirty = "class A {\n  boolean f(boolean a, boolean b) { return a && b && c; } // UNCOMMITTED\n}\n";
        write_source(&package, dirty);
        let patches = generate_into(&package);
        let listed = json::parse(&fs::read_to_string(patches.join("mutants.json")).unwrap()).unwrap();
        let count = match listed.get("mutants") {
            Some(Value::Array(items)) => items.len(),
            _ => 0,
        };
        assert_eq!(count, 2, "generate should see both && operators in the working tree");
        let out = package.join("out");
        let script = "grep -q UNCOMMITTED src/A.java && echo ok";
        let envelope = run_with(&package, &patches, &out, &["--suite-command", script]);
        assert!(envelope.ok, "{}", envelope.summary);
        let value = parsed(&envelope);
        assert_eq!(value.get("counts").and_then(|counts| counts.get("killed")).and_then(Value::as_int), Some(0));
        assert_eq!(value.get("counts").and_then(|counts| counts.get("errored")).and_then(Value::as_int), Some(0));
        assert_eq!(value.get("counts").and_then(|counts| counts.get("survived")).and_then(Value::as_int), Some(2));
        for entry in fs::read_dir(out.join("results")).unwrap() {
            let body = json::parse(&fs::read_to_string(entry.unwrap().path()).unwrap()).unwrap();
            assert_eq!(body.get("outcome").and_then(Value::as_str), Some("survived"));
        }
        let source = fs::read_to_string(package.join("src/A.java")).unwrap();
        assert!(source.contains("UNCOMMITTED"), "{source}");
        assert!(source.contains("&& c"), "{source}");
        let _ = fs::remove_dir_all(&package);
    }

    fn git(repo: &Path, args: &[&str]) {
        let status = Command::new("git").arg("-C").arg(repo).args(args).status().unwrap();
        assert!(status.success());
    }

    fn git_commit(repo: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(repo)
            .arg("-c")
            .arg("user.email=probatio@example.com")
            .arg("-c")
            .arg("user.name=probatio")
            .args(args)
            .status()
            .unwrap();
        assert!(status.success());
    }
}
