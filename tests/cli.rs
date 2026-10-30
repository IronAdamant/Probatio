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
    assert_eq!(std::fs::read_dir(out.join("results")).unwrap().count(), 30);
    assert!(started.get("nextCall").and_then(|call| call.get("argv")).is_some(), "{started:?}");

    drop(stdin);
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&root);
}

fn assert_named_suite(value: &json::Value, command: &str) {
    assert_eq!(value.get("schemaVersion").and_then(json::Value::as_int), Some(2));
    assert_eq!(value.get("suiteCommand").and_then(json::Value::as_str), Some(command));
    let next = value.get("next").and_then(json::Value::as_str).unwrap_or("");
    assert!(next.contains(command), "{next}");
}

fn assert_refused_without_kills(value: &json::Value) {
    assert_eq!(value.get("ok").and_then(json::Value::as_bool), Some(false), "{value:?}");
    let summary = value.get("summary").and_then(json::Value::as_str).unwrap_or("");
    let next = value.get("next").and_then(json::Value::as_str).unwrap_or("");
    assert!(!summary.contains("killed"), "{summary}");
    assert!(!next.contains("killed"), "{next}");
    assert!(value.get("counts").is_none(), "{value:?}");
    assert!(value.get("killed").is_none());
    assert!(value.get("kills").is_none());
}

fn result_count(out: &std::path::Path) -> usize {
    std::fs::read_dir(out.join("results")).map(|entries| entries.count()).unwrap_or(0)
}

