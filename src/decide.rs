//! Outcome rules. A kill is an assertion failure. It is not a count of everything that went wrong.

pub const LARGE_BATCH: usize = 30;
pub const DEFAULT_MAX_SELECTED: usize = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Killed,
    Survived,
    NoCoverage,
    Unviable,
    Timeout,
    Flaky,
    Error,
}

impl Verdict {
    pub fn is_kill(self) -> bool {
        self == Verdict::Killed
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Killed => "killed",
            Verdict::Survived => "survived",
            Verdict::NoCoverage => "no coverage",
            Verdict::Unviable => "unviable",
            Verdict::Timeout => "timeout",
            Verdict::Flaky => "flaky",
            Verdict::Error => "error",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Baseline {
    Green,
    Red,
    NoReport,
    TimedOut,
    MissingTool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Counts {
    pub no_coverage: usize,
    pub survived: usize,
    pub killed: usize,
    pub unviable: usize,
    pub flaky: usize,
    pub timeouts: usize,
    pub errors: usize,
    pub finished: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    pub tests: Vec<String>,
    pub selected_count: usize,
    pub capped: bool,
}

pub fn judge_baseline(spawned: bool, timed_out: bool, code: Option<i32>, stdout: &str, stderr: &str) -> Baseline {
    if !spawned {
        return Baseline::MissingTool;
    }
    if timed_out {
        return Baseline::TimedOut;
    }
    if stdout.trim().is_empty() && stderr.trim().is_empty() {
        return Baseline::NoReport;
    }
    if code == Some(0) {
        Baseline::Green
    } else {
        Baseline::Red
    }
}

pub fn baseline_refusal(kind: Baseline) -> Option<(&'static str, &'static str)> {
    let (summary, next) = match kind {
        Baseline::Green => return None,
        Baseline::Red => (
            "The baseline suite is already failing. Nothing was mutated.",
            "A red baseline is not a kill count. Fix the suite, then run again.",
        ),
        Baseline::NoReport => (
            "The baseline did not return a test report. Nothing was mutated.",
            "A run with no test report is not a kill count.",
        ),
        Baseline::TimedOut => (
            "The baseline timed out. Nothing was mutated.",
            "A timeout is the baseline clock. It is not a kill and not a gap.",
        ),
        Baseline::MissingTool => (
            "The suite tool is missing. Nothing was mutated.",
            "Install the tool the suite already uses, then run again. Nothing was fetched.",
        ),
    };
    Some((summary, next))
}

/// A compile or link failure is unviable. An assertion failure is a kill. A timeout is neither.
pub fn classify(timed_out: bool, code: Option<i32>, stderr: &str) -> Verdict {
    if timed_out {
        return Verdict::Timeout;
    }
    if code == Some(0) {
        return Verdict::Survived;
    }
    if looks_unviable(stderr) {
        return Verdict::Unviable;
    }
    if code.is_none() {
        return Verdict::Error;
    }
    Verdict::Killed
}

fn looks_unviable(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    if lower.contains("assertion") || lower.contains("panic") {
        return false;
    }
    const MARKERS: &[&str] = &[
        "error:",
        "compilation failed",
        "could not compile",
        "cannot find symbol",
        "does not compile",
        "syntax error",
        "undefined reference",
        "undefined:",
        "[build failed]",
        "build failed",
        "linker",
        "cannot find module",
    ];
    MARKERS.iter().any(|marker| lower.contains(marker))
}

pub fn counts_of(verdicts: &[Verdict]) -> Counts {
    let mut counts = Counts {
        no_coverage: 0,
        survived: 0,
        killed: 0,
        unviable: 0,
        flaky: 0,
        timeouts: 0,
        errors: 0,
        finished: verdicts.len(),
    };
    for verdict in verdicts {
        match verdict {
            Verdict::Killed => counts.killed += 1,
            Verdict::Survived => counts.survived += 1,
            Verdict::NoCoverage => counts.no_coverage += 1,
            Verdict::Unviable => counts.unviable += 1,
            Verdict::Timeout => counts.timeouts += 1,
            Verdict::Flaky => counts.flaky += 1,
            Verdict::Error => counts.errors += 1,
        }
    }
    counts
}

pub fn campaign_summary(counts: &Counts) -> String {
    let unviable = if counts.unviable > 0 {
        format!("{} did not build, ", counts.unviable)
    } else {
        String::new()
    };
    format!(
        "{} no coverage, {} survived, {} killed, {unviable}{} flaky, {} timed out, {} errored, of {} finished.",
        counts.no_coverage, counts.survived, counts.killed, counts.flaky, counts.timeouts, counts.errors, counts.finished
    )
}

/// No usable line map and a large batch stop before the first mutant.
/// A baseline under five seconds does not waive the stop.
pub fn scale_stop(has_line_map: bool, baseline_ms: Option<u64>, pending: usize) -> Option<String> {
    if has_line_map || pending <= LARGE_BATCH {
        return None;
    }
    let seconds = match baseline_ms {
        Some(ms) => format!("{:.1}", ms as f64 / 1000.0),
        None => "unknown".into(),
    };
    Some(format!(
        "No line map for this suite. The baseline took {seconds}s and {pending} mutants are waiting. Narrow --src or pass a patch slice. Nothing was started."
    ))
}

pub fn cap_tests(tests: &[String], max: usize) -> Selection {
    if tests.len() <= max {
        Selection { selected_count: tests.len(), capped: false, tests: tests.to_vec() }
    } else {
        Selection {
            selected_count: max,
            capped: true,
            tests: tests.iter().take(max).cloned().collect(),
        }
    }
}

pub fn cap_note(max: usize) -> String {
    format!("Selection capped at {max} tests. The result names the cap.")
}

/// How many mutants with no result this call may score.
/// `Err` is the real pending count when `--confirm-count` does not match it.
pub fn chunk_budget(pending: usize, confirm: Option<usize>) -> Result<usize, usize> {
    match confirm {
        Some(count) if count == pending => Ok(pending),
        Some(_) => Err(pending),
        None => Ok(pending.min(LARGE_BATCH)),
    }
}

/// Fill `{tests}` and `{tests,}`. `Ok(true)` means the command had neither token.
pub fn render_suite(command: &str, tests: &[String]) -> Result<(String, bool), String> {
    let space = command.contains("{tests}");
    let comma = command.contains("{tests,}");
    if !space && !comma {
        return Ok((command.to_string(), true));
    }
    for name in tests {
        if unsafe_test_name(name) {
            return Err(name.clone());
        }
    }
    let mut rendered = command.to_string();
    if comma {
        rendered = rendered.replace("{tests,}", &tests.join(","));
    }
    if space {
        let quoted = tests.iter().map(|name| format!("'{name}'")).collect::<Vec<_>>().join(" ");
        rendered = rendered.replace("{tests}", &quoted);
    }
    Ok((rendered, false))
}

fn unsafe_test_name(name: &str) -> bool {
    name.is_empty()
        || name.chars().any(|ch| {
            !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-' && ch != '.' && ch != '/' && ch != ':'
        })
}

/// Tally never deletes a test. The zero is the product, not a default the caller may raise.
pub fn deleted_tests() -> u64 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn red_baseline_and_missing_report_have_no_kill_count() {
        for kind in [Baseline::Red, Baseline::NoReport, Baseline::TimedOut, Baseline::MissingTool] {
            let (summary, next) = baseline_refusal(kind).unwrap();
            assert!(!summary.contains("killed"));
            assert!(!next.contains("killed"));
        }
        assert!(baseline_refusal(Baseline::Green).is_none());
        assert_eq!(
            judge_baseline(true, false, Some(1), "fail", ""),
            Baseline::Red
        );
        assert_eq!(judge_baseline(true, false, Some(0), "", ""), Baseline::NoReport);
        assert_eq!(judge_baseline(true, true, None, "", "partial"), Baseline::TimedOut);
        assert_eq!(judge_baseline(false, false, None, "", ""), Baseline::MissingTool);
    }

    #[test]
    fn compile_failure_and_timeout_are_not_kills() {
        let unviable = classify(false, Some(1), "error: cannot find module foo");
        let timed = classify(true, None, "");
        let killed = classify(false, Some(1), "assertion failed: expected 1");
        let go_build = classify(false, Some(1), "./main.go:3:6: undefined: nope\nFAIL\tdemo [build failed]\n");
        assert_eq!(unviable, Verdict::Unviable);
        assert!(!unviable.is_kill());
        assert_eq!(go_build, Verdict::Unviable);
        assert!(!go_build.is_kill());
        assert_eq!(timed, Verdict::Timeout);
        assert!(!timed.is_kill());
        assert!(killed.is_kill());
        let counts = counts_of(&[unviable, timed, killed, Verdict::Survived]);
        assert_eq!(counts.killed, 1);
        assert_eq!(counts.unviable, 1);
        assert_eq!(counts.timeouts, 1);
        let summary = campaign_summary(&counts);
        assert!(summary.contains("1 killed"));
        assert!(summary.contains("1 did not build"));
        assert!(summary.contains("1 timed out"));
    }

    #[test]
    fn unmapped_batch_stops_under_five_seconds() {
        let stop = scale_stop(false, Some(1_000), 31).unwrap();
        assert!(stop.contains("No line map"));
        assert!(stop.contains("31"));
        assert!(scale_stop(false, Some(1_000), 30).is_none());
        assert!(scale_stop(true, Some(20_000), 100).is_none());
        assert!(scale_stop(false, None, 31).is_some());
    }

    #[test]
    fn confirm_count_must_match_and_a_plain_call_stops_at_thirty() {
        assert_eq!(chunk_budget(31, None).unwrap(), 30);
        assert_eq!(chunk_budget(10, None).unwrap(), 10);
        assert_eq!(chunk_budget(31, Some(31)).unwrap(), 31);
        assert_eq!(chunk_budget(31, Some(30)).unwrap_err(), 31);
    }

    #[test]
    fn suite_tokens_quote_or_join_and_a_bad_name_refuses() {
        let names = vec!["Foo".into(), "Bar".into()];
        let (spaced, whole) = render_suite("echo {tests}", &names).unwrap();
        assert!(!whole);
        assert_eq!(spaced, "echo 'Foo' 'Bar'");
        let (comma, _) = render_suite("mvn -B test -Dtest={tests,}", &names).unwrap();
        assert_eq!(comma, "mvn -B test -Dtest=Foo,Bar");
        let (_, whole) = render_suite("echo ok", &names).unwrap();
        assert!(whole);
        assert_eq!(render_suite("echo {tests}", &["a'b".into()]).unwrap_err(), "a'b");
        assert_eq!(render_suite("echo {tests,}", &["a,b".into()]).unwrap_err(), "a,b");
    }

    #[test]
    fn selection_over_thirty_is_capped_and_marked() {
        let tests: Vec<String> = (0..40).map(|index| format!("test{index}")).collect();
        let capped = cap_tests(&tests, DEFAULT_MAX_SELECTED);
        assert!(capped.capped);
        assert_eq!(capped.selected_count, 30);
        assert_eq!(capped.tests.len(), 30);
        let exact = cap_tests(&tests[..30], 30);
        assert!(!exact.capped);
        assert_eq!(deleted_tests(), 0);
    }
}
