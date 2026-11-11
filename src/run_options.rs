//! Run flags that change how a mutant is scored: direction, history, affected tests, and the clocks.

use std::fs;
use std::path::{Path, PathBuf};

use crate::args::Flags;
use crate::hash;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Auto,
    Forward,
    Reverse,
}

#[derive(Clone, Debug)]
pub struct RunOptions {
    pub direction: Direction,
    pub timeout_multiple: u64,
    pub timeout_floor_ms: u64,
    pub test_timeout_ms: u64,
    pub concurrency: u64,
    pub unset: Vec<String>,
    pub build: Option<String>,
    pub agent: Option<String>,
    pub history: Option<PathBuf>,
    pub affected: bool,
    pub tests_dir: Option<String>,
    pub repo: Option<PathBuf>,
    pub commit: Option<String>,
}

impl RunOptions {
    pub fn from_flags(flags: &Flags) -> Result<RunOptions, String> {
        let direction = match flags.get("direction").unwrap_or("auto") {
            "auto" => Direction::Auto,
            "forward" => Direction::Forward,
            "reverse" => Direction::Reverse,
            _ => return Err("--direction must be auto, forward, or reverse".into()),
        };
        let concurrency = flags.int("concurrency", 3)?;
        if concurrency < 1 {
            return Err("--concurrency must be at least 1".into());
        }
        Ok(RunOptions {
            direction,
            timeout_multiple: flags.int("timeout-multiple", 5)?.max(1),
            timeout_floor_ms: flags.int("timeout-floor-ms", 20_000)?.max(1),
            test_timeout_ms: flags.int("test-timeout-ms", 60_000)?,
            concurrency,
            unset: flags.texts("unset"),
            build: flags.get("build").filter(|text| !text.is_empty() && *text != "true").map(str::to_string),
            agent: flags.get("agent").map(str::to_string),
            history: flags.get("history").map(PathBuf::from),
            affected: flags.bool("affected", false),
            tests_dir: flags.get("tests-dir").map(str::to_string),
            repo: flags.get("repo").map(PathBuf::from),
            commit: flags.get("commit").map(str::to_string),
        })
    }
}

/// Mutant clock. The suite timeout is the ceiling. A known baseline scales by `--timeout-multiple`
/// and never sits under `--timeout-floor-ms`.
pub fn mutant_timeout_ms(suite_ms: u64, baseline_ms: Option<u64>, multiple: u64, floor_ms: u64) -> u64 {
    let Some(baseline) = baseline_ms else {
        return suite_ms.max(1);
    };
    let scaled = baseline.saturating_mul(multiple.max(1));
    let limit = floor_ms.max(1).max(scaled);
    suite_ms.max(1).min(limit)
}

/// Apply a mutant at its byte span. Offsets read from `git show` count `\n` line ends. A checkout
/// with `core.autocrlf` (the Git for Windows default) writes `\r\n`. When the span does not match
/// and the file has `\r\n`, the same offsets are read again with each `\r\n` counted as one byte.
pub fn apply_span(text: &str, start: usize, end: usize, original: &str, replacement: &str, direction: Direction) -> Result<(String, &'static str), String> {
    let applied = apply_exact(text, start, end, original, replacement, direction);
    if applied.is_ok() || !text.contains("\r\n") {
        return applied;
    }
    let (Some(crlf_start), Some(crlf_end)) = (crlf_offset(text, start), crlf_offset(text, end)) else {
        return applied;
    };
    apply_exact(text, crlf_start, crlf_end, original, replacement, direction).or(applied)
}

/// The byte in `text` at `offset` once each `\r\n` is read as `\n`.
fn crlf_offset(text: &str, offset: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut lf = 0;
    for index in 0..=bytes.len() {
        if lf == offset {
            return Some(index);
        }
        if index == bytes.len() {
            break;
        }
        if !(bytes[index] == b'\r' && bytes.get(index + 1) == Some(&b'\n')) {
            lf += 1;
        }
    }
    None
}

