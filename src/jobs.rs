//! MCP jobs. Every tool call that runs a command is a job: the command runs as a child process,
//! and the call waits for it at most `waitSeconds` (default 20, never more than 600). A call that
//! finishes in that time answers with its envelope, as before. One that does not answers with a
//! `job_id` and the progress so far, and the run goes on; `job_status` waits for it again or
//! reads its progress, and `cancel_job` stops it and removes its mutant trees. No call is held
//! long enough for a client's idle timeout to cut it off, and a result is never lost to one.
//!
//! The shape follows Chisel's `start_job` / `job_status` / `cancel_job`: `job_id`, `status`
//! (`running`, `completed`, `cancelled`, `error`, or `lost`), and `progress_pct`. Each job also has
//! a record under `<cache>/jobs/<job_id>.json`, so a status call after the server restarted still
//! finds the final summary, or reports the job lost with what it had written.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::json::{self, Value};

/// How long a mutate call waits for its job before it answers with the `job_id`.
pub const DEFAULT_CALL_WAIT: u64 = 20;
/// How long `job_status` waits for a running job to finish.
pub const DEFAULT_STATUS_WAIT: u64 = 60;
/// No server-side wait is longer than this.
pub const MAX_WAIT: u64 = 600;
/// Records older than this are removed when the server starts.
const RECORD_DAYS: u64 = 7;

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Running,
    Completed,
    Cancelled,
    Error(String),
}

impl Status {
    fn name(&self) -> &'static str {
        match self {
            Status::Running => "running",
            Status::Completed => "completed",
            Status::Cancelled => "cancelled",
            Status::Error(_) => "error",
        }
    }
}

/// Where a job's progress is read from while it runs.
#[derive(Clone, Debug, Default)]
pub struct Watch {
    /// `mutants.json`, or the directory that holds it.
    pub patches: Option<PathBuf>,
    /// The run's out directory (`results/` and `baseline.json` are inside it).
    pub out: Option<PathBuf>,
    /// The checkout whose mutant trees a cancel sweeps.
    pub source: Option<PathBuf>,
    /// Count only results written after this: `mutate diff` clears its out directory when it
    /// starts, and results of the call before are not this job's.
    pub since: Option<SystemTime>,
}

pub struct Job {
    pub id: String,
    tool: String,
    started: SystemTime,
    clock: Instant,
    watch: Watch,
    note: Option<String>,
    state: Mutex<State>,
    changed: Condvar,
}

struct State {
    status: Status,
    pid: Option<u32>,
    envelope: Option<Value>,
    exited: bool,
    cleaned: Vec<String>,
}

#[derive(Default)]
pub struct Table {
    jobs: Mutex<HashMap<String, Arc<Job>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

pub fn records_dir() -> PathBuf {
    crate::slots::cache_root().join("jobs")
}

fn new_id() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|time| time.as_nanos()).unwrap_or(0);
    let seq = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("job-{nanos:x}{seq:x}")
}

/// Clamp a caller's wait to `0..=600` seconds.
pub fn clamp_wait(seconds: Option<i64>, default: u64) -> Duration {
    Duration::from_secs(seconds.map(|value| value.max(0) as u64).unwrap_or(default).min(MAX_WAIT))
}

impl Table {
    /// Start `command` as a job. The caller waits on the returned job.
    pub fn start(self: &Arc<Self>, tool: &str, mut command: Command, watch: Watch, note: Option<String>) -> Result<Arc<Job>, String> {
        command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
        let child = command.spawn().map_err(|error| format!("the command did not start: {error}"))?;
        let job = Arc::new(Job {
            id: new_id(),
            tool: tool.to_string(),
            started: SystemTime::now(),
            clock: Instant::now(),
            watch,
            note,
            state: Mutex::new(State { status: Status::Running, pid: Some(child.id()), envelope: None, exited: false, cleaned: Vec::new() }),
            changed: Condvar::new(),
        });
        lock(&self.jobs).insert(job.id.clone(), job.clone());
        job.save();
        let waiter = job.clone();
        std::thread::spawn(move || waiter.finish(child));
        Ok(job)
    }

    pub fn get(&self, id: &str) -> Option<Arc<Job>> {
        lock(&self.jobs).get(id).cloned()
    }

    pub fn known(&self) -> Vec<String> {
        let mut ids: Vec<String> = lock(&self.jobs).keys().cloned().collect();
        ids.sort();
        ids
    }

