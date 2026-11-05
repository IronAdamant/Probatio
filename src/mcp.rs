//! Stdio MCP server. One JSON-RPC object per line. A tool call runs this binary with the
//! CLI argv the arguments map to, and returns that envelope.
//!
//! `PROBATIO_TOOLS` and `PROBATIO_DISABLE` shape the tool list by group. See `Surface`.

use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::json::{self, Value};

const PROTOCOLS: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18"];
const LINE_MAP_BODY: &str = "{\"lines\":[1]}\n";

const INSTRUCTIONS: &str = "Probatio scores a test suite: it makes one small code change (a mutant) at a time in a git worktree or temp copy, runs the project's own tests, and records whether a test failed. The package is never edited. Call mutate_generate, then mutate_run with patches set to the generate out, then mutate_tally. Read summary and next in each reply; ok false means stop and show them.";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Str,
    Int,
    Bool,
    StrArray,
}

impl Kind {
    fn json_type(self) -> &'static str {
        match self {
            Kind::Str => "string",
            Kind::Int => "integer",
            Kind::Bool => "boolean",
            Kind::StrArray => "array",
        }
    }
}

struct Prop {
    key: &'static str,
    kind: Kind,
    help: &'static str,
}

struct ToolSpec {
    name: &'static str,
    group: &'static str,
    description: &'static str,
    props: &'static [Prop],
    required: &'static [&'static str],
}

/// Always on. The server is never left without a way to score a suite.
const CORE: &str = "core";
const GROUPS: &[&str] = &["core", "diagnostics", "cli"];

const TOOLS: &[ToolSpec] = &[
    ToolSpec {
        name: "mutate_generate",
        group: CORE,
        description: "Write mutants (one small code change each) for a package to <out>/mutants.json. Runs no tests. Next: mutate_run with patches set to this out.",
        props: &[
            Prop { key: "package", kind: Kind::Str, help: "Project directory." },
            Prop { key: "out", kind: Kind::Str, help: "Output directory, usually <package>/.probatio/generate." },
            Prop { key: "src", kind: Kind::Str, help: "Subdirectory of package to mutate. Default src." },
            Prop { key: "maxMutants", kind: Kind::Int, help: "Stop after this many mutants." },
        ],
        required: &["package", "out"],
    },
    ToolSpec {
        name: "mutate_run",
        group: CORE,
        description: "Run the suite once as a baseline, then once per mutant in a worktree or temp copy, and record killed (a test failed), survived (a gap), or no coverage. The package is not edited. A red baseline, a missing tool, or over 30 mutants with no line map stops with no score; serial=true allows a large batch. The same call resumes an interrupted run.",
        props: &[
            Prop { key: "package", kind: Kind::Str, help: "Project directory." },
            Prop { key: "patches", kind: Kind::Str, help: "The mutate_generate out directory." },
            Prop { key: "out", kind: Kind::Str, help: "Results directory, usually <package>/.probatio/run. Reuse it to resume." },
            Prop { key: "suiteCommand", kind: Kind::Str, help: "Test command. Default: .probatio.json suiteCommand, else found from the package root (cargo test, go test ./..., npm test, python3 -m pytest, ...)." },
            Prop { key: "timeoutMs", kind: Kind::Int, help: "Clock for the baseline and each mutant. Default 120000." },
            Prop { key: "workers", kind: Kind::Int, help: "Mutants scored at once, each in its own worktree. Default 1." },
            Prop { key: "confirm", kind: Kind::Bool, help: "Rerun the killing test twice before counting a kill. Default true. false is faster." },
            Prop { key: "serial", kind: Kind::Bool, help: "Allow more than 30 mutants without a line map." },
            Prop { key: "lineMap", kind: Kind::Str, help: "Line map file. A files object names the tests for each line." },
            Prop { key: "tests", kind: Kind::Str, help: "File of test names, one per line, for {tests} in suiteCommand." },
            Prop { key: "maxSelected", kind: Kind::Int, help: "Most test names recorded per mutant. Default 30." },
            Prop { key: "confirmCount", kind: Kind::Int, help: "Run only if this equals the number of mutants still waiting." },
        ],
        required: &["package", "patches", "out"],
    },
    ToolSpec {
        name: "mutate_tally",
        group: CORE,
        description: "Summarize a mutate_run out directory: tests that killed, survivors (gaps) with file:line, no coverage. Safe while a run is still going. Deletes nothing.",
        props: &[Prop { key: "out", kind: Kind::Str, help: "The mutate_run out directory." }],
        required: &["out"],
    },
    ToolSpec {
        name: "schema",
        group: "diagnostics",
        description: "List the CLI commands, or give one command's flags and output fields.",
        props: &[Prop { key: "command", kind: Kind::Str, help: "Command name with dots, e.g. mutate.run. Omit to list." }],
        required: &[],
    },
    ToolSpec {
        name: "probatio",
        group: "cli",
        description: "Run any CLI command by argv, for commands with no tool of their own: ledger, golden, matrix, queue, check-kill, verify-change, mutate sealed, gap, guard, findings, status, seal.",
        props: &[Prop { key: "argv", kind: Kind::StrArray, help: "CLI words and flags, e.g. [\"matrix\",\"report\",\"--out\",\"dir\"]." }],
        required: &["argv"],
    },
];

