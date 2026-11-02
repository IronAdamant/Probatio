//! Turn a coverage report into the `files` line map `mutate run` already reads.
//!
//! A range map is left alone. These shapes are converted:
//! the old `{ "files": { "a.go": { "3": ["TestA"] } } }` object,
//! `HIT path line TestName` lines from the Java probe,
//! and a Go cover profile whose sections start with `# test Name`.
//! A report that names no test stays a permission slip.

use std::collections::BTreeMap;

use crate::json::{self, Value};

/// JSON text of a choosing line map, when `text` is one of the reports above.
pub fn normalize(text: &str) -> Option<String> {
    let hits = legacy_hits(text).or_else(|| hit_lines(text)).or_else(|| go_profile(text))?;
    if hits.is_empty() {
        return None;
    }
    Some(json::stringify(&ranges(hits)))
}

fn ranges(hits: BTreeMap<String, BTreeMap<u32, Vec<String>>>) -> Value {
    let mut files = Vec::new();
    for (path, lines) in hits {
        let mut items = Vec::new();
        let mut current: Option<(u32, u32, Vec<String>)> = None;
        for (line, tests) in lines {
            match &mut current {
                Some((start, end, names)) if *end + 1 == line && names == &tests => *end = line,
                Some(_) => {
                    let (start, end, names) = current.take().unwrap();
                    items.push(range(start, end, names));
                    current = Some((line, line, tests));
                }
                None => current = Some((line, line, tests)),
            }
        }
        if let Some((start, end, names)) = current {
            items.push(range(start, end, names));
        }
        files.push((path, Value::Array(items)));
    }
    Value::Object(vec![("files".to_string(), Value::Object(files))])
}

fn range(start: u32, end: u32, tests: Vec<String>) -> Value {
    Value::object(vec![
        ("start", Value::Int(start as i64)),
        ("end", Value::Int(end as i64)),
        ("tests", Value::Array(tests.into_iter().map(Value::String).collect())),
    ])
}

fn legacy_hits(text: &str) -> Option<BTreeMap<String, BTreeMap<u32, Vec<String>>>> {
    let value = json::parse(text).ok()?;
    let entries = match value.get("files") {
        Some(Value::Object(entries)) if !entries.is_empty() => entries,
        _ => return None,
    };
    let mut hits = BTreeMap::new();
    for (path, lines) in entries {
        let Value::Object(rows) = lines else { return None };
        let mut mapped = BTreeMap::new();
        for (line, names) in rows {
            let line = line.parse::<u32>().ok()?;
            let Value::Array(names) = names else { return None };
            let tests = names.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>();
            if tests.is_empty() {
                continue;
            }
            mapped.insert(line, tests);
        }
        if !mapped.is_empty() {
            hits.insert(path.replace('\\', "/"), mapped);
        }
    }
    if hits.is_empty() { None } else { Some(hits) }
}

fn hit_lines(text: &str) -> Option<BTreeMap<String, BTreeMap<u32, Vec<String>>>> {
    let mut hits = BTreeMap::new();
    let mut saw = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if !line.starts_with("HIT ") {
            return None;
        }
        saw = true;
        let mut parts = line[4..].split(' ').collect::<Vec<_>>();
        if parts.len() < 3 {
            return None;
        }
        let test = parts.pop()?.to_string();
        let number = parts.pop()?.parse::<u32>().ok()?;
        let file = parts.join(" ").replace('\\', "/");
        if file.is_empty() || test.is_empty() {
            return None;
        }
        add(&mut hits, file, number, test);
    }
    if saw { Some(hits) } else { None }
}

fn go_profile(text: &str) -> Option<BTreeMap<String, BTreeMap<u32, Vec<String>>>> {
    if !text.lines().any(|line| line.trim().starts_with("# test ")) {
        return None;
    }
    let mut hits = BTreeMap::new();
    let mut test: Option<String> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(name) = line.strip_prefix("# test ") {
            let name = name.trim();
            if name.is_empty() {
                return None;
            }
            test = Some(name.to_string());
            continue;
        }
        if line.is_empty() || line.starts_with("mode:") {
            continue;
        }
        let Some(name) = test.clone() else { continue };
        let (file, start, end, count) = cover_span(line)?;
        if count == 0 {
            continue;
        }
        for number in start..=end {
            add(&mut hits, file.clone(), number, name.clone());
        }
    }
    if hits.is_empty() { None } else { Some(hits) }
}

fn cover_span(line: &str) -> Option<(String, u32, u32, u64)> {
    let colon = line.rfind(':')?;
    let (file, rest) = line.split_at(colon);
    let rest = rest.strip_prefix(':')?;
    let (start_part, end_part) = rest.split_once(',')?;
    let start = start_part.split('.').next()?.parse::<u32>().ok()?;
    let mut tail = end_part.split_whitespace();
    let end = tail.next()?.split('.').next()?.parse::<u32>().ok()?;
    let _statements = tail.next()?;
    let count = tail.next()?.parse::<u64>().ok()?;
    if tail.next().is_some() || end < start {
        return None;
    }
    Some((file.replace('\\', "/"), start, end, count))
}

fn add(hits: &mut BTreeMap<String, BTreeMap<u32, Vec<String>>>, file: String, line: u32, test: String) {
    let lines = hits.entry(file).or_default();
    let names = lines.entry(line).or_default();
    if !names.iter().any(|have| have == &test) {
        names.push(test);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decide::parse_test_map;

    #[test]
    fn legacy_line_object_becomes_a_range_map() {
        let raw = r#"{"files":{"src/a.go":{"3":["TestAdd"],"4":["TestAdd"],"6":["TestSub"]}}}"#;
        let text = normalize(raw).unwrap();
        let map = parse_test_map(&text);
        assert!(map.chooses);
        assert_eq!(map.names_for("src/a.go", 4), vec!["TestAdd".to_string()]);
        assert_eq!(map.names_for("src/a.go", 6), vec!["TestSub".to_string()]);
        assert!(map.names_for("src/a.go", 5).is_empty());
    }

    #[test]
    fn java_hit_lines_name_the_test() {
        let raw = "HIT src/A.java 3 FooTest\nHIT src/A.java 4 FooTest\n";
        let map = parse_test_map(&normalize(raw).unwrap());
        assert_eq!(map.names_for("src/A.java", 3), vec!["FooTest".to_string()]);
        assert_eq!(map.names_for("src/A.java", 4), vec!["FooTest".to_string()]);
    }

    #[test]
    fn a_go_profile_needs_a_test_marker() {
        let bare = "mode: set\nsrc/a.go:1.1,2.4 1 1\n";
        assert!(normalize(bare).is_none());
        let marked = "# test TestAdd\nmode: set\nsrc/a.go:1.1,2.4 1 1\n";
        let map = parse_test_map(&normalize(marked).unwrap());
        assert_eq!(map.names_for("src/a.go", 1), vec!["TestAdd".to_string()]);
        assert_eq!(map.names_for("src/a.go", 2), vec!["TestAdd".to_string()]);
    }

    #[test]
    fn a_range_map_is_not_rewritten() {
        let raw = r#"{"files":{"src/A.java":[{"start":3,"end":5,"tests":["FooTest"]}]}}"#;
        assert!(normalize(raw).is_none());
    }
}
