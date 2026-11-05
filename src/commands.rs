//! Every command returns one envelope. Unknown flags are refused before any work.

use std::fs;
use std::path::PathBuf;

use crate::args::{self, Flags};
use crate::envelope::Envelope;
use crate::generate;
use crate::golden;
use crate::json::{self, Value};
use crate::ledger;
use crate::matrix_cmd;
use crate::memory;
use crate::queue;
use crate::runner;
use crate::schema_cmd;
use crate::sealed;
use crate::tally;
use crate::verify;

/// Invocations that must each return an envelope. Flags may be missing on purpose.
pub const INVOCATIONS: &[&[&str]] = &[
    &["mutate", "generate"],
    &["mutate", "diff"],
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
    if args.is_empty() {
        return usage(false, "Use mutate, ledger, matrix, golden, gap, guard, findings, status, queue, check-kill, verify-change, or schema.");
    }
    if args[0] == "help" || args[0] == "--help" || args[0] == "-h" {
        return help();
    }
    let group = args[0].as_str();
    let rest = &args[1..];
    match group {
        "mutate" => mutate(rest),
        "ledger" => ledger_group(rest),
        "matrix" => matrix(rest),
        "golden" => golden_group(rest),
        "gap" => gap(rest),
        "guard" => guard(rest),
        "findings" => findings(rest),
        "status" => status_command(&Flags::parse(rest)),
        "seal" => seal(&Flags::parse(rest)),
        "queue" => queue_group(rest),
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

fn usage(ok: bool, summary: &str) -> Envelope {
    Envelope::new(
        ok,
        "mutate.help",
        summary,
        "probatio mutate generate writes operator mutants. probatio mutate run confirms each kill twice. probatio mutate tally reads a finished run and does not delete a test.",
    )
}

fn refused(command: &str, flags: &Flags) -> Option<Envelope> {
    let known = args::command_flags(command)?;
    let names = flags.unknown(known);
    if names.is_empty() {
        None
    } else {
        Some(Envelope::new(false, command, args::unknown_summary(&names), "Drop the unknown flag and run the command again."))
    }
}

fn mutate(args: &[String]) -> Envelope {
    let action = args.first().map(String::as_str).unwrap_or("");
    let flags = Flags::parse(if args.len() > 1 { &args[1..] } else { &[] });
    let command = match action {
        "generate" => "mutate.generate",
        "diff" => "mutate.diff",
        "run" => "mutate.run",
        "sealed" => "mutate.sealed",
        "tally" => "mutate.tally",
        "" => return missing("mutate", "missing diff, generate, run, sealed, or tally"),
        other => {
            return Envelope::new(false, &format!("mutate.{other}"), format!("No mutate handler for {other}."), "Use diff, generate, run, sealed, or tally.");
        }
    };
    if let Some(envelope) = refused(command, &flags) {
        return envelope;
    }
    match action {
        "generate" => generate::generate(&flags),
        "diff" => crate::diffscope::diff(&flags),
        "run" => runner::run(&flags),
        "sealed" => sealed::sealed(&flags),
        "tally" => tally_command(&flags),
        _ => unreachable!(),
    }
}

fn tally_command(flags: &Flags) -> Envelope {
    let out = match flags.get("out").or_else(|| flags.get("state")).or_else(|| flags.get("root")) {
        Some(value) => PathBuf::from(value),
        None => return missing("mutate.tally", "missing --out"),
    };
    tally::tally(&out)
}

fn ledger_group(args: &[String]) -> Envelope {
    let action = args.first().map(String::as_str).unwrap_or("");
    let flags = Flags::parse(if args.len() > 1 { &args[1..] } else { &[] });
    let command = match action {
        "build" => "ledger.build",
        "check" => "ledger.check",
        "" => return missing("ledger", "missing build or check"),
        other => return Envelope::new(false, &format!("ledger.{other}"), format!("No ledger handler for {other}."), "Use build or check."),
    };
    if let Some(envelope) = refused(command, &flags) {
        return envelope;
    }
    match action {
        "build" => ledger_build(&flags),
        "check" => ledger_check(&flags),
        _ => unreachable!(),
    }
}

fn ledger_build(flags: &Flags) -> Envelope {
    let package = match need(flags, "package", "ledger.build") {
        Ok(path) => path,
        Err(envelope) => return envelope,
    };
    let out = match need(flags, "out", "ledger.build") {
        Ok(path) => path,
        Err(envelope) => return envelope,
    };
    let commit = flags.get("commit").unwrap_or("HEAD");
    let src = flags.get("src").unwrap_or("src");
    let max_lines = flags.int("max-lines", 300).unwrap_or(300) as usize;
    let max_commits = match flags.parse_int("max-commits") {
        Ok(value) => value,
        Err(message) => return Envelope::new(false, "ledger.build", message, "Fix the command and run it again."),
    };
    let repo = flags.get("repo").map(PathBuf::from);
    if let Some(path) = &repo {
        if !path.is_dir() {
            return Envelope::new(false, "ledger.build", "repo is not a git repository", "Pass --repo as the git root.");
        }
    }
    let tests_dir = flags.get("tests-dir").unwrap_or("tests");
    ledger::build(&package, &out, commit, src, tests_dir, max_lines, max_commits, repo.as_deref())
}

pub fn ledger_check(flags: &Flags) -> Envelope {
    let command = "ledger.check";
    let ledger_path = match flags.get("ledger") {
        Some(value) => PathBuf::from(value),
        None => return missing(command, "missing --ledger"),
    };
    if ledger_path.is_file() {
        if let Ok(text) = fs::read_to_string(&ledger_path) {
            if text.lines().any(|line| line.trim() == "red") || text.contains("\"status\": \"red\"") || text.contains("\"status\":\"red\"") {
                return Envelope::new(
                    false,
                    command,
                    "A fixed bug is back in reach of the suite.",
                    "Do not re-record it away. Find the test that stopped guarding it, or give the reason in a Golden-Change: line.",
                );
            }
        }
    }
    let package = match flags.get("package") {
        Some(value) => PathBuf::from(value),
        None => return missing(command, "missing --package"),
    };
    let out = match flags.get("out") {
        Some(value) => PathBuf::from(value),
        None => return missing(command, "missing --out"),
    };
    let update = flags.bool("update", false);
    let message = flags.get("message").unwrap_or("");
    let timeout = flags.int("suite-timeout-ms", 120_000).unwrap_or(120_000);
    let mut extra = Vec::new();
    crate::runner::forward_run_flags(&mut extra, flags);
    ledger::check(&package, &ledger_path, &out, update, message, timeout, &extra)
}

fn matrix(args: &[String]) -> Envelope {
    let action = args.first().map(String::as_str).unwrap_or("");
    let flags = Flags::parse(if args.len() > 1 { &args[1..] } else { &[] });
    match action {
        "report" => {
            if let Some(envelope) = refused("matrix.report", &flags) {
                return envelope;
            }
            matrix_report(&flags)
        }
        "" => missing("matrix", "missing report"),
        other => Envelope::new(false, &format!("matrix.{other}"), format!("No matrix handler for {other}."), "Use report."),
    }
}

fn matrix_report(flags: &Flags) -> Envelope {
    let scores = match need(flags, "scores", "matrix.report") {
        Ok(path) => path,
        Err(envelope) => return envelope,
    };
    let direct = match need(flags, "direct", "matrix.report") {
        Ok(path) => path,
        Err(envelope) => return envelope,
    };
    let out = match need(flags, "out", "matrix.report") {
        Ok(path) => path,
        Err(envelope) => return envelope,
    };
    let scores_text = match fs::read_to_string(&scores) {
        Ok(text) => text,
        Err(error) => return Envelope::new(false, "matrix.report", error.to_string(), "Pass --scores that can be read."),
    };
    let direct_text = match fs::read_to_string(&direct) {
        Ok(text) => text,
        Err(error) => return Envelope::new(false, "matrix.report", error.to_string(), "Pass --direct that can be read."),
    };
    let kills = if let Some(kills) = flags.get("kills") {
        match matrix_cmd::read_kills(&PathBuf::from(kills)) {
            Ok(rows) => Some(rows),
            Err(message) => return Envelope::new(false, "matrix.report", message, "Fix --kills and run again."),
        }
    } else {
        None
    };
    match matrix_cmd::report(&scores_text, &direct_text, &out, &flags.texts("patches"), kills.as_deref()) {
        Ok(envelope) => envelope,
        Err(message) => Envelope::new(false, "matrix.report", message, "Fix the score files and run again."),
    }
}

fn golden_group(args: &[String]) -> Envelope {
    let action = args.first().map(String::as_str).unwrap_or("");
    let flags = Flags::parse(if args.len() > 1 { &args[1..] } else { &[] });
    let command = match action {
        "check" => "golden.check",
        "record" => "golden.record",
        "compare" => "golden.compare",
        "" => return missing("golden", "missing check, record, or compare"),
        other => return Envelope::new(false, &format!("golden.{other}"), format!("No golden handler for {other}."), "Use check, record, or compare."),
    };
    if let Some(envelope) = refused(command, &flags) {
        return envelope;
    }
    match action {
        "check" => golden_check(&flags),
        "record" => golden_record(&flags),
        "compare" => golden_compare(&flags),
        _ => unreachable!(),
    }
}

fn golden_check(flags: &Flags) -> Envelope {
    let recorded = match need(flags, "recorded", "golden.check") {
        Ok(path) => path,
        Err(envelope) => return envelope,
    };
    let actual = match need(flags, "actual", "golden.check") {
        Ok(path) => path,
        Err(envelope) => return envelope,
    };
    match golden::check(&recorded, &actual, flags.get("update"), flags.get("message").unwrap_or(""), &flags.texts("root")) {
        Ok(envelope) => envelope,
        Err(message) => Envelope::new(false, "golden.check", message, "Fix the command and run it again."),
    }
}

fn golden_record(flags: &Flags) -> Envelope {
    let package = match need(flags, "package", "golden.record") {
        Ok(path) => path,
        Err(envelope) => return envelope,
    };
    let out_dir = flags.get("dir").or_else(|| flags.get("out")).unwrap_or("tests/golden");
    let timeout = flags.int("suite-timeout-ms", 600_000).unwrap_or(600_000);
    if let Err(message) = golden::pin_commit(&package, flags.get("repo"), flags.get("commit")) {
        return Envelope::new(false, "golden.record", message, "Fix the command and run it again.");
    }
    golden::record(&package, &flags.texts("module"), &flags.texts("tests"), out_dir, timeout, flags.get("repo"), flags.get("commit"))
}

fn golden_compare(flags: &Flags) -> Envelope {
    let package = match need(flags, "package", "golden.compare") {
        Ok(path) => path,
        Err(envelope) => return envelope,
    };
    let out = match need(flags, "out", "golden.compare") {
        Ok(path) => path,
        Err(envelope) => return envelope,
    };
    let operators = flags.get("operators").unwrap_or("core");
    if operators != "core" && operators != "wide" {
        return Envelope::new(false, "golden.compare", "--operators must be core or wide", "Fix the command and run it again.");
    }
    let timeout = flags.int("suite-timeout-ms", 600_000).unwrap_or(600_000);
    if let Err(message) = golden::pin_commit(&package, flags.get("repo"), flags.get("commit")) {
        return Envelope::new(false, "golden.compare", message, "Fix the command and run it again.");
    }
    golden::compare(&package, &flags.texts("module"), &flags.texts("tests"), &flags.texts("golden"), &out, operators, timeout, flags.get("commit"))
}

fn gap(args: &[String]) -> Envelope {
    let action = args.first().map(String::as_str).unwrap_or("");
    let flags = Flags::parse(if args.len() > 1 { &args[1..] } else { &[] });
    let command = match action {
        "fix" => "gap.fix",
        "revert" => "gap.revert",
        "" => return missing("gap", "missing fix or revert"),
        other => return Envelope::new(false, &format!("gap.{other}"), format!("No gap handler for {other}."), "Use fix or revert."),
    };
    if let Some(envelope) = refused(command, &flags) {
        return envelope;
    }
    match action {
        "fix" => gap_fix(&flags),
        "revert" => gap_revert(&flags),
        _ => unreachable!(),
    }
}

fn gap_fix(flags: &Flags) -> Envelope {
    let command = "gap.fix";
    let state_dir = match need(flags, "state", command) {
        Ok(path) => path,
        Err(envelope) => return envelope,
    };
    let id = match flags.need("id") {
        Ok(value) => value.to_string(),
        Err(message) => return missing(command, &message),
    };
    let file = match flags.need("file") {
        Ok(value) => value.to_string(),
        Err(message) => return missing(command, &message),
    };
    let guard = match flags.need("guard") {
        Ok(value) => value.to_string(),
        Err(message) => return missing(command, &message),
    };
    let commit = match flags.need("commit") {
        Ok(value) => value.to_string(),
        Err(message) => return missing(command, &message),
    };
    let line = match flags.parse_int("line") {
        Ok(Some(value)) if value >= 1 => value as i64,
        Ok(_) => return Envelope::new(false, command, "--line must be a positive integer", "Fix the command and run it again."),
        Err(message) => return Envelope::new(false, command, message, "Fix the command and run it again."),
    };
    let root = flags.get("root").map(PathBuf::from).unwrap_or_else(|| state_dir.clone());
    let state = match memory::fix_gap(&memory::load_state(&state_dir), &id, &file, line, &guard, &commit, &root) {
        Ok(state) => state,
        Err(message) => return Envelope::new(false, command, message, "The gap is still a gap."),
    };
    if let Err(message) = memory::save_state(&state_dir, &state) {
        return Envelope::new(false, command, message, "The state file could not be written.");
    }
    Envelope::new(true, command, format!("{id} is fixed, guarded by {guard}."), "Run status.").with_extra("id", Value::String(id))
}

pub fn gap_revert(flags: &Flags) -> Envelope {
    let command = "gap.revert";
    let state_dir = match flags.get("state") {
        Some(value) => PathBuf::from(value),
        None => return missing(command, "missing --state"),
    };
    let id = match flags.need("id") {
        Ok(value) => value.to_string(),
        Err(message) => return missing(command, &message),
    };
    let loaded = memory::load_state(&state_dir);
    let (state, text) = match memory::revert_gap(&loaded, &id) {
        Ok(value) => value,
        Err(message) => return Envelope::new(false, command, message, "Nothing was deleted."),
    };
    if let Err(message) = memory::save_state(&state_dir, &state) {
        return Envelope::new(false, command, message, "Nothing was deleted.");
    }
    Envelope::new(true, command, text.clone(), text)
}

fn guard(args: &[String]) -> Envelope {
    let action = args.first().map(String::as_str).unwrap_or("");
    let flags = Flags::parse(if args.len() > 1 { &args[1..] } else { &[] });
    match action {
        "check" => {
            if let Some(envelope) = refused("guard.check", &flags) {
                return envelope;
            }
            guard_check(&flags)
        }
        "" => missing("guard", "missing check"),
        other => Envelope::new(false, &format!("guard.{other}"), format!("No guard handler for {other}."), "Use check."),
    }
}

fn guard_check(flags: &Flags) -> Envelope {
    let command = "guard.check";
    let state_dir = match need(flags, "state", command) {
        Ok(path) => path,
        Err(envelope) => return envelope,
    };
    let guard = match flags.need("guard") {
        Ok(value) => value.to_string(),
        Err(message) => return missing(command, &message),
    };
    if memory::parse_guard(&guard).is_err() {
        return Envelope::new(false, command, "guard must be file:line", "Fix the guard and run again.");
    }
    let root = flags.get("root").map(PathBuf::from).unwrap_or_else(|| state_dir.clone());
    let (ok, summary) = memory::guard_allows(&memory::load_state(&state_dir), &guard, flags.get("message").unwrap_or(""), &root);
    Envelope::new(ok, command, summary, if ok { "The guard check passed." } else { "Add a Guard-Change trailer naming the guard, or restore it." })
}

fn findings(args: &[String]) -> Envelope {
    let action = args.first().map(String::as_str).unwrap_or("");
    let flags = Flags::parse(if args.len() > 1 { &args[1..] } else { &[] });
    match action {
        "add" => {
            if let Some(envelope) = refused("findings.add", &flags) {
                return envelope;
            }
            findings_add(&flags)
        }
        "" => missing("findings", "missing add"),
        other => Envelope::new(false, &format!("findings.{other}"), format!("No findings handler for {other}."), "Use add."),
    }
}

fn findings_add(flags: &Flags) -> Envelope {
    let command = "findings.add";
    let state_dir = match need(flags, "state", command) {
        Ok(path) => path,
        Err(envelope) => return envelope,
    };
    let status = match flags.need("status") {
        Ok(value) => value.to_string(),
        Err(message) => return missing(command, &message),
    };
    if !matches!(status.as_str(), "open" | "fixed" | "equivalent" | "wont-fix") {
        return Envelope::new(false, command, "--status must be open, fixed, equivalent, or wont-fix", "Fix the command and run it again.");
    }
    let id = match flags.need("id") {
        Ok(value) => value.to_string(),
        Err(message) => return missing(command, &message),
    };
    let reason = match flags.need("reason") {
        Ok(value) => value.to_string(),
        Err(message) => return missing(command, &message),
    };
    let mut finding = memory::Finding {
        id: id.clone(),
        status: status.clone(),
        reason,
        file: flags.get("file").map(str::to_string),
        line: flags.parse_int("line").ok().flatten().map(|value| value as i64),
        hash: None,
        agent: flags.get("agent").map(str::to_string),
    };
    if let (Some(file), Some(line)) = (&finding.file, finding.line) {
        let root = flags.get("root").map(PathBuf::from).unwrap_or_else(|| state_dir.clone());
        let Some(source) = memory::read_source_line(&root.join(file), line) else {
            return Envelope::new(false, command, "decision line does not exist", "Point --file and --line at a line that exists.");
        };
        finding.hash = Some(memory::line_hash(&source));
        let mut state = memory::load_state(&state_dir);
        state.decisions.retain(|item| item.id != id);
        state.decisions.push(memory::Decision {
            id: id.clone(),
            file: file.clone(),
            line,
            hash: finding.hash.clone().unwrap_or_default(),
            status: status.clone(),
            reason: finding.reason.clone(),
        });
        if let Err(message) = memory::save_state(&state_dir, &state) {
            return Envelope::new(false, command, message, "The state file could not be written.");
        }
    }
    if let Err(message) = memory::append_finding(&state_dir, &finding) {
        return Envelope::new(false, command, message, "The finding was not written.");
    }
    Envelope::new(true, command, format!("Recorded {id} as {status}."), "Run status.")
}

fn status_command(flags: &Flags) -> Envelope {
    if let Some(envelope) = refused("status", flags) {
        return envelope;
    }
    let state_dir = match need(flags, "state", "status") {
        Ok(path) => path,
        Err(envelope) => return envelope,
    };
    let root = flags.get("root").map(PathBuf::from).unwrap_or_else(|| state_dir.clone());
    let page = flags.get("page").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("PROBATIO.md"));
    let score = flags.get("scores").and_then(|path| fs::read_to_string(path).ok()).and_then(|text| json::parse(&text).ok()).and_then(|value| {
        let visible_before = value.get("visible").and_then(|item| item.get("before")).and_then(Value::as_int)?;
        let visible_after = value.get("visible").and_then(|item| item.get("after")).and_then(Value::as_int)?;
        let sealed_before = value.get("sealed").and_then(|item| item.get("before")).and_then(Value::as_int)?;
        let sealed_after = value.get("sealed").and_then(|item| item.get("after")).and_then(Value::as_int)?;
        Some((visible_before, visible_after, sealed_before, sealed_after))
    });
    match memory::status_command(&state_dir, &root, score, flags.get("agent"), &page) {
        Ok(envelope) => envelope,
        Err(message) => Envelope::new(false, "status", message, "The status page was not written."),
    }
}

fn seal(flags: &Flags) -> Envelope {
    if let Some(envelope) = refused("seal", flags) {
        return envelope;
    }
    let state_dir = match need(flags, "state", "seal") {
        Ok(path) => path,
        Err(envelope) => return envelope,
    };
    let mut ids = flags.texts("id");
    if let Some(from) = flags.get("from") {
        let text = match fs::read_to_string(from) {
            Ok(text) => text,
            Err(error) => return Envelope::new(false, "seal", error.to_string(), "Pass --from that can be read."),
        };
        let parsed = match json::parse(&text) {
            Ok(value) => value,
            Err(error) => return Envelope::new(false, "seal", error, "Pass --from as JSON."),
        };
        let list = match &parsed {
            Value::Array(items) => items.iter().filter_map(Value::as_str).map(str::to_string).collect(),
            Value::Object(_) => match parsed.get("ids") {
                Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).map(str::to_string).collect(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        };
        let fraction = flags.get("fraction").unwrap_or("0.2").parse::<f64>().unwrap_or(-1.0);
        if !(0.0..=1.0).contains(&fraction) {
            return Envelope::new(false, "seal", "--fraction must be between 0 and 1", "Fix the command and run it again.");
        }
        let seed = flags.int("seed", 20_261_004).unwrap_or(20_261_004) as u32;
        ids = memory::choose_sealed(&list, fraction, seed);
    }
    match memory::seal_ids(&state_dir, &ids) {
        Ok(_) => Envelope::new(true, "seal", format!("Sealed {} ids.", ids.len()), "status, PROBATIO.md, and the queue omit them.").with_extra("sealedCount", Value::Int(ids.len() as i64)),
        Err(message) => Envelope::new(false, "seal", message, "Nothing was sealed."),
    }
}

fn queue_group(args: &[String]) -> Envelope {
    let action = args.first().map(String::as_str).unwrap_or("");
    let flags = Flags::parse(if args.len() > 1 { &args[1..] } else { &[] });
    let command = match action {
        "seed" => "queue.seed",
        "claim" => "queue.claim",
        "reap" => "queue.reap",
        "" => return missing("queue", "missing seed, claim, or reap"),
        other => return Envelope::new(false, &format!("queue.{other}"), format!("No queue handler for {other}."), "Use seed, claim, or reap."),
    };
    if let Some(envelope) = refused(command, &flags) {
        return envelope;
    }
    match action {
        "seed" => queue_seed(&flags),
        "claim" => queue_claim(&flags),
        "reap" => queue_reap(&flags),
        _ => unreachable!(),
    }
}

fn queue_seed(flags: &Flags) -> Envelope {
    let state_dir = match need(flags, "state", "queue.seed") {
        Ok(path) => path,
        Err(envelope) => return envelope,
    };
    match queue::seed(&state_dir, &flags.texts("id")) {
        Ok(written) => Envelope::new(true, "queue.seed", format!("Seeded {} queue items.", written.len()), "Claim one.")
            .with_extra("seeded", Value::Array(written.into_iter().map(Value::String).collect())),
        Err(message) => Envelope::new(false, "queue.seed", message, "Nothing was seeded."),
    }
}

fn queue_claim(flags: &Flags) -> Envelope {
    let state_dir = match need(flags, "state", "queue.claim") {
        Ok(path) => path,
        Err(envelope) => return envelope,
    };
    let id = match flags.need("id") {
        Ok(value) => value.to_string(),
        Err(message) => return missing("queue.claim", &message),
    };
    let agent = match flags.need("agent") {
        Ok(value) => value.to_string(),
        Err(message) => return missing("queue.claim", &message),
    };
    let now = flags.int("now", 0).unwrap_or(0);
    let now = if flags.get("now").is_none() { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|item| item.as_millis() as u64).unwrap_or(0) } else { now };
    let lease = flags.int("lease-ms", 600_000).unwrap_or(600_000);
    match queue::claim(&state_dir, &id, &agent, now, lease) {
        Ok((ok, summary)) => Envelope::new(ok, "queue.claim", summary, if ok { "Work the claim, then check-kill it." } else { "Pick another item." }),
        Err(message) => Envelope::new(false, "queue.claim", message, "Pick another item."),
    }
}

fn queue_reap(flags: &Flags) -> Envelope {
    let state_dir = match need(flags, "state", "queue.reap") {
        Ok(path) => path,
        Err(envelope) => return envelope,
    };
    let now = if flags.get("now").is_none() {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|item| item.as_millis() as u64).unwrap_or(0)
    } else {
        flags.int("now", 0).unwrap_or(0)
    };
    match queue::reap(&state_dir, now) {
        Ok(returned) => {
            let count = returned.len();
            Envelope::new(
                true,
                "queue.reap",
                format!("Returned {count} expired claims."),
                if count == 0 { "Nothing expired." } else { "The expired claims are back in the queue." },
            )
            .with_extra("returned", Value::Array(returned.into_iter().map(Value::String).collect()))
        }
        Err(message) => Envelope::new(false, "queue.reap", message, "Nothing expired."),
    }
}