fn spec(name: &str) -> Option<&'static ToolSpec> {
    TOOLS.iter().find(|tool| tool.name == name)
}

/// Where a switch came from. The operator's env var wins over the agent's `AGENTS.md` line, per key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Env,
    AgentsMd,
}

/// Which tools the server lists and accepts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Surface {
    enabled: Vec<&'static str>,
    /// The `disable` names and where they came from, for the message a disabled call returns.
    disabled_by: Vec<String>,
    disable_source: Option<Source>,
    tools_source: Option<Source>,
    /// One line per `AGENTS.md` line that was ignored.
    warnings: Vec<String>,
}

enum Refusal {
    Unknown(String),
    Core(String),
}

impl Surface {
    pub fn all() -> Surface {
        Surface {
            enabled: TOOLS.iter().map(|tool| tool.name).collect(),
            disabled_by: Vec::new(),
            disable_source: None,
            tools_source: None,
            warnings: Vec::new(),
        }
    }

    /// Env values only. An error is the one line the server prints before it exits 2.
    pub fn from_values(tools: Option<&str>, disable: Option<&str>) -> Result<Surface, String> {
        Surface::resolve(tools, disable, None)
    }

    /// `PROBATIO_TOOLS` and `PROBATIO_DISABLE` (env), then the `probatio:` lines of the first
    /// `agent-tools` block in `AGENTS.md`. A bad env value is an error. A bad file line is a warning,
    /// and that line is ignored.
    pub fn resolve(env_tools: Option<&str>, env_disable: Option<&str>, agents_md: Option<&str>) -> Result<Surface, String> {
        let env_tools = names_of(env_tools);
        let env_disable = names_of(env_disable);
        if let Some(names) = &env_tools {
            check(names, false).map_err(|refusal| env_message("PROBATIO_TOOLS", refusal))?;
        }
        if let Some(names) = &env_disable {
            check(names, true).map_err(|refusal| env_message("PROBATIO_DISABLE", refusal))?;
        }
        let mut warnings = Vec::new();
        let (mut file_tools, mut file_disable) = (None, None);
        for (key, names) in agent_tools_lines(agents_md.unwrap_or("")) {
            let checked = match key.as_str() {
                "tools" => check(&names, false),
                "disable" => check(&names, true),
                other => Err(Refusal::Unknown(other.to_string())),
            };
            match checked {
                Ok(()) if key == "tools" => file_tools = Some(names),
                Ok(()) => file_disable = Some(names),
                Err(Refusal::Unknown(name)) if key != "tools" && key != "disable" => warnings.push(format!(
                    "AGENTS.md agent-tools: unknown key \"{name}\" for probatio; use \"probatio: tools <groups>\" or \"probatio: disable <groups>\""
                )),
                Err(Refusal::Unknown(name)) => warnings.push(format!(
                    "AGENTS.md agent-tools: unknown group \"{name}\" for probatio; groups: {}",
                    GROUPS.join(", ")
                )),
                Err(Refusal::Core(what)) => warnings.push(format!(
                    "AGENTS.md agent-tools: {what} is always on for probatio; groups you can disable: {}",
                    optional_groups()
                )),
            }
        }
        let tools = env_tools.map(|names| (names, Source::Env)).or(file_tools.map(|names| (names, Source::AgentsMd)));
        let disable = env_disable.map(|names| (names, Source::Env)).or(file_disable.map(|names| (names, Source::AgentsMd)));
        let mut surface = Surface::all();
        surface.warnings = warnings;
        if let Some((names, source)) = tools {
            surface.tools_source = Some(source);
            if !names.iter().any(|name| name == "all") {
                surface.enabled = TOOLS
                    .iter()
                    .filter(|tool| tool.group == CORE || names.iter().any(|name| name == tool.group || name == tool.name))
                    .map(|tool| tool.name)
                    .collect();
            }
        }
        if let Some((names, source)) = disable {
            surface.disable_source = Some(source);
            surface.enabled.retain(|tool| {
                let group = spec(tool).map(|spec| spec.group).unwrap_or("");
                !names.iter().any(|name| name == tool || name == group)
            });
            surface.disabled_by = names;
        }
        Ok(surface)
    }

    pub fn is_enabled(&self, tool: &str) -> bool {
        self.enabled.iter().any(|name| *name == tool)
    }

    pub fn enabled(&self) -> &[&'static str] {
        &self.enabled
    }

