//! Golden tables. `check` compares rows. `record` logs plain calls. `compare` asks whether the table kills what the unit tests kill.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::envelope::Envelope;
use crate::json::{self, Value};
use crate::memory::golden_changes;
use crate::process;

const RECORDER: &str = include_str!("../embed/node/recorder.mjs");

pub fn check(recorded_path: &Path, actual_path: &Path, update: Option<&str>, message: &str, roots: &[String]) -> Result<Envelope, String> {
    if let Some(mode) = update {
        if mode != "wording" && mode != "all" {
            return Err("--update must be wording or all".into());
        }
    }
    let recorded_text = fs::read_to_string(recorded_path).map_err(|error| error.to_string())?;
    let actual_text = fs::read_to_string(actual_path).map_err(|error| error.to_string())?;
    let recorded = scrub_table(as_table(&recorded_text), roots);
    let actual = scrub_table(as_table(&actual_text), roots);
    let mut ids: Vec<String> = recorded.keys().chain(actual.keys()).cloned().collect();
    ids.sort();
    ids.dedup();
    let home = crate::envelope::home_dir();
    let mut wording = Vec::new();
    let mut contract = Vec::new();
    let mut invariant = Vec::new();
    for id in &ids {
        let before = recorded.get(id);
        let after = actual.get(id);
        if let Some(after) = after {
            if let Some(problem) = invariant_of(id, after, &home) {
                invariant.push(problem);
            }
        }
        if json::stringify_compact(&wording_of(before)) != json::stringify_compact(&wording_of(after)) {
            wording.push(id.clone());
        }
        if json::stringify_compact(&contract_of(before)) != json::stringify_compact(&contract_of(after)) {
            contract.push(id.clone());
        }
    }
    let allowed = golden_changes(message);
    let unauthorized: Vec<String> = contract.iter().filter(|id| !allowed.iter().any(|(name, _)| name == *id)).cloned().collect();
    let ok = invariant.is_empty() && unauthorized.is_empty() && (wording.is_empty() || update.is_some());
    let summary = if !invariant.is_empty() {
        format!("Invariant failed: {}.", invariant.join("; "))
    } else if !unauthorized.is_empty() {
        format!("Contract changed for {} without a Golden-Change trailer.", unauthorized.join(", "))
    } else if wording.is_empty() && contract.is_empty() {
        "Golden rows match.".into()
    } else if update.is_some() {
        format!("Re-recorded {} wording row(s) and {} contract row(s).", wording.len(), contract.len())
    } else {
        format!("Wording changed for {}. Re-record with --update wording.", wording.join(", "))
    };
    let write = ok && update.is_some() && (!wording.is_empty() || !contract.is_empty());
    if write {
        let mut table = recorded.clone();
        let wording_write = update == Some("wording") || update == Some("all");
        let contract_write = contract.iter().all(|id| allowed.iter().any(|(name, _)| name == id));
        for id in &ids {
            let after = actual.get(id);
            if contract.contains(id) && contract_write {
                match after {
                    Some(row) => {
                        table.insert(id.clone(), row.clone());
                    }
                    None => {
                        table.remove(id);
                    }
                }
            } else if wording.contains(id) && wording_write {
                if let Some(after) = after {
                    let row = table.entry(id.clone()).or_insert_with(|| Value::object(vec![]));
                    if let (Value::Object(entries), Value::Object(extra)) = (row, wording_of(Some(after))) {
                        for (key, value) in extra {
                            if let Some(slot) = entries.iter_mut().find(|(name, _)| name == &key) {
                                slot.1 = value;
                            } else {
                                entries.push((key, value));
                            }
                        }
                    }
                }
            }
        }
        let shape = if recorded_text.contains("\"rows\"") { "rows" } else { "flat" };
        write_table(recorded_path, &table, shape)?;
    }
    Ok(Envelope::new(ok, "golden.check", summary, if ok { "Review the re-recorded rows." } else { "The table was not written." })
        .with_extra("wording", Value::Array(wording.into_iter().map(Value::String).collect()))
        .with_extra("contract", Value::Array(contract.into_iter().map(Value::String).collect()))
        .with_extra("invariant", Value::Array(invariant.into_iter().map(Value::String).collect()))
        .with_extra("updated", Value::Bool(write && ok)))
}

