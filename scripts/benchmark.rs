//! Time TypeScript Probatio (c5cd175) against this Rust build. Std only.
//!
//! Five runs per command. The report is median milliseconds and the min-max.
//! Peak RSS is the highest VmHWM seen while the process and its children ran (Linux `/proc`).
//! On Windows it is the child's peak working set. Elsewhere it is 0.
//! Nothing here is estimated. A side that cannot run is recorded as such.
//!
//! Usage:
//!   cargo build --release && cargo run --release --example benchmark
//!
//! Environment:
//!   RUST_BIN   release binary (default: ./target/release/probatio)
//!   TS_NODE    node binary (default: node)
//!   TS_CLI     compiled TypeScript cli.js (default: <temp>/probatio-ts/dist/cli.js)
//!   BENCH_OSS  a real project to generate and run (default: <temp>/itsdangerous)
//!   BENCH_OUT  report path (default: scripts/benchmark_results.md)
//!
//! The samples and out dirs live under the system temp dir (`/tmp` on Linux and macOS).

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const RUNS: usize = 5;

struct Setup {
    root: PathBuf,
    rust: PathBuf,
    node: String,
    ts: PathBuf,
    oss: PathBuf,
    out: PathBuf,
    tmp: PathBuf,
}

fn env_path(key: &str, default: PathBuf) -> PathBuf {
    std::env::var_os(key).map(PathBuf::from).unwrap_or(default)
}

fn setup() -> Setup {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let tmp = std::env::temp_dir();
    let exe = if cfg!(windows) { "probatio.exe" } else { "probatio" };
    Setup {
        rust: env_path("RUST_BIN", root.join("target").join("release").join(exe)),
        node: std::env::var("TS_NODE").unwrap_or_else(|_| "node".into()),
        ts: env_path("TS_CLI", tmp.join("probatio-ts").join("dist").join("cli.js")),
        oss: env_path("BENCH_OSS", tmp.join("itsdangerous")),
        out: env_path("BENCH_OUT", root.join("scripts").join("benchmark_results.md")),
        root,
        tmp,
    }
}

fn first_line_value(path: &str, prefix: &str) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let line = text.lines().find(|line| line.starts_with(prefix))?;
    Some(line.split_once(':')?.1.trim().to_string())
}

fn machine(setup: &Setup) -> String {
    let cpu = first_line_value("/proc/cpuinfo", "model name").unwrap_or_else(|| "unknown".into());
    let mem = first_line_value("/proc/meminfo", "MemTotal").unwrap_or_else(|| "unknown".into());
    // Git Bash has a `uname` on Windows that names the MSYS runtime, so Windows asks `ver`.
    let release = if cfg!(windows) {
        command_text("cmd", &["/C", "ver"]).map(|text| text.trim_start_matches("Microsoft Windows").trim().to_string())
    } else {
        command_text("uname", &["-r"])
    }
    .unwrap_or_else(|| "unknown".into());
    let cores = thread::available_parallelism().map(|count| count.get()).unwrap_or(0);
    format!(
        "{} {release} {}, {cores} cores, {cpu}, {mem}. Rust binary {}. TypeScript cli {} under {}.",
        os_name(),
        std::env::consts::ARCH,
        setup.rust.display(),
        setup.ts.display(),
        setup.node
    )
}

fn os_name() -> &'static str {
    match std::env::consts::OS {
        "linux" => "Linux",
        "macos" => "Darwin",
        "windows" => "Windows",
        other => other,
    }
}

fn command_text(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Highest VmHWM, in kilobytes, of this pid and its children.
fn peak_rss(pid: u32) -> u64 {
    let mut best = 0;
    let mut seen = Vec::new();
    let mut stack = vec![pid];
    while let Some(current) = stack.pop() {
        if seen.contains(&current) {
            continue;
        }
        seen.push(current);
        let Ok(text) = fs::read_to_string(format!("/proc/{current}/status")) else { continue };
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("VmHWM:") {
                if let Some(kb) = rest.split_whitespace().next().and_then(|value| value.parse::<u64>().ok()) {
                    best = best.max(kb);
                }
            }
        }
        let kids = fs::read_to_string(format!("/proc/{current}/task/{current}/children")).unwrap_or_default();
        stack.extend(kids.split_whitespace().filter_map(|item| item.parse::<u32>().ok()));
    }
    best
}

