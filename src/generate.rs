//! `mutate generate` writes operator mutants from the commit, unless `--working-tree` is set.
//! The default walk is `src`, so a test directory beside it is not mutated.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use crate::args::Flags;
use crate::envelope::Envelope;
use crate::json::{self, Value};
use crate::operators::{self, Mutant};

const DEFAULT_SEED: u64 = 20_261_003;

pub fn generate(flags: &Flags) -> Envelope {
    let command = "mutate.generate";
    let package = match flags.need("package") {
        Ok(value) => PathBuf::from(value),
        Err(message) => return missing(command, &message),
    };
    let out = match flags.need("out") {
        Ok(value) => PathBuf::from(value),
        Err(message) => return missing(command, &message),
    };
    if !package.is_dir() {
        return missing(command, "package is not a directory");
    }
    let per_file = match flags.parse_int("per-file") {
        Ok(value) => value.map(|item| item as usize),
        Err(message) => return Envelope::new(false, command, message, "Fix the command and run it again."),
    };
    let skip = match flags.int("skip", 0) {
        Ok(value) => value as usize,
        Err(message) => return Envelope::new(false, command, message, "Fix the command and run it again."),
    };
    let seed = match flags.int("seed", DEFAULT_SEED) {
        Ok(value) => value,
        Err(message) => return Envelope::new(false, command, message, "Fix the command and run it again."),
    };
    let limit = match flags.parse_int("max-mutants") {
        Ok(value) => value.map(|item| item as usize),
        Err(message) => return Envelope::new(false, command, message, "Fix the command and run it again."),
    };
    let max_minutes = match flags.parse_int("max-minutes") {
        Ok(value) => value,
        Err(message) => return Envelope::new(false, command, message, "Fix the command and run it again."),
    };
    let wide = match flags.get("operators") {
        None | Some("core") => false,
        Some("wide") => true,
        Some(_) => {
            return Envelope::new(
                false,
                command,
                "--operators must be core or wide",
                "core is the default. wide adds arithmetic, integer constants, and dropped statements.",
            );
        }
    };
    let operators = if wide { "wide" } else { "core" };
    let started = Instant::now();
    let over = |minutes: Option<u64>| minutes.is_some_and(|limit| started.elapsed().as_secs() >= limit.saturating_mul(60));
    if over(max_minutes) {
        return budget_envelope(command, 0, 0, true, operators);
    }
    let src = flags.get("src").unwrap_or("src");
    let root = package.join(src);
    let skip_files = flags.texts("skip-file");
    let working_tree = flags.bool("working-tree", false);
    let commit = flags.get("commit").unwrap_or("HEAD");
    let walked = walk_sources(&root, over(max_minutes));
    let mut files = walked.files;
    files.retain(|file| !skip_files.iter().any(|name| file.file_name().and_then(|item| item.to_str()) == Some(name.as_str())));
    let mut rng = Rng::new(seed as u32);
    let mut chosen: Vec<(Mutant, String)> = Vec::new();
    let mut files_visited = 0usize;
    let mut budget_hit = walked.stopped || over(max_minutes);
    let mut source = if working_tree { "working-tree" } else { "commit" };
    let mut reader = Reader::new(&package, commit, working_tree);
    for file in &files {
        if limit.is_some_and(|limit| chosen.len() >= limit) {
            break;
        }
        if over(max_minutes) {
            budget_hit = true;
            break;
        }
        files_visited += 1;
        let rel = rel_to(&package, file);
        if rel.ends_with(".d.ts") {
            continue;
        }
        if !operators::kind_of(&rel) {
            continue;
        }
        let loaded = match reader.read(&package, file) {
            Ok(value) => value,
            Err(message) => {
                return Envelope::new(
                    false,
                    command,
                    message,
                    "Commit the file, or pass --working-tree to mutate the checkout on disk. mutate run scores the commit.",
                );
            }
        };
        source = loaded.source;
        let mut points = operators::scan(&rel, &loaded.text, wide);
        points = shuffle(points, &mut rng);
        let slice: Vec<Mutant> = if let Some(per_file) = per_file {
            points.into_iter().skip(skip).take(per_file).collect()
        } else {
            points.into_iter().skip(skip).collect()
        };
        for point in slice {
            chosen.push((point, loaded.text.clone()));
        }
    }
    let capped: Vec<(Mutant, String)> = if let Some(limit) = limit { chosen.into_iter().take(limit).collect() } else { chosen };
    if let Err(message) = write_batch(&out, &capped, seed, operators) {
        return Envelope::new(false, command, message, "The out directory could not be written.");
    }
    let bounded = limit.is_some_and(|limit| capped.len() >= limit);
    let summary = if budget_hit {
        format!("Stopped by the time budget after {files_visited} files. {} mutants written.", capped.len())
    } else if bounded {
        format!(
            "{} mutants in {files_visited} files. No string-literal mutant. Bounded by --max-mutants {}.",
            capped.len(),
            limit.unwrap_or(0)
        )
    } else {
        format!("{} mutants in {files_visited} files. No string-literal mutant.", capped.len())
    };
    let suite = discover_label(&package, flags.get("tests-dir").unwrap_or("tests"));
    let mut next = if capped.is_empty() {
        "Nothing to run.".to_string()
    } else if budget_hit {
        "Narrow --src or raise --max-minutes, then run the same command.".to_string()
    } else if bounded {
        format!(
            "{} mutants. Suite is {suite}. Bounded by --max-mutants {}. The walk stopped early.",
            capped.len(),
            limit.unwrap_or(0)
        )
    } else {
        format!("{} mutants. Suite is {suite}.", capped.len())
    };
    if source == "working-tree" && !capped.is_empty() {
        next.push_str(" These patches match the working tree. mutate run scores the commit, so commit the files before mutate run.");
    }
    let shown: Vec<Value> = capped.iter().take(10).map(|(mutant, _)| shown_mutant(mutant)).collect();
    let mut envelope = Envelope::new(true, command, summary, next)
        .with_extra("mutantCount", Value::Int(capped.len() as i64))
        .with_extra("operators", Value::String(operators.into()))
        .with_extra("stringLiteralMutants", Value::Int(0))
        .with_extra("filesVisited", Value::Int(files_visited as i64))
        .with_extra("budgetHit", Value::Bool(budget_hit))
        .with_extra("mutants", Value::Array(shown))
        .with_extra("rest", Value::Int(capped.len().saturating_sub(10) as i64))
        .with_extra("full", Value::String(out.join("mutants").join("mutants.json").display().to_string()))
        .with_extra("source", Value::String(source.into()));
    if !capped.is_empty() {
        let mut argv = vec![
            "mutate".into(),
            "run".into(),
            "--package".into(),
            package.display().to_string(),
            "--patches".into(),
            out.join("mutants").display().to_string(),
            "--out".into(),
            fresh_run_dir(&out).display().to_string(),
        ];
        if flags.get("commit").is_some() {
            argv.push("--commit".into());
            argv.push(commit.to_string());
        }
        if let Some(tests) = flags.get("tests-dir") {
            argv.push("--tests-dir".into());
            argv.push(tests.to_string());
        }
        envelope.next_call = Some(argv);
    }
    envelope
}