pub fn pin_commit(package: &Path, repo: Option<&str>, commit: Option<&str>) -> Result<(), String> {
    let root = match repo {
        Some(path) => {
            let path = PathBuf::from(path);
            if !path.is_dir() || !crate::worktree::is_git_repo(&path) {
                return Err("repo is not a git repository".into());
            }
            path
        }
        None => package.to_path_buf(),
    };
    if let Some(commit) = commit {
        crate::worktree::resolve_commit(&root, commit).map(|_| ())
    } else {
        Ok(())
    }
}

fn git_show(repo: &Path, spec: &str) -> Result<String, String> {
    let output = Command::new("git").arg("-C").arg(repo).args(["show", spec]).output().map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

pub fn record(package: &Path, modules: &[String], tests: &[String], out_dir: &str, timeout_ms: u64, repo: Option<&str>, commit: Option<&str>) -> Envelope {
    let fail = |summary: String| Envelope::new(false, "golden.record", summary, "Fix the command and run it again.");
    if modules.is_empty() {
        return fail("Pass --module with the source file to record.".into());
    }
    if tests.is_empty() {
        return fail("Pass --tests with the unit test files that call it.".into());
    }
    for module in modules {
        if !module.ends_with(".js") && !module.ends_with(".mjs") && !module.ends_with(".cjs") && !module.ends_with(".ts") && !module.ends_with(".tsx") {
            return fail(format!("{module} is not a JavaScript or TypeScript module."));
        }
        if !package.join(module).is_file() {
            return fail(format!("{module} is not in the package."));
        }
    }
    for test in tests {
        if !package.join(test).is_file() {
            return fail(format!("{test} is not in the package."));
        }
    }
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let scratch = std::env::temp_dir().join(format!("probatio-grec-scratch-{}-{id}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    if copy_package(package, &scratch).is_err() {
        return fail("The package could not be copied for recording.".into());
    }
    if let Some(commit) = commit {
        let root = repo.map(PathBuf::from).unwrap_or_else(|| package.to_path_buf());
        for rel in modules.iter().chain(tests.iter()) {
            match git_show(&root, &format!("{commit}:{rel}")) {
                Ok(text) => {
                    let _ = fs::write(scratch.join(rel), text);
                }
                Err(_) => {
                    let _ = fs::remove_dir_all(&scratch);
                    return fail(format!("{rel} is not in {commit}."));
                }
            }
        }
    }
    let log = scratch.join("calls.jsonl");
    let recorder = scratch.join("probatio-recorder.mjs");
    if fs::write(&recorder, RECORDER).is_err() {
        return fail("The recorder could not be written.".into());
    }
    let mut functions = Vec::new();
    for module in modules {
        let text = fs::read_to_string(scratch.join(module)).unwrap_or_default();
        let names = exported_functions(&text);
        functions.push((module.clone(), names.clone()));
        if wrap_module(&scratch, module, &names, &recorder).is_err() {
            let _ = fs::remove_dir_all(&scratch);
            return fail(format!("Could not wrap {module}."));
        }
    }
    let run = run_node_tests(&scratch, tests, &log, timeout_ms);
    let calls = read_calls(&log);
    let _ = fs::remove_dir_all(&scratch);
    if run.timed_out {
        return Envelope::new(
            false,
            "golden.record",
            format!("The unit tests timed out after {timeout_ms}ms."),
            "A timeout is not a contract. Run those tests green first.",
        );
    }
    if run.fail > 0 || run.code != 0 {
        return Envelope::new(
            false,
            "golden.record",
            format!("The unit tests did not pass while recording ({} passed, {} failed). A table recorded from a red run is not a contract.", run.pass, run.fail),
            "Run those tests green first.",
        );
    }
    let mut written = Vec::new();
    let mut rows_total = 0usize;
    let mut hidden = 0usize;
    let mut module_rows = Vec::new();
    for (module, names) in &functions {
        let mine: Vec<&Value> = calls.iter().filter(|call| call.get("module").and_then(Value::as_str) == Some(module.as_str())).collect();
        hidden += mine.iter().filter(|call| call.get("hidden").is_some()).count();
        let built = build_rows(&mine);
        rows_total += built.len();
        let base = Path::new(module).file_stem().and_then(|item| item.to_str()).unwrap_or("module");
        let table_rel = format!("{out_dir}/{base}.golden.json");
        let test_rel = format!("{out_dir}/{base}.golden.test.mjs");
        let table_path = package.join(&table_rel);
        if let Some(parent) = table_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let body = Value::object(vec![
            ("module", Value::String(module.clone())),
            ("rows", Value::Object(built.iter().map(|(id, row)| (id.clone(), row.clone())).collect())),
        ]);
        let _ = fs::write(&table_path, json::stringify(&body));
        let _ = fs::write(package.join(&test_rel), replay_test(module, &table_rel, &test_rel));
        written.push(table_rel);
        written.push(test_rel);
        module_rows.push(Value::object(vec![
            ("module", Value::String(module.clone())),
            ("rows", Value::Int(built.len() as i64)),
            ("functions", Value::Array(names.iter().cloned().map(Value::String).collect())),
        ]));
    }
    let summary = format!("{rows_total} rows from {} passing tests.", run.pass);
    let mut envelope = Envelope::new(
        rows_total > 0,
        "golden.record",
        if hidden > 0 { format!("{summary} {hidden} calls had a callback, an instance, or another hidden input. Tests built on those stay as code.") } else { summary },
        if rows_total > 0 {
            "Commit the tables and replay tests, then run nextCall to see whether the table kills what the unit tests kill. Nothing was deleted."
        } else {
            "Nothing to replay. Pick tests that call the module's exports with plain data."
        },
    )
    .with_extra("rows", Value::Int(rows_total as i64))
    .with_extra("hidden", Value::Int(hidden as i64))
    .with_extra("modules", Value::Array(module_rows))
    .with_extra("written", Value::Array(written.iter().cloned().map(Value::String).collect()));
    if rows_total > 0 {
        let mut argv = vec!["golden".into(), "compare".into(), "--package".into(), package.display().to_string()];
        for module in modules {
            argv.push("--module".into());
            argv.push(module.clone());
        }
        for test in tests {
            argv.push("--tests".into());
            argv.push(test.clone());
        }
        for file in written.iter().filter(|file| file.contains(".golden.test.")) {
            argv.push("--golden".into());
            argv.push(package.join(file).display().to_string());
        }
        argv.push("--out".into());
        argv.push(package.join(".probatio/golden-compare").display().to_string());
        envelope.next_call = Some(argv);
    }
    envelope
}

pub fn compare(package: &Path, modules: &[String], tests: &[String], golden: &[String], out: &Path, operators: &str, timeout_ms: u64, commit: Option<&str>) -> Envelope {
    if modules.is_empty() || tests.is_empty() || golden.is_empty() {
        return Envelope::new(false, "golden.compare", "Pass --module, --tests, and --golden.", "Fix the command and run it again.");
    }
    let wide = operators == "wide";
    let mut points = Vec::new();
    for module in modules {
        let text = if let Some(commit) = commit {
            match git_show(package, &format!("{commit}:{module}")) {
                Ok(text) => text,
                Err(_) => return Envelope::new(false, "golden.compare", format!("{module} is not in {commit}."), "Fix the command and run it again."),
            }
        } else {
            let path = package.join(module);
            let Ok(text) = fs::read_to_string(&path) else {
                return Envelope::new(false, "golden.compare", format!("{module} is not in the package."), "Fix the command and run it again.");
            };
            text
        };
        points.extend(crate::operators::scan(module, &text, wide));
    }
    if points.is_empty() {
        return Envelope::new(true, "golden.compare", "0 mutants. The table was not asked to kill anything.", "Nothing was deleted.");
    }
    let patches = out.join("mutants");
    let _ = fs::create_dir_all(&patches);
    let rows: Vec<Value> = points.iter().map(|mutant| {
        Value::object(vec![
            ("id", Value::String(mutant.id.clone())),
            ("file", Value::String(mutant.file.clone())),
            ("line", Value::Int(mutant.line as i64)),
            ("start", Value::Int(mutant.start as i64)),
            ("end", Value::Int(mutant.end as i64)),
            ("op", Value::String(mutant.op.clone())),
            ("original", Value::String(mutant.original.clone())),
            ("replacement", Value::String(mutant.replacement.clone())),
        ])
    }).collect();
    let _ = fs::write(patches.join("mutants.json"), json::stringify(&Value::object(vec![("mutants", Value::Array(rows))])));
    let unit_out = out.join("unit");
    let golden_out = out.join("golden");
    let unit_cmd = format!("node --test {}", tests.join(" "));
    let golden_cmd = format!("node --test {}", golden.join(" "));
    let unit = crate::runner::run(&crate::args::Flags::parse(&[
        "--package".into(),
        package.display().to_string(),
        "--patches".into(),
        patches.display().to_string(),
        "--out".into(),
        unit_out.display().to_string(),
        "--suite-command".into(),
        unit_cmd,
        "--timeout-ms".into(),
        timeout_ms.to_string(),
        "--no-confirm".into(),
    ]));
    let table = crate::runner::run(&crate::args::Flags::parse(&[
        "--package".into(),
        package.display().to_string(),
        "--patches".into(),
        patches.display().to_string(),
        "--out".into(),
        golden_out.display().to_string(),
        "--suite-command".into(),
        golden_cmd,
        "--timeout-ms".into(),
        timeout_ms.to_string(),
        "--no-confirm".into(),
    ]));
    if !unit.ok {
        return Envelope::new(false, "golden.compare", format!("The unit tests did not score: {}", unit.summary), unit.next);
    }
    let unit_killed = killed_ids(&unit_out);
    let table_killed = killed_ids(&golden_out);
    let missed: Vec<String> = unit_killed.iter().filter(|id| !table_killed.iter().any(|other| other == *id)).cloned().collect();
    let ok = table.ok && missed.is_empty();
    Envelope::new(
        ok,
        "golden.compare",
        format!("{} mutants. The unit tests killed {}. The table killed {}. {} missed.", points.len(), unit_killed.len(), table_killed.len(), missed.len()),
        if missed.is_empty() { "The table killed every mutant the unit tests killed. Nothing was deleted." } else { "The table missed a mutant the unit tests killed. Nothing was deleted." },
    )
    .with_extra("missed", Value::Array(missed.into_iter().map(Value::String).collect()))
    .with_extra("unitKilled", Value::Int(unit_killed.len() as i64))
    .with_extra("tableKilled", Value::Int(table_killed.len() as i64))
}

fn killed_ids(out: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(out.join("results")) else { return Vec::new() };
    let mut ids = Vec::new();
    for entry in entries.flatten() {
        let Ok(text) = fs::read_to_string(entry.path()) else { continue };
        let Ok(value) = json::parse(&text) else { continue };
        if value.get("outcome").and_then(Value::as_str) == Some("killed") {
            if let Some(id) = value.get("id").and_then(Value::as_str) {
                ids.push(id.to_string());
            }
        }
    }
    ids.sort();
    ids
}

fn exported_functions(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("export function ") {
            if let Some(name) = rest.split(['(', ' ']).next() {
                if !name.is_empty() {
                    names.push(name.to_string());
                }
            }
        } else if let Some(rest) = line.strip_prefix("export const ") {
            if let Some(name) = rest.split([' ', '=']).next() {
                if !name.is_empty() && !names.contains(&name.to_string()) {
                    names.push(name.to_string());
                }
            }
        }
    }
    names.sort();
    names
}

fn file_url(path: &Path) -> String {
    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let text = canon.to_string_lossy().replace('\\', "/");
    let text = text.strip_prefix("//?/").unwrap_or(&text);
    if text.starts_with('/') {
        format!("file://{text}")
    } else {
        format!("file:///{text}")
    }
}

fn wrap_module(root: &Path, module: &str, names: &[String], recorder: &Path) -> Result<(), String> {
    let abs = root.join(module);
    let ext = Path::new(module).extension().and_then(|item| item.to_str()).unwrap_or("js");
    let stem = Path::new(module).file_stem().and_then(|item| item.to_str()).unwrap_or("mod");
    let real_name = format!("{stem}.probatio-real.{ext}");
    let real = abs.parent().unwrap_or(root).join(&real_name);
    fs::rename(&abs, &real).map_err(|error| error.to_string())?;
    let mut lines = vec![
        format!("export * from \"./{real_name}\";"),
        format!("import * as real from \"./{real_name}\";"),
        format!("import {{ record }} from \"{}\";", file_url(recorder)),
    ];
    for name in names {
        lines.push(format!("export const {name} = record({module:?}, {name:?}, real.{name});"));
    }
    lines.push(String::new());
    fs::write(abs, lines.join("\n")).map_err(|error| error.to_string())
}

struct NodeRun {
    code: i32,
    pass: i64,
    fail: i64,
    timed_out: bool,
}

fn run_node_tests(dir: &Path, tests: &[String], log: &Path, timeout_ms: u64) -> NodeRun {
    let mut args = vec!["--test".into(), "--test-reporter=tap".into()];
    args.extend(tests.iter().cloned());
    let timed = process::run_argv(
        "node",
        &args,
        Some(dir),
        Some(dir),
        Duration::from_millis(timeout_ms.max(1)),
        &[("PROBATIO_GOLDEN_LOG", &log.display().to_string())],
    );
    let pass = count_tap(&timed.stdout, "pass");
    let fail = count_tap(&timed.stdout, "fail");
    NodeRun { code: timed.status.unwrap_or(1), pass, fail, timed_out: timed.timed_out }
}

fn count_tap(text: &str, name: &str) -> i64 {
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix(&format!("# {name} ")) {
            if let Ok(value) = rest.trim().parse() {
                return value;
            }
        }
    }
    0
}