fn apply_exact(text: &str, start: usize, end: usize, original: &str, replacement: &str, direction: Direction) -> Result<(String, &'static str), String> {
    if start > end || end > text.len() || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
        return Err("mutant range does not fit the file".into());
    }
    let span = &text[start..end];
    let forward = original.is_empty() || span == original;
    let reverse = !replacement.is_empty() && span == replacement;
    let (use_forward, chosen) = match direction {
        Direction::Forward => (true, "forward"),
        Direction::Reverse => (false, "reverse"),
        Direction::Auto => {
            if forward {
                (true, "forward")
            } else {
                (false, "reverse")
            }
        }
    };
    if use_forward && forward {
        return Ok((crate::operators::apply(text, start, end, replacement), chosen));
    }
    if !use_forward && reverse {
        return Ok((crate::operators::apply(text, start, end, original), chosen));
    }
    Err("patch does not apply forward or in reverse".into())
}

/// Tests that import `source_file`. An empty list means nothing named this file, so the caller keeps the whole suite.
pub fn affected_tests(package: &Path, tests_dir: &str, source_file: &str) -> Vec<String> {
    let root = package.join(tests_dir);
    if !root.is_dir() {
        return Vec::new();
    }
    let module = python_module(source_file);
    let base = Path::new(source_file).file_name().and_then(|name| name.to_str()).unwrap_or(source_file);
    let stem = base.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(base);
    let mut found = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let looks = name.ends_with(".test.js")
                || name.ends_with(".test.ts")
                || name.ends_with(".test.mjs")
                || name.starts_with("test_") && name.ends_with(".py")
                || name.ends_with("_test.py");
            if !looks {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else { continue };
            let hit = text.contains(base)
                || text.contains(&format!("./{stem}"))
                || module.as_ref().is_some_and(|module| contains_module(&text, module));
            if hit {
                let rel = path.strip_prefix(package).unwrap_or(&path).to_string_lossy().replace('\\', "/");
                found.push(rel);
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

fn python_module(file: &str) -> Option<String> {
    let norm = file.replace('\\', "/");
    let body = norm.strip_suffix(".py")?;
    let body = body.strip_prefix("src/").unwrap_or(body);
    let body = body.strip_suffix("/__init__").unwrap_or(body);
    if body.is_empty() || body.starts_with("../") {
        return None;
    }
    Some(body.replace('/', "."))
}

fn contains_module(text: &str, module: &str) -> bool {
    text.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '.').any(|word| word == module)
}

#[derive(Clone, Debug)]
pub struct HistoryEntry {
    pub id: String,
    pub outcome: String,
    pub direction: Option<String>,
    pub mutant_hash: String,
    pub killed_by: Vec<String>,
    pub test_hashes: Vec<(String, String)>,
    pub agent: Option<String>,
}

pub fn history_file(path: &Path) -> PathBuf {
    if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
        path.to_path_buf()
    } else {
        path.join("history.json")
    }
}

pub fn load_history(path: &Path) -> Vec<HistoryEntry> {
    let file = history_file(path);
    let Ok(text) = fs::read_to_string(file) else { return Vec::new() };
    let Ok(value) = crate::json::parse(&text) else { return Vec::new() };
    let Some(crate::json::Value::Array(items)) = value.get("entries") else { return Vec::new() };
    items
        .iter()
        .filter_map(|item| {
            let id = item.get("id").and_then(crate::json::Value::as_str)?.to_string();
            let outcome = item.get("outcome").and_then(crate::json::Value::as_str).unwrap_or("error").to_string();
            let mutant_hash = item.get("mutantHash").and_then(crate::json::Value::as_str).unwrap_or("").to_string();
            let direction = item.get("direction").and_then(crate::json::Value::as_str).map(str::to_string);
            let agent = item.get("agent").and_then(crate::json::Value::as_str).map(str::to_string);
            let killed_by = match item.get("killedBy") {
                Some(crate::json::Value::Array(names)) => names.iter().filter_map(crate::json::Value::as_str).map(str::to_string).collect(),
                _ => Vec::new(),
            };
            let test_hashes = match item.get("testHashes") {
                Some(crate::json::Value::Object(entries)) => entries
                    .iter()
                    .filter_map(|(file, hash)| hash.as_str().map(|hash| (file.clone(), hash.to_string())))
                    .collect(),
                _ => Vec::new(),
            };
            Some(HistoryEntry { id, outcome, direction, mutant_hash, killed_by, test_hashes, agent })
        })
        .collect()
}

/// Identity of a mutant. A patch file on disk wins, so a history row written from that patch still matches.
pub fn patch_hash(patches: &Path, id: &str, file: &str, start: usize, end: usize, original: &str, replacement: &str) -> String {
    for candidate in [patches.join("mutants").join(format!("{id}.patch")), patches.join(format!("{id}.patch"))] {
        if let Ok(bytes) = fs::read(&candidate) {
            return hash::sha256_hex(&bytes);
        }
    }
    mutant_hash(file, start, end, original, replacement)
}

pub fn mutant_hash(file: &str, start: usize, end: usize, original: &str, replacement: &str) -> String {
    hash::sha256_hex(format!("{file}\0{start}\0{end}\0{original}\0{replacement}").as_bytes())
}

pub fn reusable(entry: &HistoryEntry, hash_value: &str, package: &Path) -> bool {
    if entry.mutant_hash.is_empty() || entry.mutant_hash != hash_value {
        return false;
    }
    if entry.outcome == "error" || entry.outcome == "unviable" || entry.outcome == "build-failed" {
        return false;
    }
    for (file, expected) in &entry.test_hashes {
        let Some(actual) = file_hash(package, file) else { return false };
        if actual != *expected {
            return false;
        }
    }
    true
}

pub fn file_hash(package: &Path, file: &str) -> Option<String> {
    let text = fs::read(package.join(file)).ok()?;
    Some(hash::sha256_hex(&text))
}

pub fn copy_history_map(history: &Path, out: &Path) {
    let dir = if history.extension().and_then(|ext| ext.to_str()) == Some("json") {
        history.parent().unwrap_or(history)
    } else {
        history
    };
    for name in ["coverage-map.json", "lines.json"] {
        let source = dir.join(name);
        let dest = out.join(name);
        if source.is_file() && !dest.exists() {
            let _ = fs::copy(source, dest);
        }
    }
}

/// Node's per-test clock. A longer `timeout:` in a test file raises it, and the suite clock caps it.
pub fn node_test_timeout_ms(package: &Path, files: &[String], test_timeout_ms: u64, suite_timeout_ms: u64) -> u64 {
    let mut declared = 0u64;
    for rel in files {
        let Ok(text) = fs::read_to_string(package.join(rel)) else { continue };
        for line in text.lines() {
            let Some(rest) = line.split("timeout").nth(1) else { continue };
            let Some(rest) = rest.trim_start().strip_prefix(':') else { continue };
            let digits: String = rest.trim_start().chars().take_while(|ch| ch.is_ascii_digit() || *ch == '_').collect();
            if let Ok(value) = digits.replace('_', "").parse::<u64>() {
                declared = declared.max(value);
            }
        }
    }
    let mut chosen = test_timeout_ms.max(declared);
    if suite_timeout_ms > 0 && chosen > suite_timeout_ms {
        chosen = suite_timeout_ms;
    }
    if chosen == 0 { test_timeout_ms } else { chosen }
}

pub fn split_argv(command: &str) -> Vec<String> {
    command.split_whitespace().map(str::to_string).collect()
}

/// Path of `file` inside a git root, using forward slashes. Windows verbatim prefixes are dropped.
pub fn git_relative(root: &Path, file: &Path) -> String {
    let root = plain(root);
    let file = plain(file);
    let root_text = root.to_string_lossy().trim_end_matches(['/', '\\']).to_string();
    let file_text = file.to_string_lossy().to_string();
    let root_key = root_text.to_ascii_lowercase();
    let file_key = file_text.to_ascii_lowercase();
    if let Some(rest) = file_key.strip_prefix(&root_key) {
        let start = file_text.len() - rest.len();
        return file_text[start..].trim_start_matches(['/', '\\']).replace('\\', "/");
    }
    file.file_name().and_then(|name| name.to_str()).unwrap_or("").to_string()
}

fn plain(path: &Path) -> PathBuf {
    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let text = canon.to_string_lossy();
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    PathBuf::from(text.replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direction_reverse_swaps_the_span_and_auto_prefers_forward() {
        let text = "return a && b;";
        let start = text.find("&&").unwrap();
        let end = start + 2;
        let (forward, chosen) = apply_span(text, start, end, "&&", "||", Direction::Auto).unwrap();
        assert_eq!(chosen, "forward");
        assert!(forward.contains("||"));
        let reversed_source = "return a || b;";
        let (back, chosen) = apply_span(reversed_source, start, end, "&&", "||", Direction::Reverse).unwrap();
        assert_eq!(chosen, "reverse");
        assert!(back.contains("&&"), "{back}");
        assert!(apply_span(text, start, end, "&&", "||", Direction::Reverse).is_err());
    }

    #[test]
    fn an_lf_offset_applies_to_a_crlf_checkout() {
        let committed = "fn a() {\n    let x = 1;\n    x > 0 && x < 9\n}\n";
        let start = committed.find("&&").unwrap();
        let checkout = committed.replace('\n', "\r\n");
        let (updated, chosen) = apply_span(&checkout, start, start + 2, "&&", "||", Direction::Auto).unwrap();
        assert_eq!(chosen, "forward");
        assert_eq!(updated, checkout.replace("&&", "||"));
        assert!(apply_span(&checkout, start, start + 2, "<", ">", Direction::Forward).is_err());
    }

    #[test]
    fn the_floor_caps_a_fast_baseline_and_the_suite_caps_the_floor() {
        assert_eq!(mutant_timeout_ms(120_000, Some(10), 5, 20_000), 20_000);
        assert_eq!(mutant_timeout_ms(5_000, Some(10), 5, 20_000), 5_000);
        assert_eq!(mutant_timeout_ms(120_000, None, 5, 20_000), 120_000);
    }

    #[test]
    fn affected_tests_name_the_importer_only() {
        let root = crate::testdir::TempDir::new("affected");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("tests")).unwrap();
        fs::write(root.join("tests/test_calc.py"), "import calc\n").unwrap();
        fs::write(root.join("tests/test_other.py"), "import other\n").unwrap();
        let names = affected_tests(&root, "tests", "src/calc.py");
        assert_eq!(names, ["tests/test_calc.py"]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn history_reuses_only_an_unchanged_mutant_and_test() {
        let root = crate::testdir::TempDir::new("hist");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("t.py"), "assert True\n").unwrap();
        let hash = mutant_hash("src/a.py", 1, 3, "&&", "||");
        let entry = HistoryEntry {
            id: "m1".into(),
            outcome: "killed".into(),
            direction: Some("forward".into()),
            mutant_hash: hash.clone(),
            killed_by: vec!["t.py::test".into()],
            test_hashes: vec![("t.py".into(), file_hash(&root, "t.py").unwrap())],
            agent: None,
        };
        assert!(reusable(&entry, &hash, &root));
        fs::write(root.join("t.py"), "assert False\n").unwrap();
        assert!(!reusable(&entry, &hash, &root));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_bad_direction_is_rejected() {
        let flags = Flags::parse(&["--direction".into(), "sideways".into()]);
        assert!(RunOptions::from_flags(&flags).is_err());
    }
}
