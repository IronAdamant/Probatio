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
/// With no `maxMutants` and a line map, one call scores every mutant; past this much time since
/// the call began it starts no more, and `continue` goes on. A caller's `maxMinutes` replaces it.
const MAPPED_BUDGET_MS: u64 = 10 * 60_000;

/// Where Probatio keeps its output for a package: `PROBATIO_DIR` when set (a relative value is
/// read from the project root), else `.probatio` at the project root, which is the top of the git
/// checkout. A package inside a workspace no longer gets a `.probatio` of its own.
pub fn probatio_dir(package: &Path) -> PathBuf {
    let root = crate::worktree::toplevel(package).unwrap_or_else(|| package.to_path_buf());
    match std::env::var_os("PROBATIO_DIR").filter(|dir| !dir.is_empty()).map(PathBuf::from) {
        Some(dir) if dir.is_absolute() => dir,
        Some(dir) => root.join(dir),
        None => root.join(".probatio"),
    }
}

/// `mutate diff`'s default out: `<probatio dir>/diff`, then the package's path from the root, so
/// two packages of one checkout do not share a directory.
pub fn default_diff_out(package: &Path) -> PathBuf {
    let base = probatio_dir(package).join("diff");
    let Some(root) = crate::worktree::toplevel(package) else { return base };
    let root = root.canonicalize().unwrap_or(root);
    let full = package.canonicalize().unwrap_or_else(|_| package.to_path_buf());
    match full.strip_prefix(&root) {
        Ok(rel) if !rel.as_os_str().is_empty() => base.join(rel),
        _ => base,
    }
}

/// A `.probatio` directory ignores itself, so its output is never committed by accident, even in a
/// project whose `.gitignore` does not list it. Called with any directory Probatio writes to.
pub fn ignore_own_output(dir: &Path) {
    for ancestor in dir.ancestors() {
        if ancestor.file_name().is_some_and(|name| name == ".probatio") {
            let marker = ancestor.join(".gitignore");
            if !marker.exists() && fs::create_dir_all(ancestor).is_ok() {
                let _ = fs::write(marker, "# Probatio output. Not source; never committed.\n*\n");
            }
            return;
        }
    }
}

/// The commit `since` is diffed from, and how the summary names it.
struct Base {
    commit: String,
    label: String,
}

/// `since` as a base commit. Mutants run on the working tree, so the far end is always the
/// working tree (HEAD plus uncommitted edits and untracked files).
///
/// - `HEAD` (the default): uncommitted edits.
/// - One ref, such as `main`: the merge-base of that ref and HEAD, so the lines this branch
///   changed are scored and lines that only moved on `main` are not, even after `main` was merged
///   into the branch.
/// - `A..B`: from `A`. `A...B`: from the merge-base of `A` and `B`. `B` (empty means HEAD) must be
///   the commit checked out.
fn resolve_since(dir: &Path, since: &str) -> Result<Base, String> {
    let short = |sha: &str| sha.chars().take(10).collect::<String>();
    let (from, to, symmetric) = if let Some((a, b)) = since.split_once("...") {
        (a, b, true)
    } else if let Some((a, b)) = since.split_once("..") {
        (a, b, false)
    } else {
        if since == "HEAD" {
            return Ok(Base { commit: "HEAD".into(), label: "HEAD".into() });
        }
        let commit = rev(dir, since).map_err(|_| format!("since {since} is not a commit git knows here"))?;
        return Ok(match git(dir, &["merge-base", &commit, "HEAD"]) {
            Ok(base) if !base.trim().is_empty() => {
                let base = base.trim().to_string();
                let label = if base == commit { since.to_string() } else { format!("{since} (merge-base {})", short(&base)) };
                Base { commit: base, label }
            }
            // A shallow clone can lack the history: fall back to the ref itself, and say so.
            _ => Base { commit, label: format!("{since} (no merge-base with HEAD; diffed against {since} itself)") },
        });
    };
    let from = if from.is_empty() { "HEAD" } else { from };
    let to = if to.is_empty() { "HEAD" } else { to };
    let from_sha = rev(dir, from).map_err(|_| format!("since {since}: {from} is not a commit git knows here"))?;
    let to_sha = rev(dir, to).map_err(|_| format!("since {since}: {to} is not a commit git knows here"))?;
    let head = rev(dir, "HEAD").map_err(|_| "since needs a checkout with a commit".to_string())?;
    if to_sha != head {
        return Err(format!(
            "since {since}: {to} is not the commit checked out. Mutants run on the working tree, so the range must end at HEAD; check out {to} first, or pass since {from}"
        ));
    }
    if symmetric {
        let base = git(dir, &["merge-base", &from_sha, &to_sha]).map_err(|_| format!("since {since}: {from} and {to} have no common ancestor"))?;
        let base = base.trim().to_string();
        return Ok(Base { label: format!("{since} (merge-base {})", short(&base)), commit: base });
    }
    Ok(Base { commit: from_sha, label: since.to_string() })
}

fn rev(dir: &Path, name: &str) -> Result<String, String> {
    let spec = format!("{name}^{{commit}}");
    git(dir, &["rev-parse", "--verify", "--quiet", &spec]).map(|sha| sha.trim().to_string()).and_then(|sha| if sha.is_empty() { Err(String::new()) } else { Ok(sha) })
}

