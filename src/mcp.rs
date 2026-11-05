//! Stdio MCP server. One JSON-RPC object per line. Tool calls run the same
//! `dispatch` path as the CLI and return that envelope as text.

use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::envelope::Envelope;
use crate::json::{self, Value};

const PROTOCOLS: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18"];
const LINE_MAP_BODY: &str = "{\"lines\":[1]}\n";

pub fn serve() -> i32 {
    let running: Arc<Mutex<HashMap<String, u32>>> = Arc::new(Mutex::new(HashMap::new()));
    let write_lock = Arc::new(Mutex::new(()));
    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { return 1 };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(message) = json::parse(&line) else { continue };
        let method = message.get("method").and_then(Value::as_str).unwrap_or("").to_string();
        if method == "notifications/cancelled" {
            if let Some(id) = message.get("params").and_then(|params| params.get("requestId")) {
                cancel_job(&running, id);
            }
            continue;
        }
        if message.get("id").is_none() || method.starts_with("notifications/") {
            continue;
        }
        if method == "tools/call" {
            let id = message.get("id").cloned().unwrap_or(Value::Null);
            let params = message.get("params").cloned();
            let running = running.clone();
            let write_lock = write_lock.clone();
            thread::spawn(move || {
                let response = tools_call_live(&id, params.as_ref(), &running);
                if !response.is_empty() {
                    write_line(&write_lock, &response);
                }
            });
            continue;
        }
        if let Some(response) = reply_message(&message) {
            write_line(&write_lock, &response);
        }
    }
    0
}

fn write_line(lock: &Mutex<()>, line: &str) {
    let _guard = lock.lock().unwrap_or_else(|poison| poison.into_inner());
    let mut stdout = io::stdout().lock();
    let _ = writeln!(stdout, "{line}");
    let _ = stdout.flush();
}

fn cancel_job(running: &Mutex<HashMap<String, u32>>, id: &Value) {
    let key = match id {
        Value::String(text) => text.clone(),
        Value::Int(number) => number.to_string(),
        _ => return,
    };
    let pid = running.lock().unwrap_or_else(|poison| poison.into_inner()).remove(&key);
    if let Some(pid) = pid {
        kill_group(pid);
    }
}

fn kill_group(pid: u32) {
    #[cfg(unix)]
    unsafe {
        extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        let _ = kill(-(pid as i32), 15);
    }
    #[cfg(not(unix))]
    {
        let _ = Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]).status();
    }
}

fn reply_message(message: &Value) -> Option<String> {
    let id = message.get("id").cloned();
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    if method == "notifications/cancelled" {
        return None;
    }
    if id.is_none() || method.starts_with("notifications/") {
        return None;
    }
    let id = id.unwrap();
    let result = match method {
        "initialize" => initialize(message.get("params")),
        "ping" => Value::object(vec![]),
        "tools/list" => Value::object(vec![("tools", Value::Array(tools()))]),
        "tools/call" => return Some(tools_call(&id, message.get("params"))),
        _ => return Some(error_line(Some(id), -32601, &format!("method not found: {method}"))),
    };
    Some(result_line(&id, result))
}

fn initialize(params: Option<&Value>) -> Value {
    let requested = params.and_then(|params| params.get("protocolVersion")).and_then(Value::as_str).unwrap_or("");
    let version = if PROTOCOLS.contains(&requested) { requested } else { PROTOCOLS[0] };
    Value::object(vec![
        ("protocolVersion", Value::String(version.to_string())),
        (
            "capabilities",
            Value::object(vec![("tools", Value::object(vec![("listChanged", Value::Bool(false))]))]),
        ),
        (
            "serverInfo",
            Value::object(vec![
                ("name", Value::String("probatio".into())),
                ("version", Value::String(env!("CARGO_PKG_VERSION").into())),
            ]),
        ),
        ("tools", Value::Array(tools())),
    ])
}