    /// Stop every running job: the client closed the server.
    pub fn stop_all(&self) {
        let jobs: Vec<Arc<Job>> = lock(&self.jobs).values().cloned().collect();
        for job in jobs {
            if job.is_running() {
                job.cancel();
            }
        }
    }
}

impl Job {
    fn finish(&self, child: Child) {
        let output = child.wait_with_output();
        let mut state = lock(&self.state);
        state.exited = true;
        state.pid = None;
        if state.status == Status::Running {
            match output {
                Ok(output) => match json::parse(String::from_utf8_lossy(&output.stdout).trim()) {
                    Ok(envelope) if envelope.get("ok").is_some() => {
                        state.envelope = Some(envelope);
                        state.status = Status::Completed;
                    }
                    _ => state.status = Status::Error("the command did not return one JSON object".into()),
                },
                Err(error) => state.status = Status::Error(error.to_string()),
            }
        }
        drop(state);
        self.save();
        self.changed.notify_all();
    }

    pub fn is_running(&self) -> bool {
        lock(&self.state).status == Status::Running
    }

    /// Block until the job leaves `running` or `limit` passes.
    pub fn wait(&self, limit: Duration) {
        let deadline = Instant::now() + limit;
        let mut state = lock(&self.state);
        while state.status == Status::Running {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            state = self.changed.wait_timeout(state, left).map(|(guard, _)| guard).unwrap_or_else(|poison| poison.into_inner().0);
        }
    }

