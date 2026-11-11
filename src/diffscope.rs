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
    let src = flags.get("src").map(|dir| dir.trim_matches(['/', '\\']).replace('\\', "/"));
    let include_tests = flags.bool("include-tests", false);

    let base = match resolve_since(&package, &since) {
        Ok(base) => base,
        Err(message) => return Envelope::new(false, COMMAND, message, "Pass --since as a ref (HEAD, main, origin/main) or a range ending at HEAD (main..HEAD, main...HEAD), from inside a git checkout."),
    };
    let changed = match changed_lines(&package, &base.commit) {
        Ok(changed) => changed,
        Err(message) => return Envelope::new(false, COMMAND, message, "Pass --since as a ref git can diff against (HEAD, main, origin/main), from inside a git checkout."),
    };
    let since = base.label;
    let changed: Vec<(String, Vec<u32>)> = changed
        .into_iter()
        .filter(|(file, _)| operators::kind_of(file) && !file.ends_with(".d.ts"))
        .filter(|(file, _)| src.as_deref().is_none_or(|dir| dir.is_empty() || file.starts_with(&format!("{dir}/"))))
        .collect();
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
    let mut points: Vec<(Mutant, String)> = Vec::new();
    for (file, lines, text) in &kept {
        for point in operators::scan(file, text, flags.get("operators") == Some("wide")) {
            if lines.contains(&point.line) {
                points.push((point, text.clone()));
            }
        }
    }
    points.sort_by(|a, b| a.0.file.cmp(&b.0.file).then(a.0.line.cmp(&b.0.line)).then(a.0.start.cmp(&b.0.start)));
    let skipped_note = if test_lines_skipped > 0 {
        format!(" ({} skipped)", count(test_lines_skipped, "test line"))
    } else {
        String::new()
    };
    let scope = format!("{} in {} since {since}{skipped_note}", count(line_count, "changed line"), count(kept.len(), "file"));
    if points.is_empty() {
        let summary = if line_count == 0 && test_lines_skipped > 0 {
            format!("No changed source line since {since}: the {} changed are test code. Nothing was mutated.", count(test_lines_skipped, "line"))
        } else if line_count == 0 {
            format!("No changed source line since {since}. Nothing was mutated.")
        } else {
            format!("{scope}, and none has a condition, comparison, or boolean to mutate.")
        };
        let next = if line_count == 0 && test_lines_skipped > 0 {
            "Nothing to score. Test code is skipped by default; pass includeTests (--include-tests) to mutate it."
        } else {
            "Nothing to score. Edit code, or pass --since with an older ref."
        };
        return Envelope::new(true, COMMAND, summary, next)
            .with_extra("changedLines", Value::Int(line_count as i64))
            .with_extra("testLinesSkipped", Value::Int(test_lines_skipped as i64))
            .with_extra("mutantCount", Value::Int(0));
    }
    let not_run = points.len().saturating_sub(max);
    points.truncate(max);

    // A fresh directory each call: the tests may have changed since the last one, so an old
    // result or baseline must not be reused.
    let generate_out = out.join("generate");
    let run_out = out.join("run");
    ignore_own_output(&out);
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
        return envelope.with_extra("changedLines", Value::Int(line_count as i64)).with_extra("testLinesSkipped", Value::Int(test_lines_skipped as i64));
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
    let unbuildable = envelope_count(&envelope, "unviable");
    if unbuildable > 0 {
        next.push_str(&format!(
            " {} did not build: not survivors and not scored; they are listed under unbuildable.",
            count(unbuildable as usize, "mutant")
        ));
    }
    if not_run > 0 {
        next.push_str(&format!(" {not_run} more mutants on these lines were not run; pass maxMutants to score them."));
    }
    envelope.next = next;
    envelope.next_call = None;
    let unbuildable_list = outcomes(&run_out, &points, "unviable");
    envelope
        .with_extra("changedLines", Value::Int(line_count as i64))
        .with_extra("testLinesSkipped", Value::Int(test_lines_skipped as i64))
        .with_extra("mutantCount", Value::Int(points.len() as i64))
        .with_extra("notRun", Value::Int(not_run as i64))
        .with_extra("survivors", Value::Array(survivors.into_iter().take(10).collect()))
        .with_extra("unbuildable", Value::Array(unbuildable_list.into_iter().take(10).collect()))
        .with_extra("results", Value::String(run_out.join("results").display().to_string()))
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

    /// Specula M5 (editor: 5,979 lines): `since main` on a branch while main had moved on scored
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

    /// Specula M5 (renderfollow: all 6 survivors in test assertions): changed lines in test code
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

    /// Specula M5 (renderfollow: 5 of 15 mutants did not build): a mutant the compiler rejects is
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

    /// Specula M5: `.probatio/` was written inside the package (a workspace member) and committed
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
}
