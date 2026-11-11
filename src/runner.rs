//! `mutate run`. A red baseline, a missing report, a timeout, and a mutant that did not build
//! never become a kill count. A large batch with no line map stops before the first mutant.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::args::Flags;
use crate::decide::{self, Baseline, Counts, Selection, Verdict};
use crate::envelope::Envelope;
use crate::json::{self, Value};
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
    /// From `mutants.json`. A hand-made mutant may leave it out.
    line: Option<i64>,
}

#[derive(Clone, Debug)]
struct Scored {
    kind: Verdict,
    duration_ms: u64,
    timed_out: bool,
    killed_by: Vec<String>,
    direction: String,
    reused: bool,
    mutant_hash: String,
    agent: String,
    cause: String,
    /// The last lines of a run that errored or did not build, so a refusal can show why.
    output: String,
}

impl Scored {
    fn new(kind: Verdict, duration_ms: u64, timed_out: bool) -> Scored {
        Scored {
            kind,
            duration_ms,
            timed_out,
            killed_by: Vec::new(),
            direction: String::new(),
            reused: false,
            mutant_hash: String::new(),
            agent: String::new(),
            cause: String::new(),
            output: String::new(),
        }
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
    /// The last lines the baseline printed. Not stored in `baseline.json`.
    tail: String,
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
    if let Err(message) = read_project(&package) {
        return Envelope::new(false, command, message, "Fix .probatio.json and run again. Discovery was not used.");
    }
    let options = match crate::run_options::RunOptions::from_flags(flags) {
        Ok(options) => options,
        Err(message) => return missing(command, &message),
    };
    let mut suite = match suite_command(flags, &package) {
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
    if let Some(dir) = flags.get("tests-dir") {
        let explicit = flags.get("suite-command").is_some() || project_suite(&package).ok().flatten().is_some();
        if !explicit && !package.join(dir).is_dir() {
            return attach_suite(
                Envelope::new(false, command, format!("no tests in {dir}"), "Pass --suite-command, or create that directory."),
                &suite,
            );
        }
    }
    if flags.get("tests-dir").is_some() && flags.get("suite-command").is_none() && parse_python(&suite).is_some() {
        let dir = flags.get("tests-dir").unwrap_or("tests");
        if !suite.split_whitespace().any(|word| word == dir) {
            suite.push(' ');
            suite.push_str(dir);
        }
    }
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
    crate::diffscope::ignore_own_output(&out);
    if let Err(error) = fs::create_dir_all(&out) {
        return attach_suite(
            Envelope::new(false, command, error.to_string(), "The out directory could not be created."),
            &suite,
        );
    }

    let explicit_timeout = match flags.parse_int("timeout-ms") {
        Ok(value) => value,
        Err(message) => return missing(command, &message),
    };
    let timeout_ms = match explicit_timeout {
        Some(value) => value.max(1),
        None => match flags.int("suite-timeout-ms", DEFAULT_TIMEOUT_MS) {
            Ok(value) => value.max(1),
            Err(message) => return missing(command, &message),
        },
    };
    let timeout = Duration::from_millis(timeout_ms);
    let unset = options.unset.clone();
    let project = project_env(&package);
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

    if let Some(history) = &options.history {
        crate::run_options::copy_history_map(history, &out);
    }
    let baseline_saved = read_baseline(&out.join("baseline.json"))
        .is_some_and(|saved| saved.kind == Baseline::Green && saved.template.as_deref() == Some(suite.as_str()));
    if !baseline_saved {
        if let Some(message) = run_build(options.build.as_deref(), &package, &package, &project, &unset, timeout, true) {
            return attach_suite(Envelope::new(false, command, message, "Nothing was mutated."), &suite);
        }
    }
    let mut coverage_note: Option<String> = None;
    let mut baseline = if let Some(parsed) = &python {
        let python_suite = PythonSuite { parsed, program: &python_program, files: &python_files };
        let suite_env = SuiteEnv { timeout, env: &project, unset: &unset };
        match python_baseline(&package, &out, &suite, &python_suite, &baseline_tests, caller_line_map.is_none(), &suite_env) {
            Ok(value) => value,
            Err(envelope) => return attach_suite(*envelope, &suite),
        }
    } else {
        prepared_baseline(&package, &out, &suite, &baseline_suite, timeout, &project, &unset)
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
        } else if baseline.line_map.is_none() && usable_line_map(&out.join("lines.json").display().to_string()) {
            map = load_map_file(&out.join("lines.json"));
            baseline.line_map = Some("lines.json".into());
            has_line_map = true;
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
    if let Some((summary, next)) = decide::baseline_refusal(baseline.kind.clone()) {
        let mut next = next.to_string();
        if store_python_stub(&baseline.tail) {
            next.push_str(" python3 here is the Microsoft Store placeholder, not Python. Install Python, or pass suiteCommand with the full path of python.exe.");
        } else if baseline.kind == Baseline::NoReport && decide::ran_no_tests(&baseline.tail.replace(" | ", "\n")) {
            next.push_str(" The suite exited 0 and counted 0 tests (# tests 0). Check that the test runner finds the test files.");
        } else if !baseline.tail.is_empty() {
            next.push_str(&format!(" Last output: {}", baseline.tail));
        }
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
                    "--confirm-count does not match the mutants still waiting.".to_string(),
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

    let repo = if let Some(path) = options.repo.clone() {
        if !path.is_dir() || !worktree::is_git_repo(&path) {
            return attach_suite(
                Envelope::new(false, command, "repo is not a git repository", "Pass --repo as the git root."),
                &suite,
            );
        }
        path
    } else {
        // The checkout's top, not the package: a package that is a workspace member or a module
        // needs the rest of the checkout (the workspace root manifest, sibling crates) to build.
        // The suite still runs in the package's own folder inside the worktree.
        worktree::toplevel(&package).unwrap_or_else(|| package.clone())
    };
    let git = worktree::is_git_repo(&repo);
    let pinned = if let Some(name) = options.commit.clone() {
        if !git {
            return attach_suite(
                Envelope::new(false, command, "that commit does not resolve", "Pass --commit that exists in the repository."),
                &suite,
            );
        }
        match worktree::resolve_commit(&repo, &name) {
            Ok(sha) => Some(sha),
            Err(message) => {
                return attach_suite(Envelope::new(false, command, message, "Pass --commit that exists in the repository."), &suite)
            }
        }
    } else {
        None
    };
    let run_id = worktree::run_id_for(&out);
    if git {
        let _ = worktree::remove_leftovers(&repo, &run_id);
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
    let budget_ms = flags.parse_int("budget-ms").unwrap_or(None).or_else(|| {
        flags.parse_int("max-minutes").ok().flatten().map(|minutes| minutes.saturating_mul(60_000))
    });
    let score_timeout = Duration::from_millis(if explicit_timeout.is_some() {
        timeout_ms
    } else {
        crate::run_options::mutant_timeout_ms(timeout_ms, baseline.duration_ms, options.timeout_multiple, options.timeout_floor_ms)
    });
    let history = options.history.as_deref().map(crate::run_options::load_history).unwrap_or_default();
    let tests_dir = options.tests_dir.clone().unwrap_or_else(|| "tests".into());
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
        let mutant_hash = crate::run_options::patch_hash(
            &patches,
            &mutant.id,
            &mutant.file,
            mutant.start,
            mutant.end,
            &mutant.original,
            &mutant.replacement,
        );
        if let Some(entry) = history.iter().find(|entry| entry.id == mutant.id) {
            if crate::run_options::reusable(entry, &mutant_hash, &package) {
                if let Err(error) = write_reused(&path, mutant, entry, options.agent.as_deref()) {
                    return attach_suite(Envelope::new(false, command, error, "A mutant result could not be written."), &suite);
                }
                if let Some(kind) = verdict_of_file(&path) {
                    verdicts.push(kind);
                }
                started_this_call += 1;
                continue;
            }
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
        let mut selection = decide::cap_tests(&tests, max_selected);
        let mut affected_files = Vec::new();
        if options.affected {
            let found = crate::run_options::affected_tests(&package, &tests_dir, &mutant.file);
            if !found.is_empty() {
                affected_files = found.clone();
                if selection.tests.is_empty() {
                    selection = decide::cap_tests(&found, max_selected);
                } else {
                    let hit: Vec<String> = selection
                        .tests
                        .iter()
                        .filter(|name| found.iter().any(|file| *name == file || name.ends_with(file) || file.ends_with(name.as_str())))
                        .cloned()
                        .collect();
                    if !hit.is_empty() {
                        selection = decide::cap_tests(&hit, max_selected);
                    }
                }
            }
        }
        if selection.capped {
            any_capped = true;
        }
        let (rendered, launch) = if let Some(parsed) = &python {
            let pass_names = launcher_selects || command_asks_for_names(&suite);
            let mut names = if pass_names { selection.tests.clone() } else { Vec::new() };
            let mut files = python_files.clone();
            if !affected_files.is_empty() {
                if names.is_empty() {
                    names = affected_files.clone();
                }
                files = affected_files.clone();
            }
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
                    files,
                    extra: parsed.extra.clone(),
                    kind: parsed.kind,
                }),
            )
        } else if is_node_test(&suite) {
            let reporter = ensure_node_reporter(&out).unwrap_or_else(|_| out.join("fail-reporter.mjs"));
            let file_timeout = crate::run_options::node_test_timeout_ms(&package, &affected_files, options.test_timeout_ms, timeout_ms);
            let rendered = node_test_command(&reporter, &selection.tests, &affected_files, options.concurrency, file_timeout);
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
            direction: options.direction,
            unset: unset.clone(),
            build: options.build.clone(),
            agent: options.agent.clone().unwrap_or_default(),
            commit: pinned.clone(),
            mutant_hash,
            project_env: project.clone(),
        });
        started_this_call += 1;
    }
    let workers_n = flags.int("workers", 1).unwrap_or(1).max(1) as usize;
    // Written before the first mutant, so a second agent's tally can count what is still waiting.
    let _ = write_run_json(&out, flags, &suite, false);
    let place = Place { git, repo: &repo, package: &package, timeout: score_timeout };
    if let Err(error) = run_jobs(&jobs, workers_n, &place, &mut verdicts, max_selected) {
        return attach_suite(
            Envelope::new(false, command, error, "Results written so far are kept. Fix the cause, then make the same call: it scores the mutants that have no result."),
            &suite,
        );
    }

    let still: Vec<&str> = mutants.iter().filter(|mutant| !result_path(&out, &mutant.id).exists()).map(|mutant| mutant.id.as_str()).collect();
    let waiting_path = out.join("not-started.txt");
    if still.is_empty() {
        let _ = fs::remove_file(&waiting_path);
    } else {
        let _ = fs::write(&waiting_path, format!("{}\n", still.join("\n")));
    }

    let counts = decide::counts_of(&verdicts);
    if counts.errors > 0 && counts.errors * 2 > counts.finished {
        return attach_suite(broken_tree(&counts, &jobs), &suite);
    }
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
        envelope.next_call = Some(resume_argv(flags, &suite, duration_ms(score_timeout), auto.as_deref()));
    } else if counts.survived > 0 {
        envelope.next_call = Some(vec!["mutate".into(), "tally".into(), "--out".into(), out.display().to_string()]);
    }
    let _ = write_history_file(&out, options.agent.as_deref());
    let _ = write_run_json(&out, flags, &suite, counts.survived > 0);
    envelope
}

