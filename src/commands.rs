//! Every command returns one envelope. None of these names is an unknown command.

use std::fs;
use std::path::{Path, PathBuf};

use crate::args::Flags;
use crate::decide::{self, Verdict};
use crate::envelope::Envelope;
use crate::generate;
use crate::json::{self, Value};
use crate::runner;

/// Invocations that must each return an envelope. Flags may be missing on purpose.
pub const INVOCATIONS: &[&[&str]] = &[
    &["mutate", "generate"],
    &["mutate", "run"],
    &["mutate", "sealed"],
    &["mutate", "tally"],
    &["ledger", "build"],
    &["ledger", "check"],
    &["matrix", "report"],
    &["golden", "check"],
    &["golden", "record"],
    &["golden", "compare"],
    &["gap", "fix"],
    &["gap", "revert"],
    &["guard", "check"],
    &["findings", "add"],
    &["status"],
    &["seal"],
    &["queue", "seed"],
    &["queue", "claim"],
    &["queue", "reap"],
    &["check-kill"],
    &["verify-change"],
    &["schema"],
];

pub fn dispatch(args: &[String]) -> Envelope {
    if args.is_empty() || args[0] == "help" || args[0] == "--help" || args[0] == "-h" {
        return help();
    }
    let group = args[0].as_str();
    let rest = &args[1..];
    match group {
        "mutate" => mutate(rest),
        "ledger" => ledger(rest),
        "matrix" => matrix(rest),
        "golden" => golden(rest),
        "gap" => gap(rest),
        "guard" => guard(rest),
        "findings" => findings(rest),
        "status" => status_command(&Flags::parse(rest)),
        "seal" => seal(&Flags::parse(rest)),
        "queue" => queue(rest),
        "check-kill" => check_kill(&Flags::parse(rest)),
        "verify-change" => verify_change(&Flags::parse(rest)),
        "schema" => schema(rest),
        "mcp" => Envelope::new(
            false,
            "mcp",
            "The MCP server reads stdin. Launch `probatio mcp`.",
            "This call is not the server. A client speaks JSON-RPC on that process.",
        ),
        other => Envelope::new(
            false,
            other,
            format!("No handler for {other}."),
            "Use mutate, ledger, matrix, golden, gap, guard, findings, status, seal, queue, check-kill, verify-change, or schema.",
        ),
    }
}

fn help() -> Envelope {
    Envelope::new(
        true,
        "help",
        "Probatio scores a suite. A kill is an assertion failure.",
        "Use mutate generate, mutate run, mutate sealed, mutate tally, ledger, matrix, golden, gap, guard, findings, status, seal, queue, check-kill, verify-change, or schema.",
    )
}

fn mutate(args: &[String]) -> Envelope {
    let action = args.first().map(String::as_str).unwrap_or("");
    let flags = Flags::parse(if args.len() > 1 { &args[1..] } else { &[] });
    match action {
        "generate" => generate::generate(&flags),
        "run" => runner::run(&flags),
        "sealed" => sealed(&flags),
        "tally" => tally(&flags),
        "" => missing("mutate", "missing generate, run, sealed, or tally"),
        other => Envelope::new(
            false,
            &format!("mutate.{other}"),
            format!("No mutate handler for {other}."),
            "Use generate, run, sealed, or tally.",
        ),
    }
}

fn missing(command: &str, message: &str) -> Envelope {
    Envelope::new(false, command, message.to_string(), "Pass the missing flag and run the command again.")
}

fn need<'a>(flags: &'a Flags, key: &str, command: &str) -> Result<&'a str, Envelope> {
    flags.need(key).map_err(|message| missing(command, &message))
}

fn state_dir(flags: &Flags, command: &str) -> Result<PathBuf, Envelope> {
    flags
        .get("state")
        .or_else(|| flags.get("out"))
        .or_else(|| flags.get("root"))
        .map(PathBuf::from)
        .ok_or_else(|| missing(command, "missing --state"))
}

