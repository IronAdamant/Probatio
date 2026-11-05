//! One flag table for every command. Unknown flags are refused before any work.
//! `--no-name` stores `name` as false. A repeated flag keeps every value; readers use the last.

use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
enum FlagValue {
    Text(String),
    Bool(bool),
}

#[derive(Clone, Debug)]
pub struct Flags {
    values: BTreeMap<String, Vec<FlagValue>>,
    pub positionals: Vec<String>,
}

impl Flags {
    pub fn parse(args: &[String]) -> Flags {
        let mut values: BTreeMap<String, Vec<FlagValue>> = BTreeMap::new();
        let mut positionals = Vec::new();
        let mut seen_flag = false;
        let mut index = 0;
        while index < args.len() {
            let token = &args[index];
            if token == "--" {
                seen_flag = true;
                index += 1;
                continue;
            }
            if !token.starts_with("--") {
                if !seen_flag {
                    positionals.push(token.clone());
                } else {
                    values.entry("_".into()).or_default().push(FlagValue::Text(token.clone()));
                }
                index += 1;
                continue;
            }
            seen_flag = true;
            let body = &token[2..];
            if let Some((key, value)) = body.split_once('=') {
                values.entry(key.to_string()).or_default().push(FlagValue::Text(value.to_string()));
                index += 1;
                continue;
            }
            if let Some(key) = body.strip_prefix("no-") {
                if !key.is_empty() {
                    values.entry(key.to_string()).or_default().push(FlagValue::Bool(false));
                    index += 1;
                    continue;
                }
            }
            match args.get(index + 1) {
                Some(next) if !next.starts_with("--") => {
                    values.entry(body.to_string()).or_default().push(FlagValue::Text(next.clone()));
                    index += 1;
                }
                _ => values.entry(body.to_string()).or_default().push(FlagValue::Bool(true)),
            }
            index += 1;
        }
        Flags { values, positionals }
    }

    pub fn human(&self) -> bool {
        self.bool("human", false)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        let last = self.values.get(key)?.last()?;
        match last {
            FlagValue::Text(text) => Some(text.as_str()),
            FlagValue::Bool(true) => Some("true"),
            FlagValue::Bool(false) => None,
        }
    }

