use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct Flags {
    pub values: BTreeMap<String, String>,
    pub positionals: Vec<String>,
}

impl Flags {
    pub fn parse(args: &[String]) -> Flags {
        let mut values = BTreeMap::new();
        let mut positionals = Vec::new();
        let mut index = 0;
        while index < args.len() {
            let arg = &args[index];
            if let Some(body) = arg.strip_prefix("--") {
                if let Some((key, value)) = body.split_once('=') {
                    values.insert(key.to_string(), value.to_string());
                } else if args.get(index + 1).is_some_and(|next| !next.starts_with("--")) {
                    values.insert(body.to_string(), args[index + 1].clone());
                    index += 1;
                } else {
                    values.insert(body.to_string(), "true".into());
                }
            } else {
                positionals.push(arg.clone());
            }
            index += 1;
        }
        Flags { values, positionals }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    pub fn need(&self, key: &str) -> Result<&str, String> {
        self.get(key).filter(|value| !value.is_empty()).ok_or_else(|| format!("missing --{key}"))
    }

    pub fn int(&self, key: &str, default: u64) -> u64 {
        self.get(key).and_then(|value| value.parse().ok()).unwrap_or(default)
    }
}

/// `(name, required)`. `required` is true when a missing flag is an error.
pub fn command_flags(command: &str) -> Option<&'static [(&'static str, bool)]> {
    match command {
        "mutate.generate" => Some(&[
            ("package", true),
            ("out", true),
            ("src", false),
            ("max-mutants", false),
            ("operators", false),
        ]),
        "mutate.run" => Some(&[
            ("package", true),
            ("patches", true),
            ("out", true),
            ("suite-command", false),
            ("line-map", false),
            ("timeout-ms", false),
            ("tests", false),
            ("max-selected", false),
            ("confirm-count", false),
            ("workers", false),
        ]),
        "mutate.sealed" => Some(&[("package", true), ("out", true)]),
        "mutate.tally" => Some(&[("out", true), ("state", false), ("root", false)]),
        "mutate.help" => Some(&[]),
        "ledger.build" => Some(&[("package", true), ("out", true), ("commit", true)]),
        "ledger.check" => Some(&[("ledger", true)]),
        "matrix.report" => Some(&[("out", true)]),
        "golden.check" => Some(&[("recorded", true)]),
        "golden.record" => Some(&[("package", true), ("out", true), ("dir", false)]),
        "golden.compare" => Some(&[("golden", true), ("actual", true)]),
        "gap.fix" => Some(&[("id", true)]),
        "gap.revert" => Some(&[("id", true)]),
        "guard.check" => Some(&[("root", true), ("guard", false)]),
        "findings.add" => Some(&[
            ("state", true),
            ("reason", true),
            ("out", false),
            ("root", false),
            ("text", false),
        ]),
        "status" => Some(&[("state", true), ("out", false), ("root", false)]),
        "seal" => Some(&[("state", true), ("out", false), ("root", false)]),
        "queue.seed" => Some(&[("state", true), ("id", false), ("out", false), ("root", false)]),
        "queue.claim" => Some(&[("state", true), ("out", false), ("root", false)]),
        "queue.reap" => Some(&[("state", true), ("out", false), ("root", false)]),
        "check-kill" => Some(&[("out", true), ("id", true)]),
        "verify-change" => Some(&[("package", true), ("suite-command", false), ("timeout-ms", false)]),
        "schema" => Some(&[]),
        "mcp" => Some(&[]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::command_flags;

    #[test]
    fn command_flags_match_schema_files() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas");
        let mut paths = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap_or_else(|error| panic!("{}: {error}", dir.display())) {
            let path = entry.unwrap_or_else(|error| panic!("{}: {error}", dir.display())).path();
            let name = path.file_name().and_then(|item| item.to_str()).unwrap_or("");
            if name.ends_with(".schema.json") {
                paths.push(path);
            }
        }
        paths.sort();
        assert!(!paths.is_empty(), "no schema files in {}", dir.display());
        for path in paths {
            let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            let value = crate::json::parse(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            let command = value
                .get("properties")
                .and_then(|item| item.get("command"))
                .and_then(|item| item.get("const"))
                .and_then(|item| item.as_str())
                .unwrap_or_else(|| panic!("{} has no command const", path.display()));
            let flags = value.get("flags").unwrap_or_else(|| panic!("{} has no flags key", path.display()));
            let crate::json::Value::Array(items) = flags else {
                panic!("{} flags is not an array", path.display());
            };
            let mut rows = Vec::with_capacity(items.len());
            for item in items {
                let name = item
                    .get("name")
                    .and_then(|value| value.as_str())
                    .unwrap_or_else(|| panic!("{} flag is missing name", path.display()));
                let required = item.get("required").and_then(|value| value.as_bool()).unwrap_or_else(|| {
                    panic!("{command} flag {name} is missing required")
                });
                rows.push((name, required));
            }
            let Some(table) = command_flags(command) else {
                panic!("{command} is not in command_flags");
            };
            assert_eq!(rows.as_slice(), table, "{command}");
        }
    }
}