fn read_calls(log: &Path) -> Vec<Value> {
    let Ok(text) = fs::read_to_string(log) else { return Vec::new() };
    text.lines().filter_map(|line| json::parse(line).ok()).collect()
}

fn build_rows(calls: &[&Value]) -> Vec<(String, Value)> {
    let mut rows = Vec::new();
    for call in calls {
        if call.get("hidden").is_some() || call.get("args").is_none() {
            continue;
        }
        let function = call.get("fn").and_then(Value::as_str).unwrap_or("");
        let preview = json::stringify_compact(call.get("args").unwrap_or(&Value::Null));
        let id = format!("{function}({preview})");
        if rows.iter().any(|(known, _)| known == &id) {
            continue;
        }
        rows.push((id, (*call).clone()));
    }
    rows
}

fn replay_test(module: &str, table_rel: &str, test_rel: &str) -> String {
    let from = |target: &str| {
        let base = Path::new(test_rel).parent().unwrap_or(Path::new("."));
        let rel = pathdiff(base, Path::new(target));
        if rel.starts_with('.') { rel } else { format!("./{rel}") }
    };
    format!(
        "// Generated by probatio golden record from the unit tests that called {module}.\nimport assert from \"node:assert/strict\";\nimport {{ readFileSync }} from \"node:fs\";\nimport test from \"node:test\";\nimport * as subject from \"{}\";\nconst table = JSON.parse(readFileSync(new URL(\"{}\", import.meta.url), \"utf8\"));\nfor (const [id, row] of Object.entries(table.rows)) {{\n  test(`golden {module} ${{id}}`, () => {{\n    const fn = subject[row.fn];\n    assert.equal(typeof fn, \"function\");\n    const args = row.args.map((arg) => arg.u ? undefined : arg.v);\n    let actual;\n    try {{ actual = {{ result: {{ v: fn(...args) }} }}; }} catch (error) {{ actual = {{ error: {{ name: error.name, message: String(error.message) }} }}; }}\n    const expected = row.error ? {{ error: row.error }} : {{ result: row.result }};\n    assert.deepEqual(actual, expected);\n  }});\n}}\n",
        from(module),
        from(table_rel)
    )
}