pub fn tally(flags: &Flags) -> Envelope {
    let command = "mutate.tally";
    let out = match state_dir(flags, command) {
        Ok(path) => path,
        Err(envelope) => return envelope,
    };
    let rows = runner::verdicts_in(&out);
    if rows.is_empty() {
        return Envelope::new(false, command, "No mutant results in that directory.", "Run mutate run before tally. Nothing was deleted.");
    }
    let mut keep = Vec::new();
    let mut seen = Vec::new();
    for (kind, tests) in &rows {
        for test in tests {
            if !seen.contains(test) {
                seen.push(test.clone());
            }
            if *kind == Verdict::Killed && !keep.contains(test) {
                keep.push(test.clone());
            }
        }
    }
    let no_kills_yet: Vec<String> = seen.into_iter().filter(|test| !keep.contains(test)).collect();
    let verdicts = rows.iter().map(|(kind, _)| *kind).collect::<Vec<_>>();
    let counts = decide::counts_of(&verdicts);
    let deleted = decide::deleted_tests();
    Envelope::new(true, command, decide::campaign_summary(&counts), "deletedTests is 0. noKillsYet is not a deletion list.")
        .with_extra("deletedTests", Value::Int(deleted as i64))
        .with_extra(
            "pruning",
            Value::object(vec![
                ("mode", Value::String("advisory".into())),
                ("deletedTests", Value::Int(deleted as i64)),
                ("advice", Value::Array(Vec::new())),
            ]),
        )
        .with_extra("keep", Value::Array(keep.into_iter().map(Value::String).collect()))
        .with_extra("noKillsYet", Value::Array(no_kills_yet.into_iter().map(Value::String).collect()))
}

fn sealed(flags: &Flags) -> Envelope {
    let command = "mutate.sealed";
    let package = match need(flags, "package", command) {
        Ok(value) => value,
        Err(envelope) => return envelope,
    };
    let out = match need(flags, "out", command) {
        Ok(value) => value,
        Err(envelope) => return envelope,
    };
    if !Path::new(package).is_dir() {
        return missing(command, "package is not a directory");
    }
    let record = Value::object(vec![
        ("caught", Value::Bool(false)),
        ("restored", Value::Bool(false)),
        ("package", Value::String(package.into())),
    ]);
    if let Err(error) = write_json(&PathBuf::from(out).join("sealed.json"), &record) {
        return Envelope::new(false, command, error, "The sealed record could not be written.");
    }
    Envelope::new(
        true,
        command,
        "The hidden test stayed hidden. No catch was recorded.",
        "A sealed miss stays a miss. This command does not restore a hidden test.",
    )
    .with_extra("miss", Value::Bool(true))
    .with_extra("caught", Value::Bool(false))
}

fn ledger(args: &[String]) -> Envelope {
    let action = args.first().map(String::as_str).unwrap_or("");
    let flags = Flags::parse(if args.len() > 1 { &args[1..] } else { &[] });
    match action {
        "build" => ledger_build(&flags),
        "check" => ledger_check(&flags),
        "" => missing("ledger", "missing build or check"),
        other => Envelope::new(false, &format!("ledger.{other}"), format!("No ledger handler for {other}."), "Use build or check."),
    }
}

fn ledger_build(flags: &Flags) -> Envelope {
    let command = "ledger.build";
    let package = match need(flags, "package", command) {
        Ok(value) => value,
        Err(envelope) => return envelope,
    };
    let out = match need(flags, "out", command) {
        Ok(value) => value,
        Err(envelope) => return envelope,
    };
    if !Path::new(package).is_dir() {
        return missing(command, "package is not a directory");
    }
    let commit = flags.get("commit").unwrap_or("").to_string();
    if commit.is_empty() {
        return missing(command, "missing --commit");
    }
    let record = Value::object(vec![
        ("commit", Value::String(commit)),
        ("reverted", Value::Bool(false)),
    ]);
    if let Err(error) = write_json(&PathBuf::from(out).join("ledger.json"), &record) {
        return Envelope::new(false, command, error, "The ledger file could not be written.");
    }
    Envelope::new(
        true,
        command,
        "Recorded the commit id. The working tree was not reverted.",
        "A ledger entry is a record. This command does not rewrite the checkout.",
    )
}