fn budget_envelope(command: &str, files: usize, mutants: usize, hit: bool, operators: &str) -> Envelope {
    Envelope::new(
        true,
        command,
        format!("Stopped by the time budget after {files} files. {mutants} mutants written."),
        "Narrow --src or raise --max-minutes, then run the same command.",
    )
    .with_extra("mutantCount", Value::Int(mutants as i64))
    .with_extra("operators", Value::String(operators.into()))
    .with_extra("stringLiteralMutants", Value::Int(0))
    .with_extra("filesVisited", Value::Int(files as i64))
    .with_extra("budgetHit", Value::Bool(hit))
    .with_extra("mutants", Value::Array(Vec::new()))
    .with_extra("rest", Value::Int(0))
}

struct Loaded {
    text: String,
    source: &'static str,
}

/// Reads source files from the commit through one `git cat-file --batch` process, started in the
/// package with `<commit>:./<path>` specs, so no `git rev-parse` is needed first. A git start on
/// Windows costs 50 to 100 ms, and the old path spent two per file.
struct Reader {
    commit: String,
    /// `None` reads the working tree: `--working-tree`, no git, or not a git checkout.
    package: Option<PathBuf>,
    batch: Option<(std::process::Child, std::io::BufReader<std::process::ChildStdout>)>,
}

