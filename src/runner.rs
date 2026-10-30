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
    template: Option<String>,
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
            let next = if message.contains(".probatio.json") && message.contains("not valid JSON") {
                "Fix .probatio.json and run again. Discovery was not used."
            } else if message.contains("package.json") && message.contains("not valid JSON") {
                "Fix package.json and run again. Discovery was not used."
            } else {
                "Pass --suite-command for a layout this binary does not discover."
            };
            return Envelope::new(false, command, message, next);
        }
    };
    let mutants = match load_mutants(&patches) {
        Ok(value) => value,
        Err(message) => {
            return attach_suite(
                Envelope::new(
                    false,
                    command,
                    message,
                    "generate writes mutants.json. Point --patches at that directory or file.",
                ),
                &suite,
            )
        }
    };
    if mutants.is_empty() {
        return attach_suite(
            Envelope::new(false, command, "No mutants in that patch set.", "Nothing was mutated."),
            &suite,
        );
    }
    if let Err(error) = fs::create_dir_all(&out) {
        return attach_suite(
            Envelope::new(false, command, error.to_string(), "The out directory could not be created."),
            &suite,
        );
    }

    let timeout_ms = flags.int("timeout-ms", DEFAULT_TIMEOUT_MS).max(1);
    let timeout = Duration::from_millis(timeout_ms);
    let max_selected = selection_cap(flags);
    let has_line_map = flags.get("line-map").is_some_and(usable_line_map);
    let map = match flags.get("line-map") {
        Some(path) if has_line_map => decide::parse_test_map(&fs::read_to_string(path).unwrap_or_default()),
        _ => decide::TestMap::slip(),
    };
    let shared_tests = flags.get("tests").and_then(|path| read_test_list(Path::new(path)));
    let baseline_tests = if map.chooses {
        map.all_names()
    } else {
        shared_tests.clone().unwrap_or_default()
    };
    if command_asks_for_names(&suite) && baseline_tests.is_empty() && !map.chooses {
        return attach_suite(
            Envelope::new(
                false,
                command,
                "The suite command asks for test names and none were given.",
                "Pass --tests or a files line map. Nothing was mutated.",
            ),
            &suite,
        );
    }
    let (baseline_suite, whole_suite) = match decide::render_suite(&suite, &baseline_tests) {
        Ok(rendered) => rendered,
        Err(name) => {
            return attach_suite(
                Envelope::new(
                    false,
                    command,
                    format!("The selected name {name} cannot be placed in the suite command."),
                    "Nothing was mutated with that command.",
                ),
                &suite,
            )
        }
    };

    let baseline = match prepared_baseline(&package, &out, &suite, &baseline_suite, timeout) {
        Ok(value) => value,
        Err(envelope) => return attach_suite(envelope, &suite),
    };
    if let Err(error) = write_baseline(&out, &baseline) {
        return attach_suite(
            Envelope::new(false, command, error, "The baseline record could not be written."),
            &suite,
        );
    }
    if let Some((summary, next)) = decide::baseline_refusal(baseline.kind) {
        return attach_suite(Envelope::new(false, command, summary, next), &suite);
    }

    let pending: Vec<&LoadedMutant> = mutants.iter().filter(|mutant| !result_path(&out, &mutant.id).exists()).collect();
    if let Some(message) = decide::scale_stop(has_line_map, baseline.duration_ms, pending.len()) {
        let ids = pending.iter().map(|mutant| mutant.id.as_str()).collect::<Vec<_>>().join("\n");
        let _ = fs::write(out.join("not-started.txt"), format!("{ids}\n"));
        return attach_suite(
            Envelope::new(false, command, message, "The ids are in not-started.txt. Nothing was started."),
            &suite,
        );
    }

    let confirm = match flags.get("confirm-count") {
        None => None,
        Some(text) => match text.parse::<usize>() {
            Ok(count) => Some(count),
            Err(_) => {
                return attach_suite(
                    Envelope::new(
                        false,
                        command,
                        "--confirm-count is not a count.",
                        "Pass the number of mutants that have no result yet.",
                    ),
                    &suite,
                )
            }
        },
    };
    let budget = match decide::chunk_budget(pending.len(), confirm) {
        Ok(budget) => budget,
        Err(real) => {
            return attach_suite(
                Envelope::new(
                    false,
                    command,
                    format!("--confirm-count does not match the mutants still waiting."),
                    format!("{real} mutants have no result yet. Nothing new was started."),
                ),
                &suite,
            )
        }
    };
    let batch: Vec<&LoadedMutant> = pending.into_iter().take(budget).collect();
    if whole_suite && map.chooses {
        let named = batch.iter().any(|mutant| {
            mutant.tests.is_empty() && !names_for_mutant(&package, mutant, &map, shared_tests.as_deref()).is_empty()
        });
        if named {
            return attach_suite(
                Envelope::new(
                    false,
                    command,
                    "The line map named tests and the suite command has no {tests} token.",
                    "Put {tests} or {tests,} in the suite command. Nothing was mutated.",
                ),
                &suite,
            );
        }
    }
    for mutant in &batch {
        let tests = names_for_mutant(&package, mutant, &map, shared_tests.as_deref());
        let selection = decide::cap_tests(&tests, max_selected);
        if let Err(name) = decide::render_suite(&suite, &selection.tests) {
            return attach_suite(
                Envelope::new(
                    false,
                    command,
                    format!("The selected name {name} cannot be placed in the suite command."),
                    "Nothing was mutated with that command.",
                ),
                &suite,
            );
        }
    }

    let git = worktree::is_git_repo(&package);
    let run_id = worktree::run_id_for(&out);
    if git {
        let _ = worktree::remove_leftovers(&package, &run_id);
    }
    if let Err(error) = fs::create_dir_all(out.join("results")) {
        return attach_suite(
            Envelope::new(false, command, error.to_string(), "The results directory could not be created."),
            &suite,
        );
    }

    let mut verdicts = Vec::new();
    let mut any_capped = false;
    let mut started_this_call = 0usize;
    for mutant in &mutants {
        let path = result_path(&out, &mutant.id);
        if path.exists() {
            if let Some(kind) = verdict_of_file(&path) {
                verdicts.push(kind);
            }
            continue;
        }
        if started_this_call >= budget {
            continue;
        }
        let tests = names_for_mutant(&package, mutant, &map, shared_tests.as_deref());
        if map.chooses && mutant.tests.is_empty() && tests.is_empty() {
            let scored = Scored { kind: Verdict::NoCoverage, duration_ms: 0, timed_out: false };
            let selection = Selection { tests: Vec::new(), selected_count: 0, capped: false };
            if let Err(error) = write_result(&path, mutant, &scored, &selection, max_selected, None) {
                return attach_suite(Envelope::new(false, command, error, "A mutant result could not be written."), &suite);
            }
            verdicts.push(scored.kind);
            started_this_call += 1;
            continue;
        }
        let selection = decide::cap_tests(&tests, max_selected);
        if selection.capped {
            any_capped = true;
        }
        let rendered = match decide::render_suite(&suite, &selection.tests) {
            Ok((rendered, _)) => rendered,
            Err(name) => {
                return attach_suite(
                    Envelope::new(
                        false,
                        command,
                        format!("The selected name {name} cannot be placed in the suite command."),
                        "Nothing was mutated with that command.",
                    ),
                    &suite,
                )
            }
        };
        let scored = if git {
            score_git(&package, &run_id, mutant, &rendered, timeout)
        } else {
            score_copy(&package, mutant, &rendered, timeout)
        };
        if let Err(error) = write_result(&path, mutant, &scored, &selection, max_selected, Some(&rendered)) {
            return attach_suite(Envelope::new(false, command, error, "A mutant result could not be written."), &suite);
        }
        verdicts.push(scored.kind);
        started_this_call += 1;
    }

    let still: Vec<&str> = mutants.iter().filter(|mutant| !result_path(&out, &mutant.id).exists()).map(|mutant| mutant.id.as_str()).collect();
    let waiting_path = out.join("not-started.txt");
    if still.is_empty() {
        let _ = fs::remove_file(&waiting_path);
    } else {
        let _ = fs::write(&waiting_path, format!("{}\n", still.join("\n")));
    }

    let counts = decide::counts_of(&verdicts);
    let summary = decide::campaign_summary(&counts);
    let waiting = if still.len() == 1 {
        "1 is still waiting".to_string()
    } else {
        format!("{} are still waiting", still.len())
    };
    let mut next = if !still.is_empty() {
        format!("{started_this_call} started this call. {waiting}.")
    } else if any_capped {
        decide::cap_note(max_selected)
    } else {
        "Each mutant is a file under results/. A kill is an assertion failure.".to_string()
    };
    if has_line_map && !map.chooses {
        next.push_str(" The line map allowed this batch. It did not choose tests.");
    } else if map.chooses {
        next.push_str(" The line map named tests.");
    }
    if whole_suite {
        next.push_str(" The suite ran whole.");
    }
    if flags.int("workers", 1) > 1 {
        next.push_str(" Workers stay at 1.");
    }
    let mut envelope = attach_suite(Envelope::new(true, command, summary, next), &suite)
        .with_extra("counts", counts_value(&counts))
        .with_extra("finished", Value::Int(counts.finished as i64));
    if !still.is_empty() {
        envelope.next_call = Some(resume_argv(flags, &suite, timeout_ms));
    }
    envelope
}

