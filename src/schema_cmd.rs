//! The published JSON Schemas, compiled into the binary so `schema` works outside the repo.

use crate::envelope::Envelope;
use crate::json::{self, Value};

const SCHEMAS: &[(&str, &str)] = &[
    ("check-kill", include_str!("../schemas/check-kill.schema.json")),
    ("findings.add", include_str!("../schemas/findings.add.schema.json")),
    ("gap.fix", include_str!("../schemas/gap.fix.schema.json")),
    ("gap.revert", include_str!("../schemas/gap.revert.schema.json")),
    ("golden.check", include_str!("../schemas/golden.check.schema.json")),
    ("golden.compare", include_str!("../schemas/golden.compare.schema.json")),
    ("golden.record", include_str!("../schemas/golden.record.schema.json")),
    ("guard.check", include_str!("../schemas/guard.check.schema.json")),
    ("ledger.build", include_str!("../schemas/ledger.build.schema.json")),
    ("ledger.check", include_str!("../schemas/ledger.check.schema.json")),
    ("matrix.report", include_str!("../schemas/matrix.report.schema.json")),
    ("mcp", include_str!("../schemas/mcp.schema.json")),
    ("mutate.generate", include_str!("../schemas/mutate.generate.schema.json")),
    ("mutate.help", include_str!("../schemas/mutate.help.schema.json")),
    ("mutate.run", include_str!("../schemas/mutate.run.schema.json")),
    ("mutate.sealed", include_str!("../schemas/mutate.sealed.schema.json")),
    ("mutate.tally", include_str!("../schemas/mutate.tally.schema.json")),
    ("queue.claim", include_str!("../schemas/queue.claim.schema.json")),
    ("queue.reap", include_str!("../schemas/queue.reap.schema.json")),
    ("queue.seed", include_str!("../schemas/queue.seed.schema.json")),
    ("schema", include_str!("../schemas/schema.schema.json")),
    ("seal", include_str!("../schemas/seal.schema.json")),
    ("status", include_str!("../schemas/status.schema.json")),
    ("verify-change", include_str!("../schemas/verify-change.schema.json")),
];

pub fn list_names() -> Vec<String> {
    let mut names = SCHEMAS.iter().map(|(name, _)| (*name).to_string()).collect::<Vec<_>>();
    names.sort();
    names
}

pub fn run(name: Option<&str>) -> Envelope {
    let names = list_names();
    let listed = Value::Array(names.iter().cloned().map(Value::String).collect());
    let Some(name) = name.filter(|item| !item.is_empty()) else {
        return Envelope::new(
            !names.is_empty(),
            "schema",
            format!("{} commands have a published schema.", names.len()),
            "Pass one name, for example: probatio schema mutate.run",
        )
        .with_extra("commands", listed);
    };
    let Some((_, body)) = SCHEMAS.iter().find(|(item, _)| *item == name) else {
        return Envelope::new(
            false,
            "schema",
            format!("No schema named {name}."),
            format!("Use one of: {}.", names.join(", ")),
        )
        .with_extra("commands", listed);
    };
    let schema = match json::parse(body) {
        Ok(value) => value,
        Err(error) => {
            return Envelope::new(false, "schema", format!("Schema {name} did not parse: {error}"), "The embedded schema is damaged.");
        }
    };
    Envelope::new(
        true,
        "schema",
        format!("JSON Schema for {name}, schemaVersion 2."),
        "Validate a command's stdout against it. A new field does not change schemaVersion. A renamed or removed one does.",
    )
    .with_extra("commands", listed)
    .with_extra("schema", schema)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_lists_commands_and_returns_a_body_without_the_repo_cwd() {
        let listed = run(None);
        assert!(listed.ok, "{}", listed.summary);
        assert!(listed.summary.contains("24 commands"), "{}", listed.summary);
        let one = run(Some("mutate.run"));
        assert!(one.ok, "{}", one.summary);
        let value = json::parse(&one.render("")).unwrap();
        assert!(value.get("schema").and_then(|item| item.get("properties")).is_some());
        let missing = run(Some("nope"));
        assert!(!missing.ok);
    }
}
