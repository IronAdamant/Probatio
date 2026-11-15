//! `mutate patch`: score one change you give as a unified diff, the way a mutant is scored.
//! The usual use is "undo this fix: the named test must fail". The diff is applied in a mutant
//! tree, never in your tree; the suite (or the named tests) runs once without it, where it must
//! pass, and once with it. A test that fails with the diff applied noticed it (killed); a suite
//! that still passes did not (survived).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::args::Flags;
use crate::decide::Verdict;
use crate::envelope::Envelope;
use crate::json::Value;

const COMMAND: &str = "mutate.patch";
const DEFAULT_TIMEOUT_MS: u64 = 120_000;

pub fn patch(flags: &Flags) -> Envelope {
    if flags.bool("low-priority", false) {
        crate::process::lower_priority();
    }
    let Some(package) = flags.get("package").map(PathBuf::from) else {
        return Envelope::new(false, COMMAND, "missing --package", "Pass --package, the project whose tests should notice the diff.");
    };
    if !package.is_dir() {
        return Envelope::new(false, COMMAND, format!("--package {} is not a directory", package.display()), "Pass the project directory.");
    }
    let Some(given) = flags.get("patch").filter(|text| !text.trim().is_empty()) else {
        return Envelope::new(false, COMMAND, "missing --patch", "Pass --patch: a unified diff (the text, as --patch=<text>), or the path of a file that holds one.");
    };
    let timeout_ms = match flags.parse_int("timeout-ms") {
        Ok(value) => value.unwrap_or(DEFAULT_TIMEOUT_MS).max(1),
        Err(message) => return Envelope::new(false, COMMAND, message, "Pass --timeout-ms as a whole number of milliseconds."),
    };
    let template = match crate::runner::suite_command(flags, &package) {
        Ok(command) => command,
        Err(message) => return Envelope::new(false, COMMAND, message, "Pass --suite-command for a layout this binary does not discover."),
    };
    let names: Vec<String> = flags.texts("tests").into_iter().map(|name| name.trim().to_string()).filter(|name| !name.is_empty()).collect();
    let suite = match suite_with_names(&template, &names) {
        Ok(suite) => suite,
        Err((summary, next)) => return Envelope::new(false, COMMAND, summary, next).with_extra("suiteCommand", Value::String(template)),
    };
    let (patch_file, written) = match patch_path(&package, given) {
        Ok(found) => found,
        Err(message) => return Envelope::new(false, COMMAND, message, "Pass --patch as a unified diff (git diff output) or the path of a file that holds one."),
    };
    let call = crate::runner::PatchCall {
        package: &package,
        suite: suite.clone(),
        timeout: Duration::from_millis(timeout_ms),
        patch: &patch_file,
        reverse: flags.bool("reverse", false),
        confirm: flags.bool("confirm", true),
    };
    let scored = crate::runner::score_patch(&call);
    if written {
        let _ = fs::remove_file(&patch_file);
    }
    let scored = match scored {
        Ok(scored) => scored,
        Err(message) => {
            return Envelope::new(false, COMMAND, message, "Your tree was not changed. Fix the cause, then make the same call.").with_extra("suiteCommand", Value::String(suite));
        }
    };
    let tests = if names.is_empty() { "the suite".to_string() } else { format!("the named tests ({})", names.join(", ")) };
    let (ok, outcome, summary, next) = match scored.verdict {
        Verdict::Killed => (
            true,
            "killed",
            if scored.killed_by.is_empty() {
                format!("Killed: {tests} passed without the diff and failed with it.")
            } else {
                format!("Killed: {} failed with the diff applied and passed without it.", scored.killed_by.join(", "))
            },
            "A test holds the behaviour the diff changes. The kill was rerun twice unless confirm was false.".to_string(),
        ),
        Verdict::Survived => (
            true,
            "survived",
            format!("Survived: {tests} passed with the diff applied. No test noticed it."),
            "If the diff undoes a fix, no test holds that fix: add a test that fails with this diff applied and passes without it, then make the same call.".to_string(),
        ),
        Verdict::Flaky => (
            true,
            "flaky",
            format!("Flaky: {tests} failed once with the diff applied and passed on a rerun."),
            "The failing test does not fail every time. Make the same call again, or name a test that fails every time.".to_string(),
        ),
        Verdict::Unviable => (
            false,
            "unviable",
            "The tree did not build with the diff applied. Not a score.".to_string(),
            format!("The compiler named a file the diff changes. Fix the diff so it builds. Last output: {}", scored.output),
        ),
        Verdict::Timeout => (
            false,
            "timeout",
            format!("The suite did not finish within {timeout_ms} ms with the diff applied. Not a score."),
            "A diff that makes a test hang is often a kill in practice; raise timeoutMs to tell.".to_string(),
        ),
        _ => (
            false,
            "error",
            "The suite failed with the diff applied in a way that is not a test failure. Not a score.".to_string(),
            format!("Run the suite command with the diff applied to see why. Last output: {}", scored.output),
        ),
    };
    Envelope::new(ok, COMMAND, summary, next)
        .with_extra("outcome", Value::String(outcome.into()))
        .with_extra("killedBy", Value::Array(scored.killed_by.into_iter().map(Value::String).collect()))
        .with_extra("files", Value::Array(scored.files.into_iter().map(Value::String).collect()))
        .with_extra("durationMs", Value::Int(scored.duration_ms as i64))
        .with_extra("suiteCommand", Value::String(suite))
        .with_extra("treePath", Value::String(scored.tree.path.display().to_string()))
        .with_extra("treePathLength", Value::Int(scored.tree.length as i64))
        .with_extra("longestTreePathLength", Value::Int(scored.tree.longest as i64))
}