#[cfg(windows)]
fn windows_peak_kb(child: &std::process::Child) -> u64 {
    use std::os::windows::io::AsRawHandle;
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }
    extern "system" {
        fn K32GetProcessMemoryInfo(process: *mut std::ffi::c_void, counters: *mut Counters, cb: u32) -> i32;
    }
    let mut counters = Counters { cb: std::mem::size_of::<Counters>() as u32, ..Counters::default() };
    let ok = unsafe { K32GetProcessMemoryInfo(child.as_raw_handle(), &mut counters, counters.cb) };
    if ok == 0 {
        0
    } else {
        (counters.peak_working_set_size / 1024) as u64
    }
}

#[cfg(not(windows))]
fn windows_peak_kb(_child: &std::process::Child) -> u64 {
    0
}

/// Exit code, elapsed milliseconds, peak RSS in kB, and stdout followed by stderr.
fn time_command(argv: &[String], cwd: &Path, env: &[(String, String)]) -> (i32, f64, u64, String) {
    let start = Instant::now();
    let mut command = Command::new(&argv[0]);
    command.args(&argv[1..]).current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (key, value) in env {
        command.env(key, value);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return (127, start.elapsed().as_secs_f64() * 1000.0, 0, error.to_string()),
    };
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut pipe) = pipe {
                let mut bytes = Vec::new();
                let _ = pipe.read_to_end(&mut bytes);
                text = String::from_utf8_lossy(&bytes).into_owned();
            }
            text
        })
    };
    let out = drain(child.stdout.take().map(|pipe| Box::new(pipe) as Box<dyn Read + Send>));
    let err = drain(child.stderr.take().map(|pipe| Box::new(pipe) as Box<dyn Read + Send>));
    let mut rss = 0;
    let status = loop {
        if let Ok(Some(status)) = child.try_wait() {
            break status;
        }
        rss = rss.max(peak_rss(child.id()));
        thread::sleep(Duration::from_millis(10));
    };
    rss = rss.max(windows_peak_kb(&child));
    let elapsed = start.elapsed().as_secs_f64() * 1000.0;
    let text = format!("{}{}", out.join().unwrap_or_default(), err.join().unwrap_or_default());
    (status.code().unwrap_or(0), elapsed, rss, text)
}

fn median_row(samples: &[f64]) -> String {
    let mut ordered = samples.to_vec();
    ordered.sort_by(f64::total_cmp);
    let mid = if ordered.len() % 2 == 1 {
        ordered[ordered.len() / 2]
    } else {
        (ordered[ordered.len() / 2 - 1] + ordered[ordered.len() / 2]) / 2.0
    };
    format!("{:.0} ({:.0}-{:.0})", mid, ordered[0], ordered[ordered.len() - 1])
}

fn git(path: &Path, args: &[&str]) {
    let status = Command::new("git").args(args).current_dir(path).stdout(Stdio::null()).stderr(Stdio::null()).status();
    if !status.is_ok_and(|status| status.success()) {
        panic!("git {args:?} failed in {}", path.display());
    }
}

fn commit_sample(path: &Path) {
    if !path.join(".git").is_dir() {
        git(path, &["init"]);
        git(path, &["config", "user.email", "bench@example.com"]);
        git(path, &["config", "user.name", "bench"]);
    }
    git(path, &["add", "."]);
    git(path, &["commit", "-m", "sample", "--allow-empty"]);
}

fn write(path: &Path, text: &str) {
    fs::write(path, text).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
}

fn prepare_js(path: &Path) {
    fs::create_dir_all(path.join("src")).unwrap();
    fs::create_dir_all(path.join("test")).unwrap();
    write(&path.join("package.json"), "{\"scripts\":{\"test\":\"node --test\"}}\n");
    write(
        &path.join("src").join("calc.js"),
        "export function clamp(n, lo, hi) {\n  if (n < lo) return lo;\n  if (n > hi) return hi;\n  return n;\n}\n\
         export function adult(age) { return age >= 18 && age < 120; }\n\
         export function both(a, b) { return a && b; }\n",
    );
    write(
        &path.join("test").join("calc.test.js"),
        "import test from 'node:test';\nimport assert from 'node:assert/strict';\nimport { clamp, adult, both } from '../src/calc.js';\n\
         test('clamp lo', () => { assert.equal(clamp(1, 2, 5), 2); });\n\
         test('clamp hi', () => { assert.equal(clamp(9, 2, 5), 5); });\n\
         test('adult boundary', () => { assert.equal(adult(18), true); });\n\
         test('both', () => { assert.equal(both(true, false), false); });\n",
    );
    commit_sample(path);
}

