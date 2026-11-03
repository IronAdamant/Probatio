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
use crate::report;
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
    killed_by: Vec<String>,
}

impl Scored {
    fn new(kind: Verdict, duration_ms: u64, timed_out: bool) -> Scored {
        Scored { kind, duration_ms, timed_out, killed_by: Vec::new() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PythonKind {
    Pytest,
    Unittest,
}

#[derive(Clone, Debug)]
struct BaselineRun {
    kind: Baseline,
    duration_ms: Option<u64>,
    template: Option<String>,
    /// Set after a Python baseline has looked for a coverage map.
    /// `lines.json` is that map. An empty string means coverage.py wrote nothing.
    line_map: Option<String>,
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

    let timeout_ms = match flags.parse_int("timeout-ms") {
        Ok(Some(value)) => value.max(1),
        Ok(None) => match flags.int("suite-timeout-ms", DEFAULT_TIMEOUT_MS) {
            Ok(value) => value.max(1),
            Err(message) => return missing(command, &message),
        },
        Err(message) => return missing(command, &message),
    };
    let timeout = Duration::from_millis(timeout_ms);
    let max_selected = selection_cap(flags);
    let caller_line_map = flags.get("line-map").filter(|path| usable_line_map(path)).map(PathBuf::from);
    let mut has_line_map = caller_line_map.is_some();
    let mut map = match &caller_line_map {
        Some(path) => load_map_file(path),
        None => decide::TestMap::slip(),
    };
    let python = parse_python(&suite);
    let python_files = match &python {
        Some(parsed) if parsed.kind == PythonKind::Unittest => unittest_files(&package, parsed.start_dir.as_deref()),
        _ => Vec::new(),
    };
    let python_program = python.as_ref().map(|parsed| interpreter_for(&parsed.program, &package)).unwrap_or_default();
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
    let whole_suite = !command_asks_for_names(&suite);
    let baseline_suite = if python.is_some() {
        suite.clone()
    } else {
        match decide::render_suite(&suite, &baseline_tests) {
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
        }
    };

    let mut coverage_note: Option<String> = None;
    let mut baseline = if let Some(parsed) = &python {
        match python_baseline(
            &package,
            &out,
            &suite,
            parsed,
            &python_program,
            &python_files,
            &baseline_tests,
            caller_line_map.is_none(),
            timeout,
        ) {
            Ok(value) => value,
            Err(envelope) => return attach_suite(envelope, &suite),
        }
    } else {
        match prepared_baseline(&package, &out, &suite, &baseline_suite, timeout) {
            Ok(value) => value,
            Err(envelope) => return attach_suite(envelope, &suite),
        }
    };
    if baseline.kind == Baseline::Green && caller_line_map.is_none() {
        if python.is_some() {
            if baseline.line_map.as_deref() == Some("lines.json") {
                let path = out.join("lines.json");
                if path.to_str().is_some_and(usable_line_map) {
                    map = load_map_file(&path);
                    has_line_map = true;
                }
            } else if baseline.line_map.is_none() {
                match adopt_coverage(&out) {
                    Some(adopted) => {
                        baseline.line_map = Some("lines.json".into());
                        map = adopted;
                        has_line_map = true;
                    }
                    None => baseline.line_map = Some(String::new()),
                }
            }
            if baseline.line_map.as_deref() == Some("") {
                coverage_note = Some("coverage.py did not write a map.".into());
            }
        } else if baseline.line_map.as_deref() == Some("lines.json") {
            let path = out.join("lines.json");
            if path.to_str().is_some_and(usable_line_map) {
                map = load_map_file(&path);
                has_line_map = true;
            }
        } else if baseline.line_map.as_deref() == Some("") {
            coverage_note = empty_coverage_note(&suite);
        } else if baseline.line_map.is_none() {
            match collect_language(&package, &out, &suite, timeout) {
                Collect::Skip => {}
                Collect::Ready(adopted) => {
                    baseline.line_map = Some("lines.json".into());
                    map = adopted;
                    has_line_map = true;
                }
                Collect::Empty(note) => {
                    baseline.line_map = Some(String::new());
                    coverage_note = Some(note);
                }
            }
        }
    }
    let caller_is_auto = caller_line_map.as_ref().is_some_and(|path| path == &out.join("lines.json"));
    let auto_map = map.chooses
        && baseline.line_map.as_deref() == Some("lines.json")
        && (caller_line_map.is_none() || caller_is_auto);
    let launcher_selects = auto_map && python.is_some();
    if auto_map {
        has_line_map = true;
    }
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
        let mut next = "The ids are in not-started.txt. Nothing was started.".to_string();
        if let Some(note) = &coverage_note {
            next.push(' ');
            next.push_str(note);
        }
        return attach_suite(Envelope::new(false, command, message, &next), &suite);
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
    let mut budget = match decide::chunk_budget(pending.len(), confirm) {
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
    if let Some(max) = flags.parse_int("max-mutants").unwrap_or(None) {
        budget = budget.min(max as usize);
    }
    let batch: Vec<&LoadedMutant> = pending.into_iter().take(budget).collect();
    if whole_suite && map.chooses && !auto_map {
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
    if python.is_none() {
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
    let mut scored_from_exit = false;
    let mut jobs = Vec::new();
    let score_started = std::time::Instant::now();
    let budget_ms = flags.parse_int("budget-ms").unwrap_or(None);
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
        if budget_ms.is_some_and(|limit| score_started.elapsed() >= std::time::Duration::from_millis(limit) && started_this_call > 0) {
            continue;
        }
        let hit = hit_for_mutant(&package, mutant, &map);
        if matches!(hit, decide::LineHit::Dark) {
            let scored = Scored::new(Verdict::NoCoverage, 0, false);
            let selection = Selection { tests: Vec::new(), selected_count: 0, capped: false };
            if let Err(error) = write_result(&path, mutant, &scored, &selection, max_selected, None) {
                return attach_suite(Envelope::new(false, command, error, "A mutant result could not be written."), &suite);
            }
            verdicts.push(scored.kind);
            started_this_call += 1;
            continue;
        }
        let only = flags.texts("only-test");
        let tests = if !only.is_empty() {
            only
        } else {
            match &hit {
                decide::LineHit::Listed(names) => names.clone(),
                decide::LineHit::Unmapped => shared_tests.clone().unwrap_or_default(),
                decide::LineHit::Dark => Vec::new(),
            }
        };
        let selection = decide::cap_tests(&tests, max_selected);
        if selection.capped {
            any_capped = true;
        }
        let (rendered, launch) = if let Some(parsed) = &python {
            let pass_names = launcher_selects || command_asks_for_names(&suite);
            let names = if pass_names { selection.tests.clone() } else { Vec::new() };
            let reporter = match ensure_reporter(&out, parsed.kind) {
                Ok(path) => path,
                Err(error) => {
                    return attach_suite(
                        Envelope::new(false, command, error, "The Python reporter could not be written."),
                        &suite,
                    )
                }
            };
            let display = python_display(&python_program, &reporter, &names);
            (
                display,
                Some(PythonLaunch {
                    reporter,
                    program: python_program.clone(),
                    names,
                    files: python_files.clone(),
                    extra: parsed.extra.clone(),
                    kind: parsed.kind,
                }),
            )
        } else if is_node_test(&suite) {
            let reporter = ensure_node_reporter(&out).unwrap_or_else(|_| out.join("fail-reporter.mjs"));
            let rendered = node_test_command(&reporter, &selection.tests);
            (rendered, None)
        } else {
            scored_from_exit = true;
            match decide::render_suite(&suite, &selection.tests) {
                Ok((rendered, _)) => (rendered, None),
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
            }
        };
        let confirm = flags.bool("confirm", true);
        jobs.push(ScoreJob {
            mutant: (*mutant).clone(),
            rendered,
            launch,
            path,
            selection,
            confirm,
            hide: flags.texts("hide"),
        });
        started_this_call += 1;
    }
    let workers_n = flags.int("workers", 1).unwrap_or(1).max(1) as usize;
    if let Err(error) = run_jobs(&jobs, workers_n, git, &package, &run_id, timeout, &mut verdicts, max_selected) {
        return attach_suite(Envelope::new(false, command, error, "A mutant result could not be written."), &suite);
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
    } else if map.chooses && !map.all_names().is_empty() {
        next.push_str(" The line map named tests.");
    } else if map.chooses {
        next.push_str(" The line map marked covered lines.");
    }
    if whole_suite && !launcher_selects {
        next.push_str(" The suite ran whole.");
    }
    if scored_from_exit {
        next.push_str(" A crash whose text contains error: can be filed as did not build.");
    }
    if let Some(note) = &coverage_note {
        next.push(' ');
        next.push_str(note);
    }
    if still.is_empty() && counts.survived > 0 {
        next.push_str(" Run mutate tally on this out directory.");
    }
    let mut envelope = attach_suite(Envelope::new(true, command, summary, next), &suite)
        .with_extra("counts", counts_value(&counts))
        .with_extra("finished", Value::Int(counts.finished as i64))
        .with_extra("killed", Value::Int(counts.killed as i64))
        .with_extra("survived", Value::Int(counts.survived as i64))
        .with_extra("flaky", Value::Int(counts.flaky as i64))
        .with_extra("timeouts", Value::Int(counts.timeouts as i64))
        .with_extra("errors", Value::Int(counts.errors as i64))
        .with_extra("noCoverage", Value::Int(counts.no_coverage as i64))
        .with_extra("unviable", Value::Int(counts.unviable as i64));
    if !still.is_empty() {
        let auto = if auto_map { Some(out.join("lines.json")) } else { None };
        envelope.next_call = Some(resume_argv(flags, &suite, timeout_ms, auto.as_deref()));
    } else if counts.survived > 0 {
        envelope.next_call = Some(vec!["mutate".into(), "tally".into(), "--out".into(), out.display().to_string()]);
    }
    let _ = write_run_json(&out, flags, &suite, counts.survived > 0);
    envelope
}

fn write_run_json(out: &Path, flags: &Flags, suite: &str, _survived: bool) -> Result<(), String> {
    let patches = flags.texts("patches");
    let value = Value::object(vec![
        ("package", Value::String(flags.get("package").unwrap_or("").into())),
        ("patches", Value::Array(patches.into_iter().map(Value::String).collect())),
        ("out", Value::String(out.display().to_string())),
        ("suiteCommand", Value::String(suite.into())),
        ("confirm", Value::Bool(flags.bool("confirm", true))),
        ("workers", Value::Int(flags.int("workers", 1).unwrap_or(1) as i64)),
    ]);
    fs::write(out.join("run.json"), json::stringify(&value)).map_err(|error| error.to_string())
}

fn missing(command: &str, message: &str) -> Envelope {
    Envelope::new(false, command, message.to_string(), "Pass the missing flag and run the command again.")
}

fn selection_cap(flags: &Flags) -> usize {
    let raw = flags.int("max-selected", decide::DEFAULT_MAX_SELECTED as u64).unwrap_or(decide::DEFAULT_MAX_SELECTED as u64);
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
    match hit_for_mutant(package, mutant, map) {
        decide::LineHit::Listed(names) => names,
        decide::LineHit::Dark => Vec::new(),
        decide::LineHit::Unmapped => shared.unwrap_or(&[]).to_vec(),
    }
}

fn hit_for_mutant(package: &Path, mutant: &LoadedMutant, map: &decide::TestMap) -> decide::LineHit {
    if !mutant.tests.is_empty() {
        return decide::LineHit::Listed(mutant.tests.clone());
    }
    if map.chooses {
        let text = fs::read_to_string(package.join(&mutant.file)).unwrap_or_default();
        return map.line_hit(&mutant.file, decide::line_at(&text, mutant.start));
    }
    decide::LineHit::Unmapped
}

/// Resume argv for the same out dir. The suite command is the template, not a rendered `{tests}` line.
/// `--confirm-count` is omitted so the next call chunks again.
fn resume_argv(flags: &Flags, suite: &str, timeout_ms: u64, auto_line_map: Option<&Path>) -> Vec<String> {
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
    } else if let Some(path) = auto_line_map {
        argv.push("--line-map".into());
        argv.push(path.display().to_string());
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

/// A non-empty `suiteCommand` string wins over discovery.
/// `env`, `testLine`, and `passLine` are checked here. Invalid JSON is an error.
fn project_suite(package: &Path) -> Result<Option<String>, String> {
    let Some(value) = read_project(package)? else {
        return Ok(None);
    };
    match value.get("suiteCommand").and_then(Value::as_str) {
        Some(command) if !command.trim().is_empty() => Ok(Some(command.to_string())),
        _ => Ok(None),
    }
}

fn read_project(package: &Path) -> Result<Option<Value>, String> {
    let path = package.join(".probatio.json");
    if !path.is_file() {
        return Ok(None);
    }
    let text = fs::read_to_string(&path).map_err(|error| error.to_string())?;
    let value = json::parse(&text).map_err(|_| ".probatio.json is not valid JSON".to_string())?;
    if let Some(env) = value.get("env") {
        let Value::Object(entries) = env else {
            return Err(".probatio.json env must be an object of strings".into());
        };
        for (key, item) in entries {
            if item.as_str().is_none() {
                return Err(format!(".probatio.json env.{key} must be a string"));
            }
        }
    }
    for name in ["testLine", "passLine"] {
        if let Some(pattern) = value.get(name) {
            if pattern.as_str().is_none_or(|text| text.is_empty()) {
                return Err(format!(".probatio.json {name} must be a non-empty string"));
            }
        }
    }
    Ok(Some(value))
}

fn project_env(package: &Path) -> Vec<(String, String)> {
    let Ok(Some(value)) = read_project(package) else {
        return Vec::new();
    };
    let Some(Value::Object(entries)) = value.get("env") else {
        return Vec::new();
    };
    entries.iter().filter_map(|(key, item)| item.as_str().map(|text| (key.clone(), text.to_string()))).collect()
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
    } else if package.join("Package.swift").is_file() {
        Ok("swift test".into())
    } else if root_file_ends_with(package, &[".sln", ".csproj", ".fsproj"]) {
        Ok("dotnet test".into())
    } else if package.join("Makefile").is_file()
        || package.join("makefile").is_file()
        || package.join("GNUmakefile").is_file()
    {
        Ok("make test".into())
    } else if package.join(".mocharc.json").is_file()
        || package.join(".mocharc.js").is_file()
        || package.join(".mocharc.cjs").is_file()
        || package.join(".mocharc.yml").is_file()
        || package.join(".mocharc.yaml").is_file()
    {
        Ok("mocha".into())
    } else if package.join("CMakeLists.txt").is_file() {
        Ok("ctest --output-on-failure".into())
    } else if has_unittest(package) {
        Ok("python3 -m unittest discover".into())
    } else if root_file_ends_with(package, &[".cob", ".cbl", ".cobol"]) {
        Ok("cobc -xj".into())
    } else if root_file_ends_with(package, &[".c", ".cc", ".cpp", ".h", ".hpp"]) {
        Ok("make test".into())
    } else if root_file_ends_with(package, &[".s", ".S", ".asm"]) {
        Ok("make test".into())
    } else {
        Err("This layout has no suite Probatio can see.".into())
    }
}

/// Regular files in the package root whose names end with one of `suffixes`.
/// `file_type` does not follow symlinks, and this does not walk child directories.
fn root_file_ends_with(package: &Path, suffixes: &[&str]) -> bool {
    let Ok(read) = fs::read_dir(package) else {
        return false;
    };
    read.flatten().any(|entry| {
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            return false;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        suffixes.iter().any(|suffix| name.ends_with(suffix))
    })
}

/// `test_*.py` in the package root, or in a direct child directory named `tests`.
fn has_unittest(package: &Path) -> bool {
    if dir_has_test_py(package) {
        return true;
    }
    let Ok(read) = fs::read_dir(package) else {
        return false;
    };
    for entry in read.flatten() {
        if entry.file_name().to_string_lossy() != "tests" {
            continue;
        }
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            return dir_has_test_py(&entry.path());
        }
    }
    false
}

fn dir_has_test_py(dir: &Path) -> bool {
    let Ok(read) = fs::read_dir(dir) else {
        return false;
    };
    read.flatten().any(|entry| {
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            return false;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        name.starts_with("test_") && name.ends_with(".py")
    })
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
        line_map: None,
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
    let mut entries = vec![
        ("judgement", Value::String(judgement.into())),
        ("durationMs", duration),
        ("command", Value::String(recorded)),
    ];
    if let Some(line_map) = &baseline.line_map {
        entries.push(("lineMap", Value::String(line_map.clone())));
    }
    let value = Value::object(entries);
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
    let line_map = match value.get("lineMap") {
        Some(Value::String(text)) => Some(text.clone()),
        _ => None,
    };
    Some(BaselineRun { kind, duration_ms, template, line_map })
}

fn result_path(out: &Path, id: &str) -> PathBuf {
    out.join("results").join(format!("{id}.json"))
}

struct ScoreJob {
    mutant: LoadedMutant,
    rendered: String,
    launch: Option<PythonLaunch>,
    path: PathBuf,
    selection: decide::Selection,
    confirm: bool,
    hide: Vec<String>,
}

fn run_jobs(
    jobs: &[ScoreJob],
    workers: usize,
    git: bool,
    package: &Path,
    run_id: &str,
    timeout: Duration,
    verdicts: &mut Vec<Verdict>,
    max_selected: usize,
) -> Result<(), String> {
    if jobs.is_empty() {
        return Ok(());
    }
    if workers <= 1 || jobs.len() == 1 {
        for job in jobs {
            let scored = score_job(job, git, package, run_id, timeout, "0");
            write_result(&job.path, &job.mutant, &scored, &job.selection, max_selected, Some(&job.rendered))?;
            verdicts.push(scored.kind);
        }
        return Ok(());
    }
    let slots = workers.min(jobs.len());
    let next = std::sync::Mutex::new(0usize);
    let found = std::sync::Mutex::new(Vec::new());
    let error = std::sync::Mutex::new(None);
    std::thread::scope(|scope| {
        for slot in 0..slots {
            let next = &next;
            let found = &found;
            let error = &error;
            scope.spawn(move || {
                loop {
                    let index = {
                        let mut cursor = next.lock().unwrap_or_else(|poison| poison.into_inner());
                        if *cursor >= jobs.len() {
                            return;
                        }
                        let index = *cursor;
                        *cursor += 1;
                        index
                    };
                    let job = &jobs[index];
                    let scored = score_job(job, git, package, run_id, timeout, &slot.to_string());
                    if let Err(message) = write_result(&job.path, &job.mutant, &scored, &job.selection, max_selected, Some(&job.rendered)) {
                        *error.lock().unwrap_or_else(|poison| poison.into_inner()) = Some(message);
                        return;
                    }
                    found.lock().unwrap_or_else(|poison| poison.into_inner()).push(scored.kind);
                }
            });
        }
    });
    if let Some(message) = error.lock().unwrap_or_else(|poison| poison.into_inner()).clone() {
        return Err(message);
    }
    verdicts.extend(found.lock().unwrap_or_else(|poison| poison.into_inner()).iter().copied());
    Ok(())
}

fn score_job(job: &ScoreJob, git: bool, package: &Path, run_id: &str, timeout: Duration, slot: &str) -> Scored {
    if git {
        score_git(package, run_id, &job.mutant, &job.rendered, job.launch.as_ref(), timeout, job.confirm, slot, &job.hide)
    } else {
        score_copy(package, &job.mutant, &job.rendered, job.launch.as_ref(), timeout, job.confirm, &job.hide)
    }
}

fn score_git(
    package: &Path,
    run_id: &str,
    mutant: &LoadedMutant,
    suite: &str,
    python: Option<&PythonLaunch>,
    timeout: Duration,
    confirm: bool,
    slot: &str,
    hide: &[String],
) -> Scored {
    let dest = if slot == "0" {
        worktree::worktree_dest(run_id)
    } else {
        std::env::temp_dir().join(format!("probatio-{run_id}-{slot}"))
    };
    let added = if slot == "0" {
        worktree::add_worktree(package, &dest, run_id)
    } else {
        worktree::add_fresh(package, &dest)
    };
    if let Err(message) = added {
        return error_scored(message);
    }
    if let Err(message) = link_node_modules(package, &dest) {
        worktree::remove_worktree(package, &dest);
        return error_scored(message);
    }
    let scored = apply_and_run(&dest, package, mutant, suite, python, timeout, confirm, hide);
    worktree::remove_worktree(package, &dest);
    scored
}

fn score_copy(package: &Path, mutant: &LoadedMutant, suite: &str, python: Option<&PythonLaunch>, timeout: Duration, confirm: bool, hide: &[String]) -> Scored {
    let dest = std::env::temp_dir().join(format!("probatio-copy-{}-{}", worktree::run_id_for(package), mutant.id));
    let _ = fs::remove_dir_all(&dest);
    if let Err(message) = copy_tree(package, &dest) {
        return error_scored(message);
    }
    if let Err(message) = link_node_modules(package, &dest) {
        let _ = fs::remove_dir_all(&dest);
        return error_scored(message);
    }
    let scored = apply_and_run(&dest, package, mutant, suite, python, timeout, confirm, hide);
    let _ = fs::remove_dir_all(&dest);
    scored
}

fn apply_and_run(
    root: &Path,
    package: &Path,
    mutant: &LoadedMutant,
    suite: &str,
    python: Option<&PythonLaunch>,
    timeout: Duration,
    confirm: bool,
    hide: &[String],
) -> Scored {
    for rel in hide {
        if rel.is_empty() || rel.split(['/', '\\']).any(|part| part == "..") {
            continue;
        }
        let target = root.join(rel);
        if target.is_file() {
            let _ = fs::remove_file(&target);
        }
    }
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
    let timed = if let Some(launch) = python {
        launch_python(
            root,
            package,
            &launch.program,
            &launch.reporter,
            launch.kind,
            &launch.names,
            &launch.files,
            &launch.extra,
            None,
            timeout,
        )
    } else {
        let env = project_env(package);
        let pairs: Vec<(&str, &str)> = env.iter().map(|(key, value)| (key.as_str(), value.as_str())).collect();
        process::run_shell_with_env(suite, Some(root), Some(package), timeout, &pairs)
    };
    let mut scored = score_timed(timed, python.is_some() || suite.contains("--test-reporter="));
    if confirm && scored.kind == Verdict::Killed {
        scored = confirm_kill(root, package, suite, python, timeout, scored);
    }
    scored
}

fn confirm_kill(
    root: &Path,
    package: &Path,
    suite: &str,
    python: Option<&PythonLaunch>,
    timeout: Duration,
    scored: Scored,
) -> Scored {
    let targets = if scored.killed_by.is_empty() { vec![String::new()] } else { scored.killed_by.clone() };
    let mut kept = Vec::new();
    for name in &targets {
        let mut hits = 0;
        for _ in 0..2 {
            let again = rerun_one(root, package, suite, python, timeout, name);
            let matched = if name.is_empty() {
                again.kind == Verdict::Killed
            } else {
                again.killed_by.iter().any(|item| item == name || item.ends_with(&format!("::{name}")))
            };
            // A rerun that passes is the flake. A rerun that errors is not evidence the kill was a flake.
            if matched || again.kind != Verdict::Survived {
                hits += 1;
            }
        }
        if hits == 2 {
            if !name.is_empty() {
                kept.push(name.clone());
            } else {
                kept.push(String::new());
            }
        }
    }
    let mut scored = scored;
    if kept.iter().any(|name| !name.is_empty()) || (scored.killed_by.is_empty() && kept.len() == 1) {
        scored.killed_by = kept.into_iter().filter(|name| !name.is_empty()).collect();
        scored.kind = Verdict::Killed;
    } else {
        scored.kind = Verdict::Flaky;
        scored.killed_by.clear();
    }
    scored
}

fn rerun_one(root: &Path, package: &Path, suite: &str, python: Option<&PythonLaunch>, timeout: Duration, name: &str) -> Scored {
    if let Some(launch) = python {
        let names = if name.is_empty() { launch.names.clone() } else { vec![name.to_string()] };
        let timed = launch_python(root, package, &launch.program, &launch.reporter, launch.kind, &names, &launch.files, &launch.extra, None, timeout);
        return score_timed(timed, true);
    }
    let command = if name.is_empty() || !suite.contains("--test-reporter=") {
        suite.to_string()
    } else {
        node_pattern_command(suite, &[name.to_string()])
    };
    let timed = process::run_shell_with_bin_root(&command, Some(root), Some(package), timeout);
    score_timed(timed, command.contains("--test-reporter="))
}

fn node_pattern_command(suite: &str, names: &[String]) -> String {
    let pattern = names.iter().map(|name| regex_escape(name.rsplit("::").next().unwrap_or(name))).collect::<Vec<_>>().join("|");
    if let Some((head, _)) = suite.split_once("--test-name-pattern=") {
        format!("{head}--test-name-pattern=^({pattern})$")
    } else if let Some(reporter) = suite.find("--test-reporter=") {
        let at = suite[reporter..].find(' ').map(|end| reporter + end).unwrap_or(suite.len());
        format!("{} --test-name-pattern=^({pattern})${}", &suite[..at], &suite[at..])
    } else {
        suite.to_string()
    }
}

fn error_scored(message: String) -> Scored {
    let _ = message;
    Scored::new(Verdict::Error, 0, false)
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
    #[cfg(unix)]
    {
        return std::os::unix::fs::symlink(&source, &linked).map_err(|error| error.to_string());
    }
    #[cfg(not(unix))]
    {
        let _ = (source, linked);
        Ok(())
    }
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
    if !scored.killed_by.is_empty() {
        entries.push((
            "killedBy",
            Value::Array(scored.killed_by.iter().cloned().map(Value::String).collect()),
        ));
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

const PYTEST_REPORTER: &str = include_str!("reporters/pytest_reporter.py");
const UNITTEST_REPORTER: &str = include_str!("reporters/unittest_reporter.py");

struct ParsedPython {
    kind: PythonKind,
    program: String,
    extra: Vec<String>,
    start_dir: Option<String>,
}

#[derive(Clone)]
struct PythonLaunch {
    reporter: PathBuf,
    program: String,
    names: Vec<String>,
    files: Vec<String>,
    extra: Vec<String>,
    kind: PythonKind,
}

enum Collect {
    Skip,
    Ready(decide::TestMap),
    Empty(String),
}

#[cfg(test)]
fn python_kind(command: &str) -> Option<PythonKind> {
    parse_python(command).map(|parsed| parsed.kind)
}

/// Pytest is the program `pytest`, or `python`/`python3 -m pytest`, plus any later arguments.
/// Unittest is `python`/`python3 -m unittest discover`, plus an optional `-s` directory.
/// `{tests}` and `{tests,}` are tokens, not arguments. `make test` is neither.
fn parse_python(command: &str) -> Option<ParsedPython> {
    let words = command_words(command);
    if words.is_empty() {
        return None;
    }
    let base = file_name(&words[0]);
    if base == "pytest" {
        return Some(ParsedPython {
            kind: PythonKind::Pytest,
            program: words[0].clone(),
            extra: words[1..].to_vec(),
            start_dir: None,
        });
    }
    let python = base == "python" || base == "python3";
    if python && words.len() >= 3 && words[1] == "-m" && words[2] == "pytest" {
        return Some(ParsedPython {
            kind: PythonKind::Pytest,
            program: words[0].clone(),
            extra: words[3..].to_vec(),
            start_dir: None,
        });
    }
    if python && words.len() >= 4 && words[1] == "-m" && words[2] == "unittest" && words[3] == "discover" {
        let start_dir = match &words[4..] {
            [] => None,
            [flag, dir] if flag == "-s" => Some(dir.clone()),
            _ => return None,
        };
        return Some(ParsedPython {
            kind: PythonKind::Unittest,
            program: words[0].clone(),
            extra: Vec::new(),
            start_dir,
        });
    }
    None
}

fn file_name(program: &str) -> &str {
    Path::new(program).file_name().and_then(|name| name.to_str()).unwrap_or(program)
}

fn command_words(command: &str) -> Vec<String> {
    command.replace("{tests,}", " ").replace("{tests}", " ").split_whitespace().map(str::to_string).collect()
}

fn same_words(command: &str, expected: &[&str]) -> bool {
    let words = command_words(command);
    words.len() == expected.len() && words.iter().zip(expected).all(|(have, want)| have == want)
}

fn is_node_test(command: &str) -> bool {
    let words = command_words(command);
    words.first().is_some_and(|word| file_name(word) == "node") && words.iter().any(|word| word == "--test")
}

fn ensure_node_reporter(out: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(out).map_err(|error| error.to_string())?;
    let reporter = out.join("fail-reporter.mjs");
    fs::write(&reporter, include_str!("../embed/node/fail-reporter.mjs")).map_err(|error| error.to_string())?;
    fs::write(out.join("load-failure.mjs"), include_str!("../embed/node/load-failure.mjs")).map_err(|error| error.to_string())?;
    Ok(reporter)
}

fn node_test_command(reporter: &Path, names: &[String]) -> String {
    let mut command = format!("node --test --test-reporter={}", shell_quote(reporter));
    if !names.is_empty() {
        let pattern = names.iter().map(|name| regex_escape(name)).collect::<Vec<_>>().join("|");
        command.push_str(&format!(" --test-name-pattern=^({pattern})$"));
    }
    command
}

fn is_go_coverage(command: &str) -> bool {
    same_words(command, &["go", "test", "./..."])
}

fn is_node_coverage(command: &str) -> bool {
    same_words(command, &["node", "--test"])
}

fn is_rust_coverage(command: &str) -> bool {
    same_words(command, &["cargo", "test"])
}

fn is_java_coverage(command: &str) -> bool {
    let words = command_words(command);
    words.len() >= 3
        && words[0] == "mvn"
        && words[1] == "-B"
        && words[2] == "test"
        && (words.len() == 3 || command_asks_for_names(command))
}

fn python_baseline(
    package: &Path,
    out: &Path,
    template: &str,
    parsed: &ParsedPython,
    program: &str,
    files: &[String],
    baseline_tests: &[String],
    collect: bool,
    timeout: Duration,
) -> Result<BaselineRun, Envelope> {
    let path = out.join("baseline.json");
    if let Some(saved) = read_baseline(&path) {
        let same = saved.kind == Baseline::Green && saved.template.as_deref() == Some(template);
        if same && (!collect || saved.line_map.is_some()) {
            return Ok(saved);
        }
    }
    let reporter = ensure_reporter(out, parsed.kind).map_err(|error| {
        Envelope::new(false, "mutate.run", error, "The Python reporter could not be written.".to_string())
    })?;
    let names: Vec<String> = if command_asks_for_names(template) { baseline_tests.to_vec() } else { Vec::new() };
    let coverage_path = out.join("coverage-raw.json");
    let coverage = if collect && parsed.kind == PythonKind::Pytest { Some(coverage_path.as_path()) } else { None };
    let timed = launch_python(
        package,
        package,
        program,
        &reporter,
        parsed.kind,
        &names,
        files,
        &parsed.extra,
        coverage,
        timeout,
    );
    Ok(BaselineRun {
        kind: judge_python_baseline(&timed),
        duration_ms: Some(duration_ms(timed.duration)),
        template: Some(template.to_string()),
        line_map: None,
    })
}

fn judge_python_baseline(timed: &TimedRun) -> Baseline {
    if !timed.spawned {
        return Baseline::MissingTool;
    }
    if timed.timed_out {
        return Baseline::TimedOut;
    }
    if timed.status == Some(127) && timed.stderr.contains("command not found") {
        return Baseline::MissingTool;
    }
    match report::parse(&timed.stdout) {
        Some(report) => report::judge_baseline(&report),
        None => Baseline::NoReport,
    }
}

fn launch_python(
    root: &Path,
    bin_root: &Path,
    program: &str,
    reporter: &Path,
    kind: PythonKind,
    names: &[String],
    files: &[String],
    extra: &[String],
    coverage: Option<&Path>,
    timeout: Duration,
) -> TimedRun {
    let args = reporter_args(reporter, kind, names, files, extra);
    let owned = python_env(root, coverage);
    let env: Vec<(&str, &str)> = owned.iter().map(|(key, value)| (key.as_str(), value.as_str())).collect();
    process::run_argv(program, &args, Some(root), Some(bin_root), timeout, &env)
}

fn python_env(root: &Path, coverage: Option<&Path>) -> Vec<(String, String)> {
    let mut env = vec![("PYTHONDONTWRITEBYTECODE".to_string(), "1".to_string())];
    let src = root.join("src");
    if src.is_dir() {
        let mut pythonpath = src.display().to_string();
        if let Ok(existing) = std::env::var("PYTHONPATH") {
            if !existing.is_empty() {
                pythonpath.push(':');
                pythonpath.push_str(&existing);
            }
        }
        env.push(("PYTHONPATH".to_string(), pythonpath));
    }
    if let Some(path) = coverage {
        env.push(("PROBATIO_COVERAGE_MAP".to_string(), path.display().to_string()));
        env.push(("COVERAGE_CORE".to_string(), "ctrace".to_string()));
    }
    env
}

fn reporter_args(reporter: &Path, kind: PythonKind, names: &[String], files: &[String], extra: &[String]) -> Vec<String> {
    let mut args = vec![reporter.display().to_string()];
    match kind {
        PythonKind::Pytest => {
            args.extend(extra.iter().cloned());
            args.extend(names.iter().cloned());
        }
        PythonKind::Unittest => {
            args.push("--walked".into());
            args.extend(files.iter().cloned());
            if !names.is_empty() {
                args.push("--pattern".into());
                args.push(unittest_pattern(names));
            }
        }
    }
    args
}

fn unittest_pattern(names: &[String]) -> String {
    let alts = names.iter().map(|name| regex_escape(name)).collect::<Vec<_>>().join("|");
    format!("^(?:{alts})$")
}

fn regex_escape(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if matches!(ch, '\\' | '.' | '^' | '$' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn python_display(program: &str, reporter: &Path, names: &[String]) -> String {
    let mut text = format!("{program} {}", reporter.display());
    for name in names {
        text.push(' ');
        text.push_str(name);
    }
    text
}

/// `test_*.py` one directory deep: the package root, `tests/`, and `test/`.
/// `-s` limits the walk to that directory. A folder does not need `__init__.py`.
fn unittest_files(package: &Path, start: Option<&str>) -> Vec<String> {
    let dirs = if let Some(start) = start {
        if start.is_empty() || start.contains("..") || Path::new(start).is_absolute() {
            return Vec::new();
        }
        vec![package.join(start)]
    } else {
        vec![package.to_path_buf(), package.join("tests"), package.join("test")]
    };
    let mut files = Vec::new();
    for dir in dirs {
        let Ok(read) = fs::read_dir(&dir) else { continue };
        for entry in read.flatten() {
            if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !(name.starts_with("test_") && name.ends_with(".py")) {
                continue;
            }
            let path = entry.path();
            let Ok(relative) = path.strip_prefix(package) else { continue };
            files.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
    files.sort();
    files.dedup();
    files
}

fn interpreter_for(program: &str, package: &Path) -> String {
    let path = Path::new(program);
    let base = file_name(program);
    if base == "python" || base == "python3" {
        if path.components().count() > 1 {
            let joined = if path.is_absolute() { path.to_path_buf() } else { package.join(path) };
            if joined.is_file() {
                return joined.display().to_string();
            }
        }
        return program.to_string();
    }
    let parent = if path.is_absolute() {
        path.parent().map(|dir| dir.to_path_buf())
    } else if path.components().count() > 1 {
        path.parent().map(|dir| package.join(dir))
    } else {
        None
    };
    if let Some(parent) = parent {
        for name in ["python3", "python"] {
            let candidate = parent.join(name);
            if candidate.is_file() {
                return candidate.display().to_string();
            }
        }
    }
    "python3".to_string()
}

fn ensure_reporter(out: &Path, kind: PythonKind) -> Result<PathBuf, String> {
    fs::create_dir_all(out).map_err(|error| error.to_string())?;
    let path = out.join("reporter.py");
    let text = match kind {
        PythonKind::Pytest => PYTEST_REPORTER,
        PythonKind::Unittest => UNITTEST_REPORTER,
    };
    fs::write(&path, text).map_err(|error| error.to_string())?;
    Ok(path)
}

fn score_timed(timed: TimedRun, python: bool) -> Scored {
    let duration_ms = duration_ms(timed.duration);
    if timed.timed_out {
        return Scored::new(Verdict::Timeout, duration_ms, true);
    }
    if python {
        if let Some(report) = report::parse(&timed.stdout) {
            let judged = report::judge_mutant(&report);
            return Scored { kind: judged.verdict, duration_ms, timed_out: false, killed_by: judged.killed_by };
        }
    }
    let text = format!("{}\n{}", timed.stdout, timed.stderr);
    Scored::new(decide::classify(false, timed.status, &text), duration_ms, false)
}

fn load_map_file(path: &Path) -> decide::TestMap {
    let raw = fs::read_to_string(path).unwrap_or_default();
    let text = crate::coverage::normalize(&raw).unwrap_or(raw);
    decide::parse_test_map(&text)
}

fn adopt_coverage(out: &Path) -> Option<decide::TestMap> {
    let raw = fs::read_to_string(out.join("coverage-raw.json")).ok()?;
    let text = crate::coverage::normalize(&raw)?;
    store_lines(out, &text)
}

fn store_lines(out: &Path, text: &str) -> Option<decide::TestMap> {
    let map = decide::parse_test_map(text);
    if !map.chooses {
        return None;
    }
    fs::write(out.join("lines.json"), text).ok()?;
    Some(map)
}

/// A second look after a green baseline. A failure here does not change that baseline.
fn collect_language(package: &Path, out: &Path, suite: &str, timeout: Duration) -> Collect {
    if is_go_coverage(suite) {
        collect_go(package, out, timeout)
    } else if is_node_coverage(suite) {
        collect_node(package, out, timeout)
    } else if is_java_coverage(suite) {
        collect_java(package, out)
    } else if is_rust_coverage(suite) {
        collect_rust(package, out, timeout)
    } else {
        Collect::Skip
    }
}

fn empty_coverage_note(suite: &str) -> Option<String> {
    if is_go_coverage(suite) {
        Some("go did not write a coverage map.".into())
    } else if is_node_coverage(suite) {
        Some("node did not write a coverage map.".into())
    } else if is_java_coverage(suite) {
        Some("The suite did not write a coverage report.".into())
    } else if is_rust_coverage(suite) {
        if tool_on_path("cargo-llvm-cov") {
            Some("cargo llvm-cov did not write a coverage map.".into())
        } else {
            Some("cargo llvm-cov is not on PATH.".into())
        }
    } else {
        None
    }
}

fn collect_go(package: &Path, out: &Path, timeout: Duration) -> Collect {
    let dest = out.join("coverage-raw.out");
    let script = format!("go test -coverprofile={} ./...", shell_quote(&dest));
    let _timed = process::run_shell(&script, Some(package), timeout);
    match fs::read_to_string(&dest).ok().and_then(|text| crate::coverage::normalize(&text)).and_then(|json| store_lines(out, &json)) {
        Some(map) => Collect::Ready(map),
        None => Collect::Empty("go did not write a coverage map.".into()),
    }
}

fn collect_node(package: &Path, out: &Path, timeout: Duration) -> Collect {
    let dir = out.join("v8-coverage");
    let _ = fs::create_dir_all(&dir);
    let lcov = out.join("coverage-raw.lcov");
    let script = format!(
        "NODE_V8_COVERAGE={} node --test --experimental-test-coverage --test-reporter=lcov --test-reporter-destination={}",
        shell_quote(&dir),
        shell_quote(&lcov)
    );
    let _timed = process::run_shell(&script, Some(package), timeout);
    if let Some(map) = fs::read_to_string(&lcov)
        .ok()
        .and_then(|text| crate::coverage::lcov_lines(&text))
        .and_then(|json| store_lines(out, &json))
    {
        return Collect::Ready(map);
    }
    if let Some(map) = crate::coverage::v8_lines(&dir, package).and_then(|json| store_lines(out, &json)) {
        return Collect::Ready(map);
    }
    Collect::Empty("node did not write a coverage map.".into())
}

fn collect_java(package: &Path, out: &Path) -> Collect {
    let note = "The suite did not write a coverage report.".to_string();
    let pom = fs::read_to_string(package.join("pom.xml")).unwrap_or_default();
    if !pom.to_ascii_lowercase().contains("jacoco") {
        return Collect::Empty(note);
    }
    let xml = match fs::read_to_string(package.join("target/site/jacoco/jacoco.xml")) {
        Ok(text) if !text.trim().is_empty() => text,
        _ => return Collect::Empty(note),
    };
    match crate::coverage::jacoco_lines(&xml, package).and_then(|json| store_lines(out, &json)) {
        Some(map) => Collect::Ready(map),
        None => Collect::Empty(note),
    }
}

fn collect_rust(package: &Path, out: &Path, timeout: Duration) -> Collect {
    if !tool_on_path("cargo-llvm-cov") {
        return Collect::Empty("cargo llvm-cov is not on PATH.".into());
    }
    let lcov = out.join("coverage-raw.lcov");
    let script = format!("cargo llvm-cov --lcov --output-path {}", shell_quote(&lcov));
    let _timed = process::run_shell(&script, Some(package), timeout);
    match fs::read_to_string(&lcov)
        .ok()
        .and_then(|text| crate::coverage::lcov_lines(&text))
        .and_then(|json| store_lines(out, &json))
    {
        Some(map) => Collect::Ready(map),
        None => Collect::Empty("cargo llvm-cov did not write a coverage map.".into()),
    }
}

fn tool_on_path(name: &str) -> bool {
    let Ok(path) = std::env::var("PATH") else { return false };
    path.split(':').any(|dir| Path::new(dir).join(name).is_file())
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
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
        generate_with(package, &[])
    }

    fn generate_with(package: &Path, extra: &[&str]) -> PathBuf {
        let patches = package.join("patches");
        let mut args = vec!["--package".into(), package.display().to_string(), "--out".into(), patches.display().to_string()];
        args.extend(extra.iter().map(|item| (*item).to_string()));
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
    fn discovery_names_each_marker_without_launching_it() {
        let root = temp("discover-markers");
        let cases = [
            ("go.mod", "module example\n", "go test ./..."),
            ("Cargo.toml", "[package]\nname = \"ex\"\n", "cargo test"),
            ("pom.xml", "<project/>", "mvn -B test"),
            ("package.json", "{\"scripts\":{\"test\":\"npm test\"}}\n", "npm test"),
            ("package.json", "{}\n", "node --test"),
            ("pytest.ini", "[pytest]\n", "python3 -m pytest"),
            ("pyproject.toml", "[project]\nname = \"ex\"\n", "python3 -m pytest"),
            ("Package.swift", "", "swift test"),
            ("App.sln", "", "dotnet test"),
            ("Demo.csproj", "", "dotnet test"),
            ("Lib.fsproj", "", "dotnet test"),
            ("Makefile", "test:\n\ttrue\n", "make test"),
            ("makefile", "test:\n\ttrue\n", "make test"),
            ("GNUmakefile", "test:\n\ttrue\n", "make test"),
            (".mocharc.json", "{}\n", "mocha"),
            (".mocharc.js", "", "mocha"),
            (".mocharc.cjs", "", "mocha"),
            (".mocharc.yml", "", "mocha"),
            (".mocharc.yaml", "", "mocha"),
            ("CMakeLists.txt", "", "ctest --output-on-failure"),
            ("test_widget.py", "", "python3 -m unittest discover"),
            ("tests/test_widget.py", "", "python3 -m unittest discover"),
            ("prog.cob", "", "cobc -xj"),
            ("prog.cbl", "", "cobc -xj"),
            ("prog.cobol", "", "cobc -xj"),
            ("widget.c", "", "make test"),
            ("widget.cc", "", "make test"),
            ("widget.cpp", "", "make test"),
            ("widget.h", "", "make test"),
            ("widget.hpp", "", "make test"),
            ("add.s", "", "make test"),
            ("add.S", "", "make test"),
            ("routine.asm", "", "make test"),
        ];
        for (index, (marker, body, command)) in cases.iter().enumerate() {
            let package = root.join(index.to_string());
            fs::create_dir_all(&package).unwrap();
            let path = package.join(marker);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&path, body).unwrap();
            let found = discover(&package).unwrap_or_else(|error| panic!("{marker}: {error}"));
            assert_eq!(found, *command, "{marker}");
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn discovery_names_go_mod_ahead_of_a_makefile() {
        let package = temp("discover-go-make");
        fs::write(package.join("go.mod"), "module example\n").unwrap();
        fs::write(package.join("Makefile"), "test:\n\ttrue\n").unwrap();
        assert_eq!(discover(&package).unwrap(), "go test ./...");
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn discovery_names_invalid_package_json_does_not_fall_through() {
        let package = temp("discover-bad-json");
        fs::write(package.join("package.json"), "{\n").unwrap();
        fs::write(package.join("Makefile"), "test:\n\ttrue\n").unwrap();
        assert_eq!(discover(&package).unwrap_err(), "package.json is not valid JSON");
        let _ = fs::remove_dir_all(&package);
    }

    #[cfg(unix)]
    #[test]
    fn discovery_names_unmarked_layout_errors_without_launching() {
        let package = temp("discover-none");
        fs::write(package.join("README"), "no suite\n").unwrap();
        fs::write(package.join("src/widget.c"), "int x;\n").unwrap();
        fs::write(package.join("src/test_widget.py"), "").unwrap();
        fs::create_dir_all(package.join("tests/nested")).unwrap();
        fs::write(package.join("tests/nested/test_widget.py"), "").unwrap();
        fs::create_dir(package.join("widget.c")).unwrap();
        std::os::unix::fs::symlink(package.join("src/widget.c"), package.join("linked.c")).unwrap();
        std::os::unix::fs::symlink(package.join("src/test_widget.py"), package.join("tests/test_widget.py")).unwrap();
        assert_eq!(discover(&package).unwrap_err(), "This layout has no suite Probatio can see.");
        let outside = package.join("outside-tests");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("test_widget.py"), "").unwrap();
        let via = package.join("via");
        fs::create_dir(&via).unwrap();
        std::os::unix::fs::symlink(&outside, via.join("tests")).unwrap();
        assert_eq!(discover(&via).unwrap_err(), "This layout has no suite Probatio can see.");
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
        assert_eq!(fs::read_dir(out.join("results")).unwrap().count(), 31);
        assert!(envelope.next.contains("did not choose tests"), "{}", envelope.next);
        assert!(envelope.next.contains("tally"), "{}", envelope.next);
        assert_eq!(parsed(&envelope).get("suiteCommand").and_then(Value::as_str), Some("echo ok"));
        let argv = parsed(&envelope).get("nextCall").and_then(|call| call.get("argv")).cloned();
        let argv = match argv {
            Some(Value::Array(items)) => items,
            other => panic!("missing nextCall argv: {other:?}"),
        };
        assert!(argv.iter().any(|item| item.as_str() == Some("tally")));
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
        assert!(all.next_call.as_ref().is_some_and(|argv| argv.iter().any(|item| item == "tally")), "{}", all.next);
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
        fs::write(package.join(".probatio.json"), "{\"env\":[]}\n").unwrap();
        let env_err = suite_command(&bare, &package).unwrap_err();
        assert!(env_err.contains("env must be an object"), "{env_err}");
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
        let committed = generate_with(&package, &[]);
        let committed_count = match json::parse(&fs::read_to_string(committed.join("mutants.json")).unwrap()).unwrap().get("mutants") {
            Some(Value::Array(items)) => items.len(),
            _ => 0,
        };
        assert_eq!(committed_count, 1, "generate reads the commit unless --working-tree is set");
        let patches = generate_with(&package, &["--working-tree"]);
        let listed = json::parse(&fs::read_to_string(patches.join("mutants.json")).unwrap()).unwrap();
        let count = match listed.get("mutants") {
            Some(Value::Array(items)) => items.len(),
            _ => 0,
        };
        assert_eq!(count, 2, "generate --working-tree should see both && operators");
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

    #[test]
    fn python_commands_are_recognized_only_when_they_are_the_discovery_strings() {
        assert_eq!(python_kind("python3 -m pytest"), Some(PythonKind::Pytest));
        assert_eq!(python_kind("python3 -m pytest {tests}"), Some(PythonKind::Pytest));
        assert_eq!(python_kind("python3 -m unittest discover"), Some(PythonKind::Unittest));
        assert_eq!(python_kind("python3 -m unittest discover {tests,}"), Some(PythonKind::Unittest));
        assert_eq!(python_kind(".venv/bin/pytest"), Some(PythonKind::Pytest));
        assert_eq!(python_kind("python3 -m pytest -q"), Some(PythonKind::Pytest));
        assert_eq!(python_kind("make test"), None);
        let quiet = parse_python("python3 -m pytest -q {tests}").unwrap();
        assert_eq!(quiet.extra, vec!["-q".to_string()]);
        let limited = parse_python("python3 -m unittest discover -s tests").unwrap();
        assert_eq!(limited.start_dir.as_deref(), Some("tests"));
        assert!(parse_python("python3 -m unittest tests.test_gate").is_none());
    }

    fn py_package(name: &str, gate: &str, methods: &str) -> PathBuf {
        let package = temp(name);
        fs::write(package.join("src/gate.py"), gate).unwrap();
        fs::create_dir_all(package.join("tests")).unwrap();
        fs::write(package.join("tests/__init__.py"), "").unwrap();
        let body = format!(
            "import sys\nfrom pathlib import Path\nsys.path.insert(0, str(Path(__file__).resolve().parents[1] / \"src\"))\nimport unittest\nimport gate\n\nclass GateTests(unittest.TestCase):\n{methods}\n"
        );
        fs::write(package.join("tests/test_gate.py"), body).unwrap();
        package
    }

    fn hand_mutant(package: &Path, original: &str, replacement: &str) -> PathBuf {
        let text = fs::read_to_string(package.join("src/gate.py")).unwrap();
        let start = text.find(original).unwrap();
        let patches = package.join("patches");
        fs::create_dir_all(&patches).unwrap();
        let body = format!(
            "{{\"mutants\":[{{\"id\":\"mhand\",\"file\":\"src/gate.py\",\"start\":{start},\"end\":{},\"original\":{},\"replacement\":{}}}]}}\n",
            start + original.len(),
            json_string(original),
            json_string(replacement),
        );
        fs::write(patches.join("mutants.json"), body).unwrap();
        patches
    }

    fn json_string(text: &str) -> String {
        json::stringify_compact(&Value::String(text.to_string()))
    }

    fn one_result(out: &Path) -> Value {
        let mut files = fs::read_dir(out.join("results")).unwrap().map(|entry| entry.unwrap().path()).collect::<Vec<_>>();
        files.sort();
        assert_eq!(files.len(), 1, "{files:?}");
        json::parse(&fs::read_to_string(&files[0]).unwrap()).unwrap()
    }

    #[test]
    fn a_python_assertion_is_a_kill_and_the_name_stays_off_the_envelope() {
        let package = py_package(
            "py-kill",
            "def gate(n):\n    return n > 0\n",
            "    def test_zero(self):\n        self.assertFalse(gate.gate(0))\n    def test_one(self):\n        self.assertTrue(gate.gate(1))\n",
        );
        let patches = generate_into(&package);
        let out = package.join("out");
        let envelope = run_with(&package, &patches, &out, &[]);
        assert!(envelope.ok, "{}", envelope.summary);
        assert!(!envelope.render("").contains("killedBy"), "{}", envelope.render(""));
        assert!(!envelope.summary.contains("test_zero"), "{}", envelope.summary);
        let body = one_result(&out);
        assert_eq!(body.get("outcome").and_then(Value::as_str), Some("killed"));
        let names = match body.get("killedBy") {
            Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).collect::<Vec<_>>(),
            other => panic!("killedBy was {other:?}"),
        };
        assert!(names.iter().any(|name| name.contains("test_zero")), "{names:?}");
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn a_python_load_failure_does_not_build_and_a_value_error_is_a_kill() {
        let gate = "def gate(n):\n    return n > 0\n";
        let methods = "    def test_zero(self):\n        self.assertFalse(gate.gate(0))\n    def test_one(self):\n        self.assertTrue(gate.gate(1))\n";
        let package = py_package("py-load", gate, methods);
        let patches = hand_mutant(&package, "return n > 0", "return n >");
        let out = package.join("out");
        let envelope = run_with(&package, &patches, &out, &[]);
        assert!(envelope.ok, "{}", envelope.summary);
        assert!(envelope.summary.contains("did not build"), "{}", envelope.summary);
        assert_eq!(one_result(&out).get("outcome").and_then(Value::as_str), Some("unviable"));

        let value_patches = hand_mutant(&package, "return n > 0", "raise ValueError(\"boom\")");
        let value_out = package.join("value");
        let value = run_with(&package, &value_patches, &value_out, &[]);
        assert!(value.ok, "{}", value.summary);
        assert_eq!(one_result(&value_out).get("outcome").and_then(Value::as_str), Some("killed"));

        let shell_out = package.join("shell");
        let shell = run_with(
            &package,
            &value_patches,
            &shell_out,
            &["--suite-command", "python3 -m unittest tests.test_gate"],
        );
        assert!(shell.ok, "{}", shell.summary);
        assert_eq!(one_result(&shell_out).get("outcome").and_then(Value::as_str), Some("unviable"));
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn a_large_python_suite_without_coverage_stops_before_a_mutant() {
        let mut expr = String::new();
        for index in 0..20 {
            if index > 0 {
                expr.push_str(" and ");
            }
            expr.push_str(&format!("n > {index}"));
        }
        let package = py_package(
            "py-large",
            &format!("def gate(n):\n    return {expr}\n"),
            "    def test_high(self):\n        self.assertTrue(gate.gate(100))\n    def test_low(self):\n        self.assertFalse(gate.gate(-1))\n",
        );
        let patches = generate_into(&package);
        let out = package.join("out");
        let envelope = run_with(&package, &patches, &out, &[]);
        assert_no_kill_count(&envelope);
        assert!(envelope.summary.contains("No line map"), "{}", envelope.summary);
        assert!(envelope.next.contains("coverage.py did not write a map"), "{}", envelope.next);
        assert!(fs::read_dir(out.join("results")).is_err() || fs::read_dir(out.join("results")).unwrap().next().is_none());
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn a_planted_coverage_map_selects_python_tests_and_a_caller_map_wins() {
        let gate = "def low(n):\n    return n > 0\n\ndef high(n):\n    return n > 10\n";
        let methods = "    def test_low(self):\n        self.assertFalse(gate.low(0))\n        self.assertTrue(gate.low(1))\n    def test_high(self):\n        self.assertFalse(gate.high(10))\n        self.assertTrue(gate.high(11))\n";
        let package = py_package("py-map", gate, methods);
        let text = fs::read_to_string(package.join("src/gate.py")).unwrap();
        let low_line = decide::line_at(&text, text.find("return n > 0").unwrap());
        let patches = generate_into(&package);
        let out = package.join("out");
        fs::create_dir_all(&out).unwrap();
        fs::write(
            out.join("coverage-raw.json"),
            format!(r#"{{"files":{{"src/gate.py":{{"{low_line}":["tests.test_gate.GateTests.test_low"]}}}}}}"#),
        )
        .unwrap();
        let envelope = run_with(&package, &patches, &out, &[]);
        assert!(envelope.ok, "{}", envelope.summary);
        let listed = json::parse(&fs::read_to_string(patches.join("mutants.json")).unwrap()).unwrap();
        let items = match listed.get("mutants") {
            Some(Value::Array(items)) => items.clone(),
            other => panic!("mutants was {other:?}"),
        };
        let mut saw_named = false;
        let mut saw_dark = false;
        for item in &items {
            let id = item.get("id").and_then(Value::as_str).unwrap();
            let start = item.get("start").and_then(Value::as_int).unwrap() as usize;
            let line = decide::line_at(&text, start);
            let body = json::parse(&fs::read_to_string(out.join("results").join(format!("{id}.json"))).unwrap()).unwrap();
            if line == low_line {
                let command = body.get("command").and_then(Value::as_str).unwrap_or("");
                assert!(command.contains("tests.test_gate.GateTests.test_low"), "{command}");
                saw_named = true;
            } else {
                assert_eq!(body.get("outcome").and_then(Value::as_str), Some("no coverage"));
                assert!(body.get("command").is_none(), "{body:?}");
                saw_dark = true;
            }
        }
        assert!(saw_named && saw_dark);

        let caller = package.join("caller.json");
        fs::write(&caller, "{\"files\":{}}\n").unwrap();
        let caller_out = package.join("caller");
        fs::create_dir_all(&caller_out).unwrap();
        fs::write(
            caller_out.join("coverage-raw.json"),
            format!(r#"{{"files":{{"src/gate.py":{{"{low_line}":["tests.test_gate.GateTests.test_low"]}}}}}}"#),
        )
        .unwrap();
        let kept = run_with(
            &package,
            &patches,
            &caller_out,
            &["--suite-command", "python3 -m unittest discover", "--line-map", &caller.display().to_string()],
        );
        assert!(kept.ok, "{}", kept.summary);
        assert!(!caller_out.join("lines.json").exists());
        for entry in fs::read_dir(caller_out.join("results")).unwrap() {
            let body = json::parse(&fs::read_to_string(entry.unwrap().path()).unwrap()).unwrap();
            assert_eq!(body.get("outcome").and_then(Value::as_str), Some("no coverage"));
        }

        let named = package.join("named.json");
        fs::write(
            &named,
            format!(r#"{{"files":{{"src/gate.py":[{{"start":{low_line},"end":{low_line},"tests":["tests.test_gate.GateTests.test_low"]}}]}}}}"#),
        )
        .unwrap();
        let refused = run_with(
            &package,
            &patches,
            &package.join("refused"),
            &["--suite-command", "python3 -m unittest discover", "--line-map", &named.display().to_string()],
        );
        assert_no_kill_count(&refused);
        assert!(refused.summary.contains("no {{tests}} token") || refused.summary.contains("no {tests} token"), "{}", refused.summary);
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn pytest_coverage_writes_a_line_map_when_the_module_imports() {
        let pytest = Command::new("python3").args(["-c", "import pytest, coverage"]).status();
        if !pytest.map(|status| status.success()).unwrap_or(false) {
            return;
        }
        let package = temp("py-cov");
        fs::write(package.join("pytest.ini"), "[pytest]\n").unwrap();
        fs::write(package.join("src/gate.py"), "def gate(n):\n    return n > 0\n").unwrap();
        fs::create_dir_all(package.join("tests")).unwrap();
        fs::write(
            package.join("tests/test_gate.py"),
            "import sys\nfrom pathlib import Path\nsys.path.insert(0, str(Path(__file__).resolve().parents[1] / \"src\"))\nimport gate\n\ndef test_zero():\n    assert gate.gate(0) is False\n\ndef test_one():\n    assert gate.gate(1) is True\n",
        )
        .unwrap();
        let patches = generate_into(&package);
        let out = package.join("out");
        let envelope = run_with(&package, &patches, &out, &[]);
        assert!(envelope.ok, "{}", envelope.summary);
        assert!(out.join("lines.json").is_file());
        let body = one_result(&out);
        assert_ne!(body.get("outcome").and_then(Value::as_str), Some("no coverage"));
        let command = body.get("command").and_then(Value::as_str).unwrap_or("");
        assert!(command.contains("test_"), "{command}");
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn a_tests_folder_without_init_kills_a_comparison() {
        let package = temp("py-widget");
        fs::write(package.join("src/widget.py"), "def gate(n):\n    return n > 0\n").unwrap();
        fs::create_dir_all(package.join("tests")).unwrap();
        fs::write(
            package.join("tests/test_widget.py"),
            "import sys\nfrom pathlib import Path\nsys.path.insert(0, str(Path(__file__).resolve().parents[1] / \"src\"))\nimport unittest\nimport widget\n\nclass WidgetTests(unittest.TestCase):\n    def test_zero(self):\n        self.assertFalse(widget.gate(0))\n    def test_one(self):\n        self.assertTrue(widget.gate(1))\n",
        )
        .unwrap();
        assert!(!package.join("tests/__init__.py").exists());
        let patches = generate_into(&package);
        let out = package.join("out");
        let envelope = run_with(&package, &patches, &out, &[]);
        assert!(envelope.ok, "{}", envelope.summary);
        assert_eq!(one_result(&out).get("outcome").and_then(Value::as_str), Some("killed"));
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn a_go_profile_marks_a_dark_line_and_runs_a_hit_with_no_name() {
        let package = temp("goprofile");
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
        lines.sort();
        lines.dedup();
        assert!(lines.len() >= 2, "{lines:?}");
        let dark = lines[0];
        let hit = lines[1];
        let profile = format!("mode: set\nsrc/A.java:{dark}.1,{dark}.8 1 0\nsrc/A.java:{hit}.1,{hit}.8 1 1\n");
        let map = package.join("cover.out");
        fs::write(&map, profile).unwrap();
        let out = package.join("out");
        let envelope = run_with(
            &package,
            &patches,
            &out,
            &["--suite-command", "echo ok", "--line-map", &map.display().to_string()],
        );
        assert!(envelope.ok, "{}", envelope.summary);
        assert!(envelope.next.contains("error:"), "{}", envelope.next);
        let mut saw_dark = false;
        let mut saw_hit = false;
        for item in &items {
            let id = item.get("id").and_then(Value::as_str).unwrap();
            let start = item.get("start").and_then(Value::as_int).unwrap() as usize;
            let line = decide::line_at(&source, start);
            let body = json::parse(&fs::read_to_string(out.join("results").join(format!("{id}.json"))).unwrap()).unwrap();
            if line == dark {
                assert_eq!(body.get("outcome").and_then(Value::as_str), Some("no coverage"));
                assert!(body.get("command").is_none(), "{body:?}");
                saw_dark = true;
            }
            if line == hit {
                assert_eq!(body.get("command").and_then(Value::as_str), Some("echo ok"));
                saw_hit = true;
            }
        }
        assert!(saw_dark && saw_hit);
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn an_empty_test_list_runs_the_suite_and_a_missing_line_does_not() {
        let package = temp("empty-names");
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
        lines.sort();
        lines.dedup();
        let covered = lines[0];
        let map = package.join("map.json");
        fs::write(
            &map,
            format!(r#"{{"files":{{"src/A.java":[{{"start":{covered},"end":{covered},"tests":[]}}]}}}}"#),
        )
        .unwrap();
        let out = package.join("out");
        let envelope = run_with(
            &package,
            &patches,
            &out,
            &["--suite-command", "echo ok", "--line-map", &map.display().to_string()],
        );
        assert!(envelope.ok, "{}", envelope.summary);
        let mut ran = 0usize;
        let mut dark = 0usize;
        for item in &items {
            let id = item.get("id").and_then(Value::as_str).unwrap();
            let start = item.get("start").and_then(Value::as_int).unwrap() as usize;
            let line = decide::line_at(&source, start);
            let body = json::parse(&fs::read_to_string(out.join("results").join(format!("{id}.json"))).unwrap()).unwrap();
            if line == covered {
                assert_eq!(body.get("command").and_then(Value::as_str), Some("echo ok"));
                ran += 1;
            } else {
                assert_eq!(body.get("outcome").and_then(Value::as_str), Some("no coverage"));
                assert!(body.get("command").is_none(), "{body:?}");
                dark += 1;
            }
        }
        assert!(ran >= 1 && dark >= 1);
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn a_pom_without_jacoco_does_not_create_lines_json() {
        let package = temp("pom");
        fs::write(package.join("pom.xml"), "<project><build></build></project>\n").unwrap();
        fs::create_dir_all(package.join("target/site/jacoco")).unwrap();
        fs::write(package.join("target/site/jacoco/jacoco.xml"), "<report/>\n").unwrap();
        let out = package.join("out");
        fs::create_dir_all(&out).unwrap();
        let collected = collect_java(&package, &out);
        assert!(matches!(collected, Collect::Empty(note) if note.contains("did not write a coverage report")));
        assert!(!out.join("lines.json").exists());
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn coverage_commands_match_the_discovery_shapes() {
        assert!(is_go_coverage("go test ./..."));
        assert!(!is_go_coverage("go test"));
        assert!(is_node_coverage("node --test"));
        assert!(!is_node_coverage("node --test --watch"));
        assert!(is_java_coverage("mvn -B test"));
        assert!(is_java_coverage("mvn -B test -Dtest={tests,}"));
        assert!(!is_java_coverage("mvn test"));
        assert!(is_rust_coverage("cargo test"));
        assert!(!is_rust_coverage("cargo test --offline"));
    }

    #[test]
    fn rust_coverage_stays_empty_when_llvm_cov_is_absent() {
        if tool_on_path("cargo-llvm-cov") {
            return;
        }
        let package = temp("llvm");
        fs::write(package.join("Cargo.toml"), "[package]\nname = \"t\"\nversion = \"0.0.0\"\n").unwrap();
        let out = package.join("out");
        fs::create_dir_all(&out).unwrap();
        let collected = collect_rust(&package, &out, Duration::from_secs(1));
        assert!(matches!(collected, Collect::Empty(note) if note.contains("not on PATH")));
        assert!(!out.join("lines.json").exists());
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn the_contract_names_the_discovery_markers() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let readme = fs::read_to_string(root.join("README.md")).unwrap();
        let agents = fs::read_to_string(root.join("AGENTS.md")).unwrap();
        for text in [&readme, &agents] {
            assert!(text.contains("Package.swift"), "{text}");
            assert!(text.contains(".csproj"));
            assert!(text.contains("Makefile"));
            assert!(text.contains("test_*.py"));
        }
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