impl Reader {
    fn new(package: &Path, commit: &str, working_tree: bool) -> Reader {
        let package = if working_tree { None } else { Some(package.to_path_buf()) };
        Reader { commit: commit.to_string(), package, batch: None }
    }

    fn read(&mut self, package: &Path, file: &Path) -> Result<Loaded, String> {
        let Some(root) = self.package.clone() else {
            return disk(file);
        };
        let rel = crate::run_options::git_relative(&root, file);
        match self.blob(&root, &format!("{}:./{rel}", self.commit)) {
            Blob::Found(bytes) => Ok(Loaded { text: String::from_utf8_lossy(&bytes).into_owned(), source: "commit" }),
            Blob::Missing => {
                let shown = rel_to(package, file);
                Err(format!("{shown} is not in {}. Pass --working-tree to read the checkout.", self.commit))
            }
            Blob::NoRepo => {
                self.package = None;
                self.batch = None;
                disk(file)
            }
        }
    }

    fn blob(&mut self, root: &Path, spec: &str) -> Blob {
        use std::io::{BufRead, Read, Write};
        if self.batch.is_none() {
            let spawned = Command::new("git")
                .arg("-C")
                .arg(root)
                .args(["cat-file", "--batch"])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn();
            let Ok(mut child) = spawned else { return Blob::NoRepo };
            let Some(stdout) = child.stdout.take() else { return Blob::NoRepo };
            self.batch = Some((child, std::io::BufReader::new(stdout)));
        }
        let Some((child, stdout)) = self.batch.as_mut() else { return Blob::NoRepo };
        let Some(stdin) = child.stdin.as_mut() else { return Blob::NoRepo };
        if writeln!(stdin, "{spec}").and_then(|_| stdin.flush()).is_err() {
            return Blob::NoRepo;
        }
        let mut header = String::new();
        // Outside a git checkout, git exits at once: the header is end of file.
        if stdout.read_line(&mut header).unwrap_or(0) == 0 {
            return Blob::NoRepo;
        }
        let mut fields = header.split_whitespace();
        let (_, kind, size) = (fields.next(), fields.next(), fields.next());
        if kind != Some("blob") {
            return Blob::Missing;
        }
        let Some(size) = size.and_then(|text| text.parse::<usize>().ok()) else { return Blob::Missing };
        let mut bytes = vec![0; size + 1];
        if stdout.read_exact(&mut bytes).is_err() {
            return Blob::NoRepo;
        }
        bytes.truncate(size);
        Blob::Found(bytes)
    }
}

enum Blob {
    Found(Vec<u8>),
    Missing,
    NoRepo,
}

fn disk(file: &Path) -> Result<Loaded, String> {
    let text = fs::read_to_string(file).map_err(|error| error.to_string())?;
    Ok(Loaded { text, source: "working-tree" })
}

impl Drop for Reader {
    fn drop(&mut self) {
        if let Some((mut child, _)) = self.batch.take() {
            drop(child.stdin.take());
            let _ = child.wait();
        }
    }
}


struct Walk {
    files: Vec<PathBuf>,
    stopped: bool,
}

fn walk_sources(root: &Path, already_over: bool) -> Walk {
    if already_over {
        return Walk { files: Vec::new(), stopped: true };
    }
    if !root.exists() {
        return Walk { files: Vec::new(), stopped: false };
    }
    if root.is_file() {
        let name = root.file_name().and_then(|item| item.to_str()).unwrap_or("");
        return Walk { files: if operators::kind_of(name) { vec![root.to_path_buf()] } else { Vec::new() }, stopped: false };
    }
    let mut files = Vec::new();
    let mut stopped = false;
    fn walk(dir: &Path, files: &mut Vec<PathBuf>, stopped: &mut bool) {
        if *stopped {
            return;
        }
        let mut entries = match fs::read_dir(dir) {
            Ok(entries) => entries.filter_map(|item| item.ok()).collect::<Vec<_>>(),
            Err(_) => return,
        };
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || name == "node_modules" || name == "target" || name == "dist" {
                continue;
            }
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else { continue };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                walk(&path, files, stopped);
            } else if file_type.is_file() {
                let rel = path.to_string_lossy();
                if operators::kind_of(&rel) {
                    files.push(path);
                }
            }
        }
    }
    walk(root, &mut files, &mut stopped);
    Walk { files, stopped }
}