    /// Stop the job and everything under it, wait for it to exit, then remove the mutant trees it
    /// left. Results it already wrote stay on disk.
    pub fn cancel(&self) -> bool {
        let pid = {
            let mut state = lock(&self.state);
            if state.status != Status::Running {
                return false;
            }
            state.status = Status::Cancelled;
            state.pid
        };
        if let Some(pid) = pid {
            crate::process::kill_descendants(pid);
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut state = lock(&self.state);
        while !state.exited && Instant::now() < deadline {
            state = self.changed.wait_timeout(state, Duration::from_millis(200)).map(|(guard, _)| guard).unwrap_or_else(|poison| poison.into_inner().0);
        }
        drop(state);
        let cleaned = match &self.watch.source {
            Some(source) => crate::slots::sweep(source),
            None => Vec::new(),
        };
        lock(&self.state).cleaned = cleaned;
        self.save();
        self.changed.notify_all();
        true
    }

    /// The MCP `result` for this job now: the envelope when it completed, else its status.
    pub fn reply(&self) -> Value {
        let state = lock(&self.state);
        let elapsed = self.clock.elapsed().as_secs();
        if let (Status::Completed, Some(envelope)) = (&state.status, &state.envelope) {
            let mut envelope = envelope.clone();
            if let Value::Object(entries) = &mut envelope {
                entries.push(("job_id".into(), Value::String(self.id.clone())));
                entries.push(("status".into(), Value::String("completed".into())));
            }
            let ok = envelope.get("ok").and_then(Value::as_bool) == Some(true);
            let mut content = vec![text_block(&json::stringify_compact(&envelope))];
            if let Some(note) = &self.note {
                content.push(text_block(note));
            }
            return Value::object(vec![
                ("content", Value::Array(content)),
                ("structuredContent", envelope),
                ("isError", Value::Bool(!ok)),
            ]);
        }
        let progress = progress(&self.watch);
        let line = progress_line(&progress);
        let (ok, summary, next) = match &state.status {
            Status::Running => (
                true,
                format!("{} is running ({}): {line}.", self.tool, minutes(elapsed)),
                format!(
                    "Call job_status with job_id \"{}\" to wait for the result (waitSeconds up to 600; the result comes back as soon as the run ends), or cancel_job to stop it.",
                    self.id
                ),
            ),
            Status::Cancelled => (
                true,
                format!("{} was cancelled after {}: {line}. {} removed.", self.tool, minutes(elapsed), count(state.cleaned.len(), "mutant tree")),
                "Results written before the cancel are kept in the out directory. The same mutate_run call scores the mutants that have none; a mutate_diff call starts again.".into(),
            ),
            Status::Error(message) => (false, format!("{} failed: {message}.", self.tool), "Run the same call again. If it fails the same way, run the CLI command to see its output.".into()),
            Status::Completed => (false, format!("{} finished with no envelope.", self.tool), "Run the same call again.".into()),
        };
        let structured = Value::object(vec![
            ("schemaVersion", Value::Int(2)),
            ("ok", Value::Bool(ok)),
            ("command", Value::String("job.status".into())),
            ("summary", Value::String(summary)),
            ("next", Value::String(next)),
            ("nextCall", Value::Null),
            ("job_id", Value::String(self.id.clone())),
            ("status", Value::String(state.status.name().into())),
            ("tool", Value::String(self.tool.clone())),
            ("elapsedSeconds", Value::Int(elapsed as i64)),
            ("progress_pct", Value::Int(pct(&progress))),
            ("progress", progress),
            ("cleaned", Value::Array(state.cleaned.iter().cloned().map(Value::String).collect())),
        ]);
        Value::object(vec![
            ("content", Value::Array(vec![text_block(&json::stringify_compact(&structured))])),
            ("structuredContent", structured),
            ("isError", Value::Bool(!ok)),
        ])
    }

    fn save(&self) {
        let state = lock(&self.state);
        let path_text = |path: &Option<PathBuf>| path.as_ref().map(|path| Value::String(path.display().to_string())).unwrap_or(Value::Null);
        let started = self.started.duration_since(UNIX_EPOCH).map(|time| time.as_secs() as i64).unwrap_or(0);
        let mut entries = vec![
            ("job_id", Value::String(self.id.clone())),
            ("tool", Value::String(self.tool.clone())),
            ("status", Value::String(state.status.name().into())),
            ("startedAt", Value::Int(started)),
            ("serverPid", Value::Int(std::process::id() as i64)),
            ("patches", path_text(&self.watch.patches)),
            ("out", path_text(&self.watch.out)),
            ("source", path_text(&self.watch.source)),
        ];
        if let Status::Error(message) = &state.status {
            entries.push(("error", Value::String(message.clone())));
        }
        if let Some(envelope) = &state.envelope {
            entries.push(("result", envelope.clone()));
        }
        let dir = records_dir();
        if fs::create_dir_all(&dir).is_ok() {
            let path = dir.join(format!("{}.json", self.id));
            let temp = dir.join(format!("{}.json.tmp", self.id));
            if fs::write(&temp, json::stringify(&Value::object(entries))).is_ok() {
                let _ = fs::rename(&temp, &path);
            }
        }
    }
}

/// A job this server did not start, or started before it restarted: its record on disk.
pub fn from_record(id: &str) -> Option<Value> {
    if !id.starts_with("job-") || id.contains(['/', '\\', '.']) {
        return None;
    }
    let text = fs::read_to_string(records_dir().join(format!("{id}.json"))).ok()?;
    let record = json::parse(&text).ok()?;
    if let Some(result) = record.get("result") {
        let mut envelope = result.clone();
        if let Value::Object(entries) = &mut envelope {
            entries.push(("job_id".into(), Value::String(id.into())));
            entries.push(("status".into(), Value::String("completed".into())));
        }
        let ok = envelope.get("ok").and_then(Value::as_bool) == Some(true);
        return Some(Value::object(vec![
            ("content", Value::Array(vec![text_block(&json::stringify_compact(&envelope))])),
            ("structuredContent", envelope),
            ("isError", Value::Bool(!ok)),
        ]));
    }
    let path = |key: &str| record.get(key).and_then(Value::as_str).map(PathBuf::from);
    let started = record.get("startedAt").and_then(Value::as_int).map(|secs| UNIX_EPOCH + Duration::from_secs(secs.max(0) as u64));
    let since = if record.get("tool").and_then(Value::as_str) == Some("mutate_diff") { started } else { None };
    let watch = Watch { patches: path("patches"), out: path("out"), source: path("source"), since };
    let progress = progress(&watch);
    let tool = record.get("tool").and_then(Value::as_str).unwrap_or("the job").to_string();
    let status = record.get("status").and_then(Value::as_str).unwrap_or("lost");
    let server = record.get("serverPid").and_then(Value::as_int).unwrap_or(0) as u32;
    let elsewhere = status == "running" && server != std::process::id() && crate::process::pid_alive(server);
    let status = if elsewhere { "running" } else if status == "running" { "lost" } else { status };
    let summary = match status {
        "running" => format!("{tool} is running in another Probatio server (pid {server}): {}.", progress_line(&progress)),
        "lost" => format!("{tool} stopped when the server that ran it stopped: {}.", progress_line(&progress)),
        "cancelled" => format!("{tool} was cancelled: {}.", progress_line(&progress)),
        other => format!("{tool} ended with status {other}: {}.", progress_line(&progress)),
    };
    let structured = Value::object(vec![
        ("schemaVersion", Value::Int(2)),
        ("ok", Value::Bool(false)),
        ("command", Value::String("job.status".into())),
        ("summary", Value::String(summary)),
        ("next", Value::String(if elsewhere {
            "Ask through the session that started it: only its server can wait for it or cancel it. Its results appear in its out directory as they are written.".into()
        } else {
            "Results it wrote are kept in its out directory. Make the same mutate_run call to score the rest, or call mutate_diff again.".into()
        })),
        ("nextCall", Value::Null),
        ("job_id", Value::String(id.into())),
        ("status", Value::String(status.into())),
        ("tool", Value::String(tool)),
        ("progress_pct", Value::Int(pct(&progress))),
        ("progress", progress),
    ]);
    Some(Value::object(vec![
        ("content", Value::Array(vec![text_block(&json::stringify_compact(&structured))])),
        ("structuredContent", structured),
        ("isError", Value::Bool(true)),
    ]))
}

/// Remove job records older than a week.
pub fn prune_records() {
    let Ok(entries) = fs::read_dir(records_dir()) else { return };
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|when| SystemTime::now().duration_since(when).ok())
            .is_some_and(|age| age > Duration::from_secs(RECORD_DAYS * 24 * 3600));
        if old {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Mutants written, mutants with a result, and the outcomes so far. A result file is written
/// whole (to a temp name, then renamed), so a file that does not parse yet is not counted.
pub fn progress(watch: &Watch) -> Value {
    let total = watch.patches.as_deref().filter(|patches| watch.since.is_none() || fresh_dir(patches, watch.since)).and_then(mutant_count);
    let mut outcomes: Vec<(String, i64)> = Vec::new();
    let mut done = 0i64;
    if let Some(out) = &watch.out {
        if let Ok(entries) = fs::read_dir(out.join("results")) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                    continue;
                }
                if let Some(since) = watch.since {
                    if entry.metadata().and_then(|meta| meta.modified()).is_ok_and(|when| when < since) {
                        continue;
                    }
                }
                let Ok(text) = fs::read_to_string(&path) else { continue };
                let Ok(value) = json::parse(&text) else { continue };
                let Some(outcome) = value.get("outcome").and_then(Value::as_str) else { continue };
                done += 1;
                let key = match outcome {
                    "no coverage" => "noCoverage",
                    "unviable" => "unbuildable",
                    "timeout" => "timedOut",
                    "error" => "errored",
                    other => other,
                }
                .to_string();
                match outcomes.iter_mut().find(|(name, _)| *name == key) {
                    Some((_, n)) => *n += 1,
                    None => outcomes.push((key, 1)),
                }
            }
        }
    }
    let fresh = |path: &Path| match watch.since {
        Some(since) => fs::metadata(path).and_then(|meta| meta.modified()).is_ok_and(|when| when >= since),
        None => path.exists(),
    };
    let phase = match &watch.out {
        Some(out) if done == 0 && !fresh(&out.join("baseline.json")) => "baseline",
        Some(_) => "mutants",
        None => "running",
    };
    let get = |name: &str| outcomes.iter().find(|(key, _)| key == name).map(|(_, n)| *n).unwrap_or(0);
    let mut entries = vec![
        ("phase", Value::String(phase.into())),
        ("done", Value::Int(done)),
        ("total", total.map(|n| Value::Int(n as i64)).unwrap_or(Value::Null)),
        ("killed", Value::Int(get("killed"))),
        ("survived", Value::Int(get("survived"))),
    ];
    for name in ["unbuildable", "noCoverage", "timedOut", "errored", "flaky"] {
        let n = get(name);
        if n > 0 {
            entries.push((name, Value::Int(n)));
        }
    }
    Value::object(entries)
}