    pub fn texts(&self, key: &str) -> Vec<String> {
        self.values
            .get(key)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| match item {
                        FlagValue::Text(text) => Some(text.clone()),
                        FlagValue::Bool(_) => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn bool(&self, key: &str, fallback: bool) -> bool {
        let Some(last) = self.values.get(key).and_then(|items| items.last()) else {
            return fallback;
        };
        match last {
            FlagValue::Bool(value) => *value,
            FlagValue::Text(text) if text == "true" => true,
            FlagValue::Text(text) if text == "false" => false,
            FlagValue::Text(_) => fallback,
        }
    }

    pub fn need(&self, key: &str) -> Result<&str, String> {
        self.get(key).filter(|value| !value.is_empty()).ok_or_else(|| format!("missing --{key}"))
    }

    /// Last value, or `default` when the flag was omitted. A present non-integer is an error.
    pub fn parse_int(&self, key: &str) -> Result<Option<u64>, String> {
        match self.get(key) {
            None => Ok(None),
            Some(raw) => raw
                .parse::<u64>()
                .map(Some)
                .map_err(|_| format!("--{key} must be a non-negative integer")),
        }
    }

    pub fn int(&self, key: &str, default: u64) -> Result<u64, String> {
        Ok(self.parse_int(key)?.unwrap_or(default))
    }

    pub fn unknown(&self, known: &[(&str, bool)]) -> Vec<String> {
        let mut names = Vec::new();
        for key in self.values.keys() {
            if key == "human" || key == "_" {
                continue;
            }
            if known.iter().any(|(name, _)| *name == key) {
                continue;
            }
            names.push(key.clone());
        }
        names.sort();
        names
    }
}

/// `(name, required)`. This table is what the CLI refuses against, and what each schema file lists.
pub fn command_flags(command: &str) -> Option<&'static [(&'static str, bool)]> {
    match command {
        "mutate.diff" => Some(&[
            ("package", true),
            ("since", false),
            ("out", false),
            ("src", false),
            ("suite-command", false),
            ("timeout-ms", false),
            ("workers", false),
            ("confirm", false),
            ("max-mutants", false),
            ("operators", false),
        ]),
        "mutate.generate" => Some(&[
            ("package", true),
            ("out", true),
            ("per-file", false),
            ("src", false),
            ("skip", false),
            ("seed", false),
            ("max-mutants", false),
            ("max-minutes", false),
            ("skip-file", false),
            ("commit", false),
            ("working-tree", false),
            ("operators", false),
            ("tests-dir", false),
        ]),
        "mutate.run" => Some(&[
            ("hide", false),
            ("only-test", false),
            ("suite-command", false),
            ("package", true),
            ("patches", true),
            ("out", true),
            ("repo", false),
            ("workers", false),
            ("direction", false),
            ("concurrency", false),
            ("max-minutes", false),
            ("max-mutants", false),
            ("build", false),
            ("tests-dir", false),
            ("unset", false),
            ("suite-timeout-ms", false),
            ("test-timeout-ms", false),
            ("confirm", false),
            ("affected", false),
            ("agent", false),
            ("history", false),
            ("timeout-multiple", false),
            ("timeout-floor-ms", false),
            ("budget-ms", false),
            ("commit", false),
            ("line-map", false),
            ("timeout-ms", false),
            ("tests", false),
            ("max-selected", false),
            ("confirm-count", false),
        ]),
        "mutate.sealed" => Some(&[
            ("hide", false),
            ("only-test", false),
            ("suite-command", false),
            ("package", true),
            ("patches", true),
            ("out", true),
            ("repo", false),
            ("workers", false),
            ("direction", false),
            ("concurrency", false),
            ("max-minutes", false),
            ("max-mutants", false),
            ("build", false),
            ("tests-dir", false),
            ("unset", false),
            ("suite-timeout-ms", false),
            ("test-timeout-ms", false),
            ("confirm", false),
            ("affected", false),
            ("agent", false),
            ("history", false),
            ("timeout-multiple", false),
            ("timeout-floor-ms", false),
            ("budget-ms", false),
            ("commit", false),
            ("line-map", false),
            ("timeout-ms", false),
            ("tests", false),
            ("max-selected", false),
            ("confirm-count", false),
            ("label", true),
            ("fix", false),
        ]),
        "mutate.tally" => Some(&[("out", true), ("state", false), ("root", false)]),
        "mutate.help" => Some(&[]),
        "ledger.build" => Some(&[
            ("package", true),
            ("out", true),
            ("repo", false),
            ("commit", false),
            ("src", false),
            ("tests-dir", false),
            ("max-lines", false),
            ("max-commits", false),
        ]),
        "ledger.check" => Some(&[
            ("package", true),
            ("repo", false),
            ("commit", false),
            ("ledger", false),
            ("out", true),
            ("update", false),
            ("message", false),
            ("concurrency", false),
            ("suite-timeout-ms", false),
            ("test-timeout-ms", false),
        ]),
        "matrix.report" => Some(&[
            ("scores", true),
            ("direct", true),
            ("out", true),
            ("kills", false),
            ("patches", false),
        ]),
        "golden.check" => Some(&[
            ("recorded", true),
            ("update", false),
            ("actual", true),
            ("message", false),
            ("root", false),
        ]),
        "golden.record" => Some(&[
            ("package", true),
            ("repo", false),
            ("commit", false),
            ("module", false),
            ("tests", false),
            ("dir", false),
            ("suite-timeout-ms", false),
            ("out", false),
        ]),
        "golden.compare" => Some(&[
            ("package", true),
            ("repo", false),
            ("commit", false),
            ("module", false),
            ("tests", false),
            ("golden", false),
            ("out", true),
            ("operators", false),
            ("suite-timeout-ms", false),
            ("actual", false),
        ]),
        "gap.fix" => Some(&[
            ("state", true),
            ("line", true),
            ("id", true),
            ("file", true),
            ("guard", true),
            ("commit", true),
            ("root", false),
        ]),
        "gap.revert" => Some(&[("state", true), ("id", true)]),
        "guard.check" => Some(&[("state", true), ("guard", true), ("message", false), ("root", false)]),
        "findings.add" => Some(&[
            ("state", true),
            ("status", true),
            ("id", true),
            ("reason", true),
            ("agent", false),
            ("file", false),
            ("line", false),
            ("root", false),
            ("out", false),
            ("text", false),
        ]),
        "status" => Some(&[
            ("state", true),
            ("root", false),
            ("page", false),
            ("scores", false),
            ("agent", false),
            ("out", false),
        ]),
        "seal" => Some(&[
            ("state", true),
            ("from", false),
            ("id", false),
            ("fraction", false),
            ("seed", false),
            ("out", false),
        ]),
        "queue.seed" => Some(&[("state", true), ("id", false), ("out", false), ("root", false)]),
        "queue.claim" => Some(&[
            ("state", true),
            ("now", false),
            ("lease-ms", false),
            ("id", true),
            ("agent", true),
            ("out", false),
            ("root", false),
        ]),
        "queue.reap" => Some(&[("state", true), ("now", false), ("out", false), ("root", false)]),
        "check-kill" => Some(&[
            ("package", true),
            ("patches", true),
            ("repo", false),
            ("commit", false),
            ("out", true),
            ("max-minutes", false),
            ("build", false),
            ("tests-dir", false),
            ("suite-timeout-ms", false),
            ("test-timeout-ms", false),
            ("confirm", false),
            ("agent", false),
            ("id", false),
        ]),
        "verify-change" => Some(&[
            ("package", true),
            ("operators", false),
            ("repo", false),
            ("out", true),
            ("base", false),
            ("commit", false),
            ("max-mutants", false),
            ("max-minutes", false),
            ("max-tests", false),
            ("suite-command", false),
            ("timeout-ms", false),
        ]),
        "schema" => Some(&[]),
        "mcp" => Some(&[]),
        _ => None,
    }
}

pub fn unknown_summary(names: &[String]) -> String {
    let named = names.iter().map(|name| format!("--{name}")).collect::<Vec<_>>().join(", ");
    if names.len() == 1 {
        format!("Unknown flag {named}.")
    } else {
        format!("Unknown flags {named}.")
    }
}

#[cfg(test)]
mod tests {
    use super::{command_flags, Flags};

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

    #[test]
    fn negation_repeat_and_human() {
        let flags = Flags::parse(&[
            "mutate".into(),
            "run".into(),
            "--patches".into(),
            "a".into(),
            "--patches".into(),
            "b".into(),
            "--no-confirm".into(),
            "--human".into(),
        ]);
        assert_eq!(flags.positionals, ["mutate", "run"]);
        assert_eq!(flags.texts("patches"), ["a", "b"]);
        assert!(!flags.bool("confirm", true));
        assert!(flags.human());
    }
}