/// `mutants.json` under `out` and `out/mutants`, for a caller that chose its own mutants.
pub fn write_mutants(out: &Path, mutants: &[(Mutant, String)], operators: &str) -> Result<(), String> {
    write_batch(out, mutants, DEFAULT_SEED, operators)
}

fn write_batch(out: &Path, mutants: &[(Mutant, String)], seed: u64, operators: &str) -> Result<(), String> {
    let dir = out.join("mutants");
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(|error| error.to_string())?;
    }
    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let mut rows = Vec::new();
    for (mutant, text) in mutants {
        let patch_name = format!("{}.patch", mutant.id);
        let diff = unified_diff(&mutant.file, text, mutant.start, mutant.end, &mutant.replacement);
        fs::write(dir.join(&patch_name), diff).map_err(|error| error.to_string())?;
        rows.push(mutant_value(mutant, &patch_name));
    }
    let value = Value::object(vec![
        ("schemaVersion", Value::Int(1)),
        ("seed", Value::Int(seed as i64)),
        ("operators", Value::String(operators.into())),
        ("mutants", Value::Array(rows)),
    ]);
    let text = json::stringify(&value);
    fs::write(dir.join("mutants.json"), &text).map_err(|error| error.to_string())?;
    fs::create_dir_all(out).map_err(|error| error.to_string())?;
    fs::write(out.join("mutants.json"), text).map_err(|error| error.to_string())
}

fn mutant_value(mutant: &Mutant, patch: &str) -> Value {
    Value::object(vec![
        ("id", Value::String(mutant.id.clone())),
        ("file", Value::String(mutant.file.clone())),
        ("line", Value::Int(mutant.line as i64)),
        ("start", Value::Int(mutant.start as i64)),
        ("end", Value::Int(mutant.end as i64)),
        ("op", Value::String(mutant.op.clone())),
        ("original", Value::String(mutant.original.clone())),
        ("replacement", Value::String(mutant.replacement.clone())),
        ("patch", Value::String(patch.into())),
    ])
}

fn shown_mutant(mutant: &Mutant) -> Value {
    Value::object(vec![
        ("id", Value::String(mutant.id.clone())),
        ("file", Value::String(mutant.file.clone())),
        ("line", Value::Int(mutant.line as i64)),
        ("op", Value::String(mutant.op.clone())),
    ])
}

/// A one-hunk unified diff of the single edited span. The runner still applies `start`/`end`.
fn unified_diff(rel: &str, before: &str, start: usize, end: usize, replacement: &str) -> String {
    let after = operators::apply(before, start, end, replacement);
    let old_lines: Vec<&str> = before.split('\n').collect();
    let new_lines: Vec<&str> = after.split('\n').collect();
    let mut first = 0;
    while first < old_lines.len() && first < new_lines.len() && old_lines[first] == new_lines[first] {
        first += 1;
    }
    let mut old_last = old_lines.len();
    let mut new_last = new_lines.len();
    while old_last > first && new_last > first && old_lines[old_last - 1] == new_lines[new_last - 1] {
        old_last -= 1;
        new_last -= 1;
    }
    let context = 3usize;
    let old_from = first.saturating_sub(context);
    let new_from = old_from;
    let old_to = (old_last + context).min(old_lines.len());
    let new_to = (new_last + context).min(new_lines.len());
    let mut body = String::new();
    body.push_str("# probatio-mutant direction=forward meaning=apply-to-introduce-the-bug\n");
    body.push_str(&format!("--- a/{rel}\n+++ b/{rel}\n"));
    body.push_str(&format!(
        "@@ -{},{} +{},{} @@\n",
        old_from + 1,
        old_to - old_from,
        new_from + 1,
        new_to - new_from
    ));
    for line in &old_lines[old_from..first] {
        body.push(' ');
        body.push_str(line);
        body.push('\n');
    }
    for line in &old_lines[first..old_last] {
        body.push('-');
        body.push_str(line);
        body.push('\n');
    }
    for line in &new_lines[first..new_last] {
        body.push('+');
        body.push_str(line);
        body.push('\n');
    }
    let tail = old_to.saturating_sub(old_last);
    for line in old_lines.iter().skip(old_last).take(tail) {
        body.push(' ');
        body.push_str(line);
        body.push('\n');
    }
    body
}