fn tools() -> Vec<Value> {
    vec![
        Value::object(vec![
            ("name", Value::String("probatio".into())),
            ("description", Value::String("Run one probatio command. argv is the CLI words and flags.".into())),
            (
                "inputSchema",
                Value::object(vec![
                    ("type", Value::String("object".into())),
                    (
                        "properties",
                        Value::object(vec![(
                            "argv",
                            Value::object(vec![
                                ("type", Value::String("array".into())),
                                ("items", Value::object(vec![("type", Value::String("string".into()))])),
                                ("description", Value::String("CLI words and flags.".into())),
                            ]),
                        )]),
                    ),
                    ("required", Value::Array(vec![Value::String("argv".into())])),
                ]),
            ),
        ]),
        tool(
            "schema",
            "Print Probatio's command list as one envelope.",
            Vec::new(),
            &[],
        ),
        tool(
            "mutate_generate",
            "Write text-operator mutants for a package. Does not run the suite.",
            vec![
                ("package", "string", "Directory of the project to mutate."),
                ("out", "string", "Directory where mutants.json is written."),
                ("src", "string", "Optional subdirectory of package to walk."),
                ("maxMutants", "integer", "Stop the walk after this many mutants. The envelope states the bound."),
            ],
            &["package", "out"],
        ),
        tool(
            "mutate_run",
            "Score the mutants that have no result yet. A red baseline stops with no kill count. More than 30 mutants and no line map stop before the first mutant. serial writes <out>/lines.json as {\"lines\":[1]}, a permission slip. A files map names tests. confirmCount must equal the waiting count when you pass it. A kill is confirmed by rerunning the killing test. workers runs that many worktrees.",
            vec![
                ("package", "string", "Directory of the project under test."),
                ("patches", "string", "Directory or mutants.json from mutate_generate."),
                ("out", "string", "Directory for baseline.json and results/."),
                ("suiteCommand", "string", "Suite command. Omit it to use the marker at the package root, or suiteCommand in .probatio.json. A non-empty package.json scripts.test string is the Node command."),
                ("lineMap", "string", "Path to a line map. A files object names tests. Any other non-empty file only allows the batch."),
                ("serial", "boolean", "Write <out>/lines.json as a permission slip when lineMap is omitted, then score one chunk. The slip does not choose tests. The same mutate_run continues."),
                ("timeoutMs", "integer", "Clock for the baseline and for each mutant. Default 120000."),
                ("tests", "string", "Path to a file of test names, one per line. Used when the line map is absent or is a permission slip. {tests} and {tests,} are replaced from the selected names."),
                ("maxSelected", "integer", "How many of those names are recorded. Default 30."),
                ("confirmCount", "integer", "Score every mutant that has no result yet, only when this equals that count."),
            ],
            &["package", "patches", "out"],
        ),
        tool(
            "mutate_tally",
            "Read results/*.json. deletedTests stays 0. This does not delete a test.",
            vec![("out", "string", "The mutate_run output directory.")],
            &["out"],
        ),
    ]
}

fn tool(name: &str, description: &str, props: Vec<(&str, &str, &str)>, required: &[&str]) -> Value {
    let properties = Value::Object(
        props
            .into_iter()
            .map(|(key, type_name, description)| {
                (
                    key.to_string(),
                    Value::object(vec![
                        ("type", Value::String(type_name.into())),
                        ("description", Value::String(description.into())),
                    ]),
                )
            })
            .collect(),
    );
    Value::object(vec![
        ("name", Value::String(name.into())),
        ("description", Value::String(description.into())),
        (
            "inputSchema",
            Value::object(vec![
                ("type", Value::String("object".into())),
                ("properties", properties),
                ("required", Value::Array(required.iter().map(|key| Value::String((*key).into())).collect())),
            ]),
        ),
    ])
}

fn tools_call_live(id: &Value, params: Option<&Value>, running: &Mutex<HashMap<String, u32>>) -> String {
    let argv = match argv_of_call(params) {
        Ok(argv) => argv,
        Err(message) => return tool_error(id, &message),
    };
    let key = id_key(id);
    let exe = std::env::current_exe().unwrap_or_else(|_| Path::new("probatio").to_path_buf());
    let mut command = Command::new(exe);
    command.args(&argv).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return error_line(Some(id.clone()), -32000, &format!("the command did not start: {error}")),
    };
    if let Some(key) = key.clone() {
        running.lock().unwrap_or_else(|poison| poison.into_inner()).insert(key, child.id());
    }
    let output = child.wait_with_output();
    if let Some(key) = key {
        if running.lock().unwrap_or_else(|poison| poison.into_inner()).remove(&key).is_none() {
            return String::new();
        }
    }
    let Ok(output) = output else {
        return error_line(Some(id.clone()), -32000, "the command did not return one JSON object");
    };
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    wrap_child_envelope(id, &text)
}