pub fn ledger_check(flags: &Flags) -> Envelope {
    let command = "ledger.check";
    let ledger = match need(flags, "ledger", command) {
        Ok(value) => PathBuf::from(value),
        Err(envelope) => return envelope,
    };
    let text = match fs::read_to_string(&ledger) {
        Ok(text) => text,
        Err(_) => return Envelope::new(false, command, "The ledger file is missing.", "Pass --ledger. Nothing was re-recorded."),
    };
    let red = text.lines().any(|line| line.trim() == "red") || text.contains("\"status\": \"red\"") || text.contains("\"status\":\"red\"");
    if red {
        return Envelope::new(
            false,
            command,
            "A fixed bug is back in reach of the suite.",
            "Do not re-record it away. Find the test that stopped guarding it, or give the reason in a Golden-Change: line.",
        );
    }
    Envelope::new(true, command, "Ledger entries still stand.", "A red check is a regression. This one was not red, and the file was not rewritten.")
}

fn matrix(args: &[String]) -> Envelope {
    let action = args.first().map(String::as_str).unwrap_or("");
    let flags = Flags::parse(if args.len() > 1 { &args[1..] } else { &[] });
    match action {
        "report" => matrix_report(&flags),
        "" => missing("matrix", "missing report"),
        other => Envelope::new(false, &format!("matrix.{other}"), format!("No matrix handler for {other}."), "Use report."),
    }
}

fn matrix_report(flags: &Flags) -> Envelope {
    let command = "matrix.report";
    let out = match need(flags, "out", command) {
        Ok(value) => PathBuf::from(value),
        Err(envelope) => return envelope,
    };
    let rows = runner::verdicts_in(&out);
    if rows.is_empty() {
        return Envelope::new(false, command, "No scores in that directory.", "Pass --out from a finished run. No kill list was printed.");
    }
    let counts = decide::counts_of(&rows.iter().map(|(kind, _)| *kind).collect::<Vec<_>>());
    let summary = decide::campaign_summary(&counts);
    let _ = write_json(&out.join("matrix.json"), &Value::object(vec![("summary", Value::String(summary.clone()))]));
    Envelope::new(true, command, summary, "The report is matrix.json. Stdout has the counts, not the kill list.")
}

fn golden(args: &[String]) -> Envelope {
    let action = args.first().map(String::as_str).unwrap_or("");
    let flags = Flags::parse(if args.len() > 1 { &args[1..] } else { &[] });
    match action {
        "check" => golden_check(&flags),
        "record" => golden_record(&flags),
        "compare" => golden_compare(&flags),
        "" => missing("golden", "missing check, record, or compare"),
        other => Envelope::new(false, &format!("golden.{other}"), format!("No golden handler for {other}."), "Use check, record, or compare."),
    }
}

fn golden_check(flags: &Flags) -> Envelope {
    let command = "golden.check";
    let recorded = match need(flags, "recorded", command) {
        Ok(value) => PathBuf::from(value),
        Err(envelope) => return envelope,
    };
    if !recorded.is_file() {
        return Envelope::new(false, command, "The golden file is missing.", "Pass --recorded. Nothing was deleted.");
    }
    Envelope::new(true, command, "Golden file is present.", "golden compare deletes nothing.")
}

fn golden_record(flags: &Flags) -> Envelope {
    let command = "golden.record";
    let package = match need(flags, "package", command) {
        Ok(value) => value.to_string(),
        Err(envelope) => return envelope,
    };
    let out = match flags.get("out").or_else(|| flags.get("dir")) {
        Some(value) => PathBuf::from(value),
        None => return missing(command, "missing --out"),
    };
    let record = Value::object(vec![
        ("package", Value::String(package)),
        ("deleted", Value::Bool(false)),
    ]);
    if let Err(error) = write_json(&out.join("golden.json"), &record) {
        return Envelope::new(false, command, error, "The golden file could not be written.");
    }
    Envelope::new(true, command, "Recorded the golden note.", "Nothing was deleted. A kill is not proof the expected result changed.")
}