fn prepare_py(path: &Path) {
    fs::create_dir_all(path.join("src")).unwrap();
    fs::create_dir_all(path.join("tests")).unwrap();
    write(
        &path.join("src").join("calc.py"),
        "def clamp(n, lo, hi):\n    if n < lo:\n        return lo\n    if n > hi:\n        return hi\n    return n\n\n\
         def adult(age):\n    return age >= 18 and age < 120\n\n\
         def both(a, b):\n    return a and b\n",
    );
    write(
        &path.join("tests").join("test_calc.py"),
        "import sys\nfrom pathlib import Path\nsys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'src'))\nimport calc\n\n\
         def test_clamp_lo():\n    assert calc.clamp(1, 2, 5) == 2\n\n\
         def test_clamp_hi():\n    assert calc.clamp(9, 2, 5) == 5\n\n\
         def test_adult():\n    assert calc.adult(18) is True\n\n\
         def test_both():\n    assert calc.both(True, False) is False\n",
    );
    write(&path.join("pytest.ini"), "[pytest]\n");
    commit_sample(path);
}

/// The timing cell, the peak RSS cell, and the last run's output.
fn repeat(argv: &[String], cwd: &Path, env: &[(String, String)]) -> (String, String, String) {
    let mut times = Vec::new();
    let mut rss = Vec::new();
    let mut last = String::new();
    let mut code = 0;
    for index in 0..RUNS {
        let fresh: Vec<String> = argv.iter().map(|item| item.replace("{run}", &index.to_string())).collect();
        let (exit, elapsed, peak, text) = time_command(&fresh, cwd, env);
        code = exit;
        times.push(elapsed);
        rss.push(peak);
        last = text;
    }
    let note = if code != 0 { format!(" last exit {code}") } else { String::new() };
    (median_row(&times) + &note, rss.iter().max().copied().unwrap_or(0).to_string(), last)
}

fn on_path(program: &str) -> bool {
    let candidate = Path::new(program);
    if candidate.components().count() > 1 {
        return candidate.is_file();
    }
    let Some(path) = std::env::var_os("PATH") else { return false };
    let suffixes: &[&str] = if cfg!(windows) { &["", ".exe", ".cmd", ".bat"] } else { &[""] };
    std::env::split_paths(&path).any(|dir| suffixes.iter().any(|suffix| dir.join(format!("{program}{suffix}")).is_file()))
}

struct Report {
    rows: Vec<[String; 5]>,
    notes: Vec<(String, String, String)>,
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| item.to_string()).collect()
}