fn wrap_child_envelope(id: &Value, text: &str) -> String {
    let parsed = json::parse(text.trim()).ok();
    let Some(envelope) = parsed else {
        return error_line(Some(id.clone()), -32000, "the command did not return one JSON object");
    };
    let ok = envelope.get("ok").and_then(Value::as_bool) == Some(true);
    result_line(
        id,
        Value::object(vec![
            (
                "content",
                Value::Array(vec![Value::object(vec![
                    ("type", Value::String("text".into())),
                    ("text", Value::String(text.trim().to_string())),
                ])]),
            ),
            ("structuredContent", envelope),
            ("isError", Value::Bool(!ok)),
        ]),
    )
}

fn argv_of_call(params: Option<&Value>) -> Result<Vec<String>, String> {
    let Some(params) = params else {
        return Err("tools/call needs params".into());
    };
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return Err("tools/call needs a tool name".into());
    };
    let args = params.get("arguments").cloned().unwrap_or_else(|| Value::object(vec![]));
    if !matches!(args, Value::Object(_)) {
        return Err("arguments must be an object".into());
    }
    match name {
        "probatio" => {
            let Some(Value::Array(items)) = args.get("argv") else {
                return Err("argv must be an array of strings".into());
            };
            let mut argv = Vec::new();
            for item in items {
                let Some(text) = item.as_str() else {
                    return Err("argv must be an array of strings".into());
                };
                argv.push(text.to_string());
            }
            Ok(argv)
        }
        "schema" => Ok(vec!["schema".into()]),
        "mutate_generate" => Ok(generate_argv(&args)),
        "mutate_run" => run_argv(&args),
        "mutate_tally" => Ok(tally_argv(&args)),
        _ => Err("unknown tool".into()),
    }
}

fn generate_argv(args: &Value) -> Vec<String> {
    let mut argv = vec!["mutate".into(), "generate".into()];
    push_arg(&mut argv, args, "package", "package");
    push_arg(&mut argv, args, "out", "out");
    push_arg(&mut argv, args, "src", "src");
    push_int(&mut argv, args, "maxMutants", "max-mutants");
    argv
}

fn run_argv(args: &Value) -> Result<Vec<String>, String> {
    let mut argv = vec!["mutate".into(), "run".into()];
    push_arg(&mut argv, args, "package", "package");
    push_arg(&mut argv, args, "patches", "patches");
    push_arg(&mut argv, args, "out", "out");
    push_arg(&mut argv, args, "suiteCommand", "suite-command");
    push_int(&mut argv, args, "timeoutMs", "timeout-ms");
    push_arg(&mut argv, args, "tests", "tests");
    push_int(&mut argv, args, "maxSelected", "max-selected");
    push_int(&mut argv, args, "confirmCount", "confirm-count");
    if let Some(path) = ensure_line_map(args)? {
        argv.push("--line-map".into());
        argv.push(path);
    }
    Ok(argv)
}

fn tally_argv(args: &Value) -> Vec<String> {
    let mut argv = vec!["mutate".into(), "tally".into()];
    push_arg(&mut argv, args, "out", "out");
    argv
}

fn id_key(id: &Value) -> Option<String> {
    match id {
        Value::String(text) => Some(text.clone()),
        Value::Int(number) => Some(number.to_string()),
        _ => None,
    }
}

fn tools_call(id: &Value, params: Option<&Value>) -> String {
    let Some(params) = params else {
        return tool_error(id, "tools/call needs params");
    };
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return tool_error(id, "tools/call needs a tool name");
    };
    let args = params.get("arguments").cloned().unwrap_or_else(|| Value::object(vec![]));
    if !matches!(args, Value::Object(_)) {
        return tool_error(id, "arguments must be an object");
    }
    let (envelope, note) = match name {
        "probatio" => probatio_argv(&args),
        "schema" => (crate::dispatch(&["schema".into()]), None),
        "mutate_generate" => generate(&args),
        "mutate_run" => run(&args),
        "mutate_tally" => tally(&args),
        _ => return tool_error(id, "unknown tool"),
    };
    let home = crate::envelope::home_dir();
    let text = envelope.render(&home);
    let structured = json::parse(text.trim()).unwrap_or_else(|_| Value::object(vec![]));
    let mut content = vec![Value::object(vec![
        ("type", Value::String("text".into())),
        ("text", Value::String(text)),
    ])];
    if let Some(note) = note {
        content.push(Value::object(vec![
            ("type", Value::String("text".into())),
            ("text", Value::String(note)),
        ]));
    }
    result_line(
        id,
        Value::object(vec![
            ("content", Value::Array(content)),
            ("structuredContent", structured),
            ("isError", Value::Bool(!envelope.ok)),
        ]),
    )
}

