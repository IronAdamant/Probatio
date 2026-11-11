//! MCP jobs: a mutate call that outlasts its wait answers with a job_id, job_status returns the
//! final summary once the run ends (also from a new server), and cancel_job stops the run and
//! removes its mutant tree. Specula M5: `mutate_diff` and `mutate_run` died at the client's
//! 30-minute idle timeout on large crates, and a cancelled run left its worktree behind.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use probatio::json::{self, Value};

struct Server {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next: u64,
}

impl Server {
    fn start(dir: &Path, cache: &Path) -> Server {
        let mut child = Command::new(env!("CARGO_BIN_EXE_probatio"))
            .arg("mcp")
            .current_dir(dir)
            .env("PROBATIO_CACHE_DIR", cache)
            .env_remove("PROBATIO_TOOLS")
            .env_remove("PROBATIO_DISABLE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn mcp");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Server { child, stdin, stdout, next: 1 }
    }

    /// One tools/call, and its `structuredContent` (or the text of a tool error).
    fn call(&mut self, name: &str, arguments: &str) -> Value {
        let id = self.next;
        self.next += 1;
        writeln!(self.stdin, r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"{name}","arguments":{arguments}}}}}"#).unwrap();
        self.stdin.flush().unwrap();
        loop {
            let mut line = String::new();
            self.stdout.read_line(&mut line).unwrap();
            assert!(!line.trim().is_empty(), "the server closed stdout during {name}");
            let reply = json::parse(line.trim()).unwrap();
            if reply.get("id").and_then(Value::as_int) != Some(id as i64) {
                continue;
            }
            let result = reply.get("result").unwrap_or_else(|| panic!("{line}"));
            return match result.get("structuredContent") {
                Some(content) => content.clone(),
                None => result.clone(),
            };
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// A git repo with one committed Java file and an uncommitted edit that has one mutant.
fn edited_repo(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("probatio-jobs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    git(&dir, &["init", "-q"]);
    git(&dir, &["config", "core.autocrlf", "false"]);
    std::fs::write(dir.join("src/A.java"), "class A {\n  boolean g(int n) { return n == 0; }\n}\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["-c", "user.email=a@b.c", "-c", "user.name=a", "commit", "-q", "-m", "init"]);
    std::fs::write(dir.join("src/A.java"), "class A {\n  boolean g(int n) { return n > 0; }\n}\n").unwrap();
    dir
}

fn text(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn trees_in(cache: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(keys) = std::fs::read_dir(cache) else { return found };
    for key in keys.flatten() {
        let Ok(slots) = std::fs::read_dir(key.path()) else { continue };
        for slot in slots.flatten() {
            let Ok(entries) = std::fs::read_dir(slot.path()) else { continue };
            found.extend(entries.flatten().filter(|entry| entry.file_name().to_string_lossy().starts_with("tree")).map(|entry| entry.path()));
        }
    }
    found
}

#[test]
fn a_slow_diff_answers_with_a_job_and_status_returns_the_summary_after_it_ends() {
    let repo = edited_repo("status");
    let cache = repo.with_extension("cache");
    let _ = std::fs::remove_dir_all(&cache);
    let mut server = Server::start(&repo, &cache);
    let args = format!(
        r#"{{"package":{},"suiteCommand":"sleep 2; echo ok","confirm":false,"waitSeconds":0}}"#,
        json::stringify_compact(&Value::String(repo.display().to_string()))
    );
    let started = Instant::now();
    let first = server.call("mutate_diff", &args);
    assert!(started.elapsed() < Duration::from_secs(5), "the call held on: {:?}", started.elapsed());
    assert_eq!(text(&first, "status"), "running", "{first:?}");
    let job = text(&first, "job_id");
    assert!(job.starts_with("job-"), "{first:?}");
    assert!(text(&first, "next").contains("job_status"), "{first:?}");
    let progress = server.call("job_status", &format!(r#"{{"job_id":"{job}","waitSeconds":0}}"#));
    assert_eq!(text(&progress, "status"), "running");
    assert!(progress.get("progress").and_then(|p| p.get("phase")).is_some(), "{progress:?}");

    let done = server.call("job_status", &format!(r#"{{"job_id":"{job}","waitSeconds":120}}"#));
    assert_eq!(text(&done, "status"), "completed", "{done:?}");
    assert_eq!(text(&done, "command"), "mutate.diff");
    assert!(text(&done, "summary").starts_with("1 changed line in 1 file since HEAD: 1 mutant."), "{done:?}");
    assert_eq!(done.get("survivors").map(json::stringify_compact).unwrap_or_default().matches("\"line\":2").count(), 1, "{done:?}");
    let again = server.call("job_status", &format!(r#"{{"job_id":"{job}","waitSeconds":0}}"#));
    assert_eq!(text(&again, "summary"), text(&done, "summary"));
    drop(server);

    // A new server (a restarted session) reads the job's record.
    let mut fresh = Server::start(&repo, &cache);
    let later = fresh.call("job_status", &format!(r#"{{"job_id":"{job}","waitSeconds":0}}"#));
    assert_eq!(text(&later, "summary"), text(&done, "summary"), "{later:?}");
    let unknown = fresh.call("job_status", r#"{"job_id":"job-0","waitSeconds":0}"#);
    assert!(json::stringify_compact(&unknown).contains("unknown job_id"), "{unknown:?}");
    assert!(trees_in(&cache).is_empty(), "{:?}", trees_in(&cache));
    drop(fresh);
    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&cache);
}

#[test]
fn a_fast_diff_answers_with_its_result_in_the_same_call() {
    let repo = edited_repo("fast");
    let cache = repo.with_extension("cache");
    let mut server = Server::start(&repo, &cache);
    let args = format!(r#"{{"package":{},"suiteCommand":"echo ok","confirm":false}}"#, json::stringify_compact(&Value::String(repo.display().to_string())));
    let reply = server.call("mutate_diff", &args);
    assert_eq!(text(&reply, "status"), "completed", "{reply:?}");
    assert_eq!(text(&reply, "command"), "mutate.diff");
    drop(server);
    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&cache);
}

#[test]
fn cancel_job_stops_the_run_and_removes_its_mutant_tree() {
    let repo = edited_repo("cancel");
    let cache = repo.with_extension("cache");
    let _ = std::fs::remove_dir_all(&cache);
    let marker = repo.with_extension("finished");
    let _ = std::fs::remove_file(&marker);
    let mut server = Server::start(&repo, &cache);
    // The baseline (in the package) is quick; a mutant (in the cache's tree) takes long.
    let suite = format!(
        "case \"$PWD\" in *cache*) sleep 30; echo done > '{}';; esac; echo ok",
        marker.display().to_string().replace('\\', "/")
    );
    let args = format!(
        r#"{{"package":{},"suiteCommand":{},"confirm":false,"waitSeconds":0}}"#,
        json::stringify_compact(&Value::String(repo.display().to_string())),
        json::stringify_compact(&Value::String(suite))
    );
    let first = server.call("mutate_diff", &args);
    let job = text(&first, "job_id");
    assert!(job.starts_with("job-"), "{first:?}");
    let started = Instant::now();
    while trees_in(&cache).is_empty() {
        assert!(started.elapsed() < Duration::from_secs(40), "no mutant tree was made");
        std::thread::sleep(Duration::from_millis(200));
    }
    std::thread::sleep(Duration::from_millis(1500));
    let worktrees = git(&repo, &["worktree", "list", "--porcelain"]).matches("worktree ").count();
    assert_eq!(worktrees, 2, "the mutant tree is a worktree while it runs");

    let cancelled = server.call("cancel_job", &format!(r#"{{"job_id":"{job}"}}"#));
    assert_eq!(text(&cancelled, "status"), "cancelled", "{cancelled:?}");
    assert!(text(&cancelled, "summary").contains("1 mutant tree removed"), "{cancelled:?}");
    assert!(trees_in(&cache).is_empty(), "{:?}", trees_in(&cache));
    let worktrees = git(&repo, &["worktree", "list", "--porcelain"]).matches("worktree ").count();
    assert_eq!(worktrees, 1, "the worktree is still registered");
    let status = server.call("job_status", &format!(r#"{{"job_id":"{job}","waitSeconds":0}}"#));
    assert_eq!(text(&status, "status"), "cancelled");
    drop(server);
    std::thread::sleep(Duration::from_secs(2));
    assert!(!marker.exists(), "the suite kept running after the cancel");
    let source = std::fs::read_to_string(repo.join("src/A.java")).unwrap();
    assert!(source.contains("n > 0"), "the package was edited: {source}");
    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&cache);
}