fn check_kill(flags: &Flags) -> Envelope {
    if let Some(envelope) = refused("check-kill", flags) {
        return envelope;
    }
    sealed::check_kill(flags)
}

fn verify_change(flags: &Flags) -> Envelope {
    if let Some(envelope) = refused("verify-change", flags) {
        return envelope;
    }
    verify::verify(flags)
}

fn schema(args: &[String]) -> Envelope {
    let flags = Flags::parse(args);
    if let Some(envelope) = refused("schema", &flags) {
        return envelope;
    }
    let name = flags.positionals.first().map(String::as_str);
    schema_cmd::run(name)
}

fn missing(command: &str, message: &str) -> Envelope {
    Envelope::new(false, command, message.to_string(), "Pass the missing flag and run the command again.")
}

fn need(flags: &Flags, key: &str, command: &str) -> Result<PathBuf, Envelope> {
    flags.need(key).map(PathBuf::from).map_err(|message| missing(command, &message))
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
    fn an_unknown_flag_is_refused_before_work() {
        let envelope = dispatch(&args(&["mutate", "generate", "--package", "/tmp", "--out", "/tmp/out", "--bogus", "1"]));
        assert!(!envelope.ok);
        assert!(envelope.summary.contains("Unknown flag --bogus"), "{}", envelope.summary);
    }

    #[test]
    fn tally_leaves_deleted_tests_at_zero_and_names_a_gap() {
        let out = std::env::temp_dir().join(format!("probatio-tally-{}", std::process::id()));
        let _ = fs::remove_dir_all(&out);
        fs::create_dir_all(out.join("results")).unwrap();
        fs::write(out.join("SomeTest.java"), "class SomeTest {}\n").unwrap();
        let result = Value::object(vec![
            ("id", Value::String("m1".into())),
            ("outcome", Value::String("survived".into())),
            ("tests", Value::Array(vec![Value::String("SomeTest".into())])),
            ("file", Value::String("src/A.java".into())),
            ("line", Value::Int(2)),
        ]);
        fs::write(out.join("results/m1.json"), json::stringify(&result)).unwrap();
        let before = fs::read_dir(out.join("results")).unwrap().count();
        let envelope = tally_command(&Flags::parse(&args(&["--out", &out.display().to_string()])));
        assert!(envelope.ok, "{}", envelope.summary);
        let value = json::parse(&envelope.render("")).unwrap();
        assert_eq!(value.get("deletedTests").and_then(Value::as_int), Some(0));
        assert_eq!(value.get("pruning").and_then(|item| item.get("deletedTests")).and_then(Value::as_int), Some(0));
        let advice = value.get("pruning").and_then(|item| item.get("advice"));
        assert!(matches!(advice, Some(Value::Array(items)) if items.is_empty()));
        assert!(out.join("SomeTest.java").is_file());
        assert_eq!(fs::read_dir(out.join("results")).unwrap().count(), before);
        let listed = value.get("noKillsYet").and_then(|item| match item {
            Value::Array(items) => items.first().and_then(Value::as_str),
            _ => None,
        });
        assert_eq!(listed, Some("SomeTest"));
        assert!(envelope.summary.contains("1 gaps"), "{}", envelope.summary);
        assert!(envelope.next_call.is_some(), "{}", envelope.next);
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
        let missing = gap_revert(&Flags::parse(&args(&["--state", &dir.display().to_string(), "--id", "m1"])));
        assert!(!missing.ok, "{}", missing.summary);
        assert!(test.is_file());
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/guard.txt"), "keep\n").unwrap();
        let state = memory::fix_gap(&memory::empty_state(), "m1", "src/a.java", 1, "src/guard.txt:1", "abc", &dir).unwrap();
        memory::save_state(&dir, &state).unwrap();
        let envelope = gap_revert(&Flags::parse(&args(&["--state", &dir.display().to_string(), "--id", "m1"])));
        assert!(envelope.ok, "{}", envelope.summary);
        assert!(envelope.summary.contains("guard broke"), "{}", envelope.summary);
        assert_eq!(fs::read_to_string(&test).unwrap(), "class SomeTest {}\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_command_exits_as_a_failure_envelope() {
        let envelope = dispatch(&[]);
        assert!(!envelope.ok);
        assert!(envelope.summary.contains("Use mutate"), "{}", envelope.summary);
    }
}