pub fn diff(flags: &Flags) -> Envelope {
    if flags.bool("low-priority", false) {
        crate::process::lower_priority();
    }
    let Some(package) = flags.get("package").map(PathBuf::from) else {
        return Envelope::new(false, COMMAND, "missing --package", "Pass --package, the project directory you edited.");
    };
    if !package.is_dir() {
        return Envelope::new(false, COMMAND, format!("--package {} is not a directory", package.display()), "Pass the project directory you edited.");
    }
    let since = flags.get("since").unwrap_or("HEAD").to_string();
    let out = flags.get("out").map(PathBuf::from).unwrap_or_else(|| default_diff_out(&package));
    let max = match flags.parse_int("max-mutants") {
        Ok(value) => value.map(|value| value as usize).unwrap_or(DEFAULT_MAX).max(1),
        Err(message) => return Envelope::new(false, COMMAND, message, "Pass --max-mutants as a whole number."),
    };
    for key in ["budget-ms", "max-minutes", "workers", "timeout-ms"] {
        if let Err(message) = flags.parse_int(key) {
            return Envelope::new(false, COMMAND, message, format!("Pass --{key} as a whole number."));
        }
    }
    let src = flags.get("src").map(|dir| dir.trim_matches(['/', '\\']).replace('\\', "/"));
    let include_tests = flags.bool("include-tests", false);
    let paths: Vec<String> = flags.texts("path").iter().map(|path| clean_path(path)).collect();
    let serial = flags.bool("serial", false);
    let line_map = flags.get("line-map").map(str::to_string);
    if let Some(map) = &line_map {
        if !Path::new(map).is_file() {
            return Envelope::new(false, COMMAND, format!("--line-map {map} is not a file"), "Pass the path of a line map file, or leave it out: a unittest or pytest baseline writes one.");
        }
    }
    // No maxMutants: with a line map one call scores them all. maxMutants given: that many per call.
    let explicit_max = flags.get("max-mutants").is_some();

    let base = match resolve_since(&package, &since) {
        Ok(base) => base,
        Err(message) => return Envelope::new(false, COMMAND, message, "Pass --since as a ref (HEAD, main, origin/main) or a range ending at HEAD (main..HEAD, main...HEAD), from inside a git checkout."),
    };
    let changed = match changed_lines(&package, &base.commit) {
        Ok(changed) => changed,
        Err(message) => return Envelope::new(false, COMMAND, message, "Pass --since as a ref git can diff against (HEAD, main, origin/main), from inside a git checkout."),
    };
    let base_commit = base.commit.clone();
    let since = base.label;
    let changed: Vec<(String, Vec<u32>)> = changed
        .into_iter()
        .filter(|(file, _)| operators::kind_of(file) && !file.ends_with(".d.ts"))
        .filter(|(file, _)| src.as_deref().is_none_or(|dir| dir.is_empty() || file.starts_with(&format!("{dir}/"))))
        .collect();
    // `--path`: only the changed files the caller names (a prefix or a glob, from the package).
    let (changed, left_out): (Vec<_>, Vec<_>) = changed.into_iter().partition(|(file, _)| paths.is_empty() || paths.iter().any(|path| path_matches(path, file)));
    let path_lines_skipped: usize = left_out.iter().map(|(_, lines)| lines.len()).sum();
    let mut test_lines_skipped = 0usize;
    let mut kept: Vec<(String, Vec<u32>, String)> = Vec::new();
    for (file, lines) in changed {
        if !include_tests && crate::testcode::is_test_path(&file) {
            test_lines_skipped += lines.len();
            continue;
        }
        let Ok(text) = fs::read_to_string(package.join(&file)) else { continue };
        let ranges = if include_tests { Vec::new() } else { crate::testcode::test_ranges(&file, &text) };
        let (tests, source): (Vec<u32>, Vec<u32>) = lines.into_iter().partition(|line| crate::testcode::in_ranges(&ranges, *line));
        test_lines_skipped += tests.len();
        if !source.is_empty() {
            kept.push((file, source, text));
        }
    }
    let line_count: usize = kept.iter().map(|(_, lines, _)| lines.len()).sum();
    let wide = flags.get("operators") == Some("wide");
    let mut points: Vec<(Mutant, String)> = Vec::new();
    for (file, lines, text) in &kept {
        for point in operators::scan(file, text, wide) {
            if lines.contains(&point.line) {
                points.push((point, text.clone()));
            }
        }
    }
    let points = spread(points);
    let mut skipped = Vec::new();
    if test_lines_skipped > 0 {
        skipped.push(format!("{} skipped", count(test_lines_skipped, "test line")));
    }
    if path_lines_skipped > 0 {
        skipped.push(format!("{} outside path", count(path_lines_skipped, "changed line")));
    }
    let skipped_note = if skipped.is_empty() { String::new() } else { format!(" ({})", skipped.join("; ")) };
    let scope = format!("{} in {} since {since}{skipped_note}", count(line_count, "changed line"), count(kept.len(), "file"));
    if points.is_empty() {
        let summary = if line_count == 0 && test_lines_skipped > 0 {
            format!("No changed source line since {since}: the {} changed are test code. Nothing was mutated.", count(test_lines_skipped, "line"))
        } else if line_count == 0 {
            format!("No changed source line since {since}. Nothing was mutated.")
        } else {
            format!("{scope}, and none has a condition, comparison, or boolean to mutate.")
        };
        let summary = if line_count == 0 && path_lines_skipped > 0 {
            format!("No changed source line since {since} under {}: {} changed outside it. Nothing was mutated.", paths.join(", "), count(path_lines_skipped, "line"))
        } else {
            summary
        };
        let next = if line_count == 0 && path_lines_skipped > 0 && test_lines_skipped == 0 {
            "Nothing to score. Check paths (--path): each is a prefix or a glob from the package, such as src/storage or src/**/*.py."
        } else if line_count == 0 && test_lines_skipped > 0 {
            "Nothing to score. Test code is skipped by default; pass includeTests (--include-tests) to mutate it."
        } else {
            "Nothing to score. Edit code, or pass --since with an older ref."
        };
        return Envelope::new(true, COMMAND, summary, next)
            .with_extra("changedLines", Value::Int(line_count as i64))
            .with_extra("testLinesSkipped", Value::Int(test_lines_skipped as i64))
            .with_extra("pathLinesSkipped", Value::Int(path_lines_skipped as i64))
            .with_extra("mutantCount", Value::Int(0));
    }

    let generate_out = out.join("generate");
    let run_out = out.join("run");
    let plan_key = plan_key(&base_commit, &points, wide, include_tests);
    // `--continue` keeps the last call's results when it scored the same mutants: the same
    // changed lines with the same text. Anything else starts over, and says so.
    let plan_matches = read_plan(&out).as_deref() == Some(plan_key.as_str()) && run_out.join("results").is_dir();
    let asked_to_continue = flags.bool("continue", false);
    let continuing = asked_to_continue && plan_matches;
    // Read once, before this call writes results of its own.
    let done_ids: std::collections::HashSet<String> = if continuing {
        points.iter().filter(|(mutant, _)| run_out.join("results").join(format!("{}.json", mutant.id)).is_file()).map(|(mutant, _)| mutant.id.clone()).collect()
    } else {
        std::collections::HashSet::new()
    };
    let done = |mutant: &Mutant| done_ids.contains(&mutant.id);
    let pending = points.iter().filter(|(mutant, _)| !done(mutant)).count();
    // Without maxMutants every waiting mutant goes to the run, which scores them all when it has
    // a line map (passed, or written by the baseline), or when serial; else the first `max`.
    let uncapped = !explicit_max;
    let this_call = if uncapped { pending } else { pending.min(max) };

    if flags.bool("estimate", false) {
        let suite = crate::runner::suite_command(flags, &package).unwrap_or_default();
        let mapped = serial || line_map.is_some() || crate::runner::is_unittest_suite(&suite);
        let per_call = if uncapped && mapped { pending.max(1) } else { max };
        return estimate(flags, &package, &out, &points, &scope, EstimateScope { max: per_call, pending, this_call: pending.min(per_call), continuing, run_out: &run_out, lifted: uncapped && mapped });
    }

    // A fresh directory unless continuing: the tests may have changed since the last call, so
    // an old result or baseline must not be reused.
    ignore_own_output(&out);
    if !continuing {
        for dir in [&generate_out, &run_out] {
            if dir.exists() {
                if let Err(error) = fs::remove_dir_all(dir) {
                    return Envelope::new(false, COMMAND, format!("{} could not be cleared: {error}", dir.display()), "Close what holds that directory, or pass another --out.");
                }
            }
        }
    }
    if let Err(message) = write_plan(&out, &plan_key) {
        return Envelope::new(false, COMMAND, message, "The out directory could not be written. Pass another --out.");
    }
    // The mutants scored before (their results are kept), then this call's share of the rest.
    let mut taken = 0usize;
    let batch: Vec<(Mutant, String)> = points
        .iter()
        .filter(|(mutant, _)| {
            if done(mutant) {
                return true;
            }
            taken += 1;
            taken <= this_call
        })
        .cloned()
        .collect();
    if let Err(message) = crate::generate::write_mutants(&generate_out, &batch, if wide { "wide" } else { "core" }) {
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
    for key in ["suite-command", "timeout-ms", "workers", "budget-ms", "max-minutes", "line-map"] {
        if let Some(value) = flags.get(key) {
            argv.push(format!("--{key}"));
            argv.push(value.to_string());
        }
    }
    if flags.bool("low-priority", false) {
        argv.push("--low-priority".into());
    }
    if !flags.bool("confirm", true) {
        argv.push("--no-confirm".into());
    }
    if serial {
        argv.push("--serial".into());
    }
    if uncapped {
        if !serial {
            argv.push("--unmapped-max".into());
            argv.push(max.to_string());
        }
        if flags.get("budget-ms").is_none() && flags.get("max-minutes").is_none() {
            argv.push("--mapped-budget-ms".into());
            argv.push(MAPPED_BUDGET_MS.to_string());
        }
    }
    let mut envelope = crate::runner::run(&Flags::parse(&argv));
    envelope.command = COMMAND.into();
    let scored_before = done_ids.len();
    let has_result = |mutant: &Mutant| run_out.join("results").join(format!("{}.json", mutant.id)).is_file();
    let scored = points.iter().filter(|(mutant, _)| has_result(mutant)).count();
    let scored_now = scored.saturating_sub(scored_before);
    let not_run = points.len() - scored;
    let whole = scored_before == 0 && scored == points.len();
    envelope.summary = if whole {
        format!("{scope}: {}. {}", count(points.len(), "mutant"), envelope.summary)
    } else {
        format!("{scope}: {scored} of {} scored, {scored_now} this call. {}", count(points.len(), "mutant"), envelope.summary)
    };
    let started_over = if asked_to_continue && !continuing {
        "Started over: the changed lines or their text differ from the last call's, so its results were not kept. "
    } else {
        ""
    };
    if !envelope.ok {
        envelope.next = format!("{started_over}{}", envelope.next);
        return envelope
            .with_extra("changedLines", Value::Int(line_count as i64))
            .with_extra("testLinesSkipped", Value::Int(test_lines_skipped as i64))
            .with_extra("pathLinesSkipped", Value::Int(path_lines_skipped as i64))
            .with_extra("mutantsTotal", Value::Int(points.len() as i64))
            .with_extra("continued", Value::Bool(continuing));
    }
    record_cost(&out, &run_out, &batch, &done_ids, &envelope);
    let recorded = crate::equivalent::load(&package);
    let mut survivors = survivors(&run_out, &batch, &package, &recorded);
    survivors.sort_by_key(position);
    let unbuildable_list = outcomes(&run_out, &batch, "unviable");
    let equivalent = survivors.iter().filter(|item| item.get("equivalent").is_some()).count();
    let open: Vec<&Value> = survivors.iter().filter(|item| item.get("equivalent").is_none()).collect();
    let mut next = if let Some(first) = open.first() {
        let file = first.get("file").and_then(Value::as_str).unwrap_or("");
        let line = first.get("line").and_then(Value::as_int).unwrap_or(0);
        let id = first.get("id").and_then(Value::as_str).unwrap_or("");
        let lines = untested_lines(&open);
        let verb = if lines == 1 { "is" } else { "are" };
        format!(
            "{} {verb} not tested: {} survived there. First: {file}:{line}. Add a test that fails on it, then call mutate diff again. If a survivor cannot change behaviour (equivalent), record it once and later runs label it: {}.",
            count(lines, "changed line"),
            count(open.len(), "mutant"),
            crate::equivalent::record_hint(&package, id, file, line)
        )
    } else if equivalent > 0 {
        format!("No open survivor: {} survived and each is recorded as equivalent (findings add).", count(equivalent, "mutant"))
    } else if envelope_count(&envelope, "killed") > 0 {
        "No survivor: every mutant on the changed lines that ran was caught (see counts for no coverage and did not build).".to_string()
    } else {
        "No survivor and no kill: no test judged these lines (see counts). This is not a pass.".to_string()
    };
    if equivalent > 0 && !open.is_empty() {
        next.push_str(&format!(" {} also survived, recorded as equivalent.", count(equivalent, "mutant")));
    }
    let unbuildable = envelope_count(&envelope, "unviable");
    if unbuildable > 0 {
        next.push_str(&format!(
            " {} did not build: not survivors and not scored; they are listed under unbuildable.",
            count(unbuildable as usize, "mutant")
        ));
    }
    let survivors_left = survivors.len().saturating_sub(LISTED);
    let unbuildable_left = unbuildable_list.len().saturating_sub(LISTED);
    let full_list = out.join("survivors.json");
    if survivors_left > 0 || unbuildable_left > 0 {
        let _ = fs::write(&full_list, json::stringify(&Value::object(vec![("survivors", Value::Array(survivors.clone())), ("unbuildable", Value::Array(unbuildable_list.clone()))])));
        let mut left = Vec::new();
        if survivors_left > 0 {
            left.push(format!("{survivors_left} more survivors"));
        }
        if unbuildable_left > 0 {
            left.push(format!("{unbuildable_left} more that did not build"));
        }
        next.push_str(&format!(" The reply lists {LISTED} of each; {} are in {}.", left.join(" and "), full_list.display()));
    } else {
        let _ = fs::remove_file(&full_list);
    }
    let budget_hit = envelope.extra.iter().any(|(key, value)| key == "budgetHit" && value.as_bool() == Some(true));
    envelope.next_call = None;
    let mapped = envelope.extra.iter().any(|(key, value)| key == "lineMapUsed" && value.as_bool() == Some(true));
    if not_run > 0 {
        let have = if not_run == 1 { "has" } else { "have" };
        if budget_hit {
            next.push_str(&format!(" The time budget ended this call; {} on these lines {have} no result yet.", count(not_run, "mutant")));
        } else if uncapped && !mapped {
            next.push_str(&format!(
                " {} on these lines {have} no result yet: with no line map one call scores {max}, spread across the changed files. serial: true (--serial) scores them all in one call, each with the whole suite.",
                count(not_run, "more mutant")
            ));
        } else {
            next.push_str(&format!(" {} on these lines {have} no result yet (each call spreads maxMutants across the changed files).", count(not_run, "more mutant")));
        }
        let what = if uncapped && (mapped || serial) { "the rest".to_string() } else { format!("the next {}", not_run.min(max)) };
        next.push_str(&format!(" To score {what} without rerunning these, make the same call with continue: true (nextCall)."));
        envelope.next_call = Some(continue_argv(flags, &package, &out));
    }
    envelope.next = format!("{started_over}{next}");
    envelope
        .with_extra("changedLines", Value::Int(line_count as i64))
        .with_extra("testLinesSkipped", Value::Int(test_lines_skipped as i64))
        .with_extra("pathLinesSkipped", Value::Int(path_lines_skipped as i64))
        .with_extra("mutantCount", Value::Int(batch.len() as i64))
        .with_extra("mutantsTotal", Value::Int(points.len() as i64))
        .with_extra("scoredThisCall", Value::Int(scored_now as i64))
        .with_extra("continued", Value::Bool(continuing))
        .with_extra("notRun", Value::Int(not_run as i64))
        .with_extra("survivors", Value::Array(survivors.into_iter().take(LISTED).collect()))
        .with_extra("survivorsLeftOut", Value::Int(survivors_left as i64))
        .with_extra("equivalent", Value::Int(equivalent as i64))
        .with_extra("unbuildable", Value::Array(unbuildable_list.into_iter().take(LISTED).collect()))
        .with_extra("unbuildableLeftOut", Value::Int(unbuildable_left as i64))
        .with_extra("fullList", if survivors_left > 0 || unbuildable_left > 0 { Value::String(full_list.display().to_string()) } else { Value::Null })
        .with_extra("results", Value::String(run_out.join("results").display().to_string()))
}

/// A `--path` value as written: forward slashes, no leading `./`, no trailing `/`.
fn clean_path(path: &str) -> String {
    let path = path.trim().replace('\\', "/");
    let path = path.strip_prefix("./").unwrap_or(&path);
    path.trim_end_matches('/').to_string()
}

/// A changed file (from the package) is in `path` when `path` names it or a folder above it, or
/// when `path` is a glob (`*`, `?`, `**`) that matches it or a folder above it. `*` and `?` stop
/// at `/`; `**` does not.
fn path_matches(path: &str, file: &str) -> bool {
    if path.is_empty() || path == "." {
        return true;
    }
    if !path.contains(['*', '?']) {
        return file == path || file.starts_with(&format!("{path}/"));
    }
    let pattern = path.as_bytes();
    if glob(pattern, file.as_bytes()) {
        return true;
    }
    file.match_indices('/').any(|(at, _)| glob(pattern, &file.as_bytes()[..at]))
}

fn glob(pattern: &[u8], text: &[u8]) -> bool {
    match pattern.first() {
        None => text.is_empty(),
        Some(b'*') if pattern.get(1) == Some(&b'*') => {
            // `**/` also matches no folder at all.
            let rest = &pattern[2..];
            if rest.first() == Some(&b'/') && glob(&rest[1..], text) {
                return true;
            }
            (0..=text.len()).any(|at| glob(rest, &text[at..]))
        }
        Some(b'*') => {
            let rest = &pattern[1..];
            let mut at = 0;
            loop {
                if glob(rest, &text[at..]) {
                    return true;
                }
                if at == text.len() || text[at] == b'/' {
                    return false;
                }
                at += 1;
            }
        }
        Some(b'?') => text.first().is_some_and(|byte| *byte != b'/') && glob(&pattern[1..], &text[1..]),
        Some(byte) => text.first() == Some(byte) && glob(&pattern[1..], &text[1..]),
    }
}

/// Survivors and did-not-build rows listed in a reply; the rest go to `<out>/survivors.json`.
const LISTED: usize = 10;

/// The order mutants are scored in, so a capped call covers every changed file: round-robin over
/// the files (in name order), and inside a file one mutant per changed line before a second one
/// on any line. A call that stops early has still touched each file.
fn spread(points: Vec<(Mutant, String)>) -> Vec<(Mutant, String)> {
    let mut by_file: Vec<(String, Vec<(Mutant, String)>)> = Vec::new();
    for point in points {
        match by_file.iter_mut().find(|(file, _)| *file == point.0.file) {
            Some((_, items)) => items.push(point),
            None => by_file.push((point.0.file.clone(), vec![point])),
        }
    }
    by_file.sort_by(|a, b| a.0.cmp(&b.0));
    let mut queues: Vec<std::collections::VecDeque<(Mutant, String)>> = Vec::new();
    for (_, mut items) in by_file {
        items.sort_by(|a, b| a.0.line.cmp(&b.0.line).then(a.0.start.cmp(&b.0.start)));
        // Round r takes the r-th mutant of each line, lines in order.
        let mut rank: Vec<(usize, u32, usize, (Mutant, String))> = Vec::new();
        let mut last_line = None;
        let mut nth = 0usize;
        for item in items {
            if last_line == Some(item.0.line) {
                nth += 1;
            } else {
                nth = 0;
                last_line = Some(item.0.line);
            }
            rank.push((nth, item.0.line, item.0.start, item));
        }
        rank.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        queues.push(rank.into_iter().map(|(_, _, _, item)| item).collect());
    }
    let mut ordered = Vec::new();
    while queues.iter().any(|queue| !queue.is_empty()) {
        for queue in &mut queues {
            if let Some(item) = queue.pop_front() {
                ordered.push(item);
            }
        }
    }
    ordered
}

/// What a call scores: the base commit, every mutant in order, the text it was read from, and
/// the options that change the set. Equal keys mean `--continue` may keep the earlier results.
fn plan_key(base: &str, points: &[(Mutant, String)], wide: bool, include_tests: bool) -> String {
    let mut text = format!("{base}\0{wide}\0{include_tests}\0");
    let mut files: Vec<(&str, &str)> = Vec::new();
    for (mutant, body) in points {
        text.push_str(&mutant.id);
        text.push('\0');
        if !files.iter().any(|(file, _)| *file == mutant.file) {
            files.push((&mutant.file, body));
        }
    }
    for (file, body) in files {
        text.push_str(file);
        text.push('\0');
        text.push_str(body);
        text.push('\0');
    }
    crate::hash::sha256_hex(text.as_bytes())
}

fn read_plan(out: &Path) -> Option<String> {
    let value = json::parse(&fs::read_to_string(out.join("plan.json")).ok()?).ok()?;
    value.get("key").and_then(Value::as_str).map(str::to_string)
}

fn write_plan(out: &Path, key: &str) -> Result<(), String> {
    fs::create_dir_all(out).map_err(|error| format!("{} could not be created: {error}", out.display()))?;
    fs::write(out.join("plan.json"), json::stringify(&Value::object(vec![("key", Value::String(key.into()))]))).map_err(|error| error.to_string())
}

/// The same call, continued: every flag as given, plus `--continue`.
fn continue_argv(flags: &Flags, package: &Path, out: &Path) -> Vec<String> {
    let mut argv: Vec<String> = vec!["mutate".into(), "diff".into(), "--package".into(), package.display().to_string(), "--out".into(), out.display().to_string()];
    for key in ["since", "src", "suite-command", "timeout-ms", "workers", "max-mutants", "budget-ms", "max-minutes", "operators", "line-map"] {
        if let Some(value) = flags.get(key) {
            argv.push(format!("--{key}"));
            argv.push(value.to_string());
        }
    }
    for path in flags.texts("path") {
        argv.push("--path".into());
        argv.push(path);
    }
    for key in ["include-tests", "low-priority", "serial"] {
        if flags.bool(key, false) {
            argv.push(format!("--{key}"));
        }
    }
    if !flags.bool("confirm", true) {
        argv.push("--no-confirm".into());
    }
    argv.push("--continue".into());
    argv
}

/// After a call that scored mutants: what one cost, for the next `--estimate`. Kept in
/// `<out>/cost.json`, outside the folders a fresh call clears.
fn record_cost(out: &Path, run_out: &Path, batch: &[(Mutant, String)], earlier: &std::collections::HashSet<String>, envelope: &Envelope) {
    let mut durations: Vec<u64> = Vec::new();
    for (mutant, _) in batch.iter().filter(|(mutant, _)| !earlier.contains(&mutant.id)) {
        let Ok(text) = fs::read_to_string(run_out.join("results").join(format!("{}.json", mutant.id))) else { continue };
        let Ok(value) = json::parse(&text) else { continue };
        let ran = matches!(value.get("outcome").and_then(Value::as_str), Some("killed" | "survived" | "unviable" | "flaky"));
        if let (true, Some(ms)) = (ran, value.get("durationMs").and_then(Value::as_int)) {
            durations.push(ms.max(0) as u64);
        }
    }
    if durations.is_empty() {
        return;
    }
    durations.sort_unstable();
    let per = durations[durations.len() / 2];
    let read = |file: &str, key: &str| fs::read_to_string(run_out.join(file)).ok().and_then(|text| json::parse(&text).ok()).and_then(|value| value.get(key).and_then(Value::as_int));
    let suite = envelope.extra.iter().find(|(key, _)| key == "suiteCommand").and_then(|(_, value)| value.as_str()).unwrap_or("").to_string();
    let mut entries = vec![
        ("suiteCommand", Value::String(suite)),
        ("perMutantMs", Value::Int(per as i64)),
        ("samples", Value::Int(durations.len() as i64)),
    ];
    if let Some(ms) = read("baseline.json", "durationMs") {
        entries.push(("baselineMs", Value::Int(ms)));
    }
    if let Some(ms) = read("tree.json", "checkMs") {
        entries.push(("treeCheckMs", Value::Int(ms)));
    }
    let _ = fs::write(out.join("cost.json"), json::stringify(&Value::object(entries)));
}

struct EstimateScope<'a> {
    max: usize,
    pending: usize,
    this_call: usize,
    continuing: bool,
    run_out: &'a Path,
    /// No maxMutants, and a line map (or serial) lets one call score every mutant.
    lifted: bool,
}