fn pathdiff(from: &Path, to: &Path) -> String {
    let from = from.components().map(|item| item.as_os_str().to_string_lossy().to_string()).collect::<Vec<_>>();
    let to = to.components().map(|item| item.as_os_str().to_string_lossy().to_string()).collect::<Vec<_>>();
    let mut shared = 0;
    while shared < from.len() && shared < to.len() && from[shared] == to[shared] {
        shared += 1;
    }
    let mut parts = vec!["..".to_string(); from.len().saturating_sub(shared)];
    parts.extend(to[shared..].iter().cloned());
    if parts.is_empty() { ".".into() } else { parts.join("/") }
}

fn copy_package(src: &Path, dest: &Path) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|error| error.to_string())?;
    for entry in fs::read_dir(src).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == ".git" || name == "node_modules" || name == "target" || name == "dist" {
            continue;
        }
        let to = dest.join(entry.file_name());
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        if file_type.is_dir() {
            copy_package(&entry.path(), &to)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), &to).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn scrub_table(rows: std::collections::BTreeMap<String, Value>, roots: &[String]) -> std::collections::BTreeMap<String, Value> {
    rows.into_iter().map(|(key, value)| (scrub_text(&key, roots), scrub_value(value, roots))).collect()
}

fn scrub_value(value: Value, roots: &[String]) -> Value {
    match value {
        Value::String(text) => Value::String(scrub_text(&text, roots)),
        Value::Array(items) => Value::Array(items.into_iter().map(|item| scrub_value(item, roots)).collect()),
        Value::Object(entries) => Value::Object(entries.into_iter().map(|(key, item)| (key, scrub_value(item, roots))).collect()),
        other => other,
    }
}