pub fn golden_compare(flags: &Flags) -> Envelope {
    let command = "golden.compare";
    let golden = match need(flags, "golden", command) {
        Ok(value) => PathBuf::from(value),
        Err(envelope) => return envelope,
    };
    let actual = match need(flags, "actual", command) {
        Ok(value) => PathBuf::from(value),
        Err(envelope) => return envelope,
    };
    let left = fs::read_to_string(&golden).unwrap_or_default();
    let right = fs::read_to_string(&actual).unwrap_or_default();
    if !golden.is_file() || !actual.is_file() {
        return Envelope::new(false, command, "Golden compare needs both files.", "Nothing was deleted.");
    }
    if left == right {
        Envelope::new(true, command, "Golden matches.", "Nothing was deleted.")
    } else {
        Envelope::new(false, command, "Golden differs.", "Nothing was deleted. Say why in a Golden-Change: line before recording again.")
    }
}

fn gap(args: &[String]) -> Envelope {
    let action = args.first().map(String::as_str).unwrap_or("");
    let flags = Flags::parse(if args.len() > 1 { &args[1..] } else { &[] });
    match action {
        "fix" => gap_fix(&flags),
        "revert" => gap_revert(&flags),
        "" => missing("gap", "missing fix or revert"),
        other => Envelope::new(false, &format!("gap.{other}"), format!("No gap handler for {other}."), "Use fix or revert."),
    }
}

fn gap_fix(flags: &Flags) -> Envelope {
    let command = "gap.fix";
    if let Err(envelope) = need(flags, "id", command) {
        return envelope;
    }
    Envelope::new(
        false,
        command,
        "The gap is still a gap.",
        "Add a test that fails on that mutant and passes on the unmutated code. This command does not write that test and does not delete one.",
    )
}

pub fn gap_revert(flags: &Flags) -> Envelope {
    let command = "gap.revert";
    if let Err(envelope) = need(flags, "id", command) {
        return envelope;
    }
    Envelope::new(true, command, "No test was deleted.", "gap revert does not remove a file.")
}

fn guard(args: &[String]) -> Envelope {
    let action = args.first().map(String::as_str).unwrap_or("");
    let flags = Flags::parse(if args.len() > 1 { &args[1..] } else { &[] });
    match action {
        "check" => guard_check(&flags),
        "" => missing("guard", "missing check"),
        other => Envelope::new(false, &format!("guard.{other}"), format!("No guard handler for {other}."), "Use check."),
    }
}

fn guard_check(flags: &Flags) -> Envelope {
    let command = "guard.check";
    let root = match need(flags, "root", command) {
        Ok(value) => PathBuf::from(value),
        Err(envelope) => return envelope,
    };
    let guard = match flags.get("guard") {
        Some(value) => PathBuf::from(value),
        None => root.join("guard.txt"),
    };
    if !guard.is_file() {
        return Envelope::new(false, command, "No guard file.", "Pass --guard or add guard.txt. Nothing was changed.");
    }
    Envelope::new(true, command, "Guard file is in place.", "A guard check does not edit the project.")
}

fn findings(args: &[String]) -> Envelope {
    let action = args.first().map(String::as_str).unwrap_or("");
    let flags = Flags::parse(if args.len() > 1 { &args[1..] } else { &[] });
    match action {
        "add" => findings_add(&flags),
        "" => missing("findings", "missing add"),
        other => Envelope::new(false, &format!("findings.{other}"), format!("No findings handler for {other}."), "Use add."),
    }
}

fn findings_add(flags: &Flags) -> Envelope {
    let command = "findings.add";
    let state = match state_dir(flags, command) {
        Ok(path) => path,
        Err(envelope) => return envelope,
    };
    let text = match flags.get("reason").or_else(|| flags.get("text")) {
        Some(value) => value.to_string(),
        None => return missing(command, "missing --reason"),
    };
    let path = state.join("findings.json");
    let mut items = match fs::read_to_string(&path).ok().and_then(|body| json::parse(&body).ok()) {
        Some(Value::Array(items)) => items,
        _ => Vec::new(),
    };
    items.push(Value::object(vec![("reason", Value::String(text))]));
    if let Err(error) = write_json(&path, &Value::Array(items)) {
        return Envelope::new(false, command, error, "The findings file could not be written.");
    }
    Envelope::new(true, command, "Added the finding.", "The note is in findings.json.")
}

