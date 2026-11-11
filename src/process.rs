//! Run a child in its own process group and kill the group when the deadline hits.
//! A grandchild that ignores SIGTERM dies with the group.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

/// How long after the deadline the recorded duration may still run while the group is killed.
pub const KILL_GRACE: Duration = Duration::from_millis(1_500);

#[cfg(unix)]
const SIGKILL: i32 = 9;

#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
    fn setpgid(pid: i32, pgid: i32) -> i32;
    fn getppid() -> i32;
}

/// Suite processes started by this process and still running. Each leads its own process group.
static LIVE: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// Removes a suite pid from `LIVE` on every return path.
struct Tracked(u32);

impl Tracked {
    fn new(pid: u32) -> Tracked {
        LIVE.lock().unwrap_or_else(|poison| poison.into_inner()).push(pid);
        Tracked(pid)
    }
}

impl Drop for Tracked {
    fn drop(&mut self) {
        LIVE.lock().unwrap_or_else(|poison| poison.into_inner()).retain(|pid| *pid != self.0);
    }
}

/// The environment variable the MCP server sets on each call it starts: its own pid.
pub const PARENT_ENV: &str = "PROBATIO_PARENT_PID";

/// Unix: when `PROBATIO_PARENT_PID` names this process's parent, a thread checks every 200 ms that
/// the parent is still there. When it is gone (the MCP server was killed), every suite this process
/// started is killed with its group, and this process exits. A suite runs in its own process group
/// for its deadline, so the parent's death does not reach it otherwise. Windows uses a job object.
pub fn watch_parent() {
    #[cfg(unix)]
    {
        let Some(expected) = std::env::var(PARENT_ENV).ok().and_then(|text| text.parse::<i32>().ok()) else { return };
        if unsafe { getppid() } != expected {
            return;
        }
        thread::spawn(move || loop {
            thread::sleep(Duration::from_millis(200));
            if unsafe { getppid() } != expected {
                let pids: Vec<u32> = LIVE.lock().unwrap_or_else(|poison| poison.into_inner()).clone();
                for pid in pids {
                    kill_tree(pid);
                }
                std::process::exit(1);
            }
        });
    }
}

/// Windows: put this process in a job object that kills every member when its last handle closes.
/// Children join the job when they start, so killing this process alone, which closes the handle,
/// also ends every call it started and every suite under them. The handle is kept for the life
/// of the process on purpose. Unix uses `watch_parent` in each child instead.
pub fn contain_descendants() {
    #[cfg(windows)]
    windows_job::contain();
}

#[cfg(windows)]
mod windows_job {
    use std::ffi::c_void;

    type Handle = *mut c_void;

    #[repr(C)]
    #[derive(Default)]
    struct BasicLimit {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct ExtendedLimit {
        basic: BasicLimit,
        io: [u64; 6],
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    const EXTENDED_LIMIT_INFORMATION: i32 = 9;
    const KILL_ON_JOB_CLOSE: u32 = 0x2000;

    extern "system" {
        fn CreateJobObjectW(attributes: *mut c_void, name: *const u16) -> Handle;
        fn SetInformationJobObject(job: Handle, class: i32, info: *mut c_void, length: u32) -> i32;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
        fn GetCurrentProcess() -> Handle;
        fn CloseHandle(handle: Handle) -> i32;
    }