fn fresh_run_dir(out: &Path) -> PathBuf {
    let first = out.join("runs");
    if !dir_has_results(&first) {
        return first;
    }
    let mut index = 2;
    loop {
        let candidate = out.join(format!("runs-{index}"));
        if !dir_has_results(&candidate) {
            return candidate;
        }
        index += 1;
    }
}

fn dir_has_results(dir: &Path) -> bool {
    if dir.join("run.json").is_file() {
        return true;
    }
    let results = dir.join("results");
    fs::read_dir(results).ok().is_some_and(|mut entries| entries.any(|item| item.ok().is_some_and(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))))
}

fn discover_label(package: &Path, tests_dir: &str) -> String {
    crate::runner::discover(package).unwrap_or_else(|_| format!("none (tests dir {tests_dir})"))
}

fn rel_to(package: &Path, file: &Path) -> String {
    file.strip_prefix(package).unwrap_or(file).to_string_lossy().replace('\\', "/")
}

fn missing(command: &str, message: &str) -> Envelope {
    Envelope::new(false, command, message.to_string(), "Pass the missing flag and run the command again.")
}

struct Rng {
    state: u32,
}

impl Rng {
    fn new(seed: u32) -> Rng {
        Rng { state: seed }
    }

    fn next(&mut self) -> f64 {
        let product = (self.state as i32).wrapping_mul(1_103_515_245) as i64 + 12_345;
        self.state = product as u32;
        self.state as f64 / 4_294_967_296.0
    }
}

