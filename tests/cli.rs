//! Launch the shipped binary. The parser is the library the binary uses.

use std::process::Command;

use probatio::json;

fn launch(args: &[&str]) -> (i32, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_probatio")).args(args).output().expect("spawn probatio");
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    let code = output.status.code().unwrap_or(1);
    (code, stdout)
}

fn one_envelope(stdout: &str) -> json::Value {
    let value = json::parse(stdout).unwrap_or_else(|error| panic!("{error}\n{stdout}"));
    let (version, _ok, summary, next, next_call) = json::envelope_fields(&value).expect("five fields");
    assert_eq!(version, 2);
    assert!(!summary.is_empty());
    assert!(!next.is_empty());
    match &next_call {
        json::Value::Null => {}
        other => assert!(other.get("argv").is_some(), "{other:?}"),
    }
    assert!(!stdout.to_ascii_lowercase().contains("unknown command"));
    value
}

#[test]
fn schema_prints_one_envelope_and_repeats_it() {
    let (code, first) = launch(&["schema"]);
    let (_again, second) = launch(&["schema"]);
    assert_eq!(code, 0);
    assert_eq!(first, second);
    let value = one_envelope(&first);
    assert_eq!(value.get("ok").and_then(json::Value::as_bool), Some(true));
    assert!(value.get("command").is_some());
}

#[test]
fn every_command_prints_an_envelope() {
    for invocation in probatio::commands::INVOCATIONS {
        let (_code, stdout) = launch(invocation);
        one_envelope(&stdout);
    }
}