    /// True when this process is now in a kill-on-close job. A failure leaves things as they were.
    pub fn contain() -> bool {
        unsafe {
            let job = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
            if job.is_null() {
                return false;
            }
            let mut limit = ExtendedLimit::default();
            limit.basic.limit_flags = KILL_ON_JOB_CLOSE;
            let size = std::mem::size_of::<ExtendedLimit>() as u32;
            let set = SetInformationJobObject(job, EXTENDED_LIMIT_INFORMATION, &mut limit as *mut ExtendedLimit as *mut c_void, size);
            if set == 0 || AssignProcessToJobObject(job, GetCurrentProcess()) == 0 {
                CloseHandle(job);
                return false;
            }
            // Not closed: the job lives exactly as long as this process.
            true
        }
    }
}

#[derive(Clone, Debug)]
pub struct TimedRun {
    pub spawned: bool,
    pub status: Option<i32>,
    pub timed_out: bool,
    pub duration: Duration,
    pub stdout: String,
    pub stderr: String,
}

pub fn run_group(program: &str, args: &[String], dir: Option<&Path>, timeout: Duration) -> TimedRun {
    run_group_with_path(program, args, dir, timeout, None, &[], &[])
}

/// Argv launch. `env` is added to the inherited environment. It is not a replacement.
/// `bin_root` and `dir` supply `node_modules/.bin` the same way a shell suite does.
pub fn run_argv(
    program: &str,
    args: &[String],
    dir: Option<&Path>,
    bin_root: Option<&Path>,
    timeout: Duration,
    env: &[(&str, &str)],
) -> TimedRun {
    run_argv_prepared(program, args, dir, bin_root, timeout, env, &[])
}

pub fn run_argv_prepared(
    program: &str,
    args: &[String],
    dir: Option<&Path>,
    bin_root: Option<&Path>,
    timeout: Duration,
    env: &[(&str, &str)],
    unset: &[&str],
) -> TimedRun {
    let path = suite_path(bin_root, dir);
    run_group_with_path(program, args, dir, timeout, path, env, unset)
}

/// Drop the variables a nested suite must not inherit, then the caller's `--unset` keys.
fn scrub_inherited(command: &mut Command, unset: &[&str]) {
    let inherited: Vec<String> = std::env::vars()
        .filter_map(|(key, _)| {
            if key == "NODE_TEST_CONTEXT"
                || key.starts_with("NODE_TEST_")
                || key == "PROBATIO_COVERAGE_MAP"
                || key == "PROBATIO_PACKAGE_ROOT"
                || key == "PROBATIO_CHILD_DIR"
            {
                Some(key)
            } else {
                None
            }
        })
        .collect();
    for key in inherited {
        command.env_remove(key);
    }
    for key in unset {
        command.env_remove(key);
    }
    if let Ok(options) = std::env::var("NODE_OPTIONS") {
        let kept = without_child_lines(&options);
        if kept.is_empty() {
            command.env_remove("NODE_OPTIONS");
        } else if kept != options {
            command.env("NODE_OPTIONS", kept);
        }
    }
}

/// Drop the coverage hook from `NODE_OPTIONS`, with the `--import` or `--require` flag that loads it.
/// Leaving that flag behind would make it swallow the next option, or fail node on an empty value.
fn without_child_lines(options: &str) -> String {
    let parts: Vec<&str> = options.split_whitespace().collect();
    let mut kept: Vec<&str> = Vec::new();
    let mut index = 0;
    while index < parts.len() {
        let part = parts[index];
        let loader = matches!(part, "--import" | "--require" | "-r" | "--loader" | "--experimental-loader");
        if loader && parts.get(index + 1).is_some_and(|next| next.ends_with("child-lines.mjs")) {
            index += 2;
            continue;
        }
        if !part.ends_with("child-lines.mjs") {
            kept.push(part);
        }
        index += 1;
    }
    kept.join(" ")
}

fn run_group_with_path(
    program: &str,
    args: &[String],
    dir: Option<&Path>,
    timeout: Duration,
    path: Option<OsString>,
    env: &[(&str, &str)],
    unset: &[&str],
) -> TimedRun {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    command.process_group(0);
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    for (key, value) in env {
        command.env(key, value);
    }
    scrub_inherited(&mut command, unset);
    if let Some(path) = path {
        command.env("PATH", path);
    }
    let start = Instant::now();
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return TimedRun {
                spawned: false,
                status: None,
                timed_out: false,
                duration: start.elapsed(),
                stdout: String::new(),
                stderr: error.to_string(),
            };
        }
    };
    let pid = child.id();
    #[cfg(unix)]
    unsafe {
        setpgid(pid as i32, pid as i32);
    }
    let _tracked = Tracked::new(pid);
    let stdout_buf = Arc::new(Mutex::new(String::new()));
    let stderr_buf = Arc::new(Mutex::new(String::new()));
    let stdout_handle = spawn_drain(child.stdout.take(), Arc::clone(&stdout_buf));
    let stderr_handle = spawn_drain(child.stderr.take(), Arc::clone(&stderr_buf));
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) => {}
            Err(error) => {
                kill_tree(pid);
                let _ = child.kill();
                let _ = child.wait();
                drop(stdout_handle);
                drop(stderr_handle);
                return TimedRun {
                    spawned: true,
                    status: None,
                    timed_out: false,
                    duration: start.elapsed(),
                    stdout: snapshot(&stdout_buf),
                    stderr: error.to_string(),
                };
            }
        }
        if start.elapsed() >= timeout {
            // The child can exit during the 20ms sleep. Killing it then, and
            // throwing away the pipes, reported a finished suite as a timeout.
            if let Ok(Some(status)) = child.try_wait() {
                break status.code();
            }
            // Joining the readers waits until every grandchild closes the pipes.
            // Git Bash `sleep` survives `taskkill /T` and would hold that join
            // for the rest of the sleep. Keep whatever was already read.
            kill_tree(pid);
            let _ = child.kill();
            let _ = child.wait();
            let stdout = snapshot(&stdout_buf);
            let stderr = snapshot(&stderr_buf);
            drop(stdout_handle);
            drop(stderr_handle);
            return TimedRun {
                spawned: true,
                status: None,
                timed_out: true,
                duration: start.elapsed(),
                stdout,
                stderr,
            };
        }
        thread::sleep(Duration::from_millis(20));
    };
    let duration = start.elapsed();
    let _ = stdout_handle.join();
    let _ = stderr_handle.join();
    TimedRun {
        spawned: true,
        status,
        timed_out: false,
        duration,
        stdout: snapshot(&stdout_buf),
        stderr: snapshot(&stderr_buf),
    }
}