fn missing(command: &str, message: &str) -> Envelope {
    Envelope::new(false, command, message.to_string(), "Pass the missing flag and run the command again.")
}

fn selection_cap(flags: &Flags) -> usize {
    let raw = flags.int("max-selected", decide::DEFAULT_MAX_SELECTED as u64);
    if raw == 0 { decide::DEFAULT_MAX_SELECTED } else { raw as usize }
}

fn attach_suite(mut envelope: Envelope, suite: &str) -> Envelope {
    envelope.next = format!("{} The suite command was {}.", envelope.next, suite);
    envelope.with_extra("suiteCommand", Value::String(suite.to_string()))
}

fn command_asks_for_names(command: &str) -> bool {
    command.contains("{tests}") || command.contains("{tests,}")
}

fn names_for_mutant(package: &Path, mutant: &LoadedMutant, map: &decide::TestMap, shared: Option<&[String]>) -> Vec<String> {
    if !mutant.tests.is_empty() {
        return mutant.tests.clone();
    }
    if map.chooses {
        let text = fs::read_to_string(package.join(&mutant.file)).unwrap_or_default();
        return map.names_for(&mutant.file, decide::line_at(&text, mutant.start));
    }
    shared.unwrap_or(&[]).to_vec()
}

/// Resume argv for the same out dir. The suite command is the template, not a rendered `{tests}` line.
/// `--confirm-count` is omitted so the next call chunks again.
fn resume_argv(flags: &Flags, suite: &str, timeout_ms: u64) -> Vec<String> {
    let mut argv = vec![
        "mutate".into(),
        "run".into(),
        "--package".into(),
        flags.get("package").unwrap_or("").to_string(),
        "--patches".into(),
        flags.get("patches").unwrap_or("").to_string(),
        "--out".into(),
        flags.get("out").unwrap_or("").to_string(),
        "--suite-command".into(),
        suite.to_string(),
        "--timeout-ms".into(),
        timeout_ms.to_string(),
    ];
    if let Some(path) = flags.get("line-map") {
        argv.push("--line-map".into());
        argv.push(path.to_string());
    }
    if let Some(path) = flags.get("tests") {
        argv.push("--tests".into());
        argv.push(path.to_string());
    }
    if let Some(value) = flags.get("max-selected") {
        argv.push("--max-selected".into());
        argv.push(value.to_string());
    }
    argv
}