/// `--estimate`: the mutant count and what a call would cost, without scoring anything. The
/// time comes from the last call's record when it used the same suite command, else from two
/// timed runs of the suite in a mutant tree with no mutant (the first builds what the kept build
/// folder lacks; the second costs what one mutant does).
fn estimate(flags: &Flags, package: &Path, out: &Path, points: &[(Mutant, String)], scope: &str, at: EstimateScope) -> Envelope {
    let suite = match crate::runner::suite_command(flags, package) {
        Ok(suite) => suite,
        Err(message) => return Envelope::new(false, COMMAND, message, "Pass --suite-command for a layout this binary does not discover."),
    };
    let workers = flags.int("workers", 1).unwrap_or(1).max(1) as usize;
    let timeout = std::time::Duration::from_millis(flags.int("timeout-ms", 120_000).unwrap_or(120_000).max(1));
    let recorded = fs::read_to_string(out.join("cost.json")).ok().and_then(|text| json::parse(&text).ok()).filter(|value| value.get("suiteCommand").and_then(Value::as_str) == Some(suite.as_str()));
    let int = |value: &Value, key: &str| value.get(key).and_then(Value::as_int).map(|ms| ms.max(0) as u64);
    let (basis, per, baseline, check, tree) = match &recorded {
        Some(value) => {
            let per = int(value, "perMutantMs").unwrap_or(0);
            ("last run", per, int(value, "baselineMs").unwrap_or(per), int(value, "treeCheckMs").unwrap_or(per), None)
        }
        None => match crate::runner::time_tree(package, &suite, timeout, &points[0].0.file) {
            Ok(timing) => ("tree check", timing.per_mutant_ms, timing.per_mutant_ms, timing.per_mutant_ms, Some(timing)),
            Err(message) => return Envelope::new(false, COMMAND, message, "Nothing was mutated. Fix the cause, then ask for the estimate again.").with_extra("suiteCommand", Value::String(suite)),
        },
    };
    let baseline_kept = at.continuing && at.run_out.join("baseline.json").is_file();
    let baseline = if baseline_kept { 0 } else { baseline };
    let rounds = |mutants: usize| mutants.div_ceil(workers) as u64;
    let this_call_ms = baseline + check + rounds(at.this_call) * per;
    let calls = at.pending.div_ceil(at.max).max(1) as u64;
    let all_ms = baseline + calls * check + rounds(at.pending) * per;
    let limit = if at.lifted { "a line map or serial lifts maxMutants".to_string() } else { format!("maxMutants {}", at.max) };
    let mut summary = format!(
        "{scope}: {}, {} still to score. One call scores {} ({limit}): about {}. All of them: about {} in {}. One mutant costs about {} ({basis}).",
        count(points.len(), "mutant"),
        at.pending,
        at.this_call,
        minutes(this_call_ms),
        minutes(all_ms),
        count(calls as usize, "call"),
        minutes(per)
    );
    if let Some(timing) = &tree {
        summary.push_str(&format!(" The first run in the tree took {}{}.", minutes(timing.setup_ms), if timing.cold { " (a first build into an empty build folder, paid once)" } else { "" }));
    }
    let mut call = continue_argv(flags, package, out);
    call.retain(|word| word != "--continue" && word != "--estimate");
    if at.continuing {
        call.push("--continue".into());
    }
    let mut entries = vec![
        ("basis", Value::String(basis.into())),
        ("perMutantMs", Value::Int(per as i64)),
        ("baselineMs", Value::Int(baseline as i64)),
        ("treeCheckMs", Value::Int(check as i64)),
        ("thisCallMs", Value::Int(this_call_ms as i64)),
        ("allMs", Value::Int(all_ms as i64)),
        ("calls", Value::Int(calls as i64)),
        ("workers", Value::Int(workers as i64)),
    ];
    if let Some(timing) = &tree {
        entries.push(("setupMs", Value::Int(timing.setup_ms as i64)));
    }
    let mut envelope = Envelope::new(
        true,
        COMMAND,
        summary,
        "Nothing was mutated. Make the same call without estimate to score them (nextCall). maxMinutes caps a call's time; lowPriority runs it below normal priority beside other work.",
    )
    .with_extra("mutantCount", Value::Int(at.this_call as i64))
    .with_extra("mutantsTotal", Value::Int(points.len() as i64))
    .with_extra("notRun", Value::Int(at.pending as i64))
    .with_extra("estimate", Value::object(entries))
    .with_extra("suiteCommand", Value::String(suite));
    if let Some(timing) = tree {
        envelope = envelope
            .with_extra("treePath", Value::String(timing.tree.path.display().to_string()))
            .with_extra("treePathLength", Value::Int(timing.tree.length as i64))
            .with_extra("longestTreePathLength", Value::Int(timing.tree.longest as i64));
    }
    envelope.next_call = Some(call);
    envelope
}