fn probatio_argv(args: &Value) -> (Envelope, Option<String>) {
    let Some(Value::Array(items)) = args.get("argv") else {
        return (Envelope::new(false, "mcp", "argv must be an array of strings", "Pass argv as a list of strings."), None);
    };
    let mut argv = Vec::new();
    for item in items {
        let Some(text) = item.as_str() else {
            return (Envelope::new(false, "mcp", "argv must be an array of strings", "Pass argv as a list of strings."), None);
        };
        argv.push(text.to_string());
    }
    (crate::dispatch(&argv), None)
}

fn generate(args: &Value) -> (Envelope, Option<String>) {
    let mut argv = vec!["mutate".into(), "generate".into()];
    push_arg(&mut argv, args, "package", "package");
    push_arg(&mut argv, args, "out", "out");
    push_arg(&mut argv, args, "src", "src");
    push_int(&mut argv, args, "maxMutants", "max-mutants");
    (crate::dispatch(&argv), None)
}

fn run(args: &Value) -> (Envelope, Option<String>) {
    let mut argv = vec!["mutate".into(), "run".into()];
    push_arg(&mut argv, args, "package", "package");
    push_arg(&mut argv, args, "patches", "patches");
    push_arg(&mut argv, args, "out", "out");
    push_arg(&mut argv, args, "suiteCommand", "suite-command");
    push_int(&mut argv, args, "timeoutMs", "timeout-ms");
    push_arg(&mut argv, args, "tests", "tests");
    push_int(&mut argv, args, "maxSelected", "max-selected");
    push_int(&mut argv, args, "confirmCount", "confirm-count");
    let note = match ensure_line_map(args) {
        Ok(note) => note,
        Err(message) => return (Envelope::new(false, "mutate.run", message, "Fix the line map path and call mutate_run again."), None),
    };
    if let Some(path) = note.as_ref() {
        argv.push("--line-map".into());
        argv.push(path.clone());
    }
    (crate::dispatch(&argv), note.map(|path| format!("line map: {path}")))
}

fn tally(args: &Value) -> (Envelope, Option<String>) {
    let mut argv = vec!["mutate".into(), "tally".into()];
    push_arg(&mut argv, args, "out", "out");
    (crate::dispatch(&argv), None)
}

/// `serial` with no `lineMap` writes `<out>/lines.json`. An explicit path is used as given.
fn ensure_line_map(args: &Value) -> Result<Option<String>, String> {
    if let Some(path) = arg_str(args, "lineMap") {
        return Ok(Some(path.to_string()));
    }
    if args.get("serial").and_then(Value::as_bool) != Some(true) {
        return Ok(None);
    }
    let Some(out) = arg_str(args, "out") else {
        return Err("serial needs out, so the line map has a directory".into());
    };
    let path = Path::new(out).join("lines.json");
    fs::create_dir_all(out).map_err(|error| error.to_string())?;
    let existing = fs::read_to_string(&path).unwrap_or_default();
    if existing.trim().is_empty() {
        fs::write(&path, LINE_MAP_BODY).map_err(|error| error.to_string())?;
    }
    Ok(Some(path.display().to_string()))
}

fn push_arg(argv: &mut Vec<String>, args: &Value, key: &str, flag: &str) {
    if let Some(value) = arg_str(args, key) {
        argv.push(format!("--{flag}"));
        argv.push(value.to_string());
    }
}

fn push_int(argv: &mut Vec<String>, args: &Value, key: &str, flag: &str) {
    if let Some(value) = args.get(key).and_then(Value::as_int) {
        argv.push(format!("--{flag}"));
        argv.push(value.to_string());
    }
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).filter(|text| !text.is_empty())
}

fn tool_error(id: &Value, message: &str) -> String {
    result_line(
        id,
        Value::object(vec![
            (
                "content",
                Value::Array(vec![Value::object(vec![
                    ("type", Value::String("text".into())),
                    ("text", Value::String(message.into())),
                ])]),
            ),
            ("isError", Value::Bool(true)),
        ]),
    )
}

fn result_line(id: &Value, result: Value) -> String {
    json::stringify_compact(&Value::object(vec![
        ("jsonrpc", Value::String("2.0".into())),
        ("id", id.clone()),
        ("result", result),
    ]))
}

fn error_line(id: Option<Value>, code: i64, message: &str) -> String {
    json::stringify_compact(&Value::object(vec![
        ("jsonrpc", Value::String("2.0".into())),
        ("id", id.unwrap_or(Value::Null)),
        (
            "error",
            Value::object(vec![
                ("code", Value::Int(code)),
                ("message", Value::String(message.into())),
            ]),
        ),
    ]))
}