/// Most mutants errored before a test could judge them: the mutant tree did not build or start.
/// That is never a pass, so it is a refusal with no kill count. The error results are removed,
/// so the same call scores those mutants again once the cause is fixed.
fn broken_tree(counts: &Counts, jobs: &[ScoreJob]) -> Envelope {
    let mut first = String::new();
    for job in jobs {
        let Ok(text) = fs::read_to_string(&job.path) else { continue };
        let Ok(value) = json::parse(&text) else { continue };
        if value.get("outcome").and_then(Value::as_str) != Some("error") {
            continue;
        }
        if first.is_empty() {
            first = value.get("output").and_then(Value::as_str).unwrap_or("").to_string();
        }
        let _ = fs::remove_file(&job.path);
    }
    let mut next = "The mutated tree did not build or the suite did not start, so no test judged these mutants. Run the suite command in the package to see the failure, fix it, then make the same call. The tree holds your tracked files and the untracked files git does not ignore: if the build needs an ignored file, list it in .probatio.json as \"copy\": [\"path\"].".to_string();
    if !first.is_empty() {
        next.push_str(&format!(" First error: {first}"));
    }
    Envelope::new(
        false,
        "mutate.run",
        format!("{} of {} mutants errored before any test ran. This run is not a score.", counts.errors, counts.finished),
        next,
    )
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
    forward_run_flags(&mut argv, flags);
    argv
}

pub(crate) fn forward_run_flags(argv: &mut Vec<String>, flags: &Flags) {
    for key in [
        "repo",
        "direction",
        "concurrency",
        "max-minutes",
        "max-mutants",
        "build",
        "tests-dir",
        "suite-timeout-ms",
        "test-timeout-ms",
        "agent",
        "history",
        "timeout-multiple",
        "timeout-floor-ms",
        "budget-ms",
        "commit",
        "workers",
    ] {
        if let Some(value) = flags.get(key) {
            if value != "true" {
                argv.push(format!("--{key}"));
                argv.push(value.to_string());
            }
        }
    }
    for key in ["unset", "hide", "only-test"] {
        for value in flags.texts(key) {
            argv.push(format!("--{key}"));
            argv.push(value);
        }
    }
    if flags.bool("affected", false) {
        argv.push("--affected".into());
    }
    if !flags.bool("confirm", true) {
        argv.push("--no-confirm".into());
    }
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
    if let Some(copy) = value.get("copy") {
        let Value::Array(items) = copy else {
            return Err(".probatio.json copy must be an array of paths".into());
        };
        for item in items {
            let Some(path) = item.as_str().filter(|path| !path.trim().is_empty()) else {
                return Err(".probatio.json copy must be an array of non-empty paths".into());
            };
            if Path::new(path).is_absolute() || path.split(['/', '\\']).any(|part| part == "..") {
                return Err(format!(".probatio.json copy entry {path} must be a path inside the package"));
            }
        }
    }
    for name in ["testLine", "passLine"] {
        if let Some(pattern) = value.get(name) {
            let Some(text) = pattern.as_str().filter(|text| !text.is_empty()) else {
                return Err(format!(".probatio.json {name} must be a non-empty string"));
            };
            if let Err(error) = crate::script_line::compile(text) {
                return Err(format!(".probatio.json {name} is not a regular expression ({error})"));
            }
        }
    }
    Ok(Some(value))
}

/// `.probatio.json` `copy`: files or folders, from the package, copied into the mutant tree
/// though git ignores them.
fn project_copy(package: &Path) -> Vec<String> {
    let Ok(Some(value)) = read_project(package) else {
        return Vec::new();
    };
    match value.get("copy") {
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).map(|path| path.trim().replace('\\', "/")).collect(),
        _ => Vec::new(),
    }
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
    } else if root_file_ends_with(package, &[".c", ".cc", ".cpp", ".h", ".hpp", ".s", ".S", ".asm"]) {
        // C, C++, and assembly are all scored through the project's own `make test`.
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
    env: &[(String, String)],
    unset: &[String],
) -> BaselineRun {
    let path = out.join("baseline.json");
    if let Some(saved) = read_baseline(&path) {
        if saved.kind == Baseline::Green && saved.template.as_deref() == Some(template) {
            return saved;
        }
    }
    let pairs: Vec<(&str, &str)> = env.iter().map(|(key, value)| (key.as_str(), value.as_str())).collect();
    let unset_ref: Vec<&str> = unset.iter().map(String::as_str).collect();
    let timed = process::run_shell_prepared(rendered, Some(package), Some(package), timeout, &pairs, &unset_ref);
    let mut run = baseline_from(&timed);
    if timed.spawned && !timed.timed_out {
        if let Some(kind) = shell_baseline(&timed, package) {
            run.kind = kind;
        }
        if store_python_stub(&timed.stderr) {
            run.kind = Baseline::MissingTool;
        }
    }
    run.template = Some(template.to_string());
    run
}

fn baseline_from(timed: &TimedRun) -> BaselineRun {
    BaselineRun {
        kind: decide::judge_baseline(timed.spawned, timed.timed_out, timed.status, &timed.stdout, &timed.stderr),
        duration_ms: Some(duration_ms(timed.duration)),
        template: None,
        line_map: None,
        tail: if decide::ran_no_tests(&timed.stdout) { "# tests 0".into() } else { output_tail(&timed.stdout, &timed.stderr) },
    }
}

fn shell_baseline(timed: &TimedRun, package: &Path) -> Option<Baseline> {
    let config = script_config(package)?;
    match crate::script_line::report(&timed.stdout, &timed.stderr, timed.status, &config)? {
        crate::script_line::ScriptHit::Silent => Some(Baseline::NoReport),
        crate::script_line::ScriptHit::Named(failed) if failed.is_empty() => Some(Baseline::Green),
        crate::script_line::ScriptHit::Named(_) => Some(Baseline::Red),
    }
}

fn apply_shell_report(mut scored: Scored, timed: &TimedRun, package: &Path) -> Scored {
    let Some(config) = script_config(package) else { return scored };
    match crate::script_line::report(&timed.stdout, &timed.stderr, timed.status, &config) {
        Some(crate::script_line::ScriptHit::Silent) => Scored::new(Verdict::Error, scored.duration_ms, false),
        Some(crate::script_line::ScriptHit::Named(failed)) if failed.is_empty() => {
            scored.kind = Verdict::Survived;
            scored.killed_by.clear();
            scored
        }
        Some(crate::script_line::ScriptHit::Named(failed)) => {
            scored.kind = Verdict::Killed;
            scored.killed_by = failed;
            scored
        }
        None => scored,
    }
}

fn script_config(package: &Path) -> Option<crate::script_line::ScriptConfig> {
    let value = read_project(package).ok().flatten()?;
    let config = crate::script_line::config_of(&value);
    if config.test_line.is_none() && config.pass_line.is_none() {
        // TAP is still read when a shell suite prints it.
        return Some(config);
    }
    Some(config)
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
    Some(BaselineRun { kind, duration_ms, template, line_map, tail: String::new() })
}

fn result_path(out: &Path, id: &str) -> PathBuf {
    out.join("results").join(format!("{id}.json"))
}

#[derive(Clone)]
struct ScoreJob {
    mutant: LoadedMutant,
    rendered: String,
    launch: Option<PythonLaunch>,
    path: PathBuf,
    selection: decide::Selection,
    confirm: bool,
    hide: Vec<String>,
    direction: crate::run_options::Direction,
    unset: Vec<String>,
    build: Option<String>,
    agent: String,
    commit: Option<String>,
    mutant_hash: String,
    project_env: Vec<(String, String)>,
}

/// Where and how long each mutant runs: a git worktree of `repo`, or a copy of `package`.
struct Place<'a> {
    git: bool,
    repo: &'a Path,
    package: &'a Path,
    timeout: Duration,
}