fn fresh_dir(patches: &Path, since: Option<SystemTime>) -> bool {
    let Some(since) = since else { return true };
    let file = if patches.join("mutants.json").is_file() { patches.join("mutants.json") } else { patches.join("mutants").join("mutants.json") };
    fs::metadata(file).and_then(|meta| meta.modified()).is_ok_and(|when| when >= since)
}

fn mutant_count(patches: &Path) -> Option<usize> {
    // `--patches` is the generate out (`mutants/mutants.json` inside) or that `mutants` folder.
    let file = if !patches.is_dir() {
        patches.to_path_buf()
    } else if patches.join("mutants.json").is_file() {
        patches.join("mutants.json")
    } else {
        patches.join("mutants").join("mutants.json")
    };
    let value = json::parse(&fs::read_to_string(file).ok()?).ok()?;
    match value {
        Value::Array(items) => Some(items.len()),
        other => match other.get("mutants") {
            Some(Value::Array(items)) => Some(items.len()),
            _ => None,
        },
    }
}

fn progress_line(progress: &Value) -> String {
    let int = |key: &str| progress.get(key).and_then(Value::as_int).unwrap_or(0);
    let total = progress.get("total").and_then(Value::as_int);
    let phase = progress.get("phase").and_then(Value::as_str).unwrap_or("");
    if phase == "baseline" {
        return match total {
            Some(total) => format!("the baseline suite run is going, {} waiting", count(total as usize, "mutant")),
            None => "the baseline suite run is going".into(),
        };
    }
    if phase == "running" {
        return "no progress file to read for this command".into();
    }
    let of = total.map(|total| format!(" of {total}")).unwrap_or_default();
    let mut line = format!("{}{of} mutants scored, {} killed, {} survived", int("done"), int("killed"), int("survived"));
    for (key, words) in [("unbuildable", "did not build"), ("timedOut", "timed out"), ("noCoverage", "no coverage"), ("errored", "errored"), ("flaky", "flaky")] {
        if int(key) > 0 {
            line.push_str(&format!(", {} {words}", int(key)));
        }
    }
    line
}