/// Stop `pid` and every process it started. Unix signals the process group.
/// Windows `TerminateProcess` does not, so `taskkill /T` is what closes the pipes.
pub fn kill_tree(pid: u32) {
    #[cfg(unix)]
    unsafe {
        let _ = kill(-(pid as i32), SIGKILL);
        let _ = kill(pid as i32, SIGKILL);
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

/// Stop `pid` and every descendant, including ones in their own process group.
/// A suite runs in its own group so its deadline can kill it, which `kill_tree` on its parent
/// does not reach on Unix. The MCP server uses this when a call is cancelled or the client leaves.
pub fn kill_descendants(pid: u32) {
    #[cfg(unix)]
    {
        let table = Command::new("ps")
            .args(["-A", "-o", "pid=,ppid="])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
            .unwrap_or_default();
        let pairs: Vec<(u32, u32)> = table
            .lines()
            .filter_map(|line| {
                let mut fields = line.split_whitespace().filter_map(|field| field.parse::<u32>().ok());
                Some((fields.next()?, fields.next()?))
            })
            .collect();
        let mut tree = vec![pid];
        let mut index = 0;
        while index < tree.len() {
            let parent = tree[index];
            tree.extend(pairs.iter().filter(|(_, ppid)| *ppid == parent).map(|(child, _)| *child));
            index += 1;
        }
        // Children first is not needed with SIGKILL. Each group goes too, for grandchildren
        // that were already reparented.
        for member in tree {
            kill_tree(member);
        }
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

fn spawn_drain(pipe: Option<impl Read + Send + 'static>, into: Arc<Mutex<String>>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let Some(mut pipe) = pipe else { return };
        let mut chunk = [0u8; 8192];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let text = String::from_utf8_lossy(&chunk[..n]);
                    into.lock().unwrap_or_else(|poison| poison.into_inner()).push_str(&text);
                }
            }
        }
    })
}

fn snapshot(buf: &Mutex<String>) -> String {
    buf.lock().unwrap_or_else(|poison| poison.into_inner()).clone()
}