    fn disabled_message(&self, tool: &ToolSpec) -> String {
        if let Some(name) = self.disabled_by.iter().find(|name| *name == tool.name || *name == tool.group) {
            return match self.disable_source {
                Some(Source::AgentsMd) => format!(
                    "tool \"{}\" is disabled (group \"{}\") by the probatio: disable line in AGENTS.md agent-tools; remove \"{name}\" from it to enable",
                    tool.name, tool.group
                ),
                _ => format!(
                    "tool \"{}\" is disabled (group \"{}\") by PROBATIO_DISABLE; remove \"{name}\" from it to enable",
                    tool.name, tool.group
                ),
            };
        }
        match self.tools_source {
            Some(Source::AgentsMd) => format!(
                "tool \"{}\" is disabled (group \"{}\") by the probatio: tools line in AGENTS.md agent-tools; add \"{}\" to it to enable",
                tool.name, tool.group, tool.group
            ),
            _ => format!(
                "tool \"{}\" is disabled (group \"{}\") by PROBATIO_TOOLS; add \"{}\" to it to enable",
                tool.name, tool.group, tool.group
            ),
        }
    }

    /// The groups that are on and off, and where to change them. One line.
    pub fn groups_line(&self) -> String {
        let (on, off): (Vec<&str>, Vec<&str>) = GROUPS
            .iter()
            .partition(|group| TOOLS.iter().any(|tool| tool.group == **group && self.is_enabled(tool.name)));
        let off = if off.is_empty() { "none".to_string() } else { off.join(", ") };
        format!(
            "Tool groups on: {}. Off: {off}. Change them with a \"probatio: disable <groups>\" line in an agent-tools block in AGENTS.md.",
            on.join(", ")
        )
    }

    /// The `initialize` instructions: the workflow, any `AGENTS.md` warning, then the groups line.
    pub fn instructions(&self) -> String {
        let mut text = INSTRUCTIONS.to_string();
        for warning in &self.warnings {
            text.push('\n');
            text.push_str(warning);
        }
        text.push('\n');
        text.push_str(&self.groups_line());
        text
    }
}

fn check(names: &[String], disable: bool) -> Result<(), Refusal> {
    for name in names {
        if name == "all" && !disable {
            continue;
        }
        if !is_known(name) {
            return Err(Refusal::Unknown(name.clone()));
        }
        if disable && (name == CORE || spec(name).is_some_and(|tool| tool.group == CORE)) {
            let what = if name == CORE { "group \"core\"".to_string() } else { format!("tool \"{name}\" (group \"core\")") };
            return Err(Refusal::Core(what));
        }
    }
    Ok(())
}

fn env_message(var: &str, refusal: Refusal) -> String {
    match refusal {
        Refusal::Unknown(name) => unknown(var, &name),
        Refusal::Core(what) => {
            format!("{var}: {what} is always on and cannot be disabled; groups you can disable: {}", optional_groups())
        }
    }
}

fn optional_groups() -> String {
    GROUPS.iter().filter(|group| **group != CORE).copied().collect::<Vec<_>>().join(", ")
}

fn names_of(value: Option<&str>) -> Option<Vec<String>> {
    let value = value?;
    if value.trim().is_empty() {
        return None;
    }
    Some(split_names(value))
}

/// Commas or spaces, case and whitespace ignored.
fn split_names(value: &str) -> Vec<String> {
    value
        .split(|ch: char| ch == ',' || ch.is_whitespace())
        .map(|name| name.trim().to_ascii_lowercase())
        .filter(|name| !name.is_empty())
        .collect()
}

/// The `probatio: <key> <names>` lines of the first fenced block whose info string is `agent-tools`.
/// `#` starts a comment. Lines for other tools are skipped.
fn agent_tools_lines(markdown: &str) -> Vec<(String, Vec<String>)> {
    let mut lines = Vec::new();
    let mut fence: Option<String> = None;
    let mut inside = false;
    for raw in markdown.lines() {
        let trimmed = raw.trim();
        if let Some(open) = &fence {
            if trimmed.starts_with(open.as_str()) && trimmed.trim_start_matches(open.chars().next().unwrap_or('`')).is_empty() {
                if inside {
                    break;
                }
                fence = None;
            } else if inside {
                let line = trimmed.split('#').next().unwrap_or("").trim();
                let Some((tool, rest)) = line.split_once(':') else { continue };
                if tool.trim().to_ascii_lowercase() != "probatio" {
                    continue;
                }
                let rest = rest.trim();
                let (key, names) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                lines.push((key.trim().to_ascii_lowercase(), split_names(names)));
            }
            continue;
        }
        let ticks = trimmed.chars().take_while(|ch| *ch == '`' || *ch == '~').collect::<String>();
        if ticks.len() >= 3 && ticks.chars().all(|ch| ch == ticks.chars().next().unwrap_or('`')) {
            inside = trimmed[ticks.len()..].trim() == "agent-tools";
            fence = Some(ticks);
        }
    }
    lines
}

/// Restats `AGENTS.md` in the server's working directory on each request.
/// Size and modified time are compared, so an unchanged file is never reread.
pub struct Watcher {
    path: std::path::PathBuf,
    env_tools: Option<String>,
    env_disable: Option<String>,
    stamp: Option<(std::time::SystemTime, u64)>,
    surface: Surface,
}

