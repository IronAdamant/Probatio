//! Survivors you already judged. `findings add --status equivalent` records that a mutant cannot
//! change behaviour; `mutate diff` and `mutate tally` read those records from the project's
//! Probatio folder and label a survivor that matches one, so it is not read again on every run.
//!
//! A record matches a survivor by its mutant id, or by file, line, and the text of that line
//! (the hash `findings add --file --line` stores). The latest record for an id wins, so a later
//! `--status open` takes a label back.

use std::path::Path;

use crate::memory;

#[derive(Clone, Debug, PartialEq)]
pub struct Recorded {
    pub id: String,
    pub file: Option<String>,
    pub line: Option<i64>,
    pub hash: Option<String>,
    pub reason: String,
}

/// The equivalent records in `<project root>/.probatio` (`state.json` decisions, then
/// `findings.jsonl` in the order written).
pub fn load(package: &Path) -> Vec<Recorded> {
    let dir = crate::diffscope::probatio_dir(package);
    let mut latest: Vec<(String, String, Recorded)> = Vec::new();
    let mut put = |status: &str, record: Recorded| {
        latest.retain(|(id, _, _)| *id != record.id);
        latest.push((record.id.clone(), status.to_string(), record));
    };
    for decision in memory::load_state(&dir).decisions {
        put(&decision.status, Recorded { id: decision.id, file: Some(decision.file), line: Some(decision.line), hash: Some(decision.hash), reason: decision.reason });
    }
    for finding in memory::load_findings(&dir) {
        put(&finding.status, Recorded { id: finding.id, file: finding.file, line: finding.line, hash: finding.hash, reason: finding.reason });
    }
    latest.into_iter().filter(|(_, status, _)| status == "equivalent").map(|(_, _, record)| record).collect()
}

/// The record for a survivor: same id, or same file and line whose text still hashes the same.
pub fn find<'a>(recorded: &'a [Recorded], package: &Path, id: &str, file: &str, line: i64) -> Option<&'a Recorded> {
    if let Some(record) = recorded.iter().find(|record| record.id == id) {
        return Some(record);
    }
    let text = memory::read_source_line(&package.join(file), line)?;
    let hash = memory::line_hash(&text);
    recorded.iter().find(|record| record.file.as_deref() == Some(file) && record.line == Some(line) && record.hash.as_deref() == Some(hash.as_str()))
}

/// The `findings add` call that records one survivor as equivalent.
pub fn record_hint(package: &Path, id: &str, file: &str, line: i64) -> String {
    let dir = crate::diffscope::probatio_dir(package);
    format!(
        "probatio findings add --state {} --root {} --status equivalent --id {id} --file {file} --line {line} --reason <why>",
        quote(&dir.display().to_string()),
        quote(&package.display().to_string())
    )
}

fn quote(text: &str) -> String {
    if text.contains([' ', '\'', '"']) { format!("\"{text}\"") } else { text.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Specula M5: the same equivalent mutant was read again on every run. A record by id, or
    /// by file, line, and line text, labels it; a later `open` takes the label back; an edited
    /// line no longer matches.
    #[test]
    fn a_recorded_equivalent_matches_by_id_or_by_its_line() {
        let dir = crate::testdir::TempDir::new("equiv");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/a.rs"), "fn f(n: u32) -> bool {\n    n > 0\n}\n").unwrap();
        let state = dir.join(".probatio");
        let line = memory::read_source_line(&dir.join("src/a.rs"), 2).unwrap();
        let finding = |id: &str, status: &str, file: Option<&str>| memory::Finding {
            id: id.into(),
            status: status.into(),
            reason: "spin loop bound".into(),
            file: file.map(str::to_string),
            line: file.map(|_| 2),
            hash: file.map(|_| memory::line_hash(&line)),
            agent: None,
        };
        memory::append_finding(&state, &finding("m1", "equivalent", None)).unwrap();
        memory::append_finding(&state, &finding("m2", "equivalent", Some("src/a.rs"))).unwrap();
        memory::append_finding(&state, &finding("m3", "equivalent", None)).unwrap();
        memory::append_finding(&state, &finding("m3", "open", None)).unwrap();
        let recorded = load(&dir);
        assert_eq!(recorded.iter().map(|record| record.id.as_str()).collect::<Vec<_>>(), ["m1", "m2"]);
        assert_eq!(find(&recorded, &dir, "m1", "src/b.rs", 9).map(|record| record.id.as_str()), Some("m1"));
        assert_eq!(find(&recorded, &dir, "other-id", "src/a.rs", 2).map(|record| record.id.as_str()), Some("m2"));
        assert!(find(&recorded, &dir, "m3", "src/a.rs", 1).is_none());
        fs::write(dir.join("src/a.rs"), "fn f(n: u32) -> bool {\n    n >= 1\n}\n").unwrap();
        assert!(find(&recorded, &dir, "other-id", "src/a.rs", 2).is_none(), "an edited line keeps no label");
        let hint = record_hint(&dir, "m9", "src/a.rs", 2);
        assert!(hint.contains("--status equivalent --id m9 --file src/a.rs --line 2"), "{hint}");
    }
}