/// Shell a user suite command. The shell is the group leader, so its children die with it.
/// `dir`'s `node_modules/.bin` is prepended to that process's `PATH`.
/// The command string is not rewritten, and the rest of the environment stays.
pub fn run_shell(script: &str, dir: Option<&Path>, timeout: Duration) -> TimedRun {
    run_shell_with_bin_root(script, dir, dir, timeout)
}

/// Like `run_shell`, but `bin_root` supplies `node_modules/.bin` when `dir` is a worktree
/// or copy that skips that directory. The package directory comes first.
pub fn run_shell_with_bin_root(
    script: &str,
    dir: Option<&Path>,
    bin_root: Option<&Path>,
    timeout: Duration,
) -> TimedRun {
    run_shell_with_env(script, dir, bin_root, timeout, &[])
}

pub fn run_shell_with_env(
    script: &str,
    dir: Option<&Path>,
    bin_root: Option<&Path>,
    timeout: Duration,
    env: &[(&str, &str)],
) -> TimedRun {
    run_shell_prepared(script, dir, bin_root, timeout, env, &[])
}

pub fn run_shell_prepared(
    script: &str,
    dir: Option<&Path>,
    bin_root: Option<&Path>,
    timeout: Duration,
    env: &[(&str, &str)],
    unset: &[&str],
) -> TimedRun {
    let path = suite_path(bin_root, dir);
    let (program, args, script_env) = shell_argv(script);
    let mut env: Vec<(&str, &str)> = env.to_vec();
    if let Some(value) = &script_env {
        env.push((SCRIPT_ENV, value.as_str()));
    }
    run_group_with_path(&program, &args, dir, timeout, path, &env, unset)
}

/// Windows: the suite command reaches Git's `sh` through this variable, not the command line.
pub const SCRIPT_ENV: &str = "PROBATIO_SUITE_SCRIPT";

/// Windows: what `sh -c` runs. The MSYS runtime parses a Windows command line with its own
/// backslash rules before `sh` sees it, so a script passed as an argument lost a level of
/// backslashes there and another in `sh`. An environment value is not parsed, so the script
/// arrives as written and `sh` reads it once, the way it does on Linux and macOS.
#[cfg(windows)]
const EVAL_SCRIPT: &str = "probatio_script=$PROBATIO_SUITE_SCRIPT; unset PROBATIO_SUITE_SCRIPT; eval \"$probatio_script\"";

/// The POSIX shell, its arguments, and on Windows the script to pass in `SCRIPT_ENV`.
/// Windows has no `sh` on the default `PATH`, so Git for Windows' shell is found from `git.exe`.
/// With no Git shell at all, `cmd /C` runs the command as written.
pub fn shell_argv(script: &str) -> (String, Vec<String>, Option<String>) {
    #[cfg(windows)]
    {
        if let Some(sh) = windows_sh() {
            return (sh.display().to_string(), vec!["-c".into(), EVAL_SCRIPT.into()], Some(keep_windows_backslashes(script)));
        }
        ("cmd".into(), vec!["/D".into(), "/S".into(), "/C".into(), script.into()], None)
    }
    #[cfg(not(windows))]
    {
        ("sh".into(), vec!["-c".into(), script.into()], None)
    }
}