fn status_command(flags: &Flags) -> Envelope {
    let command = "status";
    let out = match state_dir(flags, command) {
        Ok(path) => path,
        Err(envelope) => return envelope,
    };
    let rows = runner::verdicts_in(&out);
    if rows.is_empty() {
        return Envelope::new(true, command, "No mutant results yet.", "mutate run writes results/. A kill is an assertion failure.");
    }
    let counts = decide::counts_of(&rows.iter().map(|(kind, _)| *kind).collect::<Vec<_>>());
    Envelope::new(true, command, decide::campaign_summary(&counts), "Status reads the out dir. It does not delete a test.")
}

fn seal(flags: &Flags) -> Envelope {
    let command = "seal";
    let state = match state_dir(flags, command) {
        Ok(path) => path,
        Err(envelope) => return envelope,
    };
    let record = Value::object(vec![
        ("sealed", Value::Bool(true)),
        ("hiddenRestored", Value::Bool(false)),
    ]);
    if let Err(error) = write_json(&state.join("seal.json"), &record) {
        return Envelope::new(false, command, error, "The seal file could not be written.");
    }
    Envelope::new(true, command, "Sealed the out dir.", "A hidden test stays hidden.")
}

fn queue(args: &[String]) -> Envelope {
    let action = args.first().map(String::as_str).unwrap_or("");
    let flags = Flags::parse(if args.len() > 1 { &args[1..] } else { &[] });
    match action {
        "seed" => queue_seed(&flags),
        "claim" => queue_claim(&flags),
        "reap" => queue_reap(&flags),
        "" => missing("queue", "missing seed, claim, or reap"),
        other => Envelope::new(false, &format!("queue.{other}"), format!("No queue handler for {other}."), "Use seed, claim, or reap."),
    }
}

fn queue_path(flags: &Flags, command: &str) -> Result<PathBuf, Envelope> {
    let state = state_dir(flags, command)?;
    Ok(state.join("queue.json"))
}

fn read_queue(path: &Path) -> Vec<Value> {
    match fs::read_to_string(path).ok().and_then(|body| json::parse(&body).ok()) {
        Some(value) => match value.get("items") {
            Some(Value::Array(items)) => items.clone(),
            _ => Vec::new(),
        },
        None => Vec::new(),
    }
}

fn queue_seed(flags: &Flags) -> Envelope {
    let command = "queue.seed";
    let path = match queue_path(flags, command) {
        Ok(path) => path,
        Err(envelope) => return envelope,
    };
    let mut items = read_queue(&path);
    if let Some(id) = flags.get("id") {
        items.push(Value::object(vec![
            ("id", Value::String(id.into())),
            ("claimed", Value::Bool(false)),
        ]));
    }
    if let Err(error) = write_json(&path, &Value::object(vec![("items", Value::Array(items))])) {
        return Envelope::new(false, command, error, "The queue file could not be written.");
    }
    Envelope::new(true, command, "Seeded the queue.", "claim takes the next id. Nothing was deleted.")
}

fn queue_claim(flags: &Flags) -> Envelope {
    let command = "queue.claim";
    let path = match queue_path(flags, command) {
        Ok(path) => path,
        Err(envelope) => return envelope,
    };
    let mut items = read_queue(&path);
    let mut claimed = None;
    for item in &mut items {
        let is_claimed = item.get("claimed").and_then(Value::as_bool).unwrap_or(false);
        if !is_claimed {
            if let Value::Object(entries) = item {
                if let Some(slot) = entries.iter_mut().find(|(key, _)| key == "claimed") {
                    slot.1 = Value::Bool(true);
                } else {
                    entries.push(("claimed".into(), Value::Bool(true)));
                }
                claimed = entries.iter().find(|(key, _)| key == "id").and_then(|(_, value)| value.as_str().map(str::to_string));
            }
            break;
        }
    }
    if let Err(error) = write_json(&path, &Value::object(vec![("items", Value::Array(items))])) {
        return Envelope::new(false, command, error, "The queue file could not be written.");
    }
    match claimed {
        Some(id) => Envelope::new(true, command, format!("Claimed {id}."), "Finish the id, then reap.").with_extra("id", Value::String(id)),
        None => Envelope::new(true, command, "Nothing to claim.", "Seed the queue first."),
    }
}

