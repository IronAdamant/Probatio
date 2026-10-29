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
    let mut found = Vec::new();
    if let Err(message) = walk(&root, &package, &mut found) {
        return Envelope::new(false, command, message, "Fix the source tree and generate again.");
    }
    found.sort_by(|left, right| left.file.cmp(&right.file).then(left.start.cmp(&right.start)).then(left.op.cmp(&right.op)));
    let limit = flags.int("max-mutants", u64::MAX);
    if (found.len() as u64) > limit {
        found.truncate(limit as usize);
    }
    if let Err(message) = write_mutants(&out, &found) {
        return Envelope::new(false, command, message, "The out directory could not be written.");
    }
    let summary = format!("{} mutants.", found.len());
    let next = if found.is_empty() {
        "Nothing to run.".to_string()
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

fn walk(dir: &Path, package: &Path, found: &mut Vec<Mutant>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|error| error.to_string())?;
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
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
            walk(&path, package, found)?;
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
    }
    Ok(())
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
}