/// The suite command with the test names in it: at `{tests}` or `{tests,}`, after `--` for
/// `cargo test`, or at the end for pytest. No names: the command as written.
fn suite_with_names(template: &str, names: &[String]) -> Result<String, (String, String)> {
    let render = |command: &str| {
        crate::decide::render_suite(command, names)
            .map(|(rendered, _)| rendered)
            .map_err(|name| (format!("The test name {name} cannot be placed in the suite command."), "A name may hold ASCII letters, digits, and _ - . / : only.".to_string()))
    };
    if template.contains("{tests}") || template.contains("{tests,}") {
        if names.is_empty() {
            return Err(("The suite command has a {tests} token and no tests were named.".into(), "Pass tests (the names to run), or a suite command without the token.".into()));
        }
        return render(template);
    }
    if names.is_empty() {
        return Ok(template.to_string());
    }
    let words: Vec<&str> = template.split_whitespace().collect();
    let program = words.first().map(|word| word.rsplit(['/', '\\']).next().unwrap_or(word)).unwrap_or("");
    if matches!(program, "cargo" | "cargo.exe") && words.get(1) == Some(&"test") {
        let joiner = if words.contains(&"--") { " " } else { " -- " };
        return render(&format!("{template}{joiner}{{tests}}"));
    }
    let pytest = matches!(program, "pytest" | "pytest.exe") || (program.starts_with("python") && words.windows(2).any(|pair| pair == ["-m", "pytest"]));
    if pytest {
        return render(&format!("{template} {{tests}}"));
    }
    Err((
        format!("The suite command {template} has no {{tests}} token, so the named tests have nowhere to go."),
        "Put {tests} (space-separated) or {tests,} (comma-separated) in suiteCommand where the names go, or name no tests to run the whole suite.".into(),
    ))
}

/// The diff's file: the path given, or the text written to `<probatio dir>/patch/`. The bool
/// says the file was written here and is removed after the call.
fn patch_path(package: &Path, given: &str) -> Result<(PathBuf, bool), String> {
    let looks_like_diff = given.contains('\n') && (given.contains("\n@@ ") || given.starts_with("@@ ")) || given.starts_with("diff --git") || given.starts_with("--- ");
    if !looks_like_diff {
        let path = PathBuf::from(given);
        if !path.is_file() {
            return Err(format!("--patch {given} is neither a unified diff nor a file"));
        }
        let path = path.canonicalize().unwrap_or(path);
        let text = fs::read_to_string(&path).map_err(|error| format!("{} could not be read: {error}", path.display()))?;
        if !text.contains("@@") {
            return Err(format!("{} holds no unified diff hunk (@@)", path.display()));
        }
        return Ok((plain(path), false));
    }
    let dir = crate::diffscope::probatio_dir(package).join("patch");
    crate::diffscope::ignore_own_output(&dir);
    fs::create_dir_all(&dir).map_err(|error| format!("{} could not be created: {error}", dir.display()))?;
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|time| time.as_nanos()).unwrap_or(0);
    let path = dir.join(format!("{}-{nanos:x}.diff", std::process::id()));
    let mut text = given.to_string();
    if !text.ends_with('\n') {
        text.push('\n');
    }
    fs::write(&path, text).map_err(|error| format!("{} could not be written: {error}", path.display()))?;
    Ok((path, true))
}

