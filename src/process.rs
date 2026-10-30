//! Run a child in its own process group and kill the group when the deadline hits.
//! A grandchild that ignores SIGTERM dies with the group.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use std::os::unix::process::CommandExt;

/// How long after the deadline the recorded duration may still run while the group is killed.
pub const KILL_GRACE: Duration = Duration::from_millis(1_500);

const SIGKILL: i32 = 9;

unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
    fn setpgid(pid: i32, pgid: i32) -> i32;
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
    run_group_with_path(program, args, dir, timeout, None)
}

fn run_group_with_path(
    program: &str,
    args: &[String],
    dir: Option<&Path>,
    timeout: Duration,
    path: Option<OsString>,
) -> TimedRun {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
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
    let pid = child.id() as i32;
    unsafe {
        setpgid(pid, pid);
    }
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let stdout_handle = thread::spawn(move || read_pipe(stdout.take()));
    let stderr_handle = thread::spawn(move || read_pipe(stderr.take()));
    let mut timed_out = false;
    let status = loop {
        if start.elapsed() >= timeout {
            timed_out = true;
            unsafe {
                kill(-pid, SIGKILL);
            }
            let _ = child.wait();
            break None;
        }
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(error) => {
                return TimedRun {
                    spawned: true,
                    status: None,
                    timed_out: false,
                    duration: start.elapsed(),
                    stdout: String::new(),
                    stderr: error.to_string(),
                };
            }
        }
    };
    let duration = start.elapsed();
    TimedRun {
        spawned: true,
        status,
        timed_out,
        duration,
        stdout: stdout_handle.join().unwrap_or_default(),
        stderr: stderr_handle.join().unwrap_or_default(),
    }
}

fn read_pipe(pipe: Option<impl Read>) -> String {
    let mut pipe = match pipe {
        Some(pipe) => pipe,
        None => return String::new(),
    };
    let mut buffer = Vec::new();
    let _ = pipe.read_to_end(&mut buffer);
    String::from_utf8_lossy(&buffer).into_owned()
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
    let path = suite_path(bin_root, dir);
    run_group_with_path("sh", &["-c".into(), script.into()], dir, timeout, path)
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
    let mut prefixed = OsString::new();
    for (index, bin) in bins.iter().enumerate() {
        if index > 0 {
            prefixed.push(":");
        }
        prefixed.push(bin.as_os_str());
    }
    prefixed.push(":");
    if let Some(current) = std::env::var_os("PATH") {
        prefixed.push(current);
    }
    Some(prefixed)
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

    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }

    #[test]
    fn deadline_kills_a_grandchild_that_ignores_sigterm() {
        let dir = std::env::temp_dir().join(format!("probatio-kill-{}", std::process::id()));
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

    #[test]
    fn a_local_node_bin_is_on_the_suite_path() {
        let dir = std::env::temp_dir().join(format!("probatio-bin-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let bin = dir.join("node_modules/.bin");
        fs::create_dir_all(&bin).unwrap();
        let tool = bin.join("probatio-local-ok");
        fs::write(&tool, "#!/bin/sh\necho from-local-bin\n").unwrap();
        let mut permissions = fs::metadata(&tool).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o755);
        fs::set_permissions(&tool, permissions).unwrap();
        let result = run_shell("probatio-local-ok", Some(&dir), Duration::from_secs(5));
        assert!(result.spawned, "{}", result.stderr);
        assert_eq!(result.status, Some(0), "{}", result.stderr);
        assert!(result.stdout.contains("from-local-bin"), "{}", result.stdout);
        let work = std::env::temp_dir().join(format!("probatio-binwork-{}", std::process::id()));
        let _ = fs::remove_dir_all(&work);
        fs::create_dir_all(&work).unwrap();
        let from_package = run_shell_with_bin_root("probatio-local-ok", Some(&work), Some(&dir), Duration::from_secs(5));
        assert_eq!(from_package.status, Some(0), "{}", from_package.stderr);
        assert!(from_package.stdout.contains("from-local-bin"), "{}", from_package.stdout);
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&work);
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
}