fn run_jobs(jobs: &[ScoreJob], workers: usize, place: &Place, verdicts: &mut Vec<Verdict>, max_selected: usize) -> Result<(), String> {
    if jobs.is_empty() {
        return Ok(());
    }
    let count = workers.max(1).min(jobs.len());
    let source = if place.git { place.repo } else { place.package };
    let mut slots = crate::slots::acquire(source, place.git, count)?;
    let pinned = jobs.iter().any(|job| job.commit.is_some());
    if !pinned {
        let extra: Vec<String> = project_copy(place.package).iter().map(|rel| tree_rel(rel, place)).collect();
        for slot in &mut slots {
            slot.prepare(&extra).map_err(|error| format!("The mutant tree could not be made: {error}"))?;
            let root = slot_root(slot, place);
            link_node_modules(place.package, &root)?;
        }
        // One unmutated run in each tree, at once: it builds what the kept target directory does not
        // have yet (a first run, or files a mutant edited last time), so no mutant's clock is spent
        // on that, and it shows the suite passes in the tree as it does in your package.
        let checks: Vec<Result<(), String>> = std::thread::scope(|scope| {
            let handles: Vec<_> = slots.iter_mut().map(|slot| scope.spawn(|| check_tree(&jobs[0], slot, place))).collect();
            handles.into_iter().map(|handle| handle.join().unwrap_or_else(|_| Err("the tree check stopped".into()))).collect()
        });
        if let Some(Err(message)) = checks.into_iter().find(Result::is_err) {
            return Err(message);
        }
    }
    let result = if count <= 1 {
        let slot = &mut slots[0];
        let mut outcome = Ok(());
        for job in jobs {
            let scored = match score_in_slot(job, slot, place) {
                Ok(scored) => scored,
                Err(error) => {
                    outcome = Err(error);
                    break;
                }
            };
            if let Err(error) = write_result(&job.path, &job.mutant, &scored, &job.selection, max_selected, Some(&ran_command(job, slot))) {
                outcome = Err(error);
                break;
            }
            verdicts.push(scored.kind);
        }
        outcome
    } else {
        let next = std::sync::Mutex::new(0usize);
        let found = std::sync::Mutex::new(Vec::new());
        let error = std::sync::Mutex::new(None);
        std::thread::scope(|scope| {
            for slot in slots.iter_mut() {
                let next = &next;
                let found = &found;
                let error = &error;
                scope.spawn(move || loop {
                    let index = {
                        let mut cursor = next.lock().unwrap_or_else(|poison| poison.into_inner());
                        if *cursor >= jobs.len() || error.lock().unwrap_or_else(|poison| poison.into_inner()).is_some() {
                            return;
                        }
                        let index = *cursor;
                        *cursor += 1;
                        index
                    };
                    let job = &jobs[index];
                    let written = score_in_slot(job, slot, place).and_then(|scored| {
                        write_result(&job.path, &job.mutant, &scored, &job.selection, max_selected, Some(&ran_command(job, slot))).map(|_| scored.kind)
                    });
                    match written {
                        Ok(kind) => found.lock().unwrap_or_else(|poison| poison.into_inner()).push(kind),
                        Err(message) => {
                            *error.lock().unwrap_or_else(|poison| poison.into_inner()) = Some(message);
                            return;
                        }
                    }
                });
            }
        });
        verdicts.extend(found.lock().unwrap_or_else(|poison| poison.into_inner()).iter().copied());
        let failed = error.lock().unwrap_or_else(|poison| poison.into_inner()).clone();
        match failed {
            Some(message) => Err(message),
            None => Ok(()),
        }
    };
    if pinned {
        // A pinned commit's tree is checked out fresh for each mutant, so its build output is not
        // reusable by a run of the working tree: it goes with the run.
        for slot in &slots {
            let _ = crate::slots::remove_dir_robust(&pinned_target(slot));
        }
    }
    // Dropping the slots removes each tree and frees its lock. `target/` stays for the next run.
    drop(slots);
    result
}

/// The suite once in a fresh tree, with no mutant, and a long clock: the first build into a cold
/// target directory can take far longer than the baseline did in your warm one. A tree where the
/// suite fails or does not build would score every mutant wrong, so that stops the run.
fn check_tree(job: &ScoreJob, slot: &mut crate::slots::Slot, place: &Place) -> Result<(), String> {
    let mut job = job.clone();
    // An empty edit at the start of the file: the same text, in either direction.
    job.mutant.start = 0;
    job.mutant.end = 0;
    job.mutant.original = String::new();
    job.mutant.replacement = String::new();
    job.confirm = false;
    let clock = place.timeout.saturating_mul(10).max(Duration::from_secs(1800));
    let check = Place { timeout: clock, ..*place };
    let scored = score_in_slot(&job, slot, &check)?;
    // A test that fails with no mutant would make every mutant read as killed. A tree that does not
    // build, or a selection that runs no test, errors every mutant too, and the most-errored
    // refusal after the mutants says so with the first error; this check does not stop for it.
    match scored.kind {
        Verdict::Killed => Err(format!(
            "The suite passes in your package but fails in the mutant tree, with no mutant applied, so no mutant can be scored.{} The tree holds your tracked files and the untracked files git does not ignore. If a test needs an ignored file or folder, list it in .probatio.json as \"copy\": [\"path\"] (from the package), then make the same call. If the same call passes, the test is flaky.",
            if scored.killed_by.is_empty() { String::new() } else { format!(" Failing: {}.", scored.killed_by.join(", ")) }
        )),
        _ => Ok(()),
    }
}

/// Where the suite runs inside a slot: the package's folder inside the tree.
fn slot_root(slot: &crate::slots::Slot, place: &Place) -> PathBuf {
    if place.git {
        suite_root(&slot.tree, place.repo, place.package)
    } else {
        slot.tree.clone()
    }
}

/// The command a mutant ran, as its result records it: `--target-dir` pointed at the slot.
fn ran_command(job: &ScoreJob, slot: &crate::slots::Slot) -> String {
    with_target_dir(&job.rendered, &slot_target(job, slot))
}

fn slot_target(job: &ScoreJob, slot: &crate::slots::Slot) -> PathBuf {
    if job.commit.is_some() { pinned_target(slot) } else { slot.target.clone() }
}

fn pinned_target(slot: &crate::slots::Slot) -> PathBuf {
    slot.dir.join("target-pinned")
}

/// The mutated file's path from the top of the slot tree.
fn tree_rel(file: &str, place: &Place) -> String {
    let file = file.replace('\\', "/");
    if !place.git {
        return file;
    }
    let repo = place.repo.canonicalize().unwrap_or_else(|_| place.repo.to_path_buf());
    let package = place.package.canonicalize().unwrap_or_else(|_| place.package.to_path_buf());
    match package.strip_prefix(&repo) {
        Ok(rel) if !rel.as_os_str().is_empty() => format!("{}/{file}", rel.display().to_string().replace('\\', "/")),
        _ => file,
    }
}

/// Score one mutant in a slot. Every mutant build gets the slot's own Cargo target directory: it
/// is set for every suite, including a script that calls cargo, and it replaces any
/// `CARGO_TARGET_DIR` from the environment or `.probatio.json`, and the value of a
/// `--target-dir` in the suite command. A mutant built into your target directory would leave
/// mutated artifacts there, which cargo could later reuse for your own tree.
fn score_in_slot(job: &ScoreJob, slot: &mut crate::slots::Slot, place: &Place) -> Result<Scored, String> {
    let target = slot_target(job, slot);
    let mut job = job.clone();
    job.project_env.retain(|(key, _)| key != "CARGO_TARGET_DIR");
    job.unset.retain(|key| key != "CARGO_TARGET_DIR");
    job.project_env.push(("CARGO_TARGET_DIR".into(), target.display().to_string()));
    job.rendered = with_target_dir(&job.rendered, &target);
    if let Some(commit) = job.commit.as_deref() {
        // A named commit: a fresh checkout for each mutant, nothing copied over it.
        if let Err(message) = slot.checkout(commit) {
            return Ok(error_scored(message));
        }
        let root = slot_root(slot, place);
        if let Err(message) = link_node_modules(place.package, &root) {
            return Ok(error_scored(message));
        }
        return Ok(apply_and_run(&job, &root, place.package, place.timeout));
    }
    slot.mark_dirty(&tree_rel(&job.mutant.file, place));
    let root = slot_root(slot, place);
    let scored = apply_and_run(&job, &root, place.package, place.timeout);
    slot.refresh().map_err(|error| format!("The mutant tree could not be restored after {}: {error}", job.mutant.id))?;
    Ok(scored)
}

/// Replace the value of every `--target-dir` (`--target-dir X` or `--target-dir=X`) with `dir`.
fn with_target_dir(command: &str, dir: &Path) -> String {
    const FLAG: &str = "--target-dir";
    let mut out = String::new();
    let mut rest = command;
    while let Some(at) = rest.find(FLAG) {
        let before_ok = at == 0 || rest[..at].ends_with(|ch: char| ch.is_whitespace());
        let after = &rest[at + FLAG.len()..];
        let joined = after.starts_with('=');
        let spaced = after.starts_with([' ', '\t']);
        if !before_ok || !(joined || spaced) {
            out.push_str(&rest[..at + FLAG.len()]);
            rest = after;
            continue;
        }
        out.push_str(&rest[..at + FLAG.len()]);
        let value_start = if joined { 1 } else { after.len() - after.trim_start().len() };
        out.push_str(&after[..value_start]);
        let value = &after[value_start..];
        let length = shell_word_len(value);
        out.push_str(&shell_quote(dir));
        rest = &value[length..];
    }
    out.push_str(rest);
    out
}

/// Bytes in the shell word at the start of `text`: quotes and backslash escapes included.
fn shell_word_len(text: &str) -> usize {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (at, ch) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match quote {
            Some(open) if ch == open => quote = None,
            Some(_) => {}
            None if ch == '\'' || ch == '"' => quote = Some(ch),
            None if ch == '\\' => escaped = true,
            None if ch.is_whitespace() || matches!(ch, ';' | '&' | '|' | ')') => return at,
            None => {}
        }
    }
    text.len()
}

fn suite_root(dest: &Path, repo: &Path, package: &Path) -> PathBuf {
    let repo_c = repo.canonicalize().unwrap_or_else(|_| repo.to_path_buf());
    let package_c = package.canonicalize().unwrap_or_else(|_| package.to_path_buf());
    match package_c.strip_prefix(&repo_c) {
        Ok(rel) if !rel.as_os_str().is_empty() => dest.join(rel),
        _ => dest.to_path_buf(),
    }
}

