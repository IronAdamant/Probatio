//! Launch the shipped binary. The parser is the library the binary uses.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

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

#[test]
fn mcp_lists_tools_and_a_serial_run_starts() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_probatio"))
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn mcp");
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    let init = mcp_call(
        &mut stdin,
        &mut stdout,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#,
    );
    assert_eq!(init.get("result").and_then(|value| value.get("serverInfo")).and_then(|value| value.get("name")).and_then(json::Value::as_str), Some("probatio"));
    writeln!(stdin, r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#).unwrap();

    let pong = mcp_call(&mut stdin, &mut stdout, r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#);
    assert!(pong.get("result").is_some(), "{pong:?}");

    let listed = mcp_call(&mut stdin, &mut stdout, r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#);
    let names = listed.get("result").and_then(|value| value.get("tools")).and_then(|value| match value {
        json::Value::Array(items) => Some(items.iter().filter_map(|item| item.get("name").and_then(json::Value::as_str)).collect::<Vec<_>>()),
        _ => None,
    }).unwrap();
    assert_eq!(names, ["schema", "mutate_generate", "mutate_run", "mutate_tally"]);

    let missing = mcp_call(&mut stdin, &mut stdout, r#"{"jsonrpc":"2.0","id":4,"method":"nope"}"#);
    assert_eq!(missing.get("error").and_then(|value| value.get("code")).and_then(json::Value::as_int), Some(-32601));

    let root = std::env::temp_dir().join(format!("probatio-mcp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    let mut source = String::from("class A {\n");
    for index in 0..31 {
        source.push_str(&format!("  boolean x{index} = a && b;\n"));
    }
    source.push_str("}\n");
    std::fs::write(root.join("src/A.java"), source).unwrap();
    let patches = root.join("patches");
    let out = root.join("out");
    let generate = tool_envelope(
        &mut stdin,
        &mut stdout,
        5,
        "mutate_generate",
        &format!(r#"{{"package":"{}","out":"{}"}}"#, root.display(), patches.display()),
    );
    assert_eq!(generate.get("ok").and_then(json::Value::as_bool), Some(true), "{generate:?}");

    let stopped = tool_envelope(
        &mut stdin,
        &mut stdout,
        6,
        "mutate_run",
        &format!(
            r#"{{"package":"{}","patches":"{}","out":"{}","suiteCommand":"echo ok"}}"#,
            root.display(),
            patches.display(),
            out.display()
        ),
    );
    assert_eq!(stopped.get("ok").and_then(json::Value::as_bool), Some(false), "{stopped:?}");
    assert!(stopped.get("summary").and_then(json::Value::as_str).unwrap_or("").contains("No line map"), "{stopped:?}");

    let started = tool_envelope(
        &mut stdin,
        &mut stdout,
        7,
        "mutate_run",
        &format!(
            r#"{{"package":"{}","patches":"{}","out":"{}","suiteCommand":"echo ok","serial":true}}"#,
            root.display(),
            patches.display(),
            out.display()
        ),
    );
    assert_eq!(started.get("ok").and_then(json::Value::as_bool), Some(true), "{started:?}");
    assert!(std::fs::read_to_string(out.join("lines.json")).unwrap().contains("lines"));
    assert_eq!(std::fs::read_dir(out.join("results")).unwrap().count(), 31);

    drop(stdin);
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&root);
}

fn mcp_call(stdin: &mut impl Write, stdout: &mut impl BufRead, line: &str) -> json::Value {
    writeln!(stdin, "{line}").unwrap();
    stdin.flush().unwrap();
    let mut response = String::new();
    stdout.read_line(&mut response).unwrap();
    json::parse(response.trim()).unwrap_or_else(|error| panic!("{error}\n{response}"))
}

fn tool_envelope(stdin: &mut impl Write, stdout: &mut impl BufRead, id: i64, name: &str, arguments: &str) -> json::Value {
    let response = mcp_call(
        stdin,
        stdout,
        &format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"{name}","arguments":{arguments}}}}}"#),
    );
    let text = response
        .get("result")
        .and_then(|value| value.get("content"))
        .and_then(|value| match value {
            json::Value::Array(items) => items.first(),
            _ => None,
        })
        .and_then(|item| item.get("text"))
        .and_then(json::Value::as_str)
        .unwrap_or_else(|| panic!("no tool text in {response:?}"));
    json::parse(text).unwrap_or_else(|error| panic!("{error}\n{text}"))
}