fn queue_reap(flags: &Flags) -> Envelope {
    let command = "queue.reap";
    let path = match queue_path(flags, command) {
        Ok(path) => path,
        Err(envelope) => return envelope,
    };
    let items = read_queue(&path);
    let before = items.len();
    let kept: Vec<Value> = items
        .into_iter()
        .filter(|item| item.get("claimed").and_then(Value::as_bool) != Some(true))
        .collect();
    let reaped = before - kept.len();
    if let Err(error) = write_json(&path, &Value::object(vec![("items", Value::Array(kept))])) {
        return Envelope::new(false, command, error, "The queue file could not be written.");
    }
    Envelope::new(true, command, format!("Reaped {reaped}."), "No test was deleted.")
}

fn check_kill(flags: &Flags) -> Envelope {
    let command = "check-kill";
    let out = match need(flags, "out", command) {
        Ok(value) => PathBuf::from(value),
        Err(envelope) => return envelope,
    };
    let id = match need(flags, "id", command) {
        Ok(value) => value,
        Err(envelope) => return envelope,
    };
    let path = out.join("results").join(format!("{id}.json"));
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => return Envelope::new(false, command, "That mutant result is missing.", "Pass --out and --id from a finished run."),
    };
    let value = match json::parse(&text) {
        Ok(value) => value,
        Err(error) => return Envelope::new(false, command, error, "The result file is not JSON."),
    };
    let outcome = value.get("outcome").and_then(Value::as_str).unwrap_or("error");
    if outcome == "killed" {
        Envelope::new(
            true,
            command,
            "The suite's assertion failed on this mutant.",
            "A kill is not proof the change is safe.",
        )
        .with_extra("outcome", Value::String(outcome.into()))
    } else {
        Envelope::new(true, command, format!("This mutant is {outcome}."), "This was not recorded as a kill.")
            .with_extra("outcome", Value::String(outcome.into()))
    }
}

fn verify_change(flags: &Flags) -> Envelope {
    let command = "verify-change";
    let package = match need(flags, "package", command) {
        Ok(value) => PathBuf::from(value),
        Err(envelope) => return envelope,
    };
    if !package.is_dir() {
        return missing(command, "package is not a directory");
    }
    let next = "A kill is not proof a change is safe. Golden and contract checks say whether the expected result is still the expected result.";
    let Some(suite) = flags.get("suite-command") else {
        return Envelope::new(true, command, "No suite was run.", next);
    };
    let timeout = std::time::Duration::from_millis(flags.int("timeout-ms", 120_000).max(1));
    let timed = crate::process::run_shell(suite, Some(&package), timeout);
    let kind = decide::judge_baseline(timed.spawned, timed.timed_out, timed.status, &timed.stdout, &timed.stderr);
    if let Some((summary, refusal)) = decide::baseline_refusal(kind) {
        return Envelope::new(false, command, summary, format!("{refusal} {next}"));
    }
    Envelope::new(true, command, "The baseline suite passed. No mutant was introduced.", next)
}

fn schema(args: &[String]) -> Envelope {
    let flags = Flags::parse(args);
    let name = flags.positionals.first().map(String::as_str);
    let Some(name) = name else {
        return Envelope::new(
            true,
            "schema",
            "schemaVersion 2.",
            "Commands: mutate generate, mutate run, mutate sealed, mutate tally, ledger build, ledger check, matrix report, golden check, golden record, golden compare, gap fix, gap revert, guard check, findings add, status, seal, queue seed, queue claim, queue reap, check-kill, verify-change, schema.",
        );
    };
    let file_name = format!("{}.schema.json", name.replace(' ', "."));
    match schema_dir().map(|dir| dir.join(&file_name)).filter(|path| path.is_file()) {
        Some(_path) => Envelope::new(true, "schema", format!("Schema file {file_name} is present."), "Read that file for the contract. schemaVersion 2.")
            .with_extra("schemaFile", Value::String(file_name)),
        None => Envelope::new(false, "schema", format!("No schema file {file_name}."), "Use a command name such as mutate.generate."),
    }
}