pub fn suite_command(flags: &Flags, package: &Path) -> Result<String, String> {
    if let Some(command) = flags.get("suite-command") {
        if command.trim().is_empty() {
            return Err("missing --suite-command".into());
        }
        return Ok(command.to_string());
    }
    if let Some(command) = project_suite(package)? {
        return Ok(command);
    }
    discover(package)
}

/// A non-empty `suiteCommand` string wins over discovery. Any other key is ignored.
/// Invalid JSON is an error so the caller does not discover a different command.
fn project_suite(package: &Path) -> Result<Option<String>, String> {
    let path = package.join(".probatio.json");
    if !path.is_file() {
        return Ok(None);
    }
    let text = fs::read_to_string(&path).map_err(|error| error.to_string())?;
    let value = match json::parse(&text) {
        Ok(value) => value,
        Err(_) => return Err(".probatio.json is not valid JSON".into()),
    };
    match value.get("suiteCommand").and_then(Value::as_str) {
        Some(command) if !command.trim().is_empty() => Ok(Some(command.to_string())),
        _ => Ok(None),
    }
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
        node_suite(package)
    } else if package.join("pytest.ini").is_file() || package.join("pyproject.toml").is_file() {
        Ok("python3 -m pytest".into())
    } else {
        Err("This layout has no suite Probatio can see.".into())
    }
}

