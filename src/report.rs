//! The one JSON line a Python reporter prints. The envelope does not carry it.

use crate::decide::{Baseline, Verdict};
use crate::json::{self, Value};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub name: String,
    pub file: String,
    pub static_load: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub tests: i64,
    pub fail: i64,
    pub failures: Vec<Failure>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Judgement {
    pub verdict: Verdict,
    pub killed_by: Vec<String>,
}

/// The last stdout line that is a reporter object.
pub fn parse(stdout: &str) -> Option<Report> {
    stdout.lines().rev().find_map(parse_line)
}

fn parse_line(line: &str) -> Option<Report> {
    let value = json::parse(line.trim()).ok()?;
    let tests = value.get("tests").and_then(Value::as_int)?;
    let failed = match value.get("failed") {
        Some(Value::Array(items)) => items,
        _ => return None,
    };
    value.get("names")?;
    let mut failures = Vec::new();
    for item in failed {
        let Value::Object(_) = item else { continue };
        let name = item.get("name").and_then(Value::as_str).unwrap_or("").to_string();
        let file = item.get("file").and_then(Value::as_str).unwrap_or("").to_string();
        let static_load = item.get("staticLoad").and_then(Value::as_bool).unwrap_or(false);
        failures.push(Failure { name, file, static_load });
    }
    let fail = value.get("fail").and_then(Value::as_int).unwrap_or(failures.len() as i64);
    Some(Report { tests, fail, failures })
}

pub fn judge_baseline(report: &Report) -> Baseline {
    if report.tests > 0 && !failing(report) {
        Baseline::Green
    } else if report.tests > 0 {
        Baseline::Red
    } else {
        Baseline::NoReport
    }
}

/// Timeout is applied by the caller before this. `tests == 0` is an error.
pub fn judge_mutant(report: &Report) -> Judgement {
    if report.tests <= 0 {
        return Judgement { verdict: Verdict::Error, killed_by: Vec::new() };
    }
    if !failing(report) {
        return Judgement { verdict: Verdict::Survived, killed_by: Vec::new() };
    }
    let killed_by = killed_names(report);
    if !killed_by.is_empty() {
        return Judgement { verdict: Verdict::Killed, killed_by };
    }
    if !report.failures.is_empty() && report.failures.iter().all(|item| item.static_load) {
        return Judgement { verdict: Verdict::Unviable, killed_by: Vec::new() };
    }
    Judgement { verdict: Verdict::Error, killed_by: Vec::new() }
}

fn failing(report: &Report) -> bool {
    report.fail > 0 || !report.failures.is_empty()
}

fn killed_names(report: &Report) -> Vec<String> {
    let mut names = Vec::new();
    for failure in &report.failures {
        if failure.static_load || failure.name.is_empty() {
            continue;
        }
        let file = failure.file.replace('\\', "/");
        let shown = if file.is_empty() || failure.name.contains("::") || failure.name.contains(&file) {
            failure.name.clone()
        } else {
            format!("{file}::{}", failure.name)
        };
        if !names.iter().any(|have| have == &shown) {
            names.push(shown);
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(failed: &str, tests: i64, fail: i64) -> String {
        format!(
            r#"{{"tests":{tests},"pass":{},"fail":{fail},"failed":{failed},"names":["tests/test_a.py::test_one"]}}"#,
            tests - fail
        )
    }

    #[test]
    fn an_assertion_and_a_value_error_are_kills() {
        let assertion = line(
            r#"[{"name":"tests/test_a.py::test_one","file":"tests/test_a.py","line":4}]"#,
            1,
            1,
        );
        let judged = judge_mutant(&parse(&assertion).unwrap());
        assert_eq!(judged.verdict, Verdict::Killed);
        assert_eq!(judged.killed_by, vec!["tests/test_a.py::test_one".to_string()]);

        let value = line(
            r#"[{"name":"tests/test_a.py::test_one","file":"tests/test_a.py","line":4,"staticLoad":false}]"#,
            1,
            1,
        );
        let judged = judge_mutant(&parse(&value).unwrap());
        assert_eq!(judged.verdict, Verdict::Killed);
        assert_eq!(judged.killed_by, vec!["tests/test_a.py::test_one".to_string()]);
    }

    #[test]
    fn a_load_failure_is_unviable_and_an_empty_run_is_an_error() {
        let syntax = line(
            r#"[{"name":"pytest","file":"src/a.py","line":1,"staticLoad":true}]"#,
            1,
            1,
        );
        assert_eq!(judge_mutant(&parse(&syntax).unwrap()).verdict, Verdict::Unviable);
        let import = line(
            r#"[{"name":"tests/test_a.py::test_one","file":"src/a.py","line":1,"staticLoad":true}]"#,
            1,
            1,
        );
        assert_eq!(judge_mutant(&parse(&import).unwrap()).verdict, Verdict::Unviable);
        let empty = line("[]", 0, 0);
        let judged = judge_mutant(&parse(&empty).unwrap());
        assert_eq!(judged.verdict, Verdict::Error);
        assert!(judged.killed_by.is_empty());
    }

    #[test]
    fn a_green_report_survives_and_the_json_line_wins_over_chatter() {
        let green = line("[]", 2, 0);
        let report = parse(&green).unwrap();
        assert_eq!(judge_mutant(&report).verdict, Verdict::Survived);
        assert_eq!(judge_baseline(&report), Baseline::Green);
        let mixed = format!(".F\n{green}");
        assert_eq!(judge_mutant(&parse(&mixed).unwrap()).verdict, Verdict::Survived);
        let red = parse(&line(
            r#"[{"name":"tests/test_a.py::test_one","staticLoad":false}]"#,
            1,
            1,
        ))
        .unwrap();
        assert_eq!(judge_baseline(&red), Baseline::Red);
        assert_eq!(judge_baseline(&parse(&line("[]", 0, 0)).unwrap()), Baseline::NoReport);
    }
}