/// A Windows path written without quotes in a suite command, such as `--target-dir C:\x\target`,
/// reaches `sh`, which reads each `\` outside quotes as an escape and drops it (`C:xtarget`).
/// On Windows such a backslash is a path separator, so it is doubled before `sh` sees it. A
/// backslash that escapes a character `sh` gives meaning to (a space, a quote, `$`, `\` itself, an
/// operator) is left alone, and so is everything inside quotes: `sh` keeps a backslash in double
/// quotes before an ordinary character, and single quotes keep everything.
pub fn keep_windows_backslashes(script: &str) -> String {
    let chars: Vec<char> = script.chars().collect();
    let mut out = String::with_capacity(script.len() + 8);
    let mut single = false;
    let mut double = false;
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        if single {
            if ch == '\'' {
                single = false;
            }
            out.push(ch);
            index += 1;
            continue;
        }
        if ch == '\\' {
            let next = chars.get(index + 1).copied();
            let path_char = next.is_some_and(|next| next.is_ascii_alphanumeric() || matches!(next, '.' | '_' | '-' | '+' | ',' | '@' | '%' | ':'));
            if !double && path_char {
                out.push_str("\\\\");
                index += 1;
                continue;
            }
            // An escape `sh` reads (or any backslash inside double quotes, which `sh` keeps
            // before an ordinary character): the pair goes through untouched.
            out.push(ch);
            if let Some(next) = next {
                out.push(next);
                index += 1;
            }
            index += 1;
            continue;
        }
        if ch == '\'' && !double {
            single = true;
        } else if ch == '"' {
            double = !double;
        }
        out.push(ch);
        index += 1;
    }
    out
}