fn shuffle<T>(mut items: Vec<T>, rng: &mut Rng) -> Vec<T> {
    let mut index = items.len();
    while index > 1 {
        index -= 1;
        let swap = (rng.next() * (index as f64 + 1.0)).floor() as usize;
        items.swap(index, swap.min(index));
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_writes_a_java_mutant_and_skips_a_comment() {
        let root = std::env::temp_dir().join(format!("probatio-gen-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("test")).unwrap();
        fs::write(root.join("test/calc.test.js"), "if (a && b) {}\n").unwrap();
        fs::write(
            root.join("src/A.java"),
            "class A {\n  boolean f(boolean a, boolean b) { return a && b; }\n  // a && b\n}\n",
        )
        .unwrap();
        let out = root.join("out");
        let args = vec!["--package".into(), root.display().to_string(), "--out".into(), out.display().to_string()];
        let envelope = generate(&Flags::parse(&args));
        assert!(envelope.ok, "{}", envelope.summary);
        let value = json::parse(&envelope.render("")).unwrap();
        json::envelope_fields(&value).unwrap();
        assert!(envelope.summary.contains("No string-literal mutant"), "{}", envelope.summary);
        let text = fs::read_to_string(out.join("mutants.json")).unwrap();
        let value = json::parse(&text).unwrap();
        let mutants = value.get("mutants").unwrap();
        match mutants {
            Value::Array(items) => {
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].get("op").and_then(Value::as_str), Some("and-to-or"));
                assert_eq!(items[0].get("file").and_then(Value::as_str), Some("src/A.java"));
                let patch = items[0].get("patch").and_then(Value::as_str).unwrap();
                assert!(out.join("mutants").join(patch).is_file(), "{patch}");
            }
            other => panic!("mutants was {other:?}"),
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn generate_returns_an_envelope_when_nbsp_precedes_and() {
        let root = std::env::temp_dir().join(format!("probatio-gen-nbsp-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("src/A.java"),
            "class A {\n  boolean f(boolean a, boolean b) { return a\u{00A0}&& b; }\n}\n",
        )
        .unwrap();
        let out = root.join("out");
        let args = vec!["--package".into(), root.display().to_string(), "--out".into(), out.display().to_string()];
        let envelope = generate(&Flags::parse(&args));
        let value = json::parse(&envelope.render("")).unwrap();
        let (version, ok, summary, next, _next_call) = json::envelope_fields(&value).unwrap();
        assert_eq!(version, 2);
        assert!(ok, "{summary}");
        assert!(!summary.is_empty());
        assert!(!next.is_empty());
        assert!(out.join("mutants.json").is_file());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn generate_states_when_max_mutants_stops_the_walk() {
        let root = std::env::temp_dir().join(format!("probatio-gen-bound-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        let mut body = String::from("class A {\n");
        for index in 0..5 {
            body.push_str(&format!("  boolean x{index} = a && b;\n"));
        }
        body.push_str("}\n");
        fs::write(root.join("src/A.java"), body).unwrap();
        let out = root.join("out");
        let args = vec![
            "--package".into(),
            root.display().to_string(),
            "--out".into(),
            out.display().to_string(),
            "--max-mutants".into(),
            "2".into(),
        ];
        let envelope = generate(&Flags::parse(&args));
        assert!(envelope.ok, "{}", envelope.summary);
        assert!(envelope.summary.contains("--max-mutants"), "{}", envelope.summary);
        assert!(envelope.summary.contains('2'), "{}", envelope.summary);
        assert!(envelope.next.contains("--max-mutants"), "{}", envelope.next);
        let text = fs::read_to_string(out.join("mutants.json")).unwrap();
        let value = json::parse(&text).unwrap();
        match value.get("mutants") {
            Some(Value::Array(items)) => assert_eq!(items.len(), 2),
            other => panic!("mutants was {other:?}"),
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn generate_reads_the_commit_and_refuses_an_untracked_file() {
        let root = std::env::temp_dir().join(format!("probatio-gen-git-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        let _ = Command::new("git").arg("-C").arg(&root).args(["init"]).output();
        let _ = Command::new("git").arg("-C").arg(&root).args(["config", "user.email", "a@b.c"]).output();
        let _ = Command::new("git").arg("-C").arg(&root).args(["config", "user.name", "A"]).output();
        fs::write(root.join("src/A.java"), "class A { boolean f(boolean a, boolean b) { return a && b; } }\n").unwrap();
        let _ = Command::new("git").arg("-C").arg(&root).args(["add", "src/A.java"]).output();
        let _ = Command::new("git").arg("-C").arg(&root).args(["commit", "-m", "add"]).output();
        fs::write(root.join("src/A.java"), "class A { boolean f(boolean a, boolean b) { return a || b; } }\n").unwrap();
        let out = root.join("out");
        let envelope = generate(&Flags::parse(&[
            "--package".into(),
            root.display().to_string(),
            "--out".into(),
            out.display().to_string(),
        ]));
        assert!(envelope.ok, "{}", envelope.summary);
        let text = fs::read_to_string(out.join("mutants/mutants.json")).unwrap();
        assert!(text.contains("&&"), "commit text was not used: {text}");
        fs::write(root.join("src/B.java"), "class B { boolean f(boolean a, boolean b) { return a && b; } }\n").unwrap();
        let bad = generate(&Flags::parse(&[
            "--package".into(),
            root.display().to_string(),
            "--out".into(),
            root.join("bad").display().to_string(),
        ]));
        assert!(!bad.ok, "{}", bad.summary);
        assert!(bad.summary.contains("not in HEAD"), "{}", bad.summary);
        let tree = generate(&Flags::parse(&[
            "--package".into(),
            root.display().to_string(),
            "--out".into(),
            root.join("tree").display().to_string(),
            "--working-tree".into(),
        ]));
        assert!(tree.ok, "{}", tree.summary);
        let _ = fs::remove_dir_all(&root);
    }
}
