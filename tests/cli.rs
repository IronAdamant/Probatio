//! Launch the shipped binary. The parser is the library the binary uses.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use probatio::json;

fn launch(args: &[&str]) -> (i32, String) {
    let output = output_with_deadline(probatio_command().args(args), Duration::from_secs(90), args);
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    let code = output.status.code().unwrap_or(1);
    (code, stdout)
}

fn output_with_deadline(command: &mut Command, limit: Duration, label: &[&str]) -> std::process::Output {
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("spawn {label:?}: {error}"));
    let pid = child.id();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(limit) {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => panic!("{label:?}: {error}"),
        Err(_) => {
            kill_tree(pid);
            panic!("{label:?} exceeded {limit:?}");
        }
    }
}

fn kill_tree(pid: u32) {
    #[cfg(unix)]
    unsafe {
        extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        let _ = kill(-(pid as i32), 9);
        let _ = kill(pid as i32, 9);
    }
    #[cfg(not(unix))]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// Kills `pid` if `drop` has not run before `limit`. A blocked read then sees EOF.
struct Deadline {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Deadline {
    fn arm(pid: u32, limit: Duration) -> Deadline {
        let (stop, rx) = mpsc::channel();
        let thread = thread::spawn(move || {
            if rx.recv_timeout(limit).is_err() {
                kill_tree(pid);
            }
        });
        Deadline { stop: Some(stop), thread: Some(thread) }
    }
}

impl Drop for Deadline {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn json_string(text: &str) -> String {
    json::stringify_compact(&json::Value::String(text.to_string()))
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
fn version_flag_reports_the_crate_version() {
    let (code, stdout) = launch(&["--version"]);
    assert_eq!(code, 0);
    let value = one_envelope(&stdout);
    assert_eq!(value.get("command").and_then(json::Value::as_str), Some("version"));
    assert_eq!(value.get("ok").and_then(json::Value::as_bool), Some(true));
    assert_eq!(value.get("summary").and_then(json::Value::as_str), Some(env!("CARGO_PKG_VERSION")));
    let (again, second) = launch(&["-V"]);
    assert_eq!(again, 0);
    assert_eq!(second, stdout);
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
fn a_windows_path_is_legal_json() {
    let encoded = json_string(r"C:\Users\runner\probatio");
    let value = json::parse(&format!(r#"{{"package":{encoded}}}"#)).unwrap();
    assert_eq!(value.get("package").and_then(json::Value::as_str), Some(r"C:\Users\runner\probatio"));
}

#[test]
fn mcp_lists_tools_and_a_serial_run_starts() {
    let mut child = probatio_command()
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn mcp");
    let _deadline = Deadline::arm(child.id(), Duration::from_secs(120));
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    let init = mcp_call(
        &mut stdin,
        &mut stdout,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#,
    );
    assert_eq!(init.get("result").and_then(|value| value.get("serverInfo")).and_then(|value| value.get("name")).and_then(json::Value::as_str), Some("probatio"));
    assert_eq!(init.get("result").and_then(|value| value.get("serverInfo")).and_then(|value| value.get("version")).and_then(json::Value::as_str), Some(env!("CARGO_PKG_VERSION")));
    writeln!(stdin, r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#).unwrap();

    let pong = mcp_call(&mut stdin, &mut stdout, r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#);
    assert!(pong.get("result").is_some(), "{pong:?}");

    let listed = mcp_call(&mut stdin, &mut stdout, r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#);
    let names = listed.get("result").and_then(|value| value.get("tools")).and_then(|value| match value {
        json::Value::Array(items) => Some(items.iter().filter_map(|item| item.get("name").and_then(json::Value::as_str)).collect::<Vec<_>>()),
        _ => None,
    }).unwrap();
    assert_eq!(names, ["mutate_diff", "mutate_generate", "mutate_run", "mutate_tally", "job_status", "cancel_job", "schema", "probatio"]);

    let missing = mcp_call(&mut stdin, &mut stdout, r#"{"jsonrpc":"2.0","id":4,"method":"nope"}"#);
    assert_eq!(missing.get("error").and_then(|value| value.get("code")).and_then(json::Value::as_int), Some(-32601));

    let root = common::TempDir::new("mcp");
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
        &format!(r#"{{"package":{},"out":{}}}"#, json_string(&root.display().to_string()), json_string(&patches.display().to_string())),
    );
    assert_eq!(generate.get("ok").and_then(json::Value::as_bool), Some(true), "{generate:?}");

    let stopped = tool_envelope(
        &mut stdin,
        &mut stdout,
        6,
        "mutate_run",
        &format!(
            r#"{{"package":{},"patches":{},"out":{},"suiteCommand":"echo ok","timeoutMs":10000}}"#,
            json_string(&root.display().to_string()),
            json_string(&patches.display().to_string()),
            json_string(&out.display().to_string())
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
            r#"{{"package":{},"patches":{},"out":{},"suiteCommand":"echo ok","serial":true,"timeoutMs":10000}}"#,
            json_string(&root.display().to_string()),
            json_string(&patches.display().to_string()),
            json_string(&out.display().to_string())
        ),
    );
    assert_eq!(started.get("ok").and_then(json::Value::as_bool), Some(true), "{started:?}");
    assert!(std::fs::read_to_string(out.join("lines.json")).unwrap().contains("lines"));
    assert_eq!(std::fs::read_dir(out.join("results")).unwrap().count(), 31);
    assert!(started.get("nextCall").and_then(|call| call.get("argv")).is_some(), "{started:?}");

    drop(stdin);
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&root);
}

fn assert_named_suite(value: &json::Value, command: &str) {
    assert_eq!(value.get("schemaVersion").and_then(json::Value::as_int), Some(2));
    assert_eq!(value.get("suiteCommand").and_then(json::Value::as_str), Some(command));
    // `next` is prose, so a home path in it reads `~`. `suiteCommand` above stays runnable.
    let next = value.get("next").and_then(json::Value::as_str).unwrap_or("");
    let shown = probatio::envelope::scrub(command, &probatio::envelope::home_dir());
    assert!(next.contains(&shown), "{next}");
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

fn java_package(name: &str, ands: usize) -> common::TempDir {
    let root = common::TempDir::new(&format!("cli-{name}"));
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
        "{\n  \"suiteCommand\": \"echo from-file\",\n  \"env\": {\"A\": \"1\"},\n  \"testLine\": \"from-\",\n  \"passLine\": \"from-file\"\n}\n",
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
    assert_eq!(result_count(&out), 31);
    let first_next = first.get("next").and_then(json::Value::as_str).unwrap_or("");
    assert!(first_next.contains("did not choose tests"), "{first_next}");
    assert!(first_next.contains("tally"), "{first_next}");
    let argv = argv_strings(&first);
    assert!(argv.windows(2).any(|pair| pair[0] == "tally" || pair[0] == "--out"));
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
    let tally = all.get("nextCall").and_then(|call| call.get("argv"));
    assert!(matches!(tally, Some(json::Value::Array(items)) if items.iter().any(|item| item.as_str() == Some("tally"))), "{all:?}");
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
    assert!(!whole_next.contains("Workers stay at 1."), "{whole_next}");

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

#[test]
fn shipped_run_discovers_scripts_test_and_refuses_invalid_package_json() {
    let root = java_package("scripts", 1);
    let package = root.display().to_string();
    let patches = root.join("patches").display().to_string();
    std::fs::write(
        root.join("package.json"),
        "{\"scripts\":{\"test\":\"echo from-script\",\"lint\":\"echo nope\"}}\n",
    )
    .unwrap();
    let from_script = one_envelope(
        &launch(&[
            "mutate",
            "run",
            "--package",
            &package,
            "--patches",
            &patches,
            "--out",
            &root.join("script").display().to_string(),
            "--timeout-ms",
            "5000",
        ])
        .1,
    );
    assert_eq!(from_script.get("ok").and_then(json::Value::as_bool), Some(true), "{from_script:?}");
    assert_named_suite(&from_script, "echo from-script");
    assert!(!from_script.get("summary").and_then(json::Value::as_str).unwrap_or("").contains("node --test"));

    let plain = root.join("plain");
    std::fs::create_dir_all(plain.join("patches")).unwrap();
    std::fs::write(plain.join("package.json"), "{}\n").unwrap();
    std::fs::write(plain.join("patches/mutants.json"), "{\"mutants\":[]}\n").unwrap();
    let plain_value = one_envelope(
        &launch(&[
            "mutate",
            "run",
            "--package",
            &plain.display().to_string(),
            "--patches",
            &plain.join("patches").display().to_string(),
            "--out",
            &plain.join("out").display().to_string(),
        ])
        .1,
    );
    assert_named_suite(&plain_value, "node --test");

    for body in ["{\"scripts\":{\"test\":\"\"}}\n", "{\"scripts\":{\"test\":1}}\n"] {
        let empty = root.join(format!("empty-{}", body.len()));
        std::fs::create_dir_all(empty.join("patches")).unwrap();
        std::fs::write(empty.join("package.json"), body).unwrap();
        std::fs::write(empty.join("patches/mutants.json"), "{\"mutants\":[]}\n").unwrap();
        let value = one_envelope(
            &launch(&[
                "mutate",
                "run",
                "--package",
                &empty.display().to_string(),
                "--patches",
                &empty.join("patches").display().to_string(),
                "--out",
                &empty.join("out").display().to_string(),
            ])
            .1,
        );
        assert_named_suite(&value, "node --test");
    }

    std::fs::write(root.join("package.json"), "{\n").unwrap();
    let invalid = one_envelope(
        &launch(&[
            "mutate",
            "run",
            "--package",
            &package,
            "--patches",
            &patches,
            "--out",
            &root.join("badpkg").display().to_string(),
            "--timeout-ms",
            "5000",
        ])
        .1,
    );
    assert_refused_without_kills(&invalid);
    assert!(invalid.get("suiteCommand").is_none(), "{invalid:?}");
    let summary = invalid.get("summary").and_then(json::Value::as_str).unwrap_or("");
    let next = invalid.get("next").and_then(json::Value::as_str).unwrap_or("");
    assert!(summary.contains("package.json"), "{summary}");
    assert!(next.contains("Discovery was not used"), "{next}");
    assert!(!summary.contains("node --test") && !next.contains("node --test"), "{invalid:?}");

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
            "echo from-flag",
            "--timeout-ms",
            "5000",
        ])
        .1,
    );
    assert_eq!(from_flag.get("ok").and_then(json::Value::as_bool), Some(true), "{from_flag:?}");
    assert_named_suite(&from_flag, "echo from-flag");

    std::fs::write(root.join(".probatio.json"), "{\"suiteCommand\":\"echo from-file\"}\n").unwrap();
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

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
#[cfg(unix)]
fn shipped_run_uses_node_bin_from_scripts_test() {
    let root = java_package("nodebin", 1);
    let package = root.display().to_string();
    let patches = root.join("patches").display().to_string();
    let bin = root.join("node_modules/.bin");
    std::fs::create_dir_all(&bin).unwrap();
    let tool = bin.join("probatio-local-ok");
    let marker = root.join("ran-marker.txt");
    std::fs::create_dir_all(bin.join("../probatio-local-dep")).unwrap();
    std::fs::write(bin.join("../probatio-local-dep/index.js"), "ok\n").unwrap();
    std::fs::write(&tool, format!("#!/bin/sh\necho from-shipped-bin\ntest -f node_modules/probatio-local-dep/index.js\npwd >> '{}'\n", marker.display())).unwrap();
    let mut permissions = std::fs::metadata(&tool).unwrap().permissions();
    use std::os::unix::fs::PermissionsExt;
    permissions.set_mode(0o755);
    std::fs::set_permissions(&tool, permissions).unwrap();
    std::fs::write(root.join("package.json"), "{\"scripts\":{\"test\":\"probatio-local-ok\"}}\n").unwrap();

    let out = root.join("out");
    let value = one_envelope(
        &launch(&[
            "mutate",
            "run",
            "--package",
            &package,
            "--patches",
            &patches,
            "--out",
            &out.display().to_string(),
            "--timeout-ms",
            "5000",
        ])
        .1,
    );
    assert_eq!(value.get("ok").and_then(json::Value::as_bool), Some(true), "{value:?}");
    assert_named_suite(&value, "probatio-local-ok");
    let baseline = std::fs::read_to_string(out.join("baseline.json")).unwrap();
    assert!(baseline.contains("\"green\""), "{baseline}");
    assert!(baseline.contains("probatio-local-ok"), "{baseline}");
    let lines = marker_lines(&marker);
    assert!(lines.iter().any(|line| line.contains("probatio-test-cli-nodebin-")), "{lines:?}");
    // A mutant ran in its slot's tree in the mutant cache.
    assert!(lines.iter().any(|line| line.contains("probatio-test-cache") && line.ends_with("/tree")), "{lines:?}");
    assert_survived_mutants(&out);
    assert_eq!(value.get("counts").and_then(|counts| counts.get("killed")).and_then(json::Value::as_int), Some(0), "{value:?}");

    std::fs::write(root.join("package.json"), "{\"scripts\":{\"test\":\"probatio-not-installed\"}}\n").unwrap();
    let missing_out = root.join("missing");
    let missing = one_envelope(
        &launch(&[
            "mutate",
            "run",
            "--package",
            &package,
            "--patches",
            &patches,
            "--out",
            &missing_out.display().to_string(),
            "--timeout-ms",
            "5000",
        ])
        .1,
    );
    assert_refused_without_kills(&missing);
    assert_named_suite(&missing, "probatio-not-installed");
    let summary = missing.get("summary").and_then(json::Value::as_str).unwrap_or("");
    assert!(summary.contains("suite tool is missing"), "{summary}");
    assert!(!summary.contains("already failing"), "{summary}");
    let missing_baseline = std::fs::read_to_string(missing_out.join("baseline.json")).unwrap();
    assert!(missing_baseline.contains("missing-tool"), "{missing_baseline}");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
#[cfg(unix)]
fn shipped_run_uses_node_bin_from_a_git_worktree() {
    let root = java_package("nodegit", 1);
    let package = root.display().to_string();
    let patches = root.join("patches").display().to_string();
    git_commit(&root, &["src"]);
    let bin = root.join("node_modules/.bin");
    std::fs::create_dir_all(&bin).unwrap();
    let tool = bin.join("probatio-local-ok");
    let marker = root.join("ran-marker.txt");
    std::fs::create_dir_all(bin.join("../probatio-local-dep")).unwrap();
    std::fs::write(bin.join("../probatio-local-dep/index.js"), "ok\n").unwrap();
    std::fs::write(&tool, format!("#!/bin/sh\necho from-shipped-bin\ntest -f node_modules/probatio-local-dep/index.js\npwd >> '{}'\n", marker.display())).unwrap();
    let mut permissions = std::fs::metadata(&tool).unwrap().permissions();
    use std::os::unix::fs::PermissionsExt;
    permissions.set_mode(0o755);
    std::fs::set_permissions(&tool, permissions).unwrap();
    std::fs::write(root.join("package.json"), "{\"scripts\":{\"test\":\"probatio-local-ok\"}}\n").unwrap();

    let out = root.join("out");
    let value = one_envelope(
        &launch(&[
            "mutate",
            "run",
            "--package",
            &package,
            "--patches",
            &patches,
            "--out",
            &out.display().to_string(),
            "--timeout-ms",
            "5000",
        ])
        .1,
    );
    assert_eq!(value.get("ok").and_then(json::Value::as_bool), Some(true), "{value:?}");
    assert_named_suite(&value, "probatio-local-ok");
    let lines = marker_lines(&marker);
    assert!(lines.iter().any(|line| line.contains("probatio-test-cli-nodegit-")), "{lines:?}");
    // A mutant ran in the slot's worktree in the mutant cache.
    assert!(lines.iter().any(|line| line.contains("probatio-test-cache") && line.ends_with("/0/tree")), "{lines:?}");
    assert_survived_mutants(&out);
    assert_eq!(value.get("counts").and_then(|counts| counts.get("killed")).and_then(json::Value::as_int), Some(0), "{value:?}");

    let _ = std::fs::remove_dir_all(&root);
}

#[cfg(unix)]
fn marker_lines(path: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(unix)]
fn assert_survived_mutants(out: &std::path::Path) {
    let mut survived = 0usize;
    for entry in std::fs::read_dir(out.join("results")).unwrap() {
        let body = json::parse(&std::fs::read_to_string(entry.unwrap().path()).unwrap()).unwrap();
        assert_eq!(body.get("outcome").and_then(json::Value::as_str), Some("survived"), "{body:?}");
        survived += 1;
    }
    assert!(survived >= 1, "no mutant result in {}", out.display());
}

#[cfg(unix)]
fn git_commit(root: &std::path::Path, paths: &[&str]) {
    let mut init = std::process::Command::new("git");
    init.arg("-C").arg(root).args(["init"]);
    assert!(init.status().unwrap().success());
    let mut add = std::process::Command::new("git");
    add.arg("-C").arg(root).arg("add").args(paths);
    assert!(add.status().unwrap().success());
    let mut commit = std::process::Command::new("git");
    commit
        .arg("-C")
        .arg(root)
        .arg("-c")
        .arg("user.email=probatio@example.com")
        .arg("-c")
        .arg("user.name=probatio")
        .args(["commit", "-m", "init"]);
    assert!(commit.status().unwrap().success());
}

#[test]
fn shipped_run_uses_a_files_map_and_records_the_command() {
    let root = java_package("filesmap", 2);
    let package = root.display().to_string();
    let patches = root.join("patches").display().to_string();
    let source = std::fs::read_to_string(root.join("src/A.java")).unwrap();
    let listed = json::parse(&std::fs::read_to_string(root.join("patches/mutants.json")).unwrap()).unwrap();
    let items = match listed.get("mutants") {
        Some(json::Value::Array(items)) => items.clone(),
        other => panic!("mutants was {other:?}"),
    };
    let mut lines = Vec::new();
    for item in &items {
        let start = item.get("start").and_then(json::Value::as_int).unwrap() as usize;
        lines.push(source.as_bytes()[..start.min(source.len())].iter().filter(|byte| **byte == b'\n').count() + 1);
    }
    let covered = *lines.iter().min().unwrap();
    assert!(lines.iter().any(|line| *line != covered));
    let map = root.join("map.json");
    std::fs::write(
        &map,
        format!(r#"{{"files":{{"src/A.java":[{{"start":{covered},"end":{covered},"tests":["FooTest"]}}]}}}}"#),
    )
    .unwrap();
    let tests = root.join("tests.txt");
    std::fs::write(&tests, "Bar\n").unwrap();
    let log = root.join("ran.log");
    let command = format!("printf '%s\\n' {{tests}} >> '{}'; printf '%s\\n' {{tests}}", log.display());
    let out = root.join("out");
    let value = one_envelope(
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
            "--line-map",
            &map.display().to_string(),
            "--tests",
            &tests.display().to_string(),
            "--timeout-ms",
            "5000",
        ])
        .1,
    );
    assert_eq!(value.get("schemaVersion").and_then(json::Value::as_int), Some(2));
    assert_eq!(value.get("ok").and_then(json::Value::as_bool), Some(true), "{value:?}");
    assert_named_suite(&value, &command);
    let next = value.get("next").and_then(json::Value::as_str).unwrap_or("");
    assert!(next.contains("The line map named tests."), "{next}");
    assert!(!next.contains("did not choose tests"), "{next}");
    let baseline = json::parse(&std::fs::read_to_string(out.join("baseline.json")).unwrap()).unwrap();
    assert_eq!(baseline.get("command").and_then(json::Value::as_str), Some(command.as_str()));
    let mut spawned = 0usize;
    let mut uncovered = 0usize;
    for entry in std::fs::read_dir(out.join("results")).unwrap() {
        let body = json::parse(&std::fs::read_to_string(entry.unwrap().path()).unwrap()).unwrap();
        match body.get("command").and_then(json::Value::as_str) {
            Some(rendered) => {
                assert!(rendered.contains("FooTest"), "{rendered}");
                assert!(!rendered.contains("Bar"), "{rendered}");
                spawned += 1;
            }
            None => {
                assert_eq!(body.get("outcome").and_then(json::Value::as_str), Some("no coverage"));
                uncovered += 1;
            }
        }
    }
    assert!(spawned >= 1, "{value:?}");
    assert!(uncovered >= 1, "{value:?}");
    // The baseline, one unmutated run in the mutant tree, and each spawned mutant.
    assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), spawned + 2);
    assert_eq!(value.get("counts").and_then(|counts| counts.get("killed")).and_then(json::Value::as_int), Some(0));

    let stopped_out = root.join("stopped");
    let stopped = one_envelope(
        &launch(&[
            "mutate",
            "run",
            "--package",
            &package,
            "--patches",
            &patches,
            "--out",
            &stopped_out.display().to_string(),
            "--suite-command",
            "echo ok",
            "--line-map",
            &map.display().to_string(),
            "--timeout-ms",
            "5000",
        ])
        .1,
    );
    assert_refused_without_kills(&stopped);
    assert!(stopped.get("next").and_then(json::Value::as_str).unwrap_or("").contains("{tests}"), "{stopped:?}");
    assert_eq!(result_count(&stopped_out), 0);

    let bad_map = root.join("bad-map.json");
    std::fs::write(
        &bad_map,
        format!(r#"{{"files":{{"src/A.java":[{{"start":{covered},"end":{covered},"tests":["a'b"]}}]}}}}"#),
    )
    .unwrap();
    let bad_out = root.join("bad");
    let before = std::fs::read_to_string(&log).unwrap();
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
            "--line-map",
            &bad_map.display().to_string(),
            "--timeout-ms",
            "5000",
        ])
        .1,
    );
    assert_refused_without_kills(&bad);
    assert!(bad.get("summary").and_then(json::Value::as_str).unwrap_or("").contains("a'b"), "{bad:?}");
    assert_eq!(result_count(&bad_out), 0);
    assert_eq!(std::fs::read_to_string(&log).unwrap(), before);

    let unnamed_out = root.join("unnamed");
    let unnamed = one_envelope(
        &launch(&[
            "mutate",
            "run",
            "--package",
            &package,
            "--patches",
            &patches,
            "--out",
            &unnamed_out.display().to_string(),
            "--suite-command",
            &command,
            "--timeout-ms",
            "5000",
        ])
        .1,
    );
    assert_refused_without_kills(&unnamed);
    assert!(unnamed.get("summary").and_then(json::Value::as_str).unwrap_or("").contains("asks for test names"), "{unnamed:?}");
    assert_eq!(result_count(&unnamed_out), 0);
    assert_eq!(std::fs::read_to_string(&log).unwrap(), before);

    let same = root.join("template");
    let first = one_envelope(
        &launch(&[
            "mutate",
            "run",
            "--package",
            &package,
            "--patches",
            &patches,
            "--out",
            &same.display().to_string(),
            "--suite-command",
            "echo ok",
            "--timeout-ms",
            "5000",
        ])
        .1,
    );
    assert_eq!(first.get("ok").and_then(json::Value::as_bool), Some(true), "{first:?}");
    let saved = json::parse(&std::fs::read_to_string(same.join("baseline.json")).unwrap()).unwrap();
    assert_eq!(saved.get("command").and_then(json::Value::as_str), Some("echo ok"));
    let second = one_envelope(
        &launch(&[
            "mutate",
            "run",
            "--package",
            &package,
            "--patches",
            &patches,
            "--out",
            &same.display().to_string(),
            "--suite-command",
            "echo fail; exit 1",
            "--timeout-ms",
            "5000",
        ])
        .1,
    );
    assert_refused_without_kills(&second);
    assert!(second.get("summary").and_then(json::Value::as_str).unwrap_or("").contains("already failing"), "{second:?}");
    let saved = json::parse(&std::fs::read_to_string(same.join("baseline.json")).unwrap()).unwrap();
    assert_eq!(saved.get("command").and_then(json::Value::as_str), Some("echo fail; exit 1"));

    let _ = std::fs::remove_dir_all(&root);
}

fn mcp_call(stdin: &mut impl Write, stdout: &mut impl BufRead, line: &str) -> json::Value {
    writeln!(stdin, "{line}").unwrap();
    stdin.flush().unwrap();
    let mut response = String::new();
    stdout.read_line(&mut response).unwrap();
    if response.trim().is_empty() {
        panic!("mcp closed stdout before a reply to {line}");
    }
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

#[test]
fn mcp_exits_2_on_an_unknown_or_core_group_in_the_env() {
    for (key, value, expected) in [
        ("PROBATIO_DISABLE", "lokcs", "PROBATIO_DISABLE: unknown group \"lokcs\"; groups: core, diagnostics, cli; tools: "),
        ("PROBATIO_TOOLS", "mutate", "PROBATIO_TOOLS: unknown group \"mutate\"; groups: "),
        ("PROBATIO_DISABLE", "core", "PROBATIO_DISABLE: group \"core\" is always on and cannot be disabled"),
    ] {
        let output = probatio_command()
            .arg("mcp")
            .env(key, value)
            .stdin(Stdio::null())
            .output()
            .expect("spawn mcp");
        assert_eq!(output.status.code(), Some(2), "{key}={value}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.starts_with(expected), "{stderr}");
        assert_eq!(stderr.trim_end().lines().count(), 1, "{stderr}");
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn mcp_sends_list_changed_when_agents_md_changes_the_groups() {
    let dir = common::TempDir::new("mcp-agents");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut child = probatio_command()
        .arg("mcp")
        .current_dir(&dir)
        .env_remove("PROBATIO_TOOLS")
        .env_remove("PROBATIO_DISABLE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn mcp");
    let _deadline = Deadline::arm(child.id(), Duration::from_secs(60));
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let count = |listed: &json::Value| match listed.get("result").and_then(|value| value.get("tools")) {
        Some(json::Value::Array(items)) => items.len(),
        other => panic!("{other:?}"),
    };
    let first = mcp_call(&mut stdin, &mut stdout, r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#);
    assert_eq!(count(&first), 8);
    std::fs::write(dir.join("AGENTS.md"), "```agent-tools\nprobatio: disable cli, diagnostics\n```\n").unwrap();
    let notice = mcp_call(&mut stdin, &mut stdout, r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
    assert_eq!(notice.get("method").and_then(json::Value::as_str), Some("notifications/tools/list_changed"), "{notice:?}");
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    let second = json::parse(line.trim()).unwrap();
    assert_eq!(count(&second), 6);
    let refused = mcp_call(&mut stdin, &mut stdout, r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"schema","arguments":{}}}"#);
    let text = json::stringify_compact(&refused);
    assert!(text.contains("is disabled (group \\\"diagnostics\\\") by the probatio: disable line in AGENTS.md"), "{text}");
    drop(stdin);
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A client that closes stdin shuts the server down. A call still running stops with it, so it
/// cannot keep scoring in a worktree after the session is gone.
#[test]
fn mcp_stops_a_running_call_when_the_client_leaves() {
    let root = common::TempDir::new("mcp-leave");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/A.java"), "class A { boolean f(boolean a, boolean b) { return a && b; } }\n").unwrap();
    let marker = root.join("finished.txt");
    let (code, generated) = launch(&["mutate", "generate", "--package", &root.display().to_string(), "--out", &root.join("patches").display().to_string()]);
    assert_eq!(code, 0, "{generated}");
    let mut child = probatio_command()
        .arg("mcp")
        .current_dir(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn mcp");
    let _deadline = Deadline::arm(child.id(), Duration::from_secs(60));
    let mut stdin = child.stdin.take().unwrap();
    let suite = format!("sleep 4; echo done > '{}'", marker.display().to_string().replace('\\', "/"));
    let call = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"mutate_run","arguments":{{"package":{},"patches":{},"out":{},"suiteCommand":{}}}}}}}"#,
        json_string(&root.display().to_string()),
        json_string(&root.join("patches").display().to_string()),
        json_string(&root.join("out").display().to_string()),
        json_string(&suite),
    );
    writeln!(stdin, "{call}").unwrap();
    stdin.flush().unwrap();
    thread::sleep(Duration::from_millis(1500));
    let started = std::time::Instant::now();
    drop(stdin);
    let status = child.wait().unwrap();
    assert!(status.success(), "{status:?}");
    assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
    thread::sleep(Duration::from_secs(5));
    assert!(!marker.exists(), "the suite kept running after the server left");
    let _ = std::fs::remove_dir_all(&root);
}

/// Killing only the server process, with stdin still open, also stops the suite under its call:
/// a job object on Windows, a parent check in the child on Unix.
#[test]
fn mcp_stops_a_running_call_when_only_the_server_is_killed() {
    let root = common::TempDir::new("mcp-killed");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/A.java"), "class A { boolean f(boolean a, boolean b) { return a && b; } }\n").unwrap();
    let marker = root.join("finished.txt");
    let (code, generated) = launch(&["mutate", "generate", "--package", &root.display().to_string(), "--out", &root.join("patches").display().to_string()]);
    assert_eq!(code, 0, "{generated}");
    let mut child = probatio_command()
        .arg("mcp")
        .current_dir(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn mcp");
    let _deadline = Deadline::arm(child.id(), Duration::from_secs(60));
    let mut stdin = child.stdin.take().unwrap();
    let suite = format!("sleep 4; echo done > '{}'", marker.display().to_string().replace('\\', "/"));
    let call = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"mutate_run","arguments":{{"package":{},"patches":{},"out":{},"suiteCommand":{}}}}}}}"#,
        json_string(&root.display().to_string()),
        json_string(&root.join("patches").display().to_string()),
        json_string(&root.join("out").display().to_string()),
        json_string(&suite),
    );
    writeln!(stdin, "{call}").unwrap();
    stdin.flush().unwrap();
    thread::sleep(Duration::from_millis(1500));
    child.kill().unwrap();
    let _ = child.wait();
    thread::sleep(Duration::from_secs(5));
    assert!(!marker.exists(), "the suite kept running after the server was killed");
    drop(stdin);
    let _ = std::fs::remove_dir_all(&root);
}

/// The binary, with its mutant cache in the temp folder instead of the user's cache folder.
fn probatio_command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_probatio"));
    command.env("PROBATIO_CACHE_DIR", std::env::temp_dir().join("probatio-test-cache"));
    command
}