fn scrub_text(text: &str, roots: &[String]) -> String {
    let mut out = text.to_string();
    for root in roots {
        if root.len() > 1 && out.contains(root) {
            out = out.replace(root, "{root}");
        }
    }
    out
}

fn as_table(text: &str) -> std::collections::BTreeMap<String, Value> {
    let Ok(value) = json::parse(text) else { return std::collections::BTreeMap::new() };
    let rows = value.get("rows").cloned().unwrap_or(value);
    let Value::Object(entries) = rows else { return std::collections::BTreeMap::new() };
    entries.into_iter().collect()
}

fn wording_of(row: Option<&Value>) -> Value {
    let Some(Value::Object(entries)) = row else { return Value::object(vec![]) };
    let kept: Vec<(String, Value)> = entries.iter().filter(|(key, _)| key == "next" || key == "nextLead").cloned().collect();
    Value::Object(kept)
}

fn contract_of(row: Option<&Value>) -> Value {
    let Some(Value::Object(entries)) = row else { return Value::object(vec![]) };
    let mut kept: Vec<(String, Value)> = entries.iter().filter(|(key, _)| key != "next" && key != "nextLead").cloned().collect();
    kept.sort_by(|a, b| a.0.cmp(&b.0));
    Value::Object(kept)
}

fn invariant_of(id: &str, row: &Value, home: &str) -> Option<String> {
    if row.get("ok").and_then(Value::as_bool) == Some(true) {
        let reason = row.get("reason").and_then(Value::as_str).unwrap_or("");
        if reason.trim().is_empty() {
            return Some(format!("{id} has ok without a reason"));
        }
    }
    if !home.is_empty() && json::stringify_compact(row).contains(home) {
        return Some(format!("{id} contains a home path"));
    }
    None
}

