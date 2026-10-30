//! `mutate generate` writes text-operator mutants. It does not run a suite.

use std::fs;
use std::path::{Path, PathBuf};

use crate::args::Flags;
use crate::envelope::Envelope;
use crate::json::{self, Value};
use crate::operators::{self, Mutant};

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
    let root = match flags.get("src") {
        Some(src) => package.join(src),
        None => package.clone(),
    };
    if !root.is_dir() {
        return missing(command, "src is not a directory");
    }
    let limit = flags.int("max-mutants", u64::MAX) as usize;
    let mut found = Vec::new();
    let bounded = match walk(&root, &package, &mut found, limit) {
        Ok(stopped) => stopped,
        Err(message) => return Envelope::new(false, command, message, "Fix the source tree and generate again."),
    };
    found.sort_by(|left, right| left.file.cmp(&right.file).then(left.start.cmp(&right.start)).then(left.op.cmp(&right.op)));
    if found.len() > limit {
        found.truncate(limit);
    }
    if let Err(message) = write_mutants(&out, &found) {
        return Envelope::new(false, command, message, "The out directory could not be written.");
    }
    let summary = if bounded {
        format!("{} mutants. Bounded by --max-mutants {}.", found.len(), limit)
    } else {
        format!("{} mutants.", found.len())
    };
    let next = if found.is_empty() {
        "Nothing to run.".to_string()
    } else if bounded {
        format!("Bounded by --max-mutants {limit}. The walk stopped early. mutate run can score this slice. A kill is an assertion failure.")
    } else {
        "mutate run scores this patch set. A kill is an assertion failure.".to_string()
    };
    let mut envelope = Envelope::new(true, command, summary, next).with_extra("mutants", Value::Int(found.len() as i64));
    if !found.is_empty() {
        envelope.next_call = Some(vec![
            "mutate".into(),
            "run".into(),
            "--package".into(),
            package.display().to_string(),
            "--patches".into(),
            out.display().to_string(),
            "--out".into(),
            format!("{}-run", out.display()),
        ]);
    }
    envelope
}

fn missing(command: &str, message: &str) -> Envelope {
    Envelope::new(false, command, message.to_string(), "Pass the missing flag and run the command again.")
}

fn walk(dir: &Path, package: &Path, found: &mut Vec<Mutant>, limit: usize) -> Result<bool, String> {
    let mut entries = fs::read_dir(dir).map_err(|error| error.to_string())?.collect::<Result<Vec<_>, _>>().map_err(|error| error.to_string())?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name == "target" || name == "node_modules" || name == "dist" {
            continue;
        }
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            if walk(&path, package, found, limit)? {
                return Ok(true);
            }
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let relative = path.strip_prefix(package).unwrap_or(&path);
        let relative = relative.to_string_lossy().replace('\\', "/");
        if !operators::kind_of(&relative) {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else { continue };
        found.extend(operators::mutants_in(&relative, &text));
        if found.len() >= limit {
            found.truncate(limit);
            return Ok(true);
        }
    }
    Ok(false)
}

fn write_mutants(out: &Path, mutants: &[Mutant]) -> Result<(), String> {
    fs::create_dir_all(out).map_err(|error| error.to_string())?;
    let items: Vec<Value> = mutants.iter().map(mutant_value).collect();
    let value = Value::object(vec![("mutants", Value::Array(items))]);
    fs::write(out.join("mutants.json"), json::stringify(&value)).map_err(|error| error.to_string())
}

fn mutant_value(mutant: &Mutant) -> Value {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_writes_a_java_mutant_and_skips_a_comment() {
        let root = std::env::temp_dir().join(format!("probatio-gen-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("src/A.java"),
            "class A {\n  boolean f(boolean a, boolean b) { return a && b; }\n  // a && b\n}\n",
        )
        .unwrap();
        let out = root.join("out");
        let args = vec![
            "--package".into(),
            root.display().to_string(),
            "--out".into(),
            out.display().to_string(),
        ];
        let envelope = generate(&Flags::parse(&args));
        assert!(envelope.ok, "{}", envelope.summary);
        let value = json::parse(&envelope.render("")).unwrap();
        json::envelope_fields(&value).unwrap();
        let text = fs::read_to_string(out.join("mutants.json")).unwrap();
        let value = json::parse(&text).unwrap();
        let mutants = value.get("mutants").unwrap();
        match mutants {
            Value::Array(items) => {
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].get("op").and_then(Value::as_str), Some("and-to-or"));
                assert_eq!(items[0].get("file").and_then(Value::as_str), Some("src/A.java"));
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
        let args = vec![
            "--package".into(),
            root.display().to_string(),
            "--out".into(),
            out.display().to_string(),
        ];
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
}