impl Watcher {
    pub fn start(path: std::path::PathBuf, env_tools: Option<String>, env_disable: Option<String>) -> Result<Watcher, String> {
        let stamp = stamp_of(&path);
        let text = fs::read_to_string(&path).ok();
        let surface = Surface::resolve(env_tools.as_deref(), env_disable.as_deref(), text.as_deref())?;
        Ok(Watcher { path, env_tools, env_disable, stamp, surface })
    }

    pub fn surface(&self) -> &Surface {
        &self.surface
    }

    /// True when `AGENTS.md` changed and the tool list changed with it.
    pub fn refresh(&mut self) -> bool {
        let stamp = stamp_of(&self.path);
        if stamp == self.stamp {
            return false;
        }
        self.stamp = stamp;
        let text = fs::read_to_string(&self.path).ok();
        // The env values were checked at startup, so only the file can change the outcome here.
        let Ok(surface) = Surface::resolve(self.env_tools.as_deref(), self.env_disable.as_deref(), text.as_deref()) else {
            return false;
        };
        let changed = surface.enabled != self.surface.enabled;
        self.surface = surface;
        changed
    }
}

fn stamp_of(path: &Path) -> Option<(std::time::SystemTime, u64)> {
    let meta = fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

fn is_known(name: &str) -> bool {
    GROUPS.contains(&name) || spec(name).is_some()
}

fn unknown(var: &str, name: &str) -> String {
    let tools = TOOLS.iter().map(|tool| tool.name).collect::<Vec<_>>().join(", ");
    format!("{var}: unknown group \"{name}\"; groups: {}; tools: {tools}", GROUPS.join(", "))
}

const LIST_CHANGED: &str = r#"{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}"#;

pub fn serve() -> i32 {
    let env = |key: &str| std::env::var(key).ok();
    let path = std::env::current_dir().unwrap_or_default().join("AGENTS.md");
    let mut watcher = match Watcher::start(path, env("PROBATIO_TOOLS"), env("PROBATIO_DISABLE")) {
        Ok(watcher) => watcher,
        Err(message) => {
            eprintln!("{message}");
            return 2;
        }
    };
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
        // An agent may edit the agent-tools block mid-session. The client refetches on this notice.
        if watcher.refresh() && method != "initialize" {
            write_line(&write_lock, LIST_CHANGED);
        }
        if method == "tools/call" {
            let id = message.get("id").cloned().unwrap_or(Value::Null);
            let params = message.get("params").cloned();
            let running = running.clone();
            let write_lock = write_lock.clone();
            let surface = watcher.surface().clone();
            thread::spawn(move || {
                let response = tools_call_live(&id, params.as_ref(), &running, &surface);
                if !response.is_empty() {
                    write_line(&write_lock, &response);
                }
            });
            continue;
        }
        if let Some(response) = reply_message(&message, watcher.surface()) {
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
    let Some(key) = id_key(id) else { return };
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

/// Every method except `tools/call`, which `serve` runs on its own thread.
pub fn reply_message(message: &Value, surface: &Surface) -> Option<String> {
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    if method.starts_with("notifications/") {
        return None;
    }
    let id = message.get("id").cloned()?;
    let result = match method {
        "initialize" => initialize(message.get("params"), surface),
        "ping" => Value::object(vec![]),
        "tools/list" => Value::object(vec![("tools", Value::Array(tools(surface)))]),
        _ => return Some(error_line(Some(id), -32601, &format!("method not found: {method}"))),
    };
    Some(result_line(&id, result))
}

fn initialize(params: Option<&Value>, surface: &Surface) -> Value {
    let requested = params.and_then(|params| params.get("protocolVersion")).and_then(Value::as_str).unwrap_or("");
    let version = if PROTOCOLS.contains(&requested) { requested } else { PROTOCOLS[0] };
    let instructions = surface.instructions();
    Value::object(vec![
        ("protocolVersion", Value::String(version.to_string())),
        (
            "capabilities",
            Value::object(vec![("tools", Value::object(vec![("listChanged", Value::Bool(true))]))]),
        ),
        (
            "serverInfo",
            Value::object(vec![
                ("name", Value::String("probatio".into())),
                ("version", Value::String(env!("CARGO_PKG_VERSION").into())),
            ]),
        ),
        ("instructions", Value::String(instructions)),
    ])
}

pub fn tools(surface: &Surface) -> Vec<Value> {
    TOOLS.iter().filter(|tool| surface.is_enabled(tool.name)).map(tool_value).collect()
}

fn tool_value(tool: &ToolSpec) -> Value {
    let properties = Value::Object(
        tool.props
            .iter()
            .map(|prop| {
                let mut fields = vec![("type", Value::String(prop.kind.json_type().into()))];
                if prop.kind == Kind::StrArray {
                    fields.push(("items", Value::object(vec![("type", Value::String("string".into()))])));
                }
                fields.push(("description", Value::String(prop.help.into())));
                (prop.key.to_string(), Value::object(fields))
            })
            .collect(),
    );
    Value::object(vec![
        ("name", Value::String(tool.name.into())),
        ("description", Value::String(tool.description.into())),
        (
            "inputSchema",
            Value::object(vec![
                ("type", Value::String("object".into())),
                ("properties", properties),
                ("required", Value::Array(tool.required.iter().map(|key| Value::String((*key).into())).collect())),
                ("additionalProperties", Value::Bool(false)),
            ]),
        ),
    ])
}

fn tools_call_live(id: &Value, params: Option<&Value>, running: &Mutex<HashMap<String, u32>>, surface: &Surface) -> String {
    let (argv, note) = match argv_of_call(params, surface) {
        Ok(found) => found,
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
    wrap_child_envelope(id, &text, note)
}

/// The envelope is sent once as compact text and once as `structuredContent`.
/// Compact text is about a third smaller than the CLI's indented form.
fn wrap_child_envelope(id: &Value, text: &str, note: Option<String>) -> String {
    let Some(envelope) = json::parse(text.trim()).ok() else {
        return error_line(Some(id.clone()), -32000, "the command did not return one JSON object");
    };
    let ok = envelope.get("ok").and_then(Value::as_bool) == Some(true);
    let mut content = vec![text_block(&json::stringify_compact(&envelope))];
    if let Some(note) = note {
        content.push(text_block(&note));
    }
    result_line(
        id,
        Value::object(vec![
            ("content", Value::Array(content)),
            ("structuredContent", envelope),
            ("isError", Value::Bool(!ok)),
        ]),
    )
}

fn text_block(text: &str) -> Value {
    Value::object(vec![("type", Value::String("text".into())), ("text", Value::String(text.into()))])
}

/// The CLI argv for one tool call, after the arguments are checked against the tool's schema.
/// Each refusal names the tool, the argument, and what is expected.
pub fn argv_of_call(params: Option<&Value>, surface: &Surface) -> Result<(Vec<String>, Option<String>), String> {
    let Some(params) = params else {
        return Err("tools/call needs params with name and arguments".into());
    };
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return Err("tools/call needs a tool name".into());
    };
    let Some(tool) = spec(name) else {
        let names = TOOLS.iter().filter(|tool| surface.is_enabled(tool.name)).map(|tool| tool.name).collect::<Vec<_>>();
        return Err(format!("unknown tool \"{name}\"; tools: {}", names.join(", ")));
    };
    if !surface.is_enabled(tool.name) {
        return Err(surface.disabled_message(tool));
    }
    let args = match params.get("arguments") {
        None | Some(Value::Null) => Value::object(vec![]),
        Some(value @ Value::Object(_)) => value.clone(),
        Some(_) => return Err(format!("{name}: arguments must be an object")),
    };
    let checked = check_args(tool, &args)?;
    build_argv(tool, &checked)
}

enum Arg {
    Str(String),
    Int(i64),
    Bool(bool),
    List(Vec<String>),
}

fn check_args(tool: &ToolSpec, args: &Value) -> Result<Vec<(&'static str, Arg)>, String> {
    let Value::Object(entries) = args else { return Ok(Vec::new()) };
    let expected = || tool.props.iter().map(|prop| prop.key).collect::<Vec<_>>().join(", ");
    let mut checked = Vec::new();
    for (key, value) in entries {
        let Some(prop) = tool.props.iter().find(|prop| prop.key == key) else {
            return Err(format!("{}: unknown argument \"{key}\"; arguments: {}", tool.name, expected()));
        };
        if matches!(value, Value::Null) {
            continue;
        }
        let arg = match (prop.kind, value) {
            (Kind::Str, Value::String(text)) if !text.trim().is_empty() => Arg::Str(text.clone()),
            (Kind::Str, Value::String(_)) => {
                return Err(format!("{}: \"{key}\" is empty; {}", tool.name, prop.help));
            }
            (Kind::Int, Value::Int(number)) if *number >= 0 => Arg::Int(*number),
            (Kind::Int, Value::Float(number)) if number.fract() == 0.0 && *number >= 0.0 => Arg::Int(*number as i64),
            (Kind::Int, Value::String(text)) if text.trim().parse::<u64>().is_ok() => Arg::Int(text.trim().parse::<i64>().unwrap_or(0)),
            (Kind::Bool, Value::Bool(flag)) => Arg::Bool(*flag),
            (Kind::Bool, Value::String(text)) if text == "true" || text == "false" => Arg::Bool(text == "true"),
            (Kind::StrArray, Value::Array(items)) if items.iter().all(|item| item.as_str().is_some()) => {
                Arg::List(items.iter().filter_map(Value::as_str).map(str::to_string).collect())
            }
            (kind, other) => {
                let wanted = match kind {
                    Kind::Int => "a non-negative integer",
                    Kind::Bool => "true or false",
                    Kind::StrArray => "an array of strings",
                    Kind::Str => "a string",
                };
                return Err(format!("{}: \"{key}\" must be {wanted}, got {}", tool.name, short(other)));
            }
        };
        checked.push((prop.key, arg));
    }
    for key in tool.required {
        if !checked.iter().any(|(have, _)| have == key) {
            let help = tool.props.iter().find(|prop| prop.key == *key).map(|prop| prop.help).unwrap_or("");
            return Err(format!("{}: missing required argument \"{key}\" ({help})", tool.name));
        }
    }
    Ok(checked)
}

fn short(value: &Value) -> String {
    let text = json::stringify_compact(value);
    if text.chars().count() > 40 {
        format!("{}...", text.chars().take(40).collect::<String>())
    } else {
        text
    }
}

fn build_argv(tool: &ToolSpec, args: &[(&'static str, Arg)]) -> Result<(Vec<String>, Option<String>), String> {
    let get = |key: &str| args.iter().find(|(have, _)| *have == key).map(|(_, arg)| arg);
    let text = |key: &str| match get(key) {
        Some(Arg::Str(value)) => Some(value.clone()),
        _ => None,
    };
    let mut argv: Vec<String> = match tool.name {
        "probatio" => match get("argv") {
            Some(Arg::List(words)) => return Ok((words.clone(), None)),
            _ => return Err("probatio: missing required argument \"argv\"".into()),
        },
        "schema" => {
            let mut argv = vec!["schema".to_string()];
            argv.extend(text("command"));
            return Ok((argv, None));
        }
        "mutate_generate" => vec!["mutate".into(), "generate".into()],
        "mutate_run" => vec!["mutate".into(), "run".into()],
        "mutate_tally" => vec!["mutate".into(), "tally".into()],
        other => return Err(format!("unknown tool \"{other}\"")),
    };
    let mut note = None;
    for (key, arg) in args {
        match (*key, arg) {
            ("serial" | "lineMap", _) => {}
            ("confirm", Arg::Bool(flag)) => {
                if !flag {
                    argv.push("--no-confirm".into());
                }
            }
            (key, Arg::Str(value)) => {
                argv.push(format!("--{}", kebab(key)));
                argv.push(value.clone());
            }
            (key, Arg::Int(value)) => {
                argv.push(format!("--{}", kebab(key)));
                argv.push(value.to_string());
            }
            _ => {}
        }
    }
    if tool.name == "mutate_run" {
        let serial = matches!(get("serial"), Some(Arg::Bool(true)));
        if let Some(path) = line_map(text("lineMap"), serial, text("out").as_deref())? {
            argv.push("--line-map".into());
            argv.push(path.clone());
            note = Some(format!("line map: {path}"));
        }
    }
    Ok((argv, note))
}

/// `maxMutants` to `max-mutants`.
fn kebab(key: &str) -> String {
    let mut out = String::new();
    for ch in key.chars() {
        if ch.is_ascii_uppercase() {
            out.push('-');
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// `serial` with no `lineMap` writes `<out>/lines.json`. An explicit path is used as given.
fn line_map(explicit: Option<String>, serial: bool, out: Option<&str>) -> Result<Option<String>, String> {
    if let Some(path) = explicit {
        return Ok(Some(path));
    }
    if !serial {
        return Ok(None);
    }
    let Some(out) = out else {
        return Err("mutate_run: serial needs out, so the line map has a directory".into());
    };
    let path = Path::new(out).join("lines.json");
    fs::create_dir_all(out).map_err(|error| format!("mutate_run: out {out} could not be created: {error}"))?;
    let existing = fs::read_to_string(&path).unwrap_or_default();
    if existing.trim().is_empty() {
        fs::write(&path, LINE_MAP_BODY).map_err(|error| error.to_string())?;
    }
    Ok(Some(path.display().to_string()))
}

fn id_key(id: &Value) -> Option<String> {
    match id {
        Value::String(text) => Some(text.clone()),
        Value::Int(number) => Some(number.to_string()),
        _ => None,
    }
}

fn tool_error(id: &Value, message: &str) -> String {
    result_line(
        id,
        Value::object(vec![("content", Value::Array(vec![text_block(message)])), ("isError", Value::Bool(true))]),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, arguments: &str) -> Value {
        json::parse(&format!(r#"{{"name":"{name}","arguments":{arguments}}}"#)).unwrap()
    }

    fn names(surface: &Surface) -> Vec<String> {
        tools(surface).iter().filter_map(|tool| tool.get("name").and_then(Value::as_str).map(str::to_string)).collect()
    }

    #[test]
    fn the_default_list_is_every_tool_and_blank_switches_are_unset() {
        let all = json::stringify_compact(&Value::Array(tools(&Surface::all())));
        for (tools_var, disable_var) in [(None, None), (Some(""), Some("  ")), (Some(" "), None)] {
            let surface = Surface::from_values(tools_var, disable_var).unwrap();
            assert_eq!(surface, Surface::all());
            assert_eq!(json::stringify_compact(&Value::Array(tools(&surface))), all);
        }
        assert_eq!(names(&Surface::all()), ["mutate_generate", "mutate_run", "mutate_tally", "schema", "probatio"]);
        let init = initialize(None, &Surface::all());
        let instructions = init.get("instructions").and_then(Value::as_str).unwrap();
        assert!(instructions.ends_with("\nTool groups on: core, diagnostics, cli. Off: none. Change them with a \"probatio: disable <groups>\" line in an agent-tools block in AGENTS.md."), "{instructions}");
        assert_eq!(init.get("capabilities").and_then(|caps| caps.get("tools")).and_then(|tools| tools.get("listChanged")).and_then(Value::as_bool), Some(true));
        assert!(init.get("tools").is_none());
    }

    const AGENTS_MD: &str = "# Agents\n\n```bash\nprobatio: disable core\n```\n\n```agent-tools\n# one agent here\ntrammel: disable locks, notes\nprobatio: disable cli # no raw argv\nprobatio: tools core diagnostics cli\n```\n\n```agent-tools\nprobatio: disable diagnostics\n```\n";

    #[test]
    fn agents_md_chooses_groups_and_env_wins_per_key() {
        let surface = Surface::resolve(None, None, Some(AGENTS_MD)).unwrap();
        assert_eq!(names(&surface), ["mutate_generate", "mutate_run", "mutate_tally", "schema"]);
        assert!(surface.warnings.is_empty(), "{:?}", surface.warnings);
        let error = argv_of_call(Some(&call("probatio", r#"{"argv":["schema"]}"#)), &surface).unwrap_err();
        assert_eq!(error, "tool \"probatio\" is disabled (group \"cli\") by the probatio: disable line in AGENTS.md agent-tools; remove \"cli\" from it to enable");
        let env_disable = Surface::resolve(None, Some("diagnostics"), Some(AGENTS_MD)).unwrap();
        assert_eq!(names(&env_disable), ["mutate_generate", "mutate_run", "mutate_tally", "probatio"]);
        let env_tools = Surface::resolve(Some("core"), None, Some(AGENTS_MD)).unwrap();
        assert_eq!(names(&env_tools), ["mutate_generate", "mutate_run", "mutate_tally"]);
        assert_eq!(Surface::resolve(None, None, Some("no block here")).unwrap(), Surface::all());
    }

    /// A docs example of the block, inside another fence, is text and not config.
    #[test]
    fn a_nested_agent_tools_example_switches_nothing() {
        let docs = "Add this:\n\n````markdown\n```agent-tools\nprobatio: disable cli, diagnostics\n```\n````\n\n```text\n```agent-tools\nprobatio: disable cli\n```\n";
        assert!(agent_tools_lines(docs).is_empty(), "{:?}", agent_tools_lines(docs));
        assert_eq!(Surface::resolve(None, None, Some(docs)).unwrap(), Surface::all());
        for (name, text) in [("AGENTS.md", include_str!("../AGENTS.md")), ("README.md", include_str!("../README.md"))] {
            assert_eq!(Surface::resolve(None, None, Some(text)).unwrap(), Surface::all(), "{name} switches a group");
        }
    }

    #[test]
    fn a_bad_agents_md_line_is_a_warning_not_a_stop() {
        let text = "```agent-tools\nprobatio: disable lokcs\nprobatio: tools diagnostics\n```\n";
        let surface = Surface::resolve(None, None, Some(text)).unwrap();
        assert_eq!(names(&surface), ["mutate_generate", "mutate_run", "mutate_tally", "schema"]);
        assert_eq!(surface.warnings, ["AGENTS.md agent-tools: unknown group \"lokcs\" for probatio; groups: core, diagnostics, cli"]);
        let core = Surface::resolve(None, None, Some("```agent-tools\nprobatio: disable core\n```\n")).unwrap();
        assert_eq!(names(&core), names(&Surface::all()));
        assert_eq!(core.warnings, ["AGENTS.md agent-tools: group \"core\" is always on for probatio; groups you can disable: diagnostics, cli"]);
        let instructions = core.instructions();
        assert!(instructions.contains("\nAGENTS.md agent-tools: group \"core\" is always on"), "{instructions}");
    }

    #[test]
    fn an_edited_agents_md_changes_the_list_once() {
        let dir = std::env::temp_dir().join(format!("probatio-watch-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("AGENTS.md");
        let mut watcher = Watcher::start(path.clone(), None, None).unwrap();
        assert!(!watcher.refresh());
        fs::write(&path, "```agent-tools\nprobatio: disable cli\n```\n").unwrap();
        assert!(watcher.refresh());
        assert!(!watcher.surface().is_enabled("probatio"));
        assert!(!watcher.refresh());
        fs::write(&path, "```agent-tools\nprobatio: disable cli # same set\n```\n").unwrap();
        assert!(!watcher.refresh(), "a changed file with the same tool set sends no notice");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tools_and_disable_filter_by_group_and_by_tool() {
        let only_core = Surface::from_values(Some("core"), None).unwrap();
        assert_eq!(names(&only_core), ["mutate_generate", "mutate_run", "mutate_tally"]);
        let with_cli = Surface::from_values(Some(" CLI "), None).unwrap();
        assert_eq!(names(&with_cli), ["mutate_generate", "mutate_run", "mutate_tally", "probatio"]);
        let by_tool = Surface::from_values(Some("schema"), None).unwrap();
        assert_eq!(names(&by_tool), ["mutate_generate", "mutate_run", "mutate_tally", "schema"]);
        let everything = Surface::from_values(Some("all"), Some("diagnostics")).unwrap();
        assert_eq!(names(&everything), ["mutate_generate", "mutate_run", "mutate_tally", "probatio"]);
        let no_cli = Surface::from_values(None, Some("probatio")).unwrap();
        assert_eq!(names(&no_cli), ["mutate_generate", "mutate_run", "mutate_tally", "schema"]);
        assert!(no_cli.groups_line().starts_with("Tool groups on: core, diagnostics. Off: cli."));
        let init = initialize(None, &no_cli);
        assert!(init.get("instructions").and_then(Value::as_str).unwrap().contains("\nTool groups on: core, diagnostics. Off: cli."));
    }

    #[test]
    fn a_disabled_tool_names_the_switch_that_hid_it() {
        let disabled = Surface::from_values(None, Some("cli")).unwrap();
        let error = argv_of_call(Some(&call("probatio", r#"{"argv":["schema"]}"#)), &disabled).unwrap_err();
        assert_eq!(error, "tool \"probatio\" is disabled (group \"cli\") by PROBATIO_DISABLE; remove \"cli\" from it to enable");
        let allowed = Surface::from_values(Some("core"), None).unwrap();
        let error = argv_of_call(Some(&call("schema", "{}")), &allowed).unwrap_err();
        assert_eq!(error, "tool \"schema\" is disabled (group \"diagnostics\") by PROBATIO_TOOLS; add \"diagnostics\" to it to enable");
    }

    #[test]
    fn an_unknown_name_or_core_is_refused_in_one_line() {
        let error = Surface::from_values(None, Some("lokcs")).unwrap_err();
        assert!(error.starts_with("PROBATIO_DISABLE: unknown group \"lokcs\"; groups: core, diagnostics, cli; tools: mutate_generate"), "{error}");
        assert!(Surface::from_values(Some("cor"), None).unwrap_err().starts_with("PROBATIO_TOOLS: unknown group \"cor\""));
        let core = Surface::from_values(None, Some("core")).unwrap_err();
        assert_eq!(core, "PROBATIO_DISABLE: group \"core\" is always on and cannot be disabled; groups you can disable: diagnostics, cli");
        let tool = Surface::from_values(None, Some("schema, mutate_run")).unwrap_err();
        assert!(tool.contains("tool \"mutate_run\" (group \"core\") is always on"), "{tool}");
        assert!(!error.contains('\n') && !core.contains('\n'));
    }

    #[test]
    fn arguments_are_checked_against_the_schema() {
        let all = Surface::all();
        let typo = argv_of_call(Some(&call("mutate_run", r#"{"package":"p","patches":"g","out":"o","suite_command":"x"}"#)), &all).unwrap_err();
        assert!(typo.starts_with("mutate_run: unknown argument \"suite_command\"; arguments: package, patches, out, suiteCommand"), "{typo}");
        let missing = argv_of_call(Some(&call("mutate_generate", r#"{"package":"p"}"#)), &all).unwrap_err();
        assert_eq!(missing, "mutate_generate: missing required argument \"out\" (Output directory, usually <package>/.probatio/generate.)");
        let wrong = argv_of_call(Some(&call("mutate_run", r#"{"package":"p","patches":"g","out":"o","timeoutMs":"soon"}"#)), &all).unwrap_err();
        assert_eq!(wrong, "mutate_run: \"timeoutMs\" must be a non-negative integer, got \"soon\"");
        let (argv, _) = argv_of_call(
            Some(&call("mutate_run", r#"{"package":"p","patches":"g","out":"o","timeoutMs":"60000","workers":2,"confirm":false,"maxSelected":5}"#)),
            &all,
        )
        .unwrap();
        assert_eq!(argv, ["mutate", "run", "--package", "p", "--patches", "g", "--out", "o", "--timeout-ms", "60000", "--workers", "2", "--no-confirm", "--max-selected", "5"]);
        let (schema, _) = argv_of_call(Some(&call("schema", r#"{"command":"mutate.run"}"#)), &all).unwrap();
        assert_eq!(schema, ["schema", "mutate.run"]);
        let unknown = argv_of_call(Some(&call("mutate", "{}")), &all).unwrap_err();
        assert!(unknown.starts_with("unknown tool \"mutate\"; tools: mutate_generate"), "{unknown}");
    }

    #[test]
    fn each_argument_has_a_flag_the_cli_accepts() {
        for tool in TOOLS.iter().filter(|tool| tool.name.starts_with("mutate_")) {
            let command = tool.name.replace('_', ".");
            let flags: Vec<&str> = crate::args::command_flags(&command).unwrap().iter().map(|(name, _)| *name).collect();
            for prop in tool.props {
                let flag = match prop.key {
                    "serial" => continue,
                    "lineMap" => "line-map".to_string(),
                    other => kebab(other),
                };
                assert!(flags.contains(&flag.as_str()), "{} {} -> --{flag}", tool.name, prop.key);
            }
        }
    }
}