fn java_package(name: &str, ands: usize) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("probatio-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    let mut body = String::from("class A {\n  boolean f(boolean a, boolean b) {\n");
    for index in 0..ands {
        body.push_str(&format!("    boolean x{index} = a && b;\n"));
    }
    body.push_str("    return a;\n  }\n}\n");
    std::fs::write(root.join("src/A.java"), body).unwrap();
    let patches = root.join("patches").display().to_string();
    let package = root.display().to_string();
    let (_code, stdout) = launch(&["mutate", "generate", "--package", &package, "--out", &patches]);
    let generated = one_envelope(&stdout);
    assert_eq!(generated.get("ok").and_then(json::Value::as_bool), Some(true), "{stdout}");
    root
}

fn argv_strings(value: &json::Value) -> Vec<String> {
    match value.get("nextCall").and_then(|call| call.get("argv")) {
        Some(json::Value::Array(items)) => items.iter().map(|item| item.as_str().unwrap_or("").to_string()).collect(),
        other => panic!("missing nextCall argv: {other:?} in {value:?}"),
    }
}

#[test]
fn shipped_run_names_the_suite_on_refusals() {
    let root = java_package("refusals", 1);
    let package = root.display().to_string();
    let patches = root.join("patches").display().to_string();

    let red = one_envelope(
        &launch(&[
            "mutate",
            "run",
            "--package",
            &package,
            "--patches",
            &patches,
            "--out",
            &root.join("red").display().to_string(),
            "--suite-command",
            "echo fail; exit 1",
            "--timeout-ms",
            "5000",
        ])
        .1,
    );
    assert_refused_without_kills(&red);
    assert_named_suite(&red, "echo fail; exit 1");
    assert!(red.get("summary").and_then(json::Value::as_str).unwrap_or("").contains("already failing"));

    let missing_report = one_envelope(
        &launch(&[
            "mutate",
            "run",
            "--package",
            &package,
            "--patches",
            &patches,
            "--out",
            &root.join("noreport").display().to_string(),
            "--suite-command",
            "exit 0",
            "--timeout-ms",
            "5000",
        ])
        .1,
    );
    assert_refused_without_kills(&missing_report);
    assert_named_suite(&missing_report, "exit 0");

    let timed_out = one_envelope(
        &launch(&[
            "mutate",
            "run",
            "--package",
            &package,
            "--patches",
            &patches,
            "--out",
            &root.join("timeout").display().to_string(),
            "--suite-command",
            "sleep 30",
            "--timeout-ms",
            "400",
        ])
        .1,
    );
    assert_refused_without_kills(&timed_out);
    assert_named_suite(&timed_out, "sleep 30");
    assert!(timed_out.get("summary").and_then(json::Value::as_str).unwrap_or("").contains("timed out"));

    let missing_tool = one_envelope(
        &launch(&[
            "mutate",
            "run",
            "--package",
            &package,
            "--patches",
            &patches,
            "--out",
            &root.join("missing").display().to_string(),
            "--suite-command",
            "probatio-missing-tool-xyz",
            "--timeout-ms",
            "5000",
        ])
        .1,
    );
    assert_refused_without_kills(&missing_tool);
    assert_named_suite(&missing_tool, "probatio-missing-tool-xyz");

    let discovered = root.join("discovered");
    std::fs::create_dir_all(&discovered).unwrap();
    std::fs::write(discovered.join("package.json"), "{}\n").unwrap();
    std::fs::create_dir_all(discovered.join("patches")).unwrap();
    std::fs::write(discovered.join("patches/mutants.json"), "{\"mutants\":[]}\n").unwrap();
    let found = one_envelope(
        &launch(&[
            "mutate",
            "run",
            "--package",
            &discovered.display().to_string(),
            "--patches",
            &discovered.join("patches").display().to_string(),
            "--out",
            &discovered.join("out").display().to_string(),
        ])
        .1,
    );
    assert_refused_without_kills(&found);
    assert_named_suite(&found, "node --test");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn shipped_run_reads_probatio_json_and_the_flag_wins() {
    let root = java_package("projjson", 1);
    let package = root.display().to_string();
    let patches = root.join("patches").display().to_string();
    std::fs::write(
        root.join(".probatio.json"),
        "{\n  \"suiteCommand\": \"echo from-file\",\n  \"env\": {\"A\": \"1\"},\n  \"testLine\": \"ignored\",\n  \"passLine\": \"ignored\"\n}\n",
    )
    .unwrap();
    std::fs::write(root.join("package.json"), "{}\n").unwrap();

    let from_file = one_envelope(
        &launch(&[
            "mutate",
            "run",
            "--package",
            &package,
            "--patches",
            &patches,
            "--out",
            &root.join("file").display().to_string(),
            "--timeout-ms",
            "5000",
        ])
        .1,
    );
    assert_eq!(from_file.get("ok").and_then(json::Value::as_bool), Some(true), "{from_file:?}");
    assert_named_suite(&from_file, "echo from-file");
    assert_eq!(result_count(&root.join("file")), 1);

    let from_flag = one_envelope(
        &launch(&[
            "mutate",
            "run",
            "--package",
            &package,
            "--patches",
            &patches,
            "--out",
            &root.join("flag").display().to_string(),
            "--suite-command",
            "echo from-flag; exit 1",
            "--timeout-ms",
            "5000",
        ])
        .1,
    );
    assert_refused_without_kills(&from_flag);
    assert_named_suite(&from_flag, "echo from-flag; exit 1");
    assert!(from_flag.get("summary").and_then(json::Value::as_str).unwrap_or("").contains("already failing"));

    std::fs::write(root.join(".probatio.json"), "{\n").unwrap();
    let invalid = one_envelope(
        &launch(&[
            "mutate",
            "run",
            "--package",
            &package,
            "--patches",
            &patches,
            "--out",
            &root.join("badjson").display().to_string(),
            "--timeout-ms",
            "5000",
        ])
        .1,
    );
    assert_refused_without_kills(&invalid);
    assert!(invalid.get("suiteCommand").is_none(), "{invalid:?}");
    let summary = invalid.get("summary").and_then(json::Value::as_str).unwrap_or("");
    let next = invalid.get("next").and_then(json::Value::as_str).unwrap_or("");
    assert!(summary.contains("not valid JSON"), "{summary}");
    assert!(next.contains("Discovery was not used"), "{next}");
    assert!(!summary.contains("node --test"), "{summary}");
    assert!(!next.contains("node --test"), "{next}");
    assert_eq!(result_count(&root.join("badjson")), 0);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn shipped_run_chunks_a_mapped_batch_and_confirm_count() {
    let root = java_package("chunk", 31);
    let package = root.display().to_string();
    let patches = root.join("patches").display().to_string();
    let map = root.join("lines.json");
    std::fs::write(&map, "{\"lines\":[1]}\n").unwrap();
    let map_arg = map.display().to_string();

    let unmapped_out = root.join("unmap");
    let unmapped = one_envelope(
        &launch(&[
            "mutate",
            "run",
            "--package",
            &package,
            "--patches",
            &patches,
            "--out",
            &unmapped_out.display().to_string(),
            "--suite-command",
            "echo ok",
            "--timeout-ms",
            "5000",
        ])
        .1,
    );
    assert_refused_without_kills(&unmapped);
    assert_named_suite(&unmapped, "echo ok");
    let unmapped_summary = unmapped.get("summary").and_then(json::Value::as_str).unwrap_or("");
    assert!(unmapped_summary.contains("No line map"), "{unmapped_summary}");
    assert!(unmapped_summary.contains("31"), "{unmapped_summary}");
    assert_eq!(std::fs::read_to_string(unmapped_out.join("not-started.txt")).unwrap().lines().count(), 31);
    assert_eq!(result_count(&unmapped_out), 0);

    let out = root.join("out");
    let out_arg = out.display().to_string();
    let first = one_envelope(
        &launch(&[
            "mutate",
            "run",
            "--package",
            &package,
            "--patches",
            &patches,
            "--out",
            &out_arg,
            "--suite-command",
            "echo ok",
            "--line-map",
            &map_arg,
            "--timeout-ms",
            "5000",
        ])
        .1,
    );
    assert_eq!(first.get("ok").and_then(json::Value::as_bool), Some(true), "{first:?}");
    assert_named_suite(&first, "echo ok");
    assert_eq!(result_count(&out), 30);
    let first_next = first.get("next").and_then(json::Value::as_str).unwrap_or("");
    assert!(first_next.contains("30 started this call"), "{first_next}");
    assert!(first_next.contains("1 is still waiting"), "{first_next}");
    assert!(first_next.contains("did not choose tests"), "{first_next}");
    let argv = argv_strings(&first);
    assert!(!argv.iter().any(|arg| arg == "--confirm-count"));
    assert!(argv.windows(2).any(|pair| pair[0] == "--package" && pair[1] == package));
    assert!(argv.windows(2).any(|pair| pair[0] == "--patches" && pair[1] == patches));
    assert!(argv.windows(2).any(|pair| pair[0] == "--out" && pair[1] == out_arg));
    assert!(argv.windows(2).any(|pair| pair[0] == "--suite-command" && pair[1] == "echo ok"));
    assert!(argv.windows(2).any(|pair| pair[0] == "--line-map" && pair[1] == map_arg));
    assert!(argv.windows(2).any(|pair| pair[0] == "--timeout-ms" && pair[1] == "5000"));

    let owned: Vec<String> = argv;
    let args: Vec<&str> = owned.iter().map(String::as_str).collect();
    let second = one_envelope(&launch(&args).1);
    assert_eq!(second.get("ok").and_then(json::Value::as_bool), Some(true), "{second:?}");
    assert!(matches!(second.get("nextCall"), Some(json::Value::Null)), "{second:?}");
    assert_eq!(result_count(&out), 31);
    assert!(!out.join("not-started.txt").exists());

    let wrong_out = root.join("wrong");
    let wrong = one_envelope(
        &launch(&[
            "mutate",
            "run",
            "--package",
            &package,
            "--patches",
            &patches,
            "--out",
            &wrong_out.display().to_string(),
            "--suite-command",
            "echo ok",
            "--line-map",
            &map_arg,
            "--timeout-ms",
            "5000",
            "--confirm-count",
            "30",
        ])
        .1,
    );
    assert_refused_without_kills(&wrong);
    assert_named_suite(&wrong, "echo ok");
    assert!(wrong.get("next").and_then(json::Value::as_str).unwrap_or("").contains("31"), "{wrong:?}");
    assert_eq!(result_count(&wrong_out), 0);

    let all_out = root.join("all");
    let all = one_envelope(
        &launch(&[
            "mutate",
            "run",
            "--package",
            &package,
            "--patches",
            &patches,
            "--out",
            &all_out.display().to_string(),
            "--suite-command",
            "echo ok",
            "--line-map",
            &map_arg,
            "--timeout-ms",
            "5000",
            "--confirm-count",
            "31",
        ])
        .1,
    );
    assert_eq!(all.get("ok").and_then(json::Value::as_bool), Some(true), "{all:?}");
    assert!(matches!(all.get("nextCall"), Some(json::Value::Null)), "{all:?}");
    assert_eq!(result_count(&all_out), 31);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn shipped_run_substitutes_test_tokens() {
    let root = java_package("tokens", 1);
    let package = root.display().to_string();
    let patches = root.join("patches").display().to_string();
    let tests = root.join("tests.txt");
    std::fs::write(&tests, "Foo\nBar\n").unwrap();
    let tests_arg = tests.display().to_string();
    let log = root.join("ran.log");
    let command = format!("printf '%s\\n' {{tests}} >> '{}'; printf '%s\\n' {{tests}}", log.display());
    let out = root.join("out");
    std::fs::write(root.join("map-names.txt"), "OnlyFromMap\n").unwrap();
    let spaced = one_envelope(
        &launch(&[
            "mutate",
            "run",
            "--package",
            &package,
            "--patches",
            &patches,
            "--out",
            &out.display().to_string(),
            "--suite-command",
            &command,
            "--tests",
            &tests_arg,
            "--timeout-ms",
            "5000",
            "--line-map",
            &root.join("map-names.txt").display().to_string(),
        ])
        .1,
    );
    assert_eq!(spaced.get("ok").and_then(json::Value::as_bool), Some(true), "{spaced:?}");
    assert_named_suite(&spaced, &command);
    let ran = std::fs::read_to_string(&log).unwrap();
    assert!(ran.contains("Foo"), "{ran}");
    assert!(ran.contains("Bar"), "{ran}");
    assert!(!ran.contains("OnlyFromMap"), "{ran}");
    let next = spaced.get("next").and_then(json::Value::as_str).unwrap_or("");
    assert!(!next.contains("ran whole"), "{next}");

    let comma_log = root.join("comma.log");
    let comma = format!("printf '%s\\n' {{tests,}} >> '{}'; echo reported", comma_log.display());
    let comma_value = one_envelope(
        &launch(&[
            "mutate",
            "run",
            "--package",
            &package,
            "--patches",
            &patches,
            "--out",
            &root.join("comma").display().to_string(),
            "--suite-command",
            &comma,
            "--tests",
            &tests_arg,
            "--timeout-ms",
            "5000",
        ])
        .1,
    );
    assert_eq!(comma_value.get("ok").and_then(json::Value::as_bool), Some(true), "{comma_value:?}");
    assert!(std::fs::read_to_string(&comma_log).unwrap().contains("Foo,Bar"));

    let whole = one_envelope(
        &launch(&[
            "mutate",
            "run",
            "--package",
            &package,
            "--patches",
            &patches,
            "--out",
            &root.join("whole").display().to_string(),
            "--suite-command",
            "echo ok",
            "--tests",
            &tests_arg,
            "--workers",
            "4",
            "--timeout-ms",
            "5000",
        ])
        .1,
    );
    assert_eq!(whole.get("ok").and_then(json::Value::as_bool), Some(true), "{whole:?}");
    let whole_next = whole.get("next").and_then(json::Value::as_str).unwrap_or("");
    assert!(whole_next.contains("The suite ran whole."), "{whole_next}");
    assert!(whole_next.contains("Workers stay at 1."), "{whole_next}");

    std::fs::write(&tests, "a'b\n").unwrap();
    let before_bad = std::fs::read_to_string(&log).unwrap_or_default();
    let bad_out = root.join("bad");
    let bad = one_envelope(
        &launch(&[
            "mutate",
            "run",
            "--package",
            &package,
            "--patches",
            &patches,
            "--out",
            &bad_out.display().to_string(),
            "--suite-command",
            &command,
            "--tests",
            &tests_arg,
            "--timeout-ms",
            "5000",
        ])
        .1,
    );
    assert_refused_without_kills(&bad);
    assert_named_suite(&bad, &command);
    assert!(bad.get("summary").and_then(json::Value::as_str).unwrap_or("").contains("a'b"), "{bad:?}");
    assert_eq!(result_count(&bad_out), 0);
    assert_eq!(std::fs::read_to_string(&log).unwrap_or_default(), before_bad);

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