fn pct(progress: &Value) -> i64 {
    let done = progress.get("done").and_then(Value::as_int).unwrap_or(0);
    match progress.get("total").and_then(Value::as_int) {
        Some(total) if total > 0 => (done * 100 / total).min(99),
        _ => 0,
    }
}

fn minutes(seconds: u64) -> String {
    if seconds < 60 { format!("{seconds}s") } else { format!("{}m{:02}s", seconds / 60, seconds % 60) }
}

fn count(n: usize, noun: &str) -> String {
    if n == 1 { format!("1 {noun}") } else { format!("{n} {noun}s") }
}

fn text_block(text: &str) -> Value {
    Value::object(vec![("type", Value::String("text".into())), ("text", Value::String(text.into()))])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_are_clamped_to_ten_minutes() {
        assert_eq!(clamp_wait(None, DEFAULT_CALL_WAIT), Duration::from_secs(20));
        assert_eq!(clamp_wait(Some(5000), DEFAULT_CALL_WAIT), Duration::from_secs(600));
        assert_eq!(clamp_wait(Some(-3), DEFAULT_STATUS_WAIT), Duration::from_secs(0));
    }

    /// A result file being written (not yet whole) is not counted, and every whole one is.
    #[test]
    fn progress_counts_whole_results_while_more_are_written() {
        let dir = crate::testdir::TempDir::new("progress");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("out/results")).unwrap();
        fs::create_dir_all(dir.join("patches")).unwrap();
        fs::write(dir.join("patches/mutants.json"), "[{\"id\":\"m1\"},{\"id\":\"m2\"},{\"id\":\"m3\"},{\"id\":\"m4\"}]").unwrap();
        let watch = Watch { patches: Some(dir.join("patches")), out: Some(dir.join("out")), source: None, since: None };
        assert_eq!(progress(&watch).get("phase").and_then(Value::as_str), Some("baseline"));
        fs::write(dir.join("out/results/m1.json"), "{\"outcome\":\"killed\"}").unwrap();
        fs::write(dir.join("out/results/m2.json"), "{\"outcome\":\"unviable\"}").unwrap();
        fs::write(dir.join("out/results/m3.json"), "{\"outcome\":\"surv").unwrap();
        fs::write(dir.join("out/results/m4.json.tmp"), "{\"outcome\":\"survived\"}").unwrap();
        let now = progress(&watch);
        assert_eq!(json::stringify_compact(&now), r#"{"phase":"mutants","done":2,"total":4,"killed":1,"survived":0,"unbuildable":1}"#);
        assert_eq!(pct(&now), 50);
        assert_eq!(progress_line(&now), "2 of 4 mutants scored, 1 killed, 0 survived, 1 did not build");
        let _ = fs::remove_dir_all(&dir);
    }
}