fn apply_and_run(job: &ScoreJob, root: &Path, package: &Path, timeout: Duration) -> Scored {
    for rel in &job.hide {
        if rel.is_empty() || rel.split(['/', '\\']).any(|part| part == "..") {
            continue;
        }
        let target = root.join(rel);
        if target.is_file() {
            let _ = fs::remove_file(&target);
        }
    }
    let file = match safe_join(root, &job.mutant.file) {
        Ok(path) => path,
        Err(message) => return error_scored(message),
    };
    let text = match fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) => return error_scored(error.to_string()),
    };
    let (updated, chosen) = match crate::run_options::apply_span(
        &text,
        job.mutant.start,
        job.mutant.end,
        &job.mutant.original,
        &job.mutant.replacement,
        job.direction,
    ) {
        Ok(applied) => applied,
        Err(message) => return error_scored(message),
    };
    if let Err(error) = fs::write(&file, updated) {
        return error_scored(error.to_string());
    }
    if let Some(message) = run_build(job.build.as_deref(), root, package, &job.project_env, &job.unset, timeout, false) {
        let mut scored = if message.contains("timed out") {
            Scored::new(Verdict::Timeout, timeout.as_millis() as u64, true)
        } else {
            let mut scored = Scored::new(Verdict::Unviable, 0, false);
            scored.cause = "build".into();
            scored
        };
        scored.direction = chosen.into();
        scored.mutant_hash = job.mutant_hash.clone();
        scored.agent = job.agent.clone();
        return scored;
    }
    let suite = &job.rendered;
    let timed = if let Some(launch) = &job.launch {
        launch_python(root, package, launch, None, None, &SuiteEnv { timeout, env: &job.project_env, unset: &job.unset })
    } else {
        let pairs: Vec<(&str, &str)> = job.project_env.iter().map(|(key, value)| (key.as_str(), value.as_str())).collect();
        let unset: Vec<&str> = job.unset.iter().map(String::as_str).collect();
        process::run_shell_prepared(suite, Some(root), Some(package), timeout, &pairs, &unset)
    };
    let mut scored = score_timed(&timed, job.launch.is_some() || suite.contains("--test-reporter="));
    if job.launch.is_none() && !suite.contains("--test-reporter=") {
        scored = apply_shell_report(scored, &timed, package);
        name_cargo_kills(&mut scored, suite, &timed);
        if scored.kind == Verdict::Unviable && !names_the_file(&timed, &job.mutant.file) {
            // A compiler rejecting the mutant names the mutated file. An error that does not
            // (cargo's "failed to find a workspace root") is the tree, and it is not a score.
            scored.kind = Verdict::Error;
            scored.cause = "build".into();
        }
    }
    if matches!(scored.kind, Verdict::Error | Verdict::Unviable) {
        scored.output = output_tail(&timed.stdout, &timed.stderr);
    }
    scored.direction = chosen.into();
    scored.mutant_hash = job.mutant_hash.clone();
    scored.agent = job.agent.clone();
    if job.confirm && scored.kind == Verdict::Killed {
        scored = confirm_kill(job, root, package, timeout, scored);
        scored.direction = chosen.into();
        scored.mutant_hash = job.mutant_hash.clone();
        scored.agent = job.agent.clone();
    }
    scored.killed_by = relative_names(&scored.killed_by, root);
    scored
}

/// The node reporter names a test by its absolute file, which is inside this mutant's worktree
/// or temp copy. That path changes per run and per worker, so the name is kept relative to it.
fn relative_names(names: &[String], root: &Path) -> Vec<String> {
    let mut prefixes = vec![root.display().to_string().replace('\\', "/")];
    if let Ok(real) = root.canonicalize() {
        let real = real.display().to_string().replace('\\', "/");
        prefixes.push(real.strip_prefix("//?/").unwrap_or(&real).to_string());
    }
    names
        .iter()
        .map(|name| {
            let forward = name.replace('\\', "/");
            let bare = forward.strip_prefix("file:///").or_else(|| forward.strip_prefix("file://")).unwrap_or(&forward);
            for prefix in &prefixes {
                let prefix = prefix.trim_start_matches('/').trim_end_matches('/');
                let body = bare.trim_start_matches('/');
                // Windows paths compare without case: node may print `c:` where Rust has `C:`.
                let same = body.len() > prefix.len()
                    && body.is_char_boundary(prefix.len())
                    && if cfg!(windows) { body[..prefix.len()].eq_ignore_ascii_case(prefix) } else { &body[..prefix.len()] == prefix };
                if same {
                    if let Some(rest) = body[prefix.len()..].strip_prefix('/') {
                        return rest.to_string();
                    }
                }
            }
            name.clone()
        })
        .collect()
}

fn confirm_kill(job: &ScoreJob, root: &Path, package: &Path, timeout: Duration, scored: Scored) -> Scored {
    let targets = if scored.killed_by.is_empty() { vec![String::new()] } else { scored.killed_by.clone() };
    let mut kept = Vec::new();
    for name in &targets {
        let mut hits = 0;
        for _ in 0..2 {
            let again = rerun_one(job, root, package, timeout, name);
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

fn rerun_one(job: &ScoreJob, root: &Path, package: &Path, timeout: Duration, name: &str) -> Scored {
    if let Some(launch) = &job.launch {
        let names = if name.is_empty() { launch.names.clone() } else { vec![name.to_string()] };
        let suite = SuiteEnv { timeout, env: &job.project_env, unset: &job.unset };
        let timed = launch_python(root, package, launch, Some(&names), None, &suite);
        return score_timed(&timed, true);
    }
    let suite = &job.rendered;
    let command = if !name.is_empty() && suite.contains("--test-reporter=") {
        with_concurrency(&node_pattern_command(suite, &[name.to_string()]), 1)
    } else if !name.is_empty() && is_cargo_test(suite) && !suite.contains(" -- ") {
        // Only the killing test, in every test binary: a confirm costs one test, not the suite.
        format!("{suite} -- --exact {}", shell_quote(Path::new(name)))
    } else {
        with_concurrency(suite, 1)
    };
    let pairs: Vec<(&str, &str)> = job.project_env.iter().map(|(key, value)| (key.as_str(), value.as_str())).collect();
    let unset: Vec<&str> = job.unset.iter().map(String::as_str).collect();
    let timed = process::run_shell_prepared(&command, Some(root), Some(package), timeout, &pairs, &unset);
    let mut scored = score_timed(&timed, command.contains("--test-reporter="));
    name_cargo_kills(&mut scored, &command, &timed);
    scored
}

/// True when the suite's output names the mutated file, by its file name.
fn names_the_file(timed: &TimedRun, file: &str) -> bool {
    let name = file.rsplit(['/', '\\']).next().unwrap_or(file);
    !name.is_empty() && (timed.stdout.contains(name) || timed.stderr.contains(name))
}

/// `cargo test`, with or without more arguments or a `{tests}` token.
fn is_cargo_test(command: &str) -> bool {
    let words = command_words(command);
    words.len() >= 2 && matches!(file_name(&words[0]), "cargo" | "cargo.exe") && words[1] == "test"
}

/// libtest prints `test <name> ... FAILED` for each failing test. Those are the kill's names,
/// with no coverage tool needed.
fn name_cargo_kills(scored: &mut Scored, command: &str, timed: &TimedRun) {
    if scored.kind == Verdict::Killed && scored.killed_by.is_empty() && is_cargo_test(command) {
        scored.killed_by = decide::cargo_failures(&timed.stdout);
    }
}

fn with_concurrency(command: &str, value: u64) -> String {
    let Some(start) = command.find("--test-concurrency=") else {
        return command.to_string();
    };
    let end = command[start..].find(' ').map(|index| start + index).unwrap_or(command.len());
    format!("{}--test-concurrency={value}{}", &command[..start], &command[end..])
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
    link_dir(&source, &linked)
}

#[cfg(unix)]
fn link_dir(source: &Path, linked: &Path) -> Result<(), String> {
    std::os::unix::fs::symlink(source, linked).map_err(|error| error.to_string())
}

/// A directory symlink needs Developer Mode or admin on Windows. A junction does not, so it is
/// the fallback. Either is removed as a link (`worktree::unlink_links`) before its tree is deleted.
#[cfg(windows)]
fn link_dir(source: &Path, linked: &Path) -> Result<(), String> {
    if std::os::windows::fs::symlink_dir(source, linked).is_ok() {
        return Ok(());
    }
    let status = std::process::Command::new("cmd")
        .args(["/D", "/C", "mklink", "/J"])
        .arg(linked)
        .arg(source)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|error| error.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("node_modules could not be linked into {}", linked.display()))
    }
}

#[cfg(not(any(unix, windows)))]
fn link_dir(_source: &Path, _linked: &Path) -> Result<(), String> {
    Ok(())
}

/// Never copied into a mutant's worktree or copy. `.probatio` is this tool's own output, which a
/// campaign usually writes inside the package. Copying it grew with every result.
pub(crate) fn skipped_in_copy(name: &str) -> bool {
    matches!(name, ".git" | "target" | "node_modules" | "dist" | ".probatio")
}

fn run_build(
    build: Option<&str>,
    dir: &Path,
    bin_root: &Path,
    env: &[(String, String)],
    unset: &[String],
    timeout: Duration,
    baseline: bool,
) -> Option<String> {
    let command = build?;
    let argv = crate::run_options::split_argv(command);
    let (program, args) = argv.split_first()?;
    let limit = timeout.min(Duration::from_millis(120_000));
    let pairs: Vec<(&str, &str)> = env.iter().map(|(key, value)| (key.as_str(), value.as_str())).collect();
    let unset_ref: Vec<&str> = unset.iter().map(String::as_str).collect();
    let timed = process::run_argv_prepared(program, args, Some(dir), Some(bin_root), limit, &pairs, &unset_ref);
    if timed.timed_out {
        return Some(if baseline { "baseline build timed out".into() } else { "build timed out".into() });
    }
    if !timed.spawned || timed.status != Some(0) {
        if baseline {
            let detail = clip_text(&format!("{}{}", timed.stderr, timed.stdout));
            return Some(format!("baseline build failed ({detail})"));
        }
        return Some("build failed".into());
    }
    None
}

fn clip_text(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    flat.chars().take(180).collect()
}

fn write_reused(path: &Path, mutant: &LoadedMutant, entry: &crate::run_options::HistoryEntry, agent: Option<&str>) -> Result<(), String> {
    let mut scored = Scored::new(parse_verdict(&entry.outcome).unwrap_or(Verdict::Error), 0, false);
    scored.killed_by = entry.killed_by.clone();
    scored.direction = entry.direction.clone().unwrap_or_default();
    scored.reused = true;
    scored.mutant_hash = entry.mutant_hash.clone();
    scored.agent = agent.unwrap_or("").to_string();
    let selection = Selection { tests: Vec::new(), selected_count: 0, capped: false };
    write_result(path, mutant, &scored, &selection, decide::DEFAULT_MAX_SELECTED, Some(""))
}

fn write_history_file(out: &Path, agent: Option<&str>) -> Result<(), String> {
    let mut files = Vec::new();
    if let Ok(read) = fs::read_dir(out.join("results")) {
        files.extend(read.filter_map(|entry| entry.ok()).map(|entry| entry.path()));
    }
    files.sort();
    let mut entries = Vec::new();
    for path in files {
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else { continue };
        let Ok(value) = json::parse(&text) else { continue };
        let mut row = Vec::new();
        for key in ["id", "outcome", "direction", "mutantHash", "agent", "command", "cause"] {
            if let Some(item) = value.get(key) {
                row.push((key.to_string(), item.clone()));
            }
        }
        if let Some(item) = value.get("killedBy") {
            row.push(("killedBy".into(), item.clone()));
        }
        if value.get("agent").is_none() {
            if let Some(agent) = agent {
                row.push(("agent".into(), Value::String(agent.into())));
            }
        }
        entries.push(Value::Object(row));
    }
    fs::write(out.join("history.json"), json::stringify(&Value::object(vec![("entries", Value::Array(entries))])))
        .map_err(|error| error.to_string())
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
        ("file", Value::String(mutant.file.clone())),
    ];
    if let Some(line) = mutant.line {
        entries.push(("line", Value::Int(line)));
    }
    entries.extend([
        ("outcome", Value::String(scored.kind.as_str().into())),
        ("selectedCount", Value::Int(selection.selected_count as i64)),
        ("selectionCapped", Value::Bool(selection.capped)),
        ("durationMs", Value::Int(scored.duration_ms as i64)),
        ("timedOut", Value::Bool(scored.timed_out)),
        ("tests", Value::Array(selection.tests.iter().cloned().map(Value::String).collect())),
    ]);
    if let Some(command) = command {
        entries.push(("command", Value::String(command.to_string())));
    }
    if selection.capped {
        entries.push(("capNote", Value::String(decide::cap_note(max_selected))));
    }
    if !scored.direction.is_empty() {
        entries.push(("direction", Value::String(scored.direction.clone())));
    }
    if !scored.agent.is_empty() {
        entries.push(("agent", Value::String(scored.agent.clone())));
    }
    if !scored.mutant_hash.is_empty() {
        entries.push(("mutantHash", Value::String(scored.mutant_hash.clone())));
    }
    if scored.reused {
        entries.push(("reused", Value::Bool(true)));
    }
    if !scored.cause.is_empty() {
        entries.push(("cause", Value::String(scored.cause.clone())));
    }
    if !scored.output.is_empty() {
        entries.push(("output", Value::String(scored.output.clone())));
    }
    if !scored.killed_by.is_empty() {
        entries.push((
            "killedBy",
            Value::Array(scored.killed_by.iter().cloned().map(Value::String).collect()),
        ));
    }
    // Whole or not at all: a status call or a tally reading the results while the run writes them
    // never sees half a file, and an interrupted write leaves no result, so resume reruns it.
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, json::stringify(&Value::Object(entries.into_iter().map(|(k, v)| (k.to_string(), v)).collect())))
        .map_err(|error| error.to_string())?;
    fs::rename(&temp, path).map_err(|error| error.to_string())
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