/// True while a process with this id is running. An id can be reused after its process exits, so
/// a stale lock can read as live until that id is free again; that only delays a cleanup.
pub fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    if pid == std::process::id() {
        return true;
    }
    #[cfg(unix)]
    {
        // Signal 0 checks existence. EPERM (another user's process) still means it exists.
        let result = unsafe { kill(pid as i32, 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(1)
    }
    #[cfg(windows)]
    {
        use std::ffi::c_void;
        extern "system" {
            fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
            fn GetExitCodeProcess(process: *mut c_void, code: *mut u32) -> i32;
            fn CloseHandle(handle: *mut c_void) -> i32;
        }
        const QUERY_LIMITED_INFORMATION: u32 = 0x1000;
        const STILL_ACTIVE: u32 = 259;
        unsafe {
            let handle = OpenProcess(QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                // Access denied (5) still means the process exists. A free id is error 87.
                return std::io::Error::last_os_error().raw_os_error() == Some(5);
            }
            let mut code = 0u32;
            let ok = GetExitCodeProcess(handle, &mut code);
            CloseHandle(handle);
            ok != 0 && code == STILL_ACTIVE
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        true
    }
}

/// Git for Windows' `sh.exe`. `bin/sh.exe` is preferred over `usr/bin/sh.exe`, because the wrapper
/// puts Git's own tools on `PATH` before it runs the command. `System32\bash.exe` is WSL and is skipped.
#[cfg(windows)]
pub fn windows_sh() -> Option<PathBuf> {
    static FOUND: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    FOUND
        .get_or_init(|| {
            // Under Git Bash, `sh` is already on PATH with Git's tools beside it, and skips the wrapper.
            if let Some(sh) = find_on_path("sh").filter(|path| !is_windows_system_dir(path)) {
                return Some(sh);
            }
            let mut roots: Vec<PathBuf> = Vec::new();
            if let Some(bash) = std::env::var_os("CLAUDE_CODE_GIT_BASH_PATH").map(PathBuf::from) {
                roots.extend(bash.ancestors().skip(1).take(3).map(Path::to_path_buf));
            }
            if let Some(git) = find_on_path("git") {
                roots.extend(git.ancestors().skip(1).take(3).map(Path::to_path_buf));
            }
            for key in ["ProgramW6432", "ProgramFiles", "ProgramFiles(x86)"] {
                if let Some(base) = std::env::var_os(key) {
                    roots.push(PathBuf::from(base).join("Git"));
                }
            }
            if let Some(base) = std::env::var_os("LOCALAPPDATA") {
                roots.push(PathBuf::from(base).join("Programs").join("Git"));
            }
            for root in &roots {
                for rel in ["bin/sh.exe", "usr/bin/sh.exe"] {
                    let candidate = root.join(rel);
                    if candidate.is_file() {
                        return Some(candidate);
                    }
                }
            }
            None
        })
        .clone()
}

#[cfg(windows)]
fn is_windows_system_dir(path: &Path) -> bool {
    let text = path.display().to_string().to_ascii_lowercase();
    text.contains("\\windows\\system32\\") || text.contains("\\windowsapps\\")
}

/// The first `name` on `PATH`. On Windows each `PATHEXT` suffix is tried, as a shell would.
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let suffixes: Vec<String> = if cfg!(windows) && Path::new(name).extension().is_none() {
        let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        pathext.split(';').filter(|ext| !ext.is_empty()).map(str::to_ascii_lowercase).collect()
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(&path) {
        for suffix in &suffixes {
            let candidate = dir.join(format!("{name}{suffix}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn suite_path(bin_root: Option<&Path>, dir: Option<&Path>) -> Option<OsString> {
    let mut bins = Vec::new();
    for root in [bin_root, dir].into_iter().flatten() {
        let bin = root.join("node_modules/.bin");
        if bin.is_dir() && !bins.iter().any(|seen: &PathBuf| seen == &bin) {
            bins.push(bin);
        }
    }
    if bins.is_empty() {
        return None;
    }
    if let Some(current) = std::env::var_os("PATH") {
        bins.extend(std::env::split_paths(&current));
    }
    // The separator is the platform's: `;` on Windows, where `:` is part of `C:\`.
    std::env::join_paths(bins).ok()
}

pub fn within_grace(duration: Duration, timeout: Duration) -> bool {
    duration <= timeout + KILL_GRACE
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command as Proc;

    #[cfg(unix)]
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }

    #[cfg(unix)]
    #[test]
    fn deadline_kills_a_grandchild_that_ignores_sigterm() {
        let dir = crate::testdir::TempDir::new("kill");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let pid_path = dir.join("grand.pid");
        let script = format!(
            "trap '' TERM; (trap '' TERM; echo $$ > '{}'; exec sleep 60)",
            pid_path.display()
        );
        let timeout = Duration::from_millis(400);
        let result = run_shell(&script, Some(&dir), timeout);
        assert!(result.spawned, "{}", result.stderr);
        assert!(result.timed_out);
        assert!(within_grace(result.duration, timeout), "duration {:?}", result.duration);
        let pid = wait_for_pid(&pid_path);
        let still_alive = unsafe { kill(pid, 0) } == 0;
        assert!(!still_alive, "grandchild {pid} survived the deadline");
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    fn wait_for_pid(path: &PathBuf) -> i32 {
        let start = Instant::now();
        loop {
            if let Ok(text) = fs::read_to_string(path) {
                if let Ok(pid) = text.trim().parse::<i32>() {
                    return pid;
                }
            }
            if start.elapsed() > Duration::from_secs(2) {
                panic!("grandchild pid was not written");
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Windows runs this too: a `:`-joined `PATH` there split `C:\...` and lost every entry.
    #[test]
    fn a_local_node_bin_is_on_the_suite_path() {
        let dir = crate::testdir::TempDir::new("bin");
        let _ = fs::remove_dir_all(&dir);
        let bin = dir.join("node_modules/.bin");
        fs::create_dir_all(&bin).unwrap();
        let tool = bin.join("probatio-local-ok");
        fs::write(&tool, "#!/bin/sh\necho from-local-bin\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&tool).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&tool, permissions).unwrap();
        }
        let prefixed = suite_path(Some(&dir), None).unwrap();
        let entries: Vec<PathBuf> = std::env::split_paths(&prefixed).collect();
        assert_eq!(entries[0], bin);
        assert!(entries.len() > 1, "{entries:?}");
        let result = run_shell("probatio-local-ok", Some(&dir), Duration::from_secs(5));
        assert!(result.spawned, "{}", result.stderr);
        assert_eq!(result.status, Some(0), "{}", result.stderr);
        assert!(result.stdout.contains("from-local-bin"), "{}", result.stdout);
        let work = crate::testdir::TempDir::new("binwork");
        let _ = fs::remove_dir_all(&work);
        fs::create_dir_all(&work).unwrap();
        let from_package = run_shell_with_bin_root("probatio-local-ok", Some(&work), Some(&dir), Duration::from_secs(5));
        assert_eq!(from_package.status, Some(0), "{}", from_package.stderr);
        assert!(from_package.stdout.contains("from-local-bin"), "{}", from_package.stdout);
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&work);
    }

    #[test]
    fn the_coverage_hook_leaves_with_its_loader_flag() {
        assert_eq!(without_child_lines("--import /tmp/child-lines.mjs --max-old-space-size=64"), "--max-old-space-size=64");
        assert_eq!(without_child_lines("--import=/tmp/child-lines.mjs -r x.js"), "-r x.js");
        assert_eq!(without_child_lines("--require ./child-lines.mjs"), "");
        assert_eq!(without_child_lines("--inspect"), "--inspect");
    }

    /// The suite shell is a POSIX `sh` on every platform. On Windows it is Git's, found from
    /// `git.exe` even when `sh` is not on `PATH`, and never WSL's `System32\bash.exe`.
    #[test]
    fn the_suite_shell_is_posix() {
        let (program, args, script) = shell_argv("echo $((1 + 2))");
        assert_eq!(script.as_deref().or(args.last().map(String::as_str)), Some("echo $((1 + 2))"));
        if cfg!(windows) && program == "cmd" {
            return;
        }
        assert!(!program.to_ascii_lowercase().contains("system32"), "{program}");
        let result = run_shell("echo $((1 + 2))", None, Duration::from_secs(10));
        assert_eq!(result.stdout.trim(), "3", "{program}: {}", result.stderr);
    }

    /// Specula M5: `cargo test --target-dir C:\x\target` lost its backslashes in `sh`.
    #[test]
    fn a_windows_path_keeps_its_backslashes() {
        assert_eq!(keep_windows_backslashes(r"cargo test --target-dir C:\x\target"), r"cargo test --target-dir C:\\x\\target");
        let left = r#"echo "C:\x" 'C:\y' a\ b \$HOME \\ \"q\""#;
        assert_eq!(keep_windows_backslashes(left), left);
        if cfg!(windows) && shell_argv("x").0 != "cmd" {
            let dir = std::env::temp_dir();
            let result = run_shell(r"printf '%s|' C:\Users\x\target --target-dir=D:\a\b", Some(&dir), Duration::from_secs(10));
            assert_eq!(result.stdout, r"C:\Users\x\target|--target-dir=D:\a\b|", "{}", result.stderr);
        }
    }

    #[test]
    fn this_process_is_alive_and_a_finished_child_is_not() {
        assert!(pid_alive(std::process::id()));
        assert!(!pid_alive(0));
        let mut child = if cfg!(windows) { Proc::new("cmd").args(["/C", "exit"]).spawn().unwrap() } else { Proc::new("true").spawn().unwrap() };
        let pid = child.id();
        child.wait().unwrap();
        assert!(!pid_alive(pid), "pid {pid} still reads alive after it exited");
    }

    #[test]
    fn a_fast_command_is_not_a_timeout() {
        let result = run_shell("echo ok", None, Duration::from_secs(2));
        assert!(result.spawned);
        assert!(!result.timed_out);
        assert_eq!(result.status, Some(0));
        assert!(result.stdout.contains("ok"));
        let _ = Proc::new("true").status();
    }

    static NODE_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn node_options_drops_the_coverage_hook() {
        let _guard = NODE_ENV.lock().unwrap_or_else(|poison| poison.into_inner());
        std::env::set_var("NODE_OPTIONS", "--import /tmp/child-lines.mjs --max-old-space-size=64");
        std::env::set_var("NODE_TEST_FOO", "1");
        let timed = run_shell("printf '%s' \"$NODE_OPTIONS|$NODE_TEST_FOO\"", None, Duration::from_secs(5));
        std::env::remove_var("NODE_OPTIONS");
        std::env::remove_var("NODE_TEST_FOO");
        assert!(timed.spawned, "{}", timed.stderr);
        assert_eq!(timed.stdout, "--max-old-space-size=64|", "{}", timed.stderr);
        assert!(!timed.stdout.contains("child-lines"));
    }
}
