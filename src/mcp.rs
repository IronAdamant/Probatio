//! Stdio MCP server. One JSON-RPC object per line. Tool calls run the same
//! `dispatch` path as the CLI and return that envelope as text.

use std::fs;
use std::io::{self, BufRead, Write};
use std::path::Path;

use crate::envelope::Envelope;
use crate::json::{self, Value};

const PROTOCOL: &str = "2024-11-05";
const LINE_MAP_BODY: &str = "{\"lines\":[1]}\n";

pub fn serve() -> i32 {
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();
    let mut stdout = io::stdout().lock();
    while let Some(line) = lines.next() {
        let line = match line {
            Ok(text) => text,
            Err(_) => return 1,
        };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = reply(&line) {
            if writeln!(stdout, "{response}").is_err() || stdout.flush().is_err() {
                return 1;
            }
        }
    }
    0
}

fn reply(line: &str) -> Option<String> {
    let message = match json::parse(line) {
        Ok(value) => value,
        Err(error) => return Some(error_line(None, -32700, &error)),
    };
    let id = message.get("id").cloned();
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    if id.is_none() || method.starts_with("notifications/") {
        return None;
    }
    let id = id.unwrap();
    let result = match method {
        "initialize" => initialize(message.get("params")),
        "ping" => Value::object(vec![]),
        "tools/list" => Value::object(vec![("tools", Value::Array(tools()))]),
        "tools/call" => return Some(tools_call(&id, message.get("params"))),
        _ => return Some(error_line(Some(id), -32601, "Method not found")),
    };
    Some(result_line(&id, result))
}

fn initialize(params: Option<&Value>) -> Value {
    let version = params
        .and_then(|params| params.get("protocolVersion"))
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .unwrap_or(PROTOCOL);
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
    ])
}

fn tools() -> Vec<Value> {
    vec![
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
            "Score mutants. A red baseline stops with no kill count. More than 30 mutants need a non-empty lineMap, or serial true. serial writes <out>/lines.json and starts one full suite run per mutant. The file does not select tests. Workers stay at 1.",
            vec![
                ("package", "string", "Directory of the project under test."),
                ("patches", "string", "Directory or mutants.json from mutate_generate."),
                ("out", "string", "Directory for baseline.json and results/."),
                ("suiteCommand", "string", "Suite command. Omit it to use the marker at the package root."),
                ("lineMap", "string", "Path to a non-empty file. Its contents are not read."),
                ("serial", "boolean", "Write <out>/lines.json when lineMap is omitted, then start the whole batch."),
                ("timeoutMs", "integer", "Clock for the baseline and for each mutant. Default 120000."),
                ("tests", "string", "Path to a file of test names, one per line. The suite command is still run as given."),
                ("maxSelected", "integer", "How many of those names are recorded. Default 30."),
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
        "schema" => (crate::dispatch(&["schema".into()]), None),
        "mutate_generate" => generate(&args),
        "mutate_run" => run(&args),
        "mutate_tally" => tally(&args),
        _ => return tool_error(id, "unknown tool"),
    };
    let mut content = vec![Value::object(vec![
        ("type", Value::String("text".into())),
        ("text", Value::String(envelope.render(""))),
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
            ("isError", Value::Bool(false)),
        ]),
    )
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