/// Windows: git does not read a `\\?\` path.
fn plain(path: PathBuf) -> PathBuf {
    let text = path.display().to_string();
    match text.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json;

    #[test]
    fn named_tests_go_where_the_runner_takes_them() {
        let names = vec!["a::b".to_string(), "c".to_string()];
        assert_eq!(suite_with_names("cargo test -p x", &names).unwrap(), "cargo test -p x -- 'a::b' 'c'");
        assert_eq!(suite_with_names("cargo test -- --test-threads 1", &names).unwrap(), "cargo test -- --test-threads 1 'a::b' 'c'");
        assert_eq!(suite_with_names("python3 -m pytest -q", &names).unwrap(), "python3 -m pytest -q 'a::b' 'c'");
        assert_eq!(suite_with_names("mvn -B test -Dtest={tests,}", &names).unwrap(), "mvn -B test -Dtest=a::b,c");
        assert_eq!(suite_with_names("go test ./...", &[]).unwrap(), "go test ./...");
        let (summary, next) = suite_with_names("go test ./...", &names).unwrap_err();
        assert!(summary.contains("no {tests} token") && next.contains("{tests}"), "{summary} {next}");
        assert!(suite_with_names("cargo test {tests}", &[]).is_err());
        assert!(suite_with_names("cargo test", &["bad name".to_string()]).is_err());
    }

    fn git_ok(dir: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
        assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    const FIXED: &str = "class A {\n  boolean ok(int n) { return n > 0; }\n  // note\n}\n";

    /// A checkout whose fix is the last commit, and a suite that fails when the fix is undone.
    fn fixed_repo(name: &str) -> (crate::testdir::TempDir, String) {
        let dir = crate::testdir::TempDir::new(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("member/src")).unwrap();
        git_ok(&dir, &["init", "-q"]);
        git_ok(&dir, &["config", "user.email", "a@b.c"]);
        git_ok(&dir, &["config", "user.name", "a"]);
        git_ok(&dir, &["config", "core.autocrlf", "false"]);
        fs::write(dir.join("member/src/A.java"), FIXED.replace("n > 0", "n >= 0")).unwrap();
        git_ok(&dir, &["add", "."]);
        git_ok(&dir, &["commit", "-q", "-m", "bug"]);
        fs::write(dir.join("member/src/A.java"), FIXED).unwrap();
        git_ok(&dir, &["commit", "-q", "-am", "fix"]);
        let fix = git_ok(&dir, &["diff", "HEAD~1", "HEAD"]);
        (dir, fix)
    }

    fn call(package: &Path, patch: &str, extra: &[&str]) -> Envelope {
        // `name` stands for the test: it fails when the fix is gone, and records that it ran.
        let suite = "echo ran {tests} >> ../ran.txt; grep -q 'n > 0' src/A.java && echo ok || { echo 'test name FAILED'; exit 1; }";
        let mut argv: Vec<String> = vec!["--package".into(), package.display().to_string(), format!("--patch={patch}"), "--suite-command".into(), suite.into(), "--tests".into(), "name".into()];
        argv.extend(extra.iter().map(|item| item.to_string()));
        super::patch(&Flags::parse(&argv))
    }

    /// Specula M5: more than twenty "undo this fix, the named test must fail" checks were made by
    /// hand. `mutate patch` takes the diff, applies it in a mutant tree, runs the named tests
    /// without it and with it, and leaves your tree as it was.
    #[test]
    fn a_patch_that_undoes_a_fix_is_killed_and_your_tree_is_untouched() {
        let (dir, fix) = fixed_repo("patch-kill");
        let member = dir.join("member");
        let killed = call(&member, &fix, &["--reverse", "--no-confirm"]);
        assert!(killed.ok, "{} {}", killed.summary, killed.next);
        let value = killed.to_value("");
        assert_eq!(value.get("outcome").and_then(Value::as_str), Some("killed"), "{}", killed.summary);
        assert_eq!(json::stringify_compact(value.get("files").unwrap()), r#"["member/src/A.java"]"#);
        assert!(value.get("suiteCommand").and_then(Value::as_str).unwrap().contains("echo ran 'name'"));
        assert_eq!(fs::read_to_string(member.join("src/A.java")).unwrap(), FIXED, "your tree was edited");
        assert_eq!(git_ok(&dir, &["status", "--porcelain", "--untracked-files=no"]), "");
        // The same undo, written by hand from inside the package (`git diff --relative` paths).
        let undo = "--- a/src/A.java\n+++ b/src/A.java\n@@ -2 +2 @@\n-  boolean ok(int n) { return n > 0; }\n+  boolean ok(int n) { return n >= 0; }\n";
        let relative = call(&member, undo, &["--no-confirm"]);
        assert_eq!(relative.to_value("").get("outcome").and_then(Value::as_str), Some("killed"), "{} {}", relative.summary, relative.next);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_patch_no_test_notices_survives_and_one_that_does_not_apply_is_refused() {
        let (dir, _) = fixed_repo("patch-survive");
        let member = dir.join("member");
        let comment = "--- a/member/src/A.java\n+++ b/member/src/A.java\n@@ -3 +3 @@\n-  // note\n+  // changed note\n";
        let survived = call(&member, comment, &[]);
        assert!(survived.ok, "{} {}", survived.summary, survived.next);
        assert_eq!(survived.to_value("").get("outcome").and_then(Value::as_str), Some("survived"));
        assert!(survived.next.contains("add a test that fails with this diff applied"), "{}", survived.next);
        let stale = "--- a/member/src/A.java\n+++ b/member/src/A.java\n@@ -3 +3 @@\n-  // not there\n+  // x\n";
        let refused = call(&member, stale, &[]);
        assert!(!refused.ok);
        assert!(refused.summary.starts_with("The diff does not apply to your tree"), "{}", refused.summary);
        let missing = super::patch(&Flags::parse(&["--package".to_string(), member.display().to_string()]));
        assert!(!missing.ok && missing.summary == "missing --patch", "{}", missing.summary);
        assert_eq!(fs::read_to_string(member.join("src/A.java")).unwrap(), FIXED);
        let _ = fs::remove_dir_all(&dir);
    }
}