fn write_table(path: &Path, rows: &std::collections::BTreeMap<String, Value>, shape: &str) -> Result<(), String> {
    let value = if shape == "flat" {
        Value::Object(rows.iter().map(|(key, value)| (key.clone(), value.clone())).collect())
    } else {
        Value::object(vec![("rows", Value::Object(rows.iter().map(|(key, value)| (key.clone(), value.clone())).collect()))])
    };
    fs::write(path, json::stringify(&value)).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wording_can_be_rerecorded_and_a_contract_change_needs_a_trailer() {
        let dir = std::env::temp_dir().join(format!("probatio-golden-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("recorded.json"), "{\n  \"rows\": {\n    \"a\": {\"ok\": true, \"reason\": \"kept\", \"next\": \"old\"}\n  }\n}\n").unwrap();
        fs::write(dir.join("actual.json"), "{\n  \"rows\": {\n    \"a\": {\"ok\": true, \"reason\": \"kept\", \"next\": \"new\"}\n  }\n}\n").unwrap();
        let changed = check(&dir.join("recorded.json"), &dir.join("actual.json"), None, "", &[]).unwrap();
        assert!(!changed.ok, "{}", changed.summary);
        let updated = check(&dir.join("recorded.json"), &dir.join("actual.json"), Some("wording"), "", &[]).unwrap();
        assert!(updated.ok, "{}", updated.summary);
        assert!(fs::read_to_string(dir.join("recorded.json")).unwrap().contains("new"));
        fs::write(dir.join("actual.json"), "{\n  \"rows\": {\n    \"a\": {\"ok\": true, \"reason\": \"changed\", \"next\": \"new\"}\n  }\n}\n").unwrap();
        let blocked = check(&dir.join("recorded.json"), &dir.join("actual.json"), Some("all"), "", &[]).unwrap();
        assert!(!blocked.ok, "{}", blocked.summary);
        assert!(blocked.summary.contains("Golden-Change"), "{}", blocked.summary);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn record_writes_rows_from_a_passing_node_test() {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("probatio-grec-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::create_dir_all(dir.join("test")).unwrap();
        fs::write(dir.join("src/calc.js"), "export function add(a, b) { return a + b; }\n").unwrap();
        fs::write(dir.join("test/calc.test.js"), "import test from 'node:test';\nimport assert from 'node:assert/strict';\nimport { add } from '../src/calc.js';\ntest('adds', () => { assert.equal(add(1, 2), 3); });\n").unwrap();
        let envelope = record(&dir, &["src/calc.js".into()], &["test/calc.test.js".into()], "tests/golden", 60_000, None, None);
        if std::process::Command::new("node").arg("--version").output().is_err() {
            return;
        }
        assert!(envelope.ok, "{}", envelope.summary);
        assert!(envelope.summary.contains("rows from"), "{}", envelope.summary);
        assert!(dir.join("tests/golden/calc.golden.json").is_file());
        let _ = fs::remove_dir_all(&dir);
    }
}