fn schema_dir() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    for _ in 0..4 {
        let candidate = dir.join("schemas");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(path, json::stringify(value)).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_string()).collect()
    }

    #[test]
    fn every_named_command_returns_an_envelope() {
        for invocation in INVOCATIONS {
            let envelope = dispatch(&args(invocation));
            let rendered = envelope.render("");
            let value = json::parse(&rendered).unwrap();
            let (version, _ok, summary, next, _next_call) = json::envelope_fields(&value).unwrap();
            assert_eq!(version, 2, "{invocation:?}");
            assert!(!summary.is_empty(), "{invocation:?}");
            assert!(!next.is_empty(), "{invocation:?}");
            assert!(!rendered.to_ascii_lowercase().contains("unknown command"), "{rendered}");
        }
    }

    #[test]
    fn tally_leaves_deleted_tests_at_zero() {
        let out = std::env::temp_dir().join(format!("probatio-tally-{}", std::process::id()));
        let _ = fs::remove_dir_all(&out);
        fs::create_dir_all(out.join("results")).unwrap();
        fs::write(out.join("SomeTest.java"), "class SomeTest {}\n").unwrap();
        let result = Value::object(vec![
            ("id", Value::String("m1".into())),
            ("outcome", Value::String("survived".into())),
            ("tests", Value::Array(vec![Value::String("SomeTest".into())])),
        ]);
        fs::write(out.join("results/m1.json"), json::stringify(&result)).unwrap();
        let before = fs::read_dir(out.join("results")).unwrap().count();
        let envelope = tally(&Flags::parse(&args(&["--out", &out.display().to_string()])));
        assert!(envelope.ok, "{}", envelope.summary);
        assert_eq!(decide::deleted_tests(), 0);
        let value = json::parse(&envelope.render("")).unwrap();
        assert_eq!(value.get("deletedTests").and_then(Value::as_int), Some(0));
        assert_eq!(
            value.get("pruning").and_then(|item| item.get("deletedTests")).and_then(Value::as_int),
            Some(0)
        );
        let advice = value.get("pruning").and_then(|item| item.get("advice"));
        assert!(matches!(advice, Some(Value::Array(items)) if items.is_empty()));
        assert!(out.join("SomeTest.java").is_file());
        assert_eq!(fs::read_dir(out.join("results")).unwrap().count(), before);
        let listed = value.get("noKillsYet").and_then(|item| match item {
            Value::Array(items) => items.first().and_then(Value::as_str),
            _ => None,
        });
        assert_eq!(listed, Some("SomeTest"));
        let _ = fs::remove_dir_all(&out);
    }

    #[test]
    fn ledger_check_does_not_rerecord_a_red_entry() {
        let dir = std::env::temp_dir().join(format!("probatio-ledger-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let ledger = dir.join("ledger.json");
        let body = "{\n  \"status\": \"red\"\n}\n";
        fs::write(&ledger, body).unwrap();
        let envelope = ledger_check(&Flags::parse(&args(&["--ledger", &ledger.display().to_string()])));
        assert!(!envelope.ok);
        assert!(envelope.summary.contains("back in reach"));
        assert!(envelope.next.contains("Do not re-record"));
        assert_eq!(fs::read_to_string(&ledger).unwrap(), body);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn gap_revert_does_not_delete_a_test() {
        let dir = std::env::temp_dir().join(format!("probatio-gap-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let test = dir.join("SomeTest.java");
        fs::write(&test, "class SomeTest {}\n").unwrap();
        let envelope = gap_revert(&Flags::parse(&args(&["--id", "m1"])));
        assert!(envelope.ok);
        assert!(test.is_file());
        assert_eq!(fs::read_to_string(&test).unwrap(), "class SomeTest {}\n");
        let _ = fs::remove_dir_all(&dir);
    }
}