fn main() {
    let setup = setup();
    let tmp = |name: &str| setup.tmp.join(name).display().to_string();
    let js = setup.tmp.join("probatio-bench-js");
    let py = setup.tmp.join("probatio-bench-py");
    prepare_js(&js);
    prepare_py(&py);
    let mut report = Report { rows: Vec::new(), notes: Vec::new() };
    let mut add = |name: &str, ts_argv: Option<Vec<String>>, rs_argv: Option<Vec<String>>, cwd: &Path, env: &[(String, String)]| {
        let (mut ts_cell, mut rs_cell) = ("not run".to_string(), "not run".to_string());
        let (mut ts_rss, mut rs_rss, mut ts_out, mut rs_out) = (String::new(), String::new(), String::new(), String::new());
        if let Some(argv) = &ts_argv {
            if setup.ts.is_file() && on_path(&setup.node) {
                let mut full = vec![setup.node.clone(), setup.ts.display().to_string()];
                full.extend(argv.iter().cloned());
                (ts_cell, ts_rss, ts_out) = repeat(&full, cwd, env);
            } else {
                ts_cell = "TypeScript cli not built".into();
            }
        }
        if let Some(argv) = &rs_argv {
            if setup.rust.is_file() {
                let mut full = vec![setup.rust.display().to_string()];
                full.extend(argv.iter().cloned());
                (rs_cell, rs_rss, rs_out) = repeat(&full, cwd, env);
            } else {
                rs_cell = "Rust binary not built".into();
            }
        }
        report.rows.push([name.to_string(), ts_cell, rs_cell, ts_rss, rs_rss]);
        report.notes.push((name.to_string(), ts_out, rs_out));
    };

    add("startup schema", Some(strings(&["schema"])), Some(strings(&["schema"])), &setup.root, &[]);
    let js_s = js.display().to_string();
    let py_s = py.display().to_string();
    let generate = |package: &str, out: String| {
        Some(vec!["mutate".into(), "generate".into(), "--package".into(), package.to_string(), "--out".into(), out, "--src".into(), "src".into()])
    };
    add("generate JS sample", generate(&js_s, tmp("pb-js-ts-{run}")), generate(&js_s, tmp("pb-js-rs-{run}")), &js, &[]);
    add("generate Python sample", generate(&py_s, tmp("pb-py-ts-{run}")), generate(&py_s, tmp("pb-py-rs-{run}")), &py, &[]);

    // The timed generate above already wrote the patches each run reads.
    let run = |package: &str, patches: String, out: String, extra: &[&str]| {
        let mut argv = vec!["mutate".to_string(), "run".into(), "--package".into(), package.to_string(), "--patches".into(), patches, "--out".into(), out];
        argv.extend(extra.iter().map(|item| item.to_string()));
        Some(argv)
    };
    let mutants = |name: &str| format!("{}{}mutants", tmp(name), std::path::MAIN_SEPARATOR);
    add(
        "run JS sample, no confirm",
        run(&js_s, mutants("pb-js-ts-{run}"), tmp("pb-js-ts-run-{run}"), &["--no-confirm"]),
        run(&js_s, mutants("pb-js-rs-{run}"), tmp("pb-js-rs-run-{run}"), &["--no-confirm", "--timeout-ms", "60000"]),
        &js,
        &[],
    );
    add(
        "run Python sample, no confirm",
        run(&py_s, mutants("pb-py-ts-{run}"), tmp("pb-py-ts-run-{run}"), &["--no-confirm", "--tests-dir", "tests"]),
        run(&py_s, mutants("pb-py-rs-{run}"), tmp("pb-py-rs-run-{run}"), &["--no-confirm", "--timeout-ms", "60000"]),
        &py,
        &[],
    );
    add(
        "run Python sample, workers 4, no confirm",
        run(&py_s, mutants("pb-py-ts-{run}"), tmp("pb-py-ts-w-{run}"), &["--no-confirm", "--workers", "4", "--tests-dir", "tests"]),
        run(&py_s, mutants("pb-py-rs-{run}"), tmp("pb-py-rs-w-{run}"), &["--no-confirm", "--workers", "4", "--timeout-ms", "60000"]),
        &py,
        &[],
    );

    let oss = setup.oss.clone();
    if oss.is_dir() && oss.join("src").is_dir() {
        let env = vec![("PYTHONPATH".to_string(), oss.join("src").display().to_string())];
        let oss_s = oss.display().to_string();
        add(
            "generate itsdangerous src",
            generate(&oss_s, tmp("pb-oss-ts-{run}")),
            generate(&oss_s, tmp("pb-oss-rs-{run}")),
            &oss,
            &env,
        );
        add(
            "run itsdangerous, max 20 mutants, no confirm",
            run(&oss_s, mutants("pb-oss-ts-{run}"), tmp("pb-oss-ts-run-{run}"), &["--no-confirm", "--max-mutants", "20", "--tests-dir", "tests"]),
            run(&oss_s, mutants("pb-oss-rs-{run}"), tmp("pb-oss-rs-run-{run}"), &["--no-confirm", "--max-mutants", "20", "--timeout-ms", "120000"]),
            &oss,
            &env,
        );
    } else {
        report.rows.push(["itsdangerous".into(), "checkout missing".into(), "checkout missing".into(), String::new(), String::new()]);
    }

    let mut lines = vec![
        "# Benchmark results".to_string(),
        String::new(),
        machine(&setup),
        String::new(),
        "Five runs of each command. The cell is median milliseconds, then min-max. Peak RSS is the highest VmHWM in kilobytes seen during those runs. TypeScript is c5cd175. Rust is the release binary built in this checkout. Re-running this script overwrites this file with new measurements.".into(),
        String::new(),
        "| Command | TypeScript ms | Rust ms | TS peak RSS kB | Rust peak RSS kB |".into(),
        "| --- | --- | --- | --- | --- |".into(),
    ];
    for [name, ts_cell, rs_cell, ts_rss, rs_rss] in &report.rows {
        lines.push(format!("| {name} | {ts_cell} | {rs_cell} | {ts_rss} | {rs_rss} |"));
    }
    lines.push(String::new());
    lines.push("## What the runs printed".into());
    lines.push(String::new());
    let first_40 = |text: &str| text.lines().take(40).collect::<Vec<_>>().join("\n");
    for (name, ts_out, rs_out) in &report.notes {
        lines.extend([
            format!("### {name}"),
            String::new(),
            "TypeScript, last run, first 40 lines:".into(),
            String::new(),
            "```".into(),
            first_40(ts_out),
            "```".into(),
            String::new(),
            "Rust, last run, first 40 lines:".into(),
            String::new(),
            "```".into(),
            first_40(rs_out),
            "```".into(),
            String::new(),
        ]);
    }
    let text = lines.join("\n");
    fs::write(&setup.out, &text).unwrap_or_else(|error| panic!("{}: {error}", setup.out.display()));
    println!("{}", text.split("## What").next().unwrap_or(""));
}