/// The ids a patches path holds, for a reader that only counts what is still waiting.
pub(crate) fn mutant_ids(patches: &Path) -> Option<Vec<String>> {
    load_mutants(patches).ok().map(|mutants| mutants.into_iter().map(|mutant| mutant.id).collect())
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
        line: value.get("line").and_then(Value::as_int),
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

/// Node loads `--test-reporter` as an ES module. On Windows an absolute `C:` path is read as a
/// URL scheme and refused (ERR_UNSUPPORTED_ESM_URL_SCHEME), so it is passed as a `file:` URL.
fn reporter_arg(reporter: &Path) -> String {
    if cfg!(windows) && reporter.is_absolute() {
        let text = reporter.display().to_string().replace('\\', "/");
        let text = text.strip_prefix("//?/").unwrap_or(&text).to_string();
        return format!("'file:///{}'", text.replace(' ', "%20").replace('\'', "%27"));
    }
    shell_quote(reporter)
}

fn node_test_command(reporter: &Path, names: &[String], files: &[String], concurrency: u64, test_timeout_ms: u64) -> String {
    let mut command = format!(
        "node --test --test-reporter={} --test-concurrency={concurrency} --test-timeout={test_timeout_ms}",
        reporter_arg(reporter)
    );
    if !names.is_empty() {
        let pattern = names.iter().map(|name| regex_escape(name)).collect::<Vec<_>>().join("|");
        command.push_str(&format!(" --test-name-pattern=^({pattern})$"));
    }
    for file in files {
        command.push(' ');
        command.push_str(&shell_quote(Path::new(file)));
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

/// What the Python baseline runs: the parsed command, its interpreter, and the files it walks.
struct PythonSuite<'a> {
    parsed: &'a ParsedPython,
    program: &'a str,
    files: &'a [String],
}

fn python_baseline(
    package: &Path,
    out: &Path,
    template: &str,
    python: &PythonSuite,
    baseline_tests: &[String],
    collect: bool,
    suite: &SuiteEnv,
) -> Result<BaselineRun, Box<Envelope>> {
    let parsed = python.parsed;
    let path = out.join("baseline.json");
    if let Some(saved) = read_baseline(&path) {
        let same = saved.kind == Baseline::Green && saved.template.as_deref() == Some(template);
        if same && (!collect || saved.line_map.is_some()) {
            return Ok(saved);
        }
    }
    let reporter = ensure_reporter(out, parsed.kind).map_err(|error| {
        Box::new(Envelope::new(false, "mutate.run", error, "The Python reporter could not be written.".to_string()))
    })?;
    let names: Vec<String> = if command_asks_for_names(template) { baseline_tests.to_vec() } else { Vec::new() };
    let coverage_path = out.join("coverage-raw.json");
    let coverage = if collect && parsed.kind == PythonKind::Pytest { Some(coverage_path.as_path()) } else { None };
    let launch = PythonLaunch {
        reporter,
        program: python.program.to_string(),
        names,
        files: python.files.to_vec(),
        extra: parsed.extra.clone(),
        kind: parsed.kind,
    };
    let timed = launch_python(package, package, &launch, None, coverage, suite);
    Ok(BaselineRun {
        kind: judge_python_baseline(&timed),
        duration_ms: Some(duration_ms(timed.duration)),
        template: Some(template.to_string()),
        line_map: None,
        tail: output_tail(&timed.stdout, &timed.stderr),
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
    if store_python_stub(&timed.stderr) || store_python_stub(&timed.stdout) {
        return Baseline::MissingTool;
    }
    match report::parse(&timed.stdout) {
        Some(report) => report::judge_baseline(&report),
        None => Baseline::NoReport,
    }
}

/// The clock and environment one suite process gets.
struct SuiteEnv<'a> {
    timeout: Duration,
    env: &'a [(String, String)],
    unset: &'a [String],
}

/// Run the Python reporter in `root`. `names` replaces the launch's own names when given.
fn launch_python(root: &Path, bin_root: &Path, launch: &PythonLaunch, names: Option<&[String]>, coverage: Option<&Path>, suite: &SuiteEnv) -> TimedRun {
    let names = names.unwrap_or(&launch.names);
    let args = reporter_args(&launch.reporter, launch.kind, names, &launch.files, &launch.extra);
    let mut owned = python_env(root, coverage);
    owned.extend(suite.env.iter().cloned());
    let env: Vec<(&str, &str)> = owned.iter().map(|(key, value)| (key.as_str(), value.as_str())).collect();
    let unset_ref: Vec<&str> = suite.unset.iter().map(String::as_str).collect();
    process::run_argv_prepared(&launch.program, &args, Some(root), Some(bin_root), suite.timeout, &env, &unset_ref)
}

fn python_env(root: &Path, coverage: Option<&Path>) -> Vec<(String, String)> {
    let mut env = vec![("PYTHONDONTWRITEBYTECODE".to_string(), "1".to_string())];
    let src = root.join("src");
    if src.is_dir() {
        let mut dirs = vec![src];
        if let Some(existing) = std::env::var_os("PYTHONPATH") {
            dirs.extend(std::env::split_paths(&existing).filter(|dir| !dir.as_os_str().is_empty()));
        }
        // `;` on Windows. A `:` there splits `C:\src` into two entries.
        let pythonpath = std::env::join_paths(dirs).map(|joined| joined.to_string_lossy().into_owned()).unwrap_or_default();
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
        return bare_python(program);
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
    bare_python("python3")
}

/// A bare `python3` or `python`. On Windows the Microsoft Store placeholder in `WindowsApps`
/// prints "Python was not found" and exits, so a real interpreter elsewhere on `PATH` wins.
/// With none, the name is returned unchanged and the baseline reports the missing tool.
fn bare_python(program: &str) -> String {
    #[cfg(windows)]
    {
        let order: [&str; 2] = if program == "python" { ["python", "python3"] } else { ["python3", "python"] };
        let Some(path) = std::env::var_os("PATH") else { return program.to_string() };
        for name in order {
            for dir in std::env::split_paths(&path) {
                if dir.to_string_lossy().to_ascii_lowercase().contains("windowsapps") {
                    continue;
                }
                let candidate = dir.join(format!("{name}.exe"));
                if candidate.is_file() {
                    return candidate.display().to_string();
                }
            }
        }
    }
    program.to_string()
}

/// The Microsoft Store placeholder for `python`/`python3` on Windows. It is not an interpreter.
fn store_python_stub(text: &str) -> bool {
    text.contains("Python was not found; run without arguments to install from the Microsoft Store")
}

/// The last few lines the suite printed, for a refusal's `next`. Stderr first, since that is
/// where a crash or a missing module is written.
fn output_tail(stdout: &str, stderr: &str) -> String {
    let source = if stderr.trim().is_empty() { stdout } else { stderr };
    let lines: Vec<&str> = source.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
    let start = lines.len().saturating_sub(3);
    let mut tail = lines[start..].join(" | ");
    if tail.chars().count() > 300 {
        let skip = tail.chars().count() - 300;
        tail = format!("...{}", tail.chars().skip(skip).collect::<String>());
    }
    tail
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

fn score_timed(timed: &TimedRun, python: bool) -> Scored {
    let duration_ms = duration_ms(timed.duration);
    if timed.timed_out {
        return Scored::new(Verdict::Timeout, duration_ms, true);
    }
    if python {
        if let Some(report) = report::parse(&timed.stdout) {
            let judged = report::judge_mutant(&report);
            let mut scored = Scored::new(judged.verdict, duration_ms, false);
            scored.killed_by = judged.killed_by;
            return scored;
        }
        // The reporter always prints its report. Without one, the reporter never ran, so no
        // assertion was checked. Counting that exit as a kill scored a broken launch as a catch.
        return Scored::new(Verdict::Error, duration_ms, false);
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
    process::find_on_path(name).is_some()
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
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("probatio-{name}-{}-{id}", std::process::id()));
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
            // Windows runners start python while the rest of the suite is also
            // spawning it. 10s was inside that cold start, so the baseline was
            // reported as timed out and the run never reached "No line map".
            "--timeout-ms".into(),
            "60000".into(),
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
        let empty = run_with(&package, &patches, &package.join("empty"), &["--suite-command", "printf 'TAP version 13\\n# tests 0\\n# pass 0\\n# duration_ms 1\\n'"]);
        assert_no_kill_count(&empty);
        assert!(empty.next.contains("counted 0 tests"), "{}", empty.next);
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn run_compile_failure_is_not_a_kill() {
        let package = temp("unviable");
        write_source(&package, one_and());
        let patches = generate_into(&package);
        let out = package.join("out");
        let script = "if grep -q '||' src/A.java; then echo 'src/A.java:2: error: cannot find symbol Widget' >&2; exit 1; else echo ok; fi";
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
        let script = "if grep -q '||' src/A.java; then printf '%s\\n' './src/A.java:3:6: undefined: nope' 'FAIL demo [build failed]' >&2; exit 1; else echo ok; fi";
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

    /// Specula B10: a tree that does not build makes every mutant fail the same way, with an error
    /// that does not name the mutated file. That is a refusal, never a clean score, and the error
    /// results are removed so the same call scores them once the cause is fixed.
    #[test]
    fn a_tree_that_does_not_build_is_a_refusal_not_a_score() {
        let package = temp("broken-tree");
        write_source(&package, &many_ands(3));
        let patches = generate_into(&package);
        let out = package.join("out");
        let script = "if grep -q '||' src/A.java; then echo 'error: failed to find a workspace root' >&2; exit 101; else echo ok; fi";
        let envelope = run_with(&package, &patches, &out, &["--suite-command", script, "--no-confirm"]);
        assert_no_kill_count(&envelope);
        assert!(envelope.summary.contains("errored before any test ran. This run is not a score."), "{}", envelope.summary);
        assert!(envelope.next.contains("failed to find a workspace root"), "{}", envelope.next);
        assert_eq!(fs::read_dir(out.join("results")).map(|entries| entries.count()).unwrap_or(0), 0);
        let _ = fs::remove_dir_all(&package);
    }

    /// Specula B10: a Cargo workspace member as the package. The mutant tree holds the whole
    /// checkout, so the member finds its workspace root, and the suite runs in the member folder.
    #[test]
    fn a_workspace_member_builds_with_its_workspace() {
        if Command::new("cargo").arg("--version").output().is_err() {
            return;
        }
        let root = temp("workspace");
        fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = [\"crates/gate\"]\nresolver = \"2\"\n\n[workspace.package]\nedition = \"2021\"\n").unwrap();
        let member = root.join("crates/gate");
        fs::create_dir_all(member.join("src")).unwrap();
        fs::write(member.join("Cargo.toml"), "[package]\nname = \"gate\"\nversion = \"0.0.0\"\nedition.workspace = true\n").unwrap();
        fs::write(
            member.join("src/lib.rs"),
            "pub fn open(n: i32) -> bool {\n    n > 0\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn zero_stays_shut() {\n        assert!(!super::open(0));\n    }\n}\n",
        )
        .unwrap();
        fs::write(root.join(".gitignore"), "target/\nout/\n").unwrap();
        git(&root, &["init", "-q"]);
        git(&root, &["config", "user.email", "a@b.c"]);
        git(&root, &["config", "user.name", "a"]);
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "init"]);
        let patches = generate_into(&member);
        let out = root.join("out");
        let envelope = run_with(&member, &patches, &out, &["--timeout-ms", "300000"]);
        assert!(envelope.ok, "{} {}", envelope.summary, envelope.next);
        let body = one_result(&out);
        assert_eq!(body.get("outcome").and_then(Value::as_str), Some("killed"), "{body:?}");
        let _ = fs::remove_dir_all(&root);
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
            "2000".into(),
            "--suite-command".into(),
            script.into(),
        ];
        let started = std::time::Instant::now();
        let envelope = run(&Flags::parse(&args));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(8),
            "the suite process was still alive after the deadline: {:?}",
            started.elapsed()
        );
        assert!(envelope.ok, "{}", envelope.summary);
        let value = parsed(&envelope);
        assert_eq!(value.get("counts").and_then(|counts| counts.get("killed")).and_then(Value::as_int), Some(0));
        assert_eq!(value.get("counts").and_then(|counts| counts.get("timedOut")).and_then(Value::as_int), Some(1));
        let result = fs::read_dir(out.join("results")).unwrap().next().unwrap().unwrap().path();
        let body = json::parse(&fs::read_to_string(&result).unwrap()).unwrap();
        assert_eq!(body.get("outcome").and_then(Value::as_str), Some("timeout"));
        let duration = body.get("durationMs").and_then(Value::as_int).unwrap() as u64;
        assert!(process::within_grace(Duration::from_millis(duration), Duration::from_millis(2000)), "{duration}");
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
        // The baseline, one unmutated run in the mutant tree, and the mutant.
        assert_eq!(fs::read_to_string(&log).unwrap().lines().count(), 3);
        let second = run_with(&package, &patches, &out, &["--suite-command", &script]);
        assert!(second.ok, "{}", second.summary);
        assert_eq!(fs::read_to_string(&log).unwrap().lines().count(), 3, "resume ran the suite again");
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
        // The baseline, one unmutated run in the mutant tree, and each spawned mutant.
        assert_eq!(fs::read_to_string(&log).unwrap().lines().count(), spawned + 2);
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
        assert!(!log.exists() || fs::read_to_string(&log).unwrap().lines().count() == spawned + 2);
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

    #[test]
    fn a_kill_name_is_relative_to_the_worktree() {
        let root = Path::new("/tmp/probatio-abc-0");
        let names = vec![
            "/tmp/probatio-abc-0/test/a.test.js::adds".to_string(),
            "file:///tmp/probatio-abc-0/test/b.test.js::subtracts".to_string(),
            "tests/test_gate.py::test_zero".to_string(),
        ];
        assert_eq!(relative_names(&names, root), ["test/a.test.js::adds", "test/b.test.js::subtracts", "tests/test_gate.py::test_zero"]);
        if cfg!(windows) {
            let names = vec!["c:/Temp/probatio-abc-0/test/a.test.js::adds".to_string()];
            assert_eq!(relative_names(&names, Path::new(r"C:\Temp\probatio-abc-0")), ["test/a.test.js::adds"]);
        }
    }

    /// A Rust kill names the failing test from libtest's output, without `cargo llvm-cov`, and the
    /// confirm reruns only that test.
    #[test]
    fn a_rust_kill_names_its_test() {
        if Command::new("cargo").arg("--version").output().is_err() {
            return;
        }
        let package = temp("cargo-named");
        fs::write(package.join("Cargo.toml"), "[package]\nname = \"gate\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[dependencies]\n").unwrap();
        fs::write(
            package.join("src/lib.rs"),
            "pub fn open(n: i32) -> bool {\n    n > 0\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn zero_stays_shut() {\n        assert!(!super::open(0));\n    }\n}\n",
        )
        .unwrap();
        let patches = generate_into(&package);
        let out = package.join("out");
        let envelope = run_with(&package, &patches, &out, &["--suite-command", "cargo test", "--timeout-ms", "300000"]);
        assert!(envelope.ok, "{} {}", envelope.summary, envelope.next);
        let body = one_result(&out);
        assert_eq!(body.get("outcome").and_then(Value::as_str), Some("killed"), "{body:?}");
        assert_eq!(json::stringify_compact(body.get("killedBy").unwrap_or(&Value::Null)), r#"["tests::zero_stays_shut"]"#);
        let _ = fs::remove_dir_all(&package);
    }

    /// A mutant never builds into the user's target directory, even when one is set in the
    /// environment, in `.probatio.json`, or as `--target-dir` in the suite command.
    #[test]
    fn a_mutant_build_never_shares_the_users_target_directory() {
        let package = temp("target-env");
        write_source(&package, one_and());
        let patches = generate_into(&package);
        let out = package.join("out");
        let seen = package.join("seen.txt").display().to_string().replace('\\', "/");
        fs::write(package.join(".probatio.json"), "{\"env\":{\"CARGO_TARGET_DIR\":\"/user/target\"}}\n").unwrap();
        let script = format!("printf '%s|%s\\n' \"$CARGO_TARGET_DIR\" --target-dir='/user/x' >> '{seen}'; echo ok");
        let envelope = run_with(&package, &patches, &out, &["--suite-command", &script, "--no-confirm"]);
        assert!(envelope.ok, "{} {}", envelope.summary, envelope.next);
        let text = fs::read_to_string(package.join("seen.txt")).unwrap_or_default();
        // The first line is the baseline, which runs in the package as written.
        assert!(text.lines().next().unwrap_or("").starts_with("/user/target|"), "{text}");
        let mutant = text.lines().nth(1).unwrap_or("").replace('\\', "/");
        let (env_dir, flag) = mutant.split_once('|').unwrap_or_default();
        let slot = crate::slots::cache_root().join(crate::slots::key_for(&package));
        let slot = slot.display().to_string().replace('\\', "/");
        assert!(env_dir.starts_with(&slot) && env_dir.ends_with("target"), "{mutant} {slot}");
        assert_eq!(flag, format!("--target-dir={env_dir}"), "{mutant}");
        assert!(!text.lines().skip(1).any(|line| line.contains("/user/")), "{text}");
        let recorded = one_result(&out);
        let command = recorded.get("command").and_then(Value::as_str).unwrap_or("");
        assert!(!command.contains("/user/x") && command.contains("--target-dir="), "the result records the command as run: {command}");
        let _ = fs::remove_dir_all(&package);
    }

    /// The mutant tree holds what git tracks, not ignored files. A suite that needs an ignored
    /// file fails there with no mutant applied; that stops the run before any mutant is scored
    /// (every mutant would read as killed), and `.probatio.json` `copy` brings the file along.
    #[test]
    fn a_tree_that_fails_unmutated_stops_the_run_and_copy_fixes_it() {
        let package = temp("tree-check");
        write_source(&package, one_and());
        fs::write(package.join(".gitignore"), "fixture.txt\npatches/\nout*/\n").unwrap();
        fs::write(package.join("fixture.txt"), "needed\n").unwrap();
        git(&package, &["init", "-q"]);
        git(&package, &["add", "src/A.java", ".gitignore"]);
        git_commit(&package, &["commit", "-q", "-m", "init"]);
        let patches = generate_into(&package);
        let suite = "test -f fixture.txt || exit 1; echo ok";
        let refused = run_with(&package, &patches, &package.join("out1"), &["--suite-command", suite, "--no-confirm"]);
        assert!(!refused.ok, "{}", refused.summary);
        assert!(refused.summary.contains("fails in the mutant tree"), "{}", refused.summary);
        assert!(refused.summary.contains("\"copy\""), "{}", refused.summary);
        assert_no_kill_count(&refused);
        assert!(fs::read_dir(package.join("out1/results")).map(|mut entries| entries.next().is_none()).unwrap_or(true), "a result was written");
        fs::write(package.join(".probatio.json"), "{\"copy\":[\"fixture.txt\"]}\n").unwrap();
        let scored = run_with(&package, &patches, &package.join("out2"), &["--suite-command", suite, "--no-confirm"]);
        assert!(scored.ok, "{} {}", scored.summary, scored.next);
        assert_eq!(parsed(&scored).get("counts").and_then(|counts| counts.get("survived")).and_then(Value::as_int), Some(1));
        fs::write(package.join(".probatio.json"), "{\"copy\":[\"../outside\"]}\n").unwrap();
        let bad = run_with(&package, &patches, &package.join("out3"), &["--suite-command", suite]);
        assert!(!bad.ok && bad.summary.contains("must be a path inside the package"), "{}", bad.summary);
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn a_target_dir_flag_is_pointed_at_the_slot() {
        let dir = Path::new("/c/slot/target");
        assert_eq!(with_target_dir(r"cargo test --target-dir C:\x\t -p a", dir), "cargo test --target-dir '/c/slot/target' -p a");
        assert_eq!(with_target_dir("cargo test --target-dir=\"a b\" && x", dir), "cargo test --target-dir='/c/slot/target' && x");
        assert_eq!(with_target_dir("cargo test --target-directory x", dir), "cargo test --target-directory x");
        assert_eq!(with_target_dir("cargo test", dir), "cargo test");
    }

    /// Windows linked no `node_modules` at all, so a Node suite in a worktree could not resolve its
    /// dependencies. It is a junction there now. Deleting the copy must not reach the package.
    #[test]
    fn node_modules_is_linked_into_a_copy_and_survives_its_removal() {
        let package = temp("nm-link");
        fs::create_dir_all(package.join("node_modules/dep")).unwrap();
        fs::write(package.join("node_modules/dep/index.js"), "module.exports = 1;\n").unwrap();
        let dest = temp("nm-link-dest");
        link_node_modules(&package, &dest).unwrap();
        assert_eq!(fs::read_to_string(dest.join("node_modules/dep/index.js")).unwrap(), "module.exports = 1;\n");
        worktree::unlink_links(&dest);
        assert!(!dest.join("node_modules").exists());
        let _ = fs::remove_dir_all(&dest);
        assert!(package.join("node_modules/dep/index.js").is_file(), "the package node_modules was deleted");
        let _ = fs::remove_dir_all(&package);
    }

    fn shell_python() -> String {
        bare_python("python3").replace('\\', "/")
    }

    #[test]
    fn the_store_placeholder_is_a_missing_tool_and_a_red_baseline_shows_its_output() {
        let stub = TimedRun {
            spawned: true,
            status: Some(9009),
            timed_out: false,
            duration: Duration::from_millis(5),
            stdout: String::new(),
            stderr: "Python was not found; run without arguments to install from the Microsoft Store, or disable this shortcut from Settings > Apps > Advanced app settings > App execution aliases.\n".into(),
        };
        assert!(judge_python_baseline(&stub) == Baseline::MissingTool);
        let package = temp("red-tail");
        write_source(&package, one_and());
        let patches = generate_into(&package);
        let envelope = run_with(&package, &patches, &package.join("out"), &["--suite-command", "echo first; echo ModuleNotFoundError: no module named gate >&2; exit 1"]);
        assert_no_kill_count(&envelope);
        assert!(envelope.next.contains("Last output: ModuleNotFoundError: no module named gate"), "{}", envelope.next);
        assert_eq!(output_tail("a\n\nb\nc\nd\n", ""), "b | c | d");
        assert!(output_tail("", &"x".repeat(900)).chars().count() <= 303);
        let _ = fs::remove_dir_all(&package);
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
            &["--suite-command", &format!("{} -m unittest tests.test_gate", shell_python())],
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
        let pytest = Command::new(bare_python("python3")).args(["-c", "import pytest, coverage"]).status();
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
        // The README points newcomers at docs/reference.md, which carries the discovery table.
        let readme = fs::read_to_string(root.join("docs/reference.md")).unwrap();
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

    fn js_mutant(package: &Path, body: &str, original: &str, replacement: &str) -> PathBuf {
        fs::create_dir_all(package.join("src")).unwrap();
        fs::write(package.join("src/a.js"), body).unwrap();
        let start = body.find(original).unwrap();
        let patches = package.join("patches");
        fs::create_dir_all(&patches).unwrap();
        let listed = format!(
            "{{\"mutants\":[{{\"id\":\"m1\",\"file\":\"src/a.js\",\"start\":{start},\"end\":{},\"original\":{},\"replacement\":{}}}]}}\n",
            start + original.len(),
            json_string(original),
            json_string(replacement),
        );
        fs::write(patches.join("mutants.json"), listed).unwrap();
        patches
    }

    #[test]
    fn reverse_direction_puts_the_original_text_back() {
        let package = temp("reverse");
        let body = "return a || b;\n";
        fs::create_dir_all(package.join("src")).unwrap();
        fs::write(package.join("src/a.js"), body).unwrap();
        let start = body.find("||").unwrap();
        let patches = package.join("patches");
        fs::create_dir_all(&patches).unwrap();
        fs::write(
            patches.join("mutants.json"),
            format!(
                "{{\"mutants\":[{{\"id\":\"m1\",\"file\":\"src/a.js\",\"start\":{start},\"end\":{},\"original\":\"&&\",\"replacement\":\"||\"}}]}}\n",
                start + 2
            ),
        )
        .unwrap();
        let out = package.join("out");
        let envelope = run_with(
            &package,
            &patches,
            &out,
            &[
                "--suite-command",
                &format!("{} -c 'import pathlib,sys; t=pathlib.Path(\"src/a.js\").read_text(); print(\"ok\"); sys.exit(0 if \"||\" in t else 1)'", shell_python()),
                "--direction",
                "reverse",
                "--no-confirm",
            ],
        );
        assert!(envelope.ok, "{}", envelope.summary);
        let body = one_result(&out);
        assert_eq!(body.get("outcome").and_then(Value::as_str), Some("killed"));
        assert_eq!(body.get("direction").and_then(Value::as_str), Some("reverse"));
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn history_reuses_a_row_when_the_mutant_and_tests_are_unchanged() {
        let package = temp("history");
        let patches = js_mutant(&package, "return a && b;\n", "&&", "||");
        let first = package.join("first");
        let envelope = run_with(&package, &patches, &first, &["--suite-command", "echo ok", "--no-confirm"]);
        assert!(envelope.ok, "{}", envelope.summary);
        assert!(first.join("history.json").is_file());
        let mut history = json::parse(&fs::read_to_string(first.join("history.json")).unwrap()).unwrap();
        if let Value::Object(entries) = &mut history {
            if let Some((_, Value::Array(items))) = entries.iter_mut().find(|(key, _)| key == "entries") {
                if let Some(Value::Object(row)) = items.first_mut() {
                    if let Some((_, outcome)) = row.iter_mut().find(|(key, _)| key == "outcome") {
                        *outcome = Value::String("killed".into());
                    }
                }
            }
        }
        fs::write(first.join("history.json"), json::stringify(&history)).unwrap();
        let second = package.join("second");
        let again = run_with(
            &package,
            &patches,
            &second,
            &["--suite-command", "echo ok", "--no-confirm", "--history", &first.display().to_string()],
        );
        assert!(again.ok, "{}", again.summary);
        let body = one_result(&second);
        assert_eq!(body.get("outcome").and_then(Value::as_str), Some("killed"));
        assert_eq!(body.get("reused").and_then(Value::as_bool), Some(true));
        assert_eq!(body.get("durationMs").and_then(Value::as_int), Some(0));
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn affected_keeps_only_the_test_that_imports_the_file() {
        let package = temp("affected");
        fs::create_dir_all(package.join("src")).unwrap();
        fs::create_dir_all(package.join("tests")).unwrap();
        fs::write(package.join("src/calc.py"), "VALUE = 1\n").unwrap();
        fs::write(package.join("tests/test_calc.py"), "import calc\n").unwrap();
        fs::write(package.join("tests/test_other.py"), "import other\n").unwrap();
        let text = fs::read_to_string(package.join("src/calc.py")).unwrap();
        let start = text.find('1').unwrap();
        let patches = package.join("patches");
        fs::create_dir_all(&patches).unwrap();
        fs::write(
            patches.join("mutants.json"),
            format!(
                "{{\"mutants\":[{{\"id\":\"m1\",\"file\":\"src/calc.py\",\"start\":{start},\"end\":{},\"original\":\"1\",\"replacement\":\"0\"}}]}}\n",
                start + 1
            ),
        )
        .unwrap();
        fs::write(package.join("names.txt"), "tests/test_calc.py\ntests/test_other.py\n").unwrap();
        let out = package.join("out");
        let envelope = run_with(
            &package,
            &patches,
            &out,
            &[
                "--suite-command",
                "echo {tests}",
                "--tests",
                &package.join("names.txt").display().to_string(),
                "--affected",
                "--no-confirm",
            ],
        );
        assert!(envelope.ok, "{}", envelope.summary);
        let body = one_result(&out);
        let command = body.get("command").and_then(Value::as_str).unwrap_or("");
        assert!(command.contains("test_calc.py"), "{command}");
        assert!(!command.contains("test_other.py"), "{command}");
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn a_failing_build_stops_the_baseline_and_a_mutant_that_no_longer_builds_is_unviable() {
        let package = temp("build");
        let patches = js_mutant(&package, "MARK\n", "MARK", "GONE");
        fs::write(package.join("build.sh"), "#!/bin/sh\ncase \"$(cat src/a.js)\" in *MARK*) exit 0 ;; *) exit 1 ;; esac\n").unwrap();
        let failed = package.join("failed");
        let stopped = run_with(&package, &patches, &failed, &["--suite-command", "echo ok", "--build", "false", "--no-confirm"]);
        assert!(!stopped.ok, "{}", stopped.summary);
        assert!(stopped.summary.contains("baseline build failed"), "{}", stopped.summary);
        assert!(fs::read_dir(failed.join("results")).is_err() || fs::read_dir(failed.join("results")).unwrap().count() == 0);
        let out = package.join("out");
        let envelope = run_with(
            &package,
            &patches,
            &out,
            &["--suite-command", "echo ok", "--build", "sh build.sh", "--no-confirm"],
        );
        assert!(envelope.ok, "{}", envelope.summary);
        let body = one_result(&out);
        assert_eq!(body.get("outcome").and_then(Value::as_str), Some("unviable"));
        assert_eq!(body.get("cause").and_then(Value::as_str), Some("build"));
        let _ = fs::remove_dir_all(&package);
    }

    static UNSET_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn unset_removes_the_named_variable_before_the_suite() {
        let _guard = UNSET_LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
        std::env::set_var("PROBATIO_UNSET_PROBE", "1");
        let package = temp("unset");
        let patches = js_mutant(&package, "return 1;\n", "1", "0");
        let out = package.join("out");
        let envelope = run_with(
            &package,
            &patches,
            &out,
            &["--suite-command", "test -z \"$PROBATIO_UNSET_PROBE\" && echo ok", "--unset", "PROBATIO_UNSET_PROBE", "--no-confirm"],
        );
        std::env::remove_var("PROBATIO_UNSET_PROBE");
        assert!(envelope.ok, "{}", envelope.summary);
        assert_eq!(one_result(&out).get("outcome").and_then(Value::as_str), Some("survived"));
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn agent_is_stored_on_the_result_and_a_bad_direction_is_refused() {
        let package = temp("agent");
        let patches = js_mutant(&package, "return 1;\n", "1", "0");
        let out = package.join("out");
        let envelope = run_with(&package, &patches, &out, &["--suite-command", "echo ok", "--agent", "ada", "--no-confirm"]);
        assert!(envelope.ok, "{}", envelope.summary);
        assert_eq!(one_result(&out).get("agent").and_then(Value::as_str), Some("ada"));
        let refused = run_with(&package, &patches, &package.join("bad"), &["--suite-command", "echo ok", "--direction", "sideways"]);
        assert!(!refused.ok, "{}", refused.summary);
        assert!(refused.summary.contains("--direction"), "{}", refused.summary);
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn repo_commit_and_tests_dir_change_the_run() {
        let package = temp("pins");
        fs::create_dir_all(package.join("src")).unwrap();
        fs::write(package.join("src/a.js"), "return 1;\n").unwrap();
        fs::write(package.join("pytest.ini"), "[pytest]\n").unwrap();
        let missing = run(&Flags::parse(&[
            "--package".into(),
            package.display().to_string(),
            "--patches".into(),
            package.display().to_string(),
            "--out".into(),
            package.join("out").display().to_string(),
            "--tests-dir".into(),
            "missing".into(),
        ]));
        assert!(!missing.ok, "{}", missing.summary);
        assert!(missing.summary.contains("no tests in missing"), "{}", missing.summary);
        let repo = package.join("not-a-repo");
        fs::create_dir_all(&repo).unwrap();
        let patches = js_mutant(&package, "return 1;\n", "1", "0");
        let bad_repo = run_with(
            &package,
            &patches,
            &package.join("repo-out"),
            &["--suite-command", "echo ok", "--repo", &repo.display().to_string()],
        );
        assert!(!bad_repo.ok, "{}", bad_repo.summary);
        assert!(bad_repo.summary.contains("git"), "{}", bad_repo.summary);
        git(&package, &["init"]);
        git_commit(&package, &["add", "."]);
        git_commit(&package, &["commit", "-m", "base"]);
        let bad_commit = run_with(&package, &patches, &package.join("commit-out"), &["--suite-command", "echo ok", "--commit", "deadbeef"]);
        assert!(!bad_commit.ok, "{}", bad_commit.summary);
        assert!(bad_commit.summary.contains("does not resolve"), "{}", bad_commit.summary);
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn max_minutes_stops_after_the_first_mutant() {
        let package = temp("minutes");
        fs::create_dir_all(package.join("src")).unwrap();
        let body = "return a && b && c;\n";
        fs::write(package.join("src/a.js"), body).unwrap();
        let mut rows = Vec::new();
        for (index, needle) in ["a", "b", "c"].iter().enumerate() {
            let start = body.find(needle).unwrap();
            rows.push(format!(
                "{{\"id\":\"m{index}\",\"file\":\"src/a.js\",\"start\":{start},\"end\":{},\"original\":{},\"replacement\":\"x\"}}",
                start + 1,
                json_string(needle),
            ));
        }
        let patches = package.join("patches");
        fs::create_dir_all(&patches).unwrap();
        fs::write(patches.join("mutants.json"), format!("{{\"mutants\":[{}]}}\n", rows.join(","))).unwrap();
        let out = package.join("out");
        let envelope = run_with(&package, &patches, &out, &["--suite-command", "echo ok", "--max-minutes", "0", "--no-confirm"]);
        assert!(envelope.ok, "{}", envelope.summary);
        assert_eq!(fs::read_dir(out.join("results")).unwrap().count(), 1);
        let waiting = fs::read_to_string(out.join("not-started.txt")).unwrap();
        assert_eq!(waiting.lines().count(), 2, "{waiting}");
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn a_test_line_names_the_shell_failure_and_a_bad_pattern_is_refused() {
        let package = temp("script-line");
        let patches = js_mutant(&package, "return 1;\n", "1", "0");
        fs::write(package.join(".probatio.json"), "{\"testLine\":\"^Testing .+$\"}\n").unwrap();
        fs::write(
            package.join("suite.sh"),
            "#!/bin/sh\ntext=$(cat src/a.js)\ncase \"$text\" in\n*1*) printf '%s\\n' 'Testing one' ;;\n*) printf '%s\\n%s\\n' 'Testing one' 'Testing two'; exit 1 ;;\nesac\n",
        )
        .unwrap();
        let out = package.join("out");
        let envelope = run_with(&package, &patches, &out, &["--suite-command", "sh suite.sh", "--no-confirm"]);
        assert!(envelope.ok, "{}", envelope.summary);
        let body = one_result(&out);
        assert_eq!(body.get("outcome").and_then(Value::as_str), Some("killed"), "{body:?}");
        let killed = match body.get("killedBy") {
            Some(Value::Array(items)) => items.first().and_then(Value::as_str),
            _ => None,
        };
        assert_eq!(killed, Some("Testing two"));
        fs::write(package.join(".probatio.json"), "{\"testLine\":\"([\"}\n").unwrap();
        let refused = run_with(&package, &patches, &package.join("bad"), &["--suite-command", "echo ok"]);
        assert!(!refused.ok, "{}", refused.summary);
        assert!(refused.summary.contains(".probatio.json testLine"), "{}", refused.summary);
        let _ = fs::remove_dir_all(&package);
    }

    #[test]
    fn node_commands_carry_concurrency_and_the_test_timeout() {
        let command = node_test_command(Path::new("rep.mjs"), &["adds".into()], &["test/a.test.js".into()], 3, 60_000);
        assert!(command.contains("--test-concurrency=3"), "{command}");
        assert!(command.contains("--test-timeout=60000"), "{command}");
        assert!(command.contains("test/a.test.js"), "{command}");
        assert_eq!(with_concurrency(&command, 1).matches("--test-concurrency=1").count(), 1);
    }

    /// Windows node refused `--test-reporter='C:\...'`, every mutant exited 7, and each exit was
    /// counted as a kill. The reporter is a `file:` URL there, and a run with no report is an error.
    #[test]
    fn a_node_mutant_runs_the_reporter_and_a_missing_report_is_not_a_kill() {
        let reporter = std::env::temp_dir().join("probatio dir").join("fail-reporter.mjs");
        let command = node_test_command(&reporter, &[], &[], 1, 60_000);
        if cfg!(windows) {
            assert!(command.contains("--test-reporter='file:///"), "{command}");
            assert!(command.contains("probatio%20dir/fail-reporter.mjs'"), "{command}");
        }
        let crashed = TimedRun {
            spawned: true,
            status: Some(7),
            timed_out: false,
            duration: Duration::from_millis(100),
            stdout: String::new(),
            stderr: "Error [ERR_UNSUPPORTED_ESM_URL_SCHEME]".into(),
        };
        assert!(score_timed(&crashed, true).kind == Verdict::Error);
        if Command::new("node").arg("--version").output().is_err() {
            return;
        }
        let package = temp("node-reporter");
        fs::create_dir_all(package.join("test")).unwrap();
        fs::write(package.join("src/calc.js"), "export function both(a, b) { return a && b; }\n").unwrap();
        fs::write(package.join("package.json"), "{\"type\":\"module\"}\n").unwrap();
        fs::write(
            package.join("test/calc.test.js"),
            "import test from 'node:test';\nimport assert from 'node:assert/strict';\nimport { both } from '../src/calc.js';\ntest('both', () => { assert.equal(both(true, false), false); });\n",
        )
        .unwrap();
        let patches = generate_into(&package);
        let out = package.join("out");
        let envelope = run_with(&package, &patches, &out, &["--suite-command", "node --test", "--no-confirm"]);
        assert!(envelope.ok, "{}", envelope.summary);
        let body = one_result(&out);
        assert_eq!(body.get("outcome").and_then(Value::as_str), Some("killed"), "{body:?}");
        let names = json::stringify_compact(body.get("killedBy").unwrap_or(&Value::Null));
        assert_eq!(names, r#"["test/calc.test.js::both"]"#);
        let _ = fs::remove_dir_all(&package);
    }
}