/// `40s`, `3 min`, `1 h 05 min`.
fn minutes(ms: u64) -> String {
    let seconds = ms.div_ceil(1000);
    if seconds < 90 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{} min", seconds.div_ceil(60))
    } else {
        format!("{} h {:02} min", seconds / 3600, (seconds % 3600).div_ceil(60).min(59))
    }
}

fn position(item: &Value) -> (String, i64, i64) {
    (
        item.get("file").and_then(Value::as_str).unwrap_or("").to_string(),
        item.get("line").and_then(Value::as_int).unwrap_or(0),
        item.get("column").and_then(Value::as_int).unwrap_or(0),
    )
}

/// Changed lines per file, relative to the package: tracked edits from `git diff <base>` and
/// every line of an untracked file.
fn changed_lines(package: &Path, base: &str) -> Result<Vec<(String, Vec<u32>)>, String> {
    let diff = git(package, &["diff", "-U0", "--no-renames", "--no-color", "--relative", base, "--"])?;
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

fn survivors(run_out: &Path, points: &[(Mutant, String)], package: &Path, recorded: &[crate::equivalent::Recorded]) -> Vec<Value> {
    let mut found = Vec::new();
    for (mutant, text) in points {
        let path = run_out.join("results").join(format!("{}.json", mutant.id));
        let Ok(body) = fs::read_to_string(&path) else { continue };
        let Ok(value) = json::parse(&body) else { continue };
        if value.get("outcome").and_then(Value::as_str) != Some("survived") {
            continue;
        }
        let mut entries = vec![
            ("file", Value::String(mutant.file.clone())),
            ("line", Value::Int(mutant.line as i64)),
            ("column", Value::Int(column(text, mutant.start) as i64)),
            ("original", Value::String(mutant.original.clone())),
            ("replacement", Value::String(mutant.replacement.clone())),
            ("id", Value::String(mutant.id.clone())),
        ];
        if let Some(record) = crate::equivalent::find(recorded, package, &mutant.id, &mutant.file, mutant.line as i64) {
            entries.push(("equivalent", Value::String(record.reason.clone())));
        }
        found.push(Value::object(entries));
    }
    found
}

/// The mutants with this outcome, as `file`, `line`, `original`, and `replacement`.
fn outcomes(run_out: &Path, points: &[(Mutant, String)], outcome: &str) -> Vec<Value> {
    let mut found = Vec::new();
    for (mutant, _) in points {
        let path = run_out.join("results").join(format!("{}.json", mutant.id));
        let Ok(body) = fs::read_to_string(&path) else { continue };
        let Ok(value) = json::parse(&body) else { continue };
        if value.get("outcome").and_then(Value::as_str) == Some(outcome) {
            found.push(Value::object(vec![
                ("file", Value::String(mutant.file.clone())),
                ("line", Value::Int(mutant.line as i64)),
                ("original", Value::String(mutant.original.clone())),
                ("replacement", Value::String(mutant.replacement.clone())),
            ]));
        }
    }
    found
}

/// `1 mutant`, `2 mutants`.
fn count(n: usize, noun: &str) -> String {
    if n == 1 { format!("1 {noun}") } else { format!("{n} {noun}s") }
}

/// Distinct `file:line` pairs among the survivors: two mutants on one line are one untested line.
fn untested_lines(survivors: &[&Value]) -> usize {
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
        assert_eq!(untested_lines(&[&survivor(34), &survivor(34)]), 1);
        assert_eq!(untested_lines(&[&survivor(34), &survivor(35)]), 2);
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
        let dir = crate::testdir::TempDir::new("diff");
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

    fn repo(name: &str) -> crate::testdir::TempDir {
        let dir = crate::testdir::TempDir::new(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        git_ok(&dir, &["init", "-q", "-b", "main"]);
        git_ok(&dir, &["config", "user.email", "a@b.c"]);
        git_ok(&dir, &["config", "user.name", "a"]);
        git_ok(&dir, &["config", "core.autocrlf", "false"]);
        dir
    }

    fn argv(dir: &Path, since: &str) -> Vec<String> {
        let out = dir.join("out").display().to_string();
        ["--package", &dir.display().to_string(), "--out", &out, "--suite-command", "echo ok", "--no-confirm", "--since", since]
            .iter()
            .map(|item| item.to_string())
            .collect()
    }

    /// A large Rust workspace (one lane: 5,979 lines): `since main` on a branch while main had moved on scored
    /// main's new lines too. One ref is read from its merge-base with HEAD, so only the branch's
    /// own change is scored. A range works too, and must end at HEAD.
    #[test]
    fn since_a_branch_scores_from_the_merge_base() {
        let dir = repo("since");
        let body = |f: &str, g: &str| format!("class A {{\n  boolean f(int n) {{ return n {f} 0; }}\n  boolean g(int n) {{ return n {g} 0; }}\n}}\n");
        fs::write(dir.join("src/A.java"), body(">", "<")).unwrap();
        git_ok(&dir, &["add", "."]);
        git_ok(&dir, &["commit", "-q", "-m", "base"]);
        git_ok(&dir, &["checkout", "-q", "-b", "feature"]);
        fs::write(dir.join("src/A.java"), body(">=", "<")).unwrap();
        git_ok(&dir, &["commit", "-q", "-am", "feature edits f"]);
        git_ok(&dir, &["checkout", "-q", "main"]);
        fs::write(dir.join("src/A.java"), body(">", "<=")).unwrap();
        git_ok(&dir, &["commit", "-q", "-am", "main edits g"]);
        git_ok(&dir, &["checkout", "-q", "feature"]);

        for since in ["main", "main...HEAD", "main..."] {
            let envelope = diff(&Flags::parse(&argv(&dir, since)));
            assert!(envelope.ok, "{since}: {} {}", envelope.summary, envelope.next);
            assert!(envelope.summary.starts_with("1 changed line in 1 file since main"), "{since}: {}", envelope.summary);
            assert!(envelope.summary.contains("(merge-base "), "{since}: {}", envelope.summary);
        }
        // Two dots start at main itself, as git reads them: main's edit to g shows as a change.
        let two_dots = diff(&Flags::parse(&argv(&dir, "main..HEAD")));
        assert!(two_dots.summary.starts_with("2 changed lines in 1 file since main..HEAD"), "{}", two_dots.summary);
        let elsewhere = diff(&Flags::parse(&argv(&dir, "HEAD~1..main")));
        assert!(!elsewhere.ok);
        assert!(elsewhere.summary.contains("main is not the commit checked out"), "{}", elsewhere.summary);
        let unknown = diff(&Flags::parse(&argv(&dir, "no-such-branch")));
        assert!(!unknown.ok && unknown.summary.contains("not a commit"), "{}", unknown.summary);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A large Rust workspace (one crate: all 6 survivors in test assertions): changed lines in test code
    /// are not mutated unless asked, and the summary counts them.
    #[test]
    fn changed_test_lines_are_skipped_and_counted() {
        let dir = repo("difftests");
        fs::create_dir_all(dir.join("tests")).unwrap();
        fs::write(dir.join("src/lib.rs"), "pub fn f(n: u32) -> bool {\n    n > 1\n}\n").unwrap();
        git_ok(&dir, &["add", "."]);
        git_ok(&dir, &["commit", "-q", "-m", "base"]);
        fs::write(
            dir.join("src/lib.rs"),
            "pub fn f(n: u32) -> bool {\n    n >= 1\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn one() {\n        assert!(super::f(1) && true);\n    }\n}\n",
        )
        .unwrap();
        fs::write(dir.join("tests/it.rs"), "#[test]\nfn it() { assert!(1 > 0); }\n").unwrap();
        let envelope = diff(&Flags::parse(&argv(&dir, "HEAD")));
        assert!(envelope.ok, "{} {}", envelope.summary, envelope.next);
        // git aligns the old closing brace with the last new one, so lines 2-10 are the change:
        // 2-4 are source, 5-10 the test module, and the two lines of tests/it.rs are test code.
        assert!(envelope.summary.starts_with("3 changed lines in 1 file since HEAD (8 test lines skipped): 1 mutant."), "{}", envelope.summary);
        let value = envelope.to_value("");
        assert_eq!(value.get("testLinesSkipped").and_then(Value::as_int), Some(8));
        let mut with_tests = argv(&dir, "HEAD");
        with_tests.push("--include-tests".into());
        let included = diff(&Flags::parse(&with_tests));
        assert!(included.summary.starts_with("11 changed lines in 2 files since HEAD: 4 mutants."), "{}", included.summary);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A large Rust workspace (one crate: 5 of 15 mutants did not build): a mutant the compiler rejects is
    /// listed under `unbuildable`, never as a survivor, and `next` says so.
    #[test]
    fn a_mutant_that_does_not_build_is_listed_apart_from_survivors() {
        let dir = repo("unbuildable");
        fs::write(dir.join("src/A.java"), "class A {\n  boolean g(int n) { return n == 0; }\n}\n").unwrap();
        git_ok(&dir, &["add", "."]);
        git_ok(&dir, &["commit", "-q", "-m", "base"]);
        fs::write(dir.join("src/A.java"), "class A {\n  boolean g(int n) { return n > 0 && n < 9; }\n}\n").unwrap();
        let suite = "if grep -q '>=' src/A.java; then echo 'src/A.java:2: error: incompatible types' >&2; exit 1; else echo ok; fi";
        let out = dir.join("out").display().to_string();
        let argv: Vec<String> = ["--package", &dir.display().to_string(), "--out", &out, "--suite-command", suite, "--no-confirm"].iter().map(|item| item.to_string()).collect();
        let envelope = diff(&Flags::parse(&argv));
        assert!(envelope.ok, "{} {}", envelope.summary, envelope.next);
        let value = envelope.to_value("");
        let unbuildable = json::stringify_compact(value.get("unbuildable").unwrap());
        assert_eq!(unbuildable, r#"[{"file":"src/A.java","line":2,"original":">","replacement":">="}]"#);
        let survivors = json::stringify_compact(value.get("survivors").unwrap());
        assert!(!survivors.contains("\">\""), "{survivors}");
        assert!(envelope.next.contains("1 mutant did not build: not survivors and not scored"), "{}", envelope.next);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A large Rust workspace: `.probatio/` was written inside the package (a workspace member) and committed
    /// by mistake twice. It is at the project root now, and it ignores itself.
    #[test]
    fn output_goes_to_the_project_root_and_ignores_itself() {
        let dir = repo("rootout");
        let member = dir.join("crates").join("member");
        fs::create_dir_all(member.join("src")).unwrap();
        fs::write(member.join("src/A.java"), "class A { boolean f(int n) { return n > 0; } }\n").unwrap();
        git_ok(&dir, &["add", "."]);
        git_ok(&dir, &["commit", "-q", "-m", "base"]);
        let expected = dir.join(".probatio").join("diff").join("crates").join("member");
        let normal = |path: &Path| path.display().to_string().replace('\\', "/").to_ascii_lowercase().trim_start_matches("//?/").to_string();
        let found = default_diff_out(&member);
        let root = normal(&crate::worktree::toplevel(&dir).unwrap());
        assert_eq!(normal(&found), format!("{root}/.probatio/diff/crates/member"));
        fs::write(member.join("src/A.java"), "class A { boolean f(int n) { return n >= 0; } }\n").unwrap();
        let argv: Vec<String> = ["--package", &member.display().to_string(), "--suite-command", "echo ok", "--no-confirm"].iter().map(|item| item.to_string()).collect();
        let envelope = diff(&Flags::parse(&argv));
        assert!(envelope.ok, "{} {}", envelope.summary, envelope.next);
        assert!(expected.join("run").join("results").is_dir());
        assert!(!member.join(".probatio").exists());
        assert!(fs::read_to_string(dir.join(".probatio/.gitignore")).unwrap().lines().any(|line| line == "*"));
        let status = Command::new("git").arg("-C").arg(&dir).args(["status", "--porcelain"]).output().unwrap();
        let status = String::from_utf8_lossy(&status.stdout).to_string();
        assert!(!status.contains(".probatio"), "{status}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A repo with one commit, and `files` written as untracked edits: every line counts.
    fn edited(name: &str, files: &[(&str, String)]) -> crate::testdir::TempDir {
        let dir = repo(name);
        fs::write(dir.join("README.md"), "base\n").unwrap();
        git_ok(&dir, &["add", "."]);
        git_ok(&dir, &["commit", "-q", "-m", "base"]);
        for (file, text) in files {
            fs::write(dir.join(file), text).unwrap();
        }
        dir
    }

    /// `count` methods, one `>=` mutant each.
    fn java(class: &str, count: usize) -> String {
        let mut text = format!("class {class} {{\n");
        for index in 0..count {
            text.push_str(&format!("  boolean g{index}(int n) {{ return n >= {index}; }}\n"));
        }
        text.push_str("}\n");
        text
    }

    fn flags_of(dir: &Path, extra: &[&str]) -> Flags {
        let out = dir.join("out").display().to_string();
        let mut argv: Vec<String> = ["--package", &dir.display().to_string(), "--out", &out, "--suite-command", "echo ok", "--no-confirm"].iter().map(|item| item.to_string()).collect();
        argv.extend(extra.iter().map(|item| item.to_string()));
        Flags::parse(&argv)
    }

    fn files_of(value: &Value, key: &str) -> Vec<String> {
        match value.get(key) {
            Some(Value::Array(items)) => items.iter().filter_map(|item| item.get("file").and_then(Value::as_str)).map(str::to_string).collect(),
            _ => Vec::new(),
        }
    }

    fn result_times(dir: &Path) -> Vec<(String, std::time::SystemTime)> {
        let mut found: Vec<(String, std::time::SystemTime)> = fs::read_dir(dir.join("out/run/results"))
            .unwrap()
            .flatten()
            .map(|entry| (entry.file_name().to_string_lossy().into_owned(), entry.metadata().unwrap().modified().unwrap()))
            .collect();
        found.sort();
        found
    }

    /// A large Rust workspace: a capped run took the whole budget from the alphabetically first file, and a
    /// larger cap reran the first mutants (one lane ran 24 of 293). The budget is spread across
    /// the changed files, and `--continue` (the reply's nextCall) scores the rest without
    /// rerunning what has a result. A changed tree starts over and says so.
    #[test]
    fn a_capped_diff_covers_every_file_and_continues_without_rerunning() {
        let dir = edited("diff-continue", &[("src/A.java", java("A", 5)), ("src/B.java", java("B", 1))]);
        let first = diff(&flags_of(&dir, &["--max-mutants", "2"]));
        assert!(first.ok, "{} {}", first.summary, first.next);
        assert!(first.summary.starts_with("10 changed lines in 2 files since HEAD: 2 of 6 mutants scored, 2 this call."), "{}", first.summary);
        let value = first.to_value("");
        assert_eq!(files_of(&value, "survivors"), ["src/A.java", "src/B.java"], "the cap covers both files");
        assert_eq!(value.get("notRun").and_then(Value::as_int), Some(4));
        assert!(first.next.contains("continue: true"), "{}", first.next);
        let call = first.next_call.clone().expect("a capped run names the next call");
        assert_eq!(&call[..2], ["mutate", "diff"]);
        assert!(call.contains(&"--continue".to_string()) && call.contains(&"--max-mutants".to_string()), "{call:?}");
        let before = result_times(&dir);
        assert_eq!(before.len(), 2);

        let second = diff(&Flags::parse(&call[2..]));
        assert!(second.ok, "{} {}", second.summary, second.next);
        assert!(second.summary.contains(": 4 of 6 mutants scored, 2 this call."), "{}", second.summary);
        let value = second.to_value("");
        assert_eq!(value.get("continued").and_then(Value::as_bool), Some(true));
        assert_eq!(files_of(&value, "survivors").len(), 4, "survivors so far, not only this call's");
        let after = result_times(&dir);
        assert_eq!(after.len(), 4);
        assert!(before.iter().all(|kept| after.contains(kept)), "a result of the first call was rewritten: {before:?} {after:?}");

        let third = diff(&Flags::parse(&second.next_call.clone().unwrap()[2..]));
        assert!(third.summary.contains(": 6 of 6 mutants scored, 2 this call."), "{}", third.summary);
        assert!(third.next_call.is_none(), "{:?}", third.next_call);
        assert_eq!(third.to_value("").get("notRun").and_then(Value::as_int), Some(0));

        fs::write(dir.join("src/B.java"), java("B", 1).replace(">= 0", ">= 7")).unwrap();
        let changed = diff(&flags_of(&dir, &["--max-mutants", "2", "--continue"]));
        assert!(changed.next.starts_with("Started over"), "{}", changed.next);
        assert!(changed.summary.contains(": 2 of 6 mutants scored, 2 this call."), "{}", changed.summary);
        let fresh = diff(&flags_of(&dir, &["--max-mutants", "2"]));
        assert!(fresh.summary.contains(": 2 of 6 mutants scored"), "a call without continue starts over: {}", fresh.summary);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_path_is_a_prefix_or_a_glob() {
        assert!(path_matches("minisql/storage", "minisql/storage/btree.py"));
        assert!(path_matches("minisql/storage", "minisql/storage"));
        assert!(!path_matches("minisql/storage", "minisql/storage_old/a.py"));
        assert!(!path_matches("minisql/storage", "minisql/sql/parser.py"));
        assert!(path_matches("", "a.py") && path_matches(".", "a.py"));
        assert!(path_matches("src/*.py", "src/a.py"));
        assert!(!path_matches("src/*.py", "src/sub/a.py"));
        assert!(path_matches("src/**/*.py", "src/sub/deep/a.py"));
        assert!(path_matches("src/**/*.py", "src/a.py"));
        assert!(path_matches("**/engine.py", "minisql/storage/engine.py"));
        assert!(path_matches("min*/stor?ge", "minisql/storage/pager.py"), "a glob that names a folder keeps what is under it");
        assert!(!path_matches("src/?.py", "src/ab.py"));
        assert_eq!(clean_path("./src/storage/"), "src/storage");
        assert_eq!(clean_path("src\\storage"), "src/storage");
    }

    /// An A/B run: a diff since the seed commit held other agents' files, and the mutant budget
    /// went to them. `--path` keeps only the caller's files and counts the lines it left out.
    #[test]
    fn a_path_filter_scores_only_those_files_and_counts_the_rest() {
        let dir = edited("diff-path", &[("src/A.java", java("A", 2)), ("src/B.java", java("B", 3))]);
        let envelope = diff(&flags_of(&dir, &["--path", "src/B.java"]));
        assert!(envelope.ok, "{} {}", envelope.summary, envelope.next);
        assert!(envelope.summary.starts_with("5 changed lines in 1 file since HEAD (4 changed lines outside path): 3 mutants."), "{}", envelope.summary);
        let value = envelope.to_value("");
        assert_eq!(value.get("pathLinesSkipped").and_then(Value::as_int), Some(4));
        assert_eq!(files_of(&value, "survivors"), ["src/B.java", "src/B.java", "src/B.java"]);
        let both = diff(&flags_of(&dir, &["--path", "src/*.java"]));
        assert_eq!(both.to_value("").get("mutantsTotal").and_then(Value::as_int), Some(5), "{}", both.summary);
        let none = diff(&flags_of(&dir, &["--path", "lib"]));
        assert!(none.ok);
        assert!(none.summary.contains("under lib: 9 lines changed outside it"), "{}", none.summary);
        assert!(none.next.contains("Check paths"), "{}", none.next);
        let _ = fs::remove_dir_all(&dir);
    }

    /// With no line map and no maxMutants a call scores 30 and says serial scores them all;
    /// serial: true scores every mutant in one call. A continued call keeps --path and --serial.
    #[test]
    fn serial_scores_every_mutant_in_one_call() {
        let dir = edited("diff-serial", &[("src/A.java", java("A", 33))]);
        let capped = diff(&flags_of(&dir, &[]));
        assert!(capped.ok, "{} {}", capped.summary, capped.next);
        let value = capped.to_value("");
        assert_eq!(value.get("scoredThisCall").and_then(Value::as_int), Some(30), "{}", capped.summary);
        assert_eq!(value.get("notRun").and_then(Value::as_int), Some(3));
        assert!(capped.next.contains("serial: true"), "{}", capped.next);
        let serial = diff(&flags_of(&dir, &["--serial", "--path", "src"]));
        assert!(serial.ok, "{} {}", serial.summary, serial.next);
        let value = serial.to_value("");
        assert_eq!(value.get("scoredThisCall").and_then(Value::as_int), Some(33), "{}", serial.summary);
        assert_eq!(value.get("notRun").and_then(Value::as_int), Some(0));
        assert!(serial.next_call.is_none(), "{:?}", serial.next_call);
        let mut argv: Vec<String> = Vec::new();
        for (key, value) in [("max-mutants", "2"), ("path", "src")] {
            argv.push(format!("--{key}"));
            argv.push(value.to_string());
        }
        argv.push("--serial".into());
        let more = diff(&flags_of(&dir, &argv.iter().map(String::as_str).collect::<Vec<_>>()));
        let call = more.next_call.clone().expect("a capped call continues");
        assert!(call.windows(2).any(|pair| pair == ["--path", "src"]) && call.contains(&"--serial".to_string()), "{call:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// An A/B run on a stdlib-unittest project: no coverage.py, so no line map, and every diff
    /// over 30 mutants needed several calls. The unittest baseline now writes its own map, and
    /// one call scores every mutant, each with the tests that ran its line.
    #[test]
    fn a_unittest_diff_scores_every_mutant_in_one_call() {
        let dir = repo("diff-unittest");
        fs::write(dir.join("README.md"), "base\n").unwrap();
        git_ok(&dir, &["add", "."]);
        git_ok(&dir, &["commit", "-q", "-m", "base"]);
        let mut expr = String::new();
        for index in 0..20 {
            if index > 0 {
                expr.push_str(" and ");
            }
            expr.push_str(&format!("n > {index}"));
        }
        fs::create_dir_all(dir.join("pkg")).unwrap();
        fs::write(dir.join("pkg/__init__.py"), "").unwrap();
        fs::write(dir.join("pkg/gate.py"), format!("def gate(n):\n    return {expr}\n\ndef dark(n):\n    return n < 0\n")).unwrap();
        fs::create_dir_all(dir.join("tests")).unwrap();
        fs::write(
            dir.join("tests/test_gate.py"),
            "import unittest\nfrom pkg import gate\n\nclass GateTests(unittest.TestCase):\n    def test_high(self):\n        self.assertTrue(gate.gate(100))\n    def test_low(self):\n        self.assertFalse(gate.gate(-1))\n",
        )
        .unwrap();
        let out = dir.join("out").display().to_string();
        let argv: Vec<String> = ["--package", &dir.display().to_string(), "--out", &out, "--suite-command", "python3 -m unittest discover -s tests", "--no-confirm", "--path", "pkg"]
            .iter()
            .map(|item| item.to_string())
            .collect();
        let envelope = diff(&Flags::parse(&argv));
        assert!(envelope.ok, "{} {}", envelope.summary, envelope.next);
        let value = envelope.to_value("");
        let total = value.get("mutantsTotal").and_then(Value::as_int).unwrap();
        assert!(total > 30, "{total}");
        assert_eq!(value.get("scoredThisCall").and_then(Value::as_int), Some(total), "{}", envelope.summary);
        assert_eq!(value.get("lineMapUsed").and_then(Value::as_bool), Some(true));
        assert!(envelope.next_call.is_none(), "{:?}", envelope.next_call);
        assert!(dir.join("out/run/lines.json").is_file());
        let counts = value.get("counts").unwrap();
        assert!(counts.get("noCoverage").and_then(Value::as_int).unwrap_or(0) >= 1, "the dark line is no coverage: {}", envelope.summary);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A large Rust workspace: survivors and unbuildable rows were cut to 10 with no sign (one lane had 11).
    #[test]
    fn survivors_past_ten_are_counted_and_written_whole() {
        let dir = edited("diff-left-out", &[("src/A.java", java("A", 12))]);
        let envelope = diff(&flags_of(&dir, &["--max-mutants", "12"]));
        assert!(envelope.ok, "{} {}", envelope.summary, envelope.next);
        let value = envelope.to_value("");
        assert_eq!(files_of(&value, "survivors").len(), 10);
        assert_eq!(value.get("survivorsLeftOut").and_then(Value::as_int), Some(2));
        assert_eq!(value.get("unbuildableLeftOut").and_then(Value::as_int), Some(0));
        let full = value.get("fullList").and_then(Value::as_str).expect("the full list is named");
        let listed = json::parse(&fs::read_to_string(full).unwrap()).unwrap();
        assert_eq!(files_of(&listed, "survivors").len(), 12);
        assert!(envelope.next.contains("2 more survivors are in"), "{}", envelope.next);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A large Rust workspace: `mutate diff` did not forward a time budget. With one, a call stops starting
    /// mutants once it has passed (the baseline counts; one mutant is always scored), says so,
    /// and continue goes on from there.
    #[test]
    fn a_time_budget_stops_the_call_and_continue_goes_on() {
        let dir = edited("diff-budget", &[("src/A.java", java("A", 3))]);
        let envelope = diff(&flags_of(&dir, &["--budget-ms", "1"]));
        assert!(envelope.ok, "{} {}", envelope.summary, envelope.next);
        let value = envelope.to_value("");
        assert_eq!(value.get("scoredThisCall").and_then(Value::as_int), Some(1), "{}", envelope.summary);
        assert_eq!(value.get("budgetHit").and_then(Value::as_bool), Some(true));
        assert!(envelope.next.contains("The time budget ended this call"), "{}", envelope.next);
        let call = envelope.next_call.clone().unwrap();
        assert!(call.windows(2).any(|pair| pair == ["--budget-ms", "1"]), "{call:?}");
        let rest = diff(&flags_of(&dir, &["--continue"]));
        assert!(rest.summary.contains(": 3 of 3 mutants scored, 2 this call."), "{}", rest.summary);
        let _ = fs::remove_dir_all(&dir);
    }

    /// 0.8 checked the budget only while it listed the mutants, before any ran, so a budget that
    /// passed during scoring stopped nothing. Now no mutant starts after it.
    #[test]
    fn a_budget_that_passes_while_scoring_stops_before_the_next_mutant() {
        let dir = edited("diff-budget-late", &[("src/A.java", java("A", 3))]);
        // Fast in your package (the baseline), two seconds in the mutant tree (where .git is a file).
        let flags = flags_of(&dir, &["--budget-ms", "2500"]);
        let mut argv: Vec<String> = Vec::new();
        for key in ["package", "out", "budget-ms"] {
            argv.push(format!("--{key}"));
            argv.push(flags.get(key).unwrap().to_string());
        }
        argv.extend(["--suite-command".to_string(), "if [ -f .git ]; then sleep 2; fi; echo ok".to_string(), "--no-confirm".to_string()]);
        let envelope = diff(&Flags::parse(&argv));
        assert!(envelope.ok, "{} {}", envelope.summary, envelope.next);
        let value = envelope.to_value("");
        assert_eq!(value.get("scoredThisCall").and_then(Value::as_int), Some(1), "{}", envelope.summary);
        assert_eq!(value.get("notRun").and_then(Value::as_int), Some(2));
        assert!(envelope.next.contains("The time budget ended this call"), "{}", envelope.next);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A large Rust workspace: the big crate was never scored because nothing said what a run would cost.
    /// `--estimate` scores nothing and gives the count and the time: from two timed suite runs
    /// in a mutant tree at first, then from the last call's record.
    #[test]
    fn an_estimate_scores_nothing_and_learns_from_the_last_run() {
        let dir = edited("diff-estimate", &[("src/A.java", java("A", 4))]);
        let asked = diff(&flags_of(&dir, &["--estimate", "--max-mutants", "3"]));
        assert!(asked.ok, "{} {}", asked.summary, asked.next);
        assert!(asked.summary.starts_with("6 changed lines in 1 file since HEAD: 4 mutants, 4 still to score. One call scores 3 (maxMutants 3): about"), "{}", asked.summary);
        let value = asked.to_value("");
        let estimate = value.get("estimate").unwrap();
        assert_eq!(estimate.get("basis").and_then(Value::as_str), Some("tree check"));
        assert_eq!(estimate.get("calls").and_then(Value::as_int), Some(2));
        assert!(value.get("treePathLength").and_then(Value::as_int).is_some_and(|length| length > 0));
        assert!(!dir.join("out/run/results").exists(), "an estimate scored a mutant");
        let call = asked.next_call.clone().unwrap();
        assert!(!call.contains(&"--estimate".to_string()), "{call:?}");
        let ran = diff(&Flags::parse(&call[2..]));
        assert!(ran.ok, "{} {}", ran.summary, ran.next);
        let again = diff(&flags_of(&dir, &["--estimate", "--max-mutants", "3"]));
        let value = again.to_value("");
        assert_eq!(value.get("estimate").and_then(|item| item.get("basis")).and_then(Value::as_str), Some("last run"), "{}", again.summary);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A large Rust workspace: line numbers are the working tree's at call time, and an edit during the job
    /// changed what was scored: the tree was restored from the live file after each mutant, and
    /// the next span then failed with "patch does not apply". The run keeps a snapshot.
    #[test]
    fn an_edit_during_the_run_does_not_change_what_is_scored() {
        let dir = edited("diff-snapshot", &[("src/A.java", java("A", 2))]);
        let live = dir.join("src/A.java").display().to_string().replace('\\', "/");
        let flag = dir.join("edited.flag").display().to_string().replace('\\', "/");
        // In the mutant tree (a worktree, where .git is a file), the first run prepends a line to
        // your file, which moves every byte offset in it.
        let suite = format!("if [ -f .git ] && [ ! -f '{flag}' ]; then touch '{flag}'; {{ printf '// edited\\n'; cat '{live}'; }} > '{live}.tmp' && mv '{live}.tmp' '{live}'; fi; echo ok");
        let out = dir.join("out").display().to_string();
        let argv: Vec<String> = ["--package", &dir.display().to_string(), "--out", &out, "--suite-command", &suite, "--no-confirm"].iter().map(|item| item.to_string()).collect();
        let envelope = diff(&Flags::parse(&argv));
        assert!(fs::read_to_string(dir.join("src/A.java")).unwrap().starts_with("// edited\n"), "the suite did not edit the file");
        assert!(envelope.ok, "{} {}", envelope.summary, envelope.next);
        let value = envelope.to_value("");
        assert_eq!(value.get("survived").and_then(Value::as_int), Some(2), "{}", envelope.summary);
        assert_eq!(value.get("errors").and_then(Value::as_int), Some(0), "{}", envelope.summary);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A large Rust workspace: `findings add --status equivalent` was never read, so the same equivalent
    /// mutant was read again on every run. A matching record labels the survivor.
    #[test]
    fn a_survivor_recorded_as_equivalent_is_labelled() {
        let dir = edited("diff-equivalent", &[("src/A.java", java("A", 2))]);
        let first = diff(&flags_of(&dir, &[]));
        let value = first.to_value("");
        let Some(Value::Array(items)) = value.get("survivors") else { panic!("no survivors: {}", first.summary) };
        let id = items[0].get("id").and_then(Value::as_str).unwrap().to_string();
        assert!(first.next.contains(&format!("--status equivalent --id {id} --file src/A.java --line 2")), "{}", first.next);
        let finding = crate::memory::Finding { id, status: "equivalent".into(), reason: "same result for every n".into(), file: None, line: None, hash: None, agent: None };
        crate::memory::append_finding(&probatio_dir(&dir), &finding).unwrap();
        let second = diff(&flags_of(&dir, &[]));
        let value = second.to_value("");
        assert_eq!(value.get("equivalent").and_then(Value::as_int), Some(1));
        let survivors = json::stringify_compact(value.get("survivors").unwrap());
        assert!(survivors.contains(r#""equivalent":"same result for every n""#), "{survivors}");
        assert!(second.next.contains("First: src/A.java:3") && second.next.contains("1 mutant also survived, recorded as equivalent"), "{}", second.next);
        let _ = fs::remove_dir_all(&dir);
    }
}