fn node_suite(package: &Path) -> Result<String, String> {
    let path = package.join("package.json");
    let text = fs::read_to_string(&path).map_err(|error| error.to_string())?;
    let value = match json::parse(&text) {
        Ok(value) => value,
        Err(_) => return Err("package.json is not valid JSON".into()),
    };
    match value.get("scripts").and_then(|scripts| scripts.get("test")).and_then(Value::as_str) {
        Some(command) if !command.trim().is_empty() => Ok(command.to_string()),
        _ => Ok("node --test".into()),
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

fn prepared_baseline(
    package: &Path,
    out: &Path,
    template: &str,
    rendered: &str,
    timeout: Duration,
) -> Result<BaselineRun, Envelope> {
    let path = out.join("baseline.json");
    if let Some(saved) = read_baseline(&path) {
        if saved.kind == Baseline::Green && saved.template.as_deref() == Some(template) {
            return Ok(saved);
        }
    }
    let timed = process::run_shell(rendered, Some(package), timeout);
    let mut run = baseline_from(&timed);
    run.template = Some(template.to_string());
    Ok(run)
}

fn baseline_from(timed: &TimedRun) -> BaselineRun {
    BaselineRun {
        kind: decide::judge_baseline(timed.spawned, timed.timed_out, timed.status, &timed.stdout, &timed.stderr),
        duration_ms: Some(duration_ms(timed.duration)),
        template: None,
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
    let recorded = baseline.template.clone().unwrap_or_default();
    let value = Value::object(vec![
        ("judgement", Value::String(judgement.into())),
        ("durationMs", duration),
        ("command", Value::String(recorded)),
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
    let template = value.get("command").and_then(Value::as_str).map(str::to_string);
    Some(BaselineRun { kind, duration_ms, template })
}

fn result_path(out: &Path, id: &str) -> PathBuf {
    out.join("results").join(format!("{id}.json"))
}

fn score_git(package: &Path, run_id: &str, mutant: &LoadedMutant, suite: &str, timeout: Duration) -> Scored {
    let dest = worktree::worktree_dest(run_id);
    if let Err(message) = worktree::add_worktree(package, &dest, run_id) {
        return error_scored(message);
    }
    if let Err(message) = link_node_modules(package, &dest) {
        worktree::remove_worktree(package, &dest);
        return error_scored(message);
    }
    let scored = apply_and_run(&dest, package, mutant, suite, timeout);
    worktree::remove_worktree(package, &dest);
    scored
}

fn score_copy(package: &Path, mutant: &LoadedMutant, suite: &str, timeout: Duration) -> Scored {
    let dest = std::env::temp_dir().join(format!("probatio-copy-{}-{}", worktree::run_id_for(package), mutant.id));
    let _ = fs::remove_dir_all(&dest);
    if let Err(message) = copy_tree(package, &dest) {
        return error_scored(message);
    }
    if let Err(message) = link_node_modules(package, &dest) {
        let _ = fs::remove_dir_all(&dest);
        return error_scored(message);
    }
    let scored = apply_and_run(&dest, package, mutant, suite, timeout);
    let _ = fs::remove_dir_all(&dest);
    scored
}

fn apply_and_run(root: &Path, package: &Path, mutant: &LoadedMutant, suite: &str, timeout: Duration) -> Scored {
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
    let timed = process::run_shell_with_bin_root(suite, Some(root), Some(package), timeout);
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

/// The copy and the git worktree skip `node_modules`. Point at the package tree so a
/// suite can resolve the dependencies it already has. The link is removed with the tree.
fn link_node_modules(package: &Path, dest: &Path) -> Result<(), String> {
    let source = package.join("node_modules");
    let linked = dest.join("node_modules");
    if !source.is_dir() || linked.exists() {
        return Ok(());
    }
    std::os::unix::fs::symlink(&source, &linked).map_err(|error| error.to_string())
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

fn write_result(
    path: &Path,
    mutant: &LoadedMutant,
    scored: &Scored,
    selection: &Selection,
    max_selected: usize,
    command: Option<&str>,
) -> Result<(), String> {
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
    if let Some(command) = command {
        entries.push(("command", Value::String(command.to_string())));
    }
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
        assert_eq!(parsed(&envelope).get("suiteCommand").and_then(Value::as_str), Some("echo fail; exit 1"));
        assert!(envelope.next.contains("echo fail; exit 1"));
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
        assert!(envelope.next.contains(&script), "{}", envelope.next);
        assert_eq!(parsed(&envelope).get("suiteCommand").and_then(Value::as_str), Some(script.as_str()));
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
        assert_eq!(fs::read_dir(out.join("results")).unwrap().count(), 30);
        assert!(envelope.next.contains("30 started this call"), "{}", envelope.next);
        assert!(envelope.next.contains("1 is still waiting"), "{}", envelope.next);
        assert!(envelope.next.contains("did not choose tests"), "{}", envelope.next);
        assert_eq!(parsed(&envelope).get("suiteCommand").and_then(Value::as_str), Some("echo ok"));
        let argv = parsed(&envelope).get("nextCall").and_then(|call| call.get("argv")).cloned();
        let argv = match argv {
            Some(Value::Array(items)) => items,
            other => panic!("missing nextCall argv: {other:?}"),
        };
        assert!(argv.iter().all(|item| item.as_str() != Some("--confirm-count")));
        assert_eq!(fs::read_to_string(out.join("not-started.txt")).unwrap().lines().count(), 1);
        let second = run_with(
            &package,
            &patches,
            &out,
            &["--suite-command", "echo ok", "--line-map", &map.display().to_string()],
        );
        assert!(second.ok, "{}", second.summary);
        assert!(second.next_call.is_none(), "{}", second.next);
        assert_eq!(fs::read_dir(out.join("results")).unwrap().count(), 31);
        assert!(!out.join("not-started.txt").exists());
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn confirm_count_mismatch_writes_no_result_and_a_match_scores_the_batch() {
        let package = temp("confirm");
        write_source(&package, &many_ands(31));
        let patches = generate_into(&package);
        let map = package.join("lines.json");
        fs::write(&map, "{\"lines\":[1]}\n").unwrap();
        let map_arg = map.display().to_string();
        let refused = package.join("refused");
        let wrong = run_with(
            &package,
            &patches,
            &refused,
            &["--suite-command", "echo ok", "--line-map", &map_arg, "--confirm-count", "30"],
        );
        assert_no_kill_count(&wrong);
        assert!(wrong.next.contains("31"), "{}", wrong.next);
        assert!(wrong.next.contains("echo ok"), "{}", wrong.next);
        assert!(fs::read_dir(refused.join("results")).is_err());
        let out = package.join("out");
        let all = run_with(
            &package,
            &patches,
            &out,
            &["--suite-command", "echo ok", "--line-map", &map_arg, "--confirm-count", "31"],
        );
        assert!(all.ok, "{}", all.summary);
        assert!(all.next_call.is_none(), "{}", all.next);
        assert_eq!(fs::read_dir(out.join("results")).unwrap().count(), 31);
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn project_file_suite_loses_to_the_flag_and_invalid_json_refuses() {
        let package = temp("projfile");
        fs::write(package.join("package.json"), "{}\n").unwrap();
        fs::write(
            package.join(".probatio.json"),
            "{\n  \"suiteCommand\": \"echo from-file\",\n  \"env\": {\"A\": \"1\"},\n  \"testLine\": \"x\",\n  \"passLine\": \"y\"\n}\n",
        )
        .unwrap();
        let bare = Flags::parse(&[]);
        assert_eq!(suite_command(&bare, &package).unwrap(), "echo from-file");
        let flagged = Flags::parse(&["--suite-command".into(), "echo from-flag".into()]);
        assert_eq!(suite_command(&flagged, &package).unwrap(), "echo from-flag");
        fs::write(package.join(".probatio.json"), "{\"suiteCommand\":\"\"}\n").unwrap();
        assert_eq!(suite_command(&bare, &package).unwrap(), "node --test");
        fs::write(package.join(".probatio.json"), "{\n").unwrap();
        let err = suite_command(&bare, &package).unwrap_err();
        assert!(err.contains("not valid JSON"), "{err}");
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn a_test_token_runs_the_names_and_a_metacharacter_starts_nothing() {
        let package = temp("tokens");
        write_source(&package, one_and());
        let patches = generate_into(&package);
        let tests = package.join("tests.txt");
        fs::write(&tests, "Foo\nBar\n").unwrap();
        let log = package.join("ran.log");
        let command = format!("printf '%s\\n' {{tests}} >> '{}'; printf '%s\\n' {{tests}}", log.display());
        let out = package.join("out");
        let envelope = run_with(
            &package,
            &patches,
            &out,
            &["--suite-command", &command, "--tests", &tests.display().to_string()],
        );
        assert!(envelope.ok, "{}", envelope.summary);
        assert!(!envelope.next.contains("ran whole"), "{}", envelope.next);
        let ran = fs::read_to_string(&log).unwrap();
        assert!(ran.contains("Foo"), "{ran}");
        assert!(ran.contains("Bar"), "{ran}");
        let comma_log = package.join("comma.log");
        let comma = format!("printf '%s\\n' {{tests,}} >> '{}'; echo reported", comma_log.display());
        let comma_out = package.join("comma-out");
        let comma_envelope = run_with(
            &package,
            &patches,
            &comma_out,
            &["--suite-command", &comma, "--tests", &tests.display().to_string()],
        );
        assert!(comma_envelope.ok, "{}", comma_envelope.summary);
        assert!(fs::read_to_string(&comma_log).unwrap().contains("Foo,Bar"));
        let whole = run_with(&package, &patches, &package.join("whole"), &["--suite-command", "echo ok"]);
        assert!(whole.next.contains("The suite ran whole."), "{}", whole.next);
        fs::write(&tests, "a'b\n").unwrap();
        let bad_out = package.join("bad");
        let bad = run_with(
            &package,
            &patches,
            &bad_out,
            &["--suite-command", &command, "--tests", &tests.display().to_string()],
        );
        assert_no_kill_count(&bad);
        assert!(bad.summary.contains("a'b"), "{}", bad.summary);
        assert!(fs::read_dir(bad_out.join("results")).is_err());
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

    #[test]
    fn package_json_scripts_test_is_the_suite_and_invalid_json_refuses() {
        let package = temp("nodetest");
        fs::write(package.join("package.json"), "{\"scripts\":{\"test\":\"echo from-script\",\"lint\":\"nope\"}}\n").unwrap();
        assert_eq!(discover(&package).unwrap(), "echo from-script");
        fs::write(package.join("package.json"), "{\"scripts\":{\"test\":\"  \"}}\n").unwrap();
        assert_eq!(discover(&package).unwrap(), "node --test");
        fs::write(package.join("package.json"), "{\"scripts\":{\"test\":1}}\n").unwrap();
        assert_eq!(discover(&package).unwrap(), "node --test");
        fs::write(package.join("package.json"), "{}\n").unwrap();
        assert_eq!(discover(&package).unwrap(), "node --test");
        fs::write(package.join("package.json"), "{\n").unwrap();
        let err = discover(&package).unwrap_err();
        assert!(err.contains("package.json"), "{err}");
        assert!(err.contains("not valid JSON"), "{err}");
        assert!(!err.contains("node --test"), "{err}");
        let flagged = Flags::parse(&["--suite-command".into(), "echo from-flag".into()]);
        assert_eq!(suite_command(&flagged, &package).unwrap(), "echo from-flag");
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn a_changed_suite_template_reruns_the_baseline() {
        let package = temp("template");
        write_source(&package, one_and());
        let patches = generate_into(&package);
        let out = package.join("out");
        let first = run_with(&package, &patches, &out, &["--suite-command", "echo ok"]);
        assert!(first.ok, "{}", first.summary);
        let saved = json::parse(&fs::read_to_string(out.join("baseline.json")).unwrap()).unwrap();
        assert_eq!(saved.get("command").and_then(Value::as_str), Some("echo ok"));
        let second = run_with(&package, &patches, &out, &["--suite-command", "echo fail; exit 1"]);
        assert_no_kill_count(&second);
        assert!(second.summary.contains("already failing"), "{}", second.summary);
        let saved = json::parse(&fs::read_to_string(out.join("baseline.json")).unwrap()).unwrap();
        assert_eq!(saved.get("command").and_then(Value::as_str), Some("echo fail; exit 1"));
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn a_files_map_records_the_spawned_command_and_skips_an_uncovered_line() {
        let package = temp("filesmap");
        let source = many_ands(2);
        write_source(&package, &source);
        let patches = generate_into(&package);
        let listed = json::parse(&fs::read_to_string(patches.join("mutants.json")).unwrap()).unwrap();
        let items = match listed.get("mutants") {
            Some(Value::Array(items)) => items.clone(),
            other => panic!("mutants was {other:?}"),
        };
        let mut lines = Vec::new();
        for item in &items {
            let start = item.get("start").and_then(Value::as_int).unwrap() as usize;
            lines.push(decide::line_at(&source, start));
        }
        let covered = *lines.iter().min().unwrap();
        assert!(lines.iter().any(|line| *line != covered));
        let map = package.join("map.json");
        fs::write(
            &map,
            format!(
                r#"{{"files":{{"src/A.java":[{{"start":{covered},"end":{covered},"tests":["FooTest"]}}]}}}}"#
            ),
        )
        .unwrap();
        let tests = package.join("tests.txt");
        fs::write(&tests, "Bar\n").unwrap();
        let log = package.join("ran.log");
        let command = format!("printf '%s\\n' {{tests}} >> '{}'; printf '%s\\n' {{tests}}", log.display());
        let out = package.join("out");
        let envelope = run_with(
            &package,
            &patches,
            &out,
            &[
                "--suite-command",
                &command,
                "--line-map",
                &map.display().to_string(),
                "--tests",
                &tests.display().to_string(),
            ],
        );
        assert!(envelope.ok, "{}", envelope.summary);
        assert!(envelope.next.contains("The line map named tests."), "{}", envelope.next);
        assert!(!envelope.next.contains("did not choose tests"), "{}", envelope.next);
        let baseline = json::parse(&fs::read_to_string(out.join("baseline.json")).unwrap()).unwrap();
        assert_eq!(baseline.get("command").and_then(Value::as_str), Some(command.as_str()));
        let mut spawned = 0usize;
        let mut uncovered = 0usize;
        for entry in fs::read_dir(out.join("results")).unwrap() {
            let body = json::parse(&fs::read_to_string(entry.unwrap().path()).unwrap()).unwrap();
            match body.get("command").and_then(Value::as_str) {
                Some(rendered) => {
                    assert!(rendered.contains("FooTest"), "{rendered}");
                    assert!(!rendered.contains("Bar"), "{rendered}");
                    spawned += 1;
                }
                None => {
                    assert_eq!(body.get("outcome").and_then(Value::as_str), Some("no coverage"));
                    uncovered += 1;
                }
            }
        }
        assert!(spawned >= 1);
        assert!(uncovered >= 1);
        assert_eq!(fs::read_to_string(&log).unwrap().lines().count(), spawned + 1);
        let stopped = package.join("stopped");
        let refusal = run_with(
            &package,
            &patches,
            &stopped,
            &["--suite-command", "echo ok", "--line-map", &map.display().to_string()],
        );
        assert_no_kill_count(&refusal);
        assert!(refusal.next.contains("{tests}"), "{}", refusal.next);
        assert!(fs::read_dir(stopped.join("results")).is_err());
        let bad_map = package.join("bad-map.json");
        fs::write(
            &bad_map,
            format!(r#"{{"files":{{"src/A.java":[{{"start":{covered},"end":{covered},"tests":["a'b"]}}]}}}}"#),
        )
        .unwrap();
        let bad_out = package.join("bad");
        let bad = run_with(
            &package,
            &patches,
            &bad_out,
            &["--suite-command", &command, "--line-map", &bad_map.display().to_string()],
        );
        assert_no_kill_count(&bad);
        assert!(bad.summary.contains("a'b"), "{}", bad.summary);
        assert!(fs::read_dir(bad_out.join("results")).is_err());
        let unnamed = run_with(&package, &patches, &package.join("unnamed"), &["--suite-command", &command]);
        assert_no_kill_count(&unnamed);
        assert!(unnamed.summary.contains("asks for test names"), "{}", unnamed.summary);
        assert!(!log.exists() || fs::read_to_string(&log).unwrap().lines().count() == spawned + 1);
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
