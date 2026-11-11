//! A file-per-id queue. A claim is an atomic rename, and reap returns an expired lease.

use std::fs;
use std::path::Path;

use crate::json::{self, Value};
use crate::memory;

pub fn safe_file_part(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

pub fn seed(root: &Path, ids: &[String]) -> Result<Vec<String>, String> {
    let sealed = memory::load_sealed(root);
    let queue = root.join("queue");
    fs::create_dir_all(&queue).map_err(|error| error.to_string())?;
    let mut written = Vec::new();
    let mut ordered = ids.to_vec();
    ordered.sort();
    for id in ordered {
        if sealed.iter().any(|item| item == &id) || !safe_file_part(&id) {
            continue;
        }
        let dest = queue.join(format!("{id}.json"));
        if dest.exists() {
            continue;
        }
        let body = Value::object(vec![("id", Value::String(id.clone()))]);
        fs::write(&dest, json::stringify(&body)).map_err(|error| error.to_string())?;
        written.push(id);
    }
    Ok(written)
}

pub fn claim(root: &Path, id: &str, agent: &str, now: u64, lease_ms: u64) -> Result<(bool, String), String> {
    if !safe_file_part(id) || !safe_file_part(agent) {
        return Ok((false, "agent and id must be a single file name.".into()));
    }
    let source = root.join("queue").join(format!("{id}.json"));
    let claimed = root.join("claimed");
    let dest = claimed.join(format!("{agent}-{id}.json"));
    if !source.is_file() {
        return Ok((false, format!("{id} is not in the queue.")));
    }
    fs::create_dir_all(&claimed).map_err(|error| error.to_string())?;
    if fs::rename(&source, &dest).is_err() {
        return Ok((false, format!("{id} was claimed by someone else.")));
    }
    let until = now.saturating_add(lease_ms);
    let body = Value::object(vec![
        ("id", Value::String(id.into())),
        ("agent", Value::String(agent.into())),
        ("leasedUntil", Value::Int(until as i64)),
    ]);
    fs::write(&dest, json::stringify(&body)).map_err(|error| error.to_string())?;
    Ok((true, format!("{agent} claimed {id} until {until}.")))
}

pub fn reap(root: &Path, now: u64) -> Result<Vec<String>, String> {
    let claimed = root.join("claimed");
    let queue = root.join("queue");
    if !claimed.is_dir() {
        return Ok(Vec::new());
    }
    fs::create_dir_all(&queue).map_err(|error| error.to_string())?;
    let mut names = fs::read_dir(&claimed).map_err(|error| error.to_string())?.filter_map(|item| item.ok()).map(|item| item.file_name()).collect::<Vec<_>>();
    names.sort();
    let mut returned = Vec::new();
    for name in names {
        let name = name.to_string_lossy();
        if !name.ends_with(".json") {
            continue;
        }
        let file = claimed.join(name.as_ref());
        let Ok(text) = fs::read_to_string(&file) else { continue };
        let Ok(value) = json::parse(&text) else { continue };
        let Some(until) = value.get("leasedUntil").and_then(Value::as_int) else { continue };
        if until > now as i64 {
            continue;
        }
        let Some(id) = value.get("id").and_then(Value::as_str) else { continue };
        if !safe_file_part(id) {
            continue;
        }
        fs::rename(&file, queue.join(format!("{id}.json"))).map_err(|error| error.to_string())?;
        returned.push(id.to_string());
    }
    returned.sort();
    Ok(returned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_is_exclusive_and_reap_returns_an_expired_lease() {
        let root = crate::testdir::TempDir::new("queue");
        let _ = fs::remove_dir_all(&root);
        let seeded = seed(&root, &["b".into(), "a".into(), "../no".into()]).unwrap();
        assert_eq!(seeded, ["a", "b"]);
        let (ok, summary) = claim(&root, "a", "ada", 1_000, 50).unwrap();
        assert!(ok, "{summary}");
        let (again, _) = claim(&root, "a", "bea", 1_000, 50).unwrap();
        assert!(!again);
        let early = reap(&root, 1_000).unwrap();
        assert!(early.is_empty());
        let back = reap(&root, 1_050).unwrap();
        assert_eq!(back, ["a"]);
        let (reclaimed, _) = claim(&root, "a", "bea", 2_000, 10).unwrap();
        assert!(reclaimed);
        let _ = fs::remove_dir_all(&root);
    }
}
