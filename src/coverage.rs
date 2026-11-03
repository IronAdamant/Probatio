//! Turn a coverage report into the `files` line map `mutate run` already reads.
//!
//! A range map is left alone. These shapes are converted:
//! the old `{ "files": { "a.go": { "3": ["TestA"] } } }` object,
//! `HIT path line TestName` lines from the Java probe,
//! and a Go cover profile. A span with count 0 is not a hit.
//! A profile with no `# test` line records the hit and names no test.
//! lcov, JaCoCo XML, and Node's V8 JSON are the same kind of hit.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

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
    let looks = text.lines().any(|line| {
        let line = line.trim();
        line.starts_with("mode:") || line.starts_with("# test ") || cover_span(line).is_some()
    });
    if !looks {
        return None;
    }
    let mut hits = BTreeMap::new();
    let mut test: Option<String> = None;
    let mut saw_span = false;
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
        let Some((file, start, end, count)) = cover_span(line) else {
            continue;
        };
        saw_span = true;
        if count == 0 {
            continue;
        }
        for number in start..=end {
            if let Some(name) = test.clone() {
                add(&mut hits, file.clone(), number, name);
            } else {
                touch(&mut hits, file.clone(), number);
            }
        }
    }
    if !saw_span || hits.is_empty() { None } else { Some(hits) }
}

/// lcov `DA` records. A count of 0 is not a hit. A positive count names no test.
pub fn lcov_lines(text: &str) -> Option<String> {
    let mut hits = BTreeMap::new();
    let mut file: Option<String> = None;
    let mut saw = false;
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(path) = line.strip_prefix("SF:") {
            saw = true;
            let path = path.trim().replace('\\', "/");
            file = if path.is_empty() { None } else { Some(path) };
            continue;
        }
        let Some(rest) = line.strip_prefix("DA:") else { continue };
        let Some(path) = file.clone() else { continue };
        let Some((number, count)) = da_count(rest) else { continue };
        if count > 0 {
            touch(&mut hits, path, number);
        }
    }
    if !saw || hits.is_empty() { None } else { Some(json::stringify(&ranges(hits))) }
}

fn da_count(rest: &str) -> Option<(u32, u64)> {
    let (number, tail) = rest.split_once(',')?;
    let count = tail.split(',').next()?.trim();
    Some((number.trim().parse().ok()?, count.parse().ok()?))
}

/// JaCoCo `jacoco.xml`. `ci` above 0 is a hit with no test name.
/// A source path is kept when it matches a file, or when exactly one file ends with it.
pub fn jacoco_lines(xml: &str, package: &Path) -> Option<String> {
    let hits = jacoco_hits(xml, package);
    if hits.is_empty() { None } else { Some(json::stringify(&ranges(hits))) }
}

fn jacoco_hits(xml: &str, package: &Path) -> BTreeMap<String, BTreeMap<u32, Vec<String>>> {
    let mut hits = BTreeMap::new();
    let mut pkg = String::new();
    let mut source = String::new();
    let mut rest = xml;
    while let Some(start) = rest.find('<') {
        rest = &rest[start + 1..];
        if rest.starts_with('!') || rest.starts_with('?') {
            if let Some(end) = rest.find('>') {
                rest = &rest[end + 1..];
            }
            continue;
        }
        let closing = rest.starts_with('/');
        if closing {
            rest = &rest[1..];
        }
        let end = match rest.find('>') {
            Some(end) => end,
            None => break,
        };
        let tag = &rest[..end];
        rest = rest.get(end + 1..).unwrap_or("");
        let name_end = tag.find(|ch: char| ch.is_whitespace() || ch == '/').unwrap_or(tag.len());
        let name = &tag[..name_end];
        if closing {
            if name == "sourcefile" {
                source.clear();
            }
            if name == "package" {
                pkg.clear();
                source.clear();
            }
            continue;
        }
        if name == "package" {
            if let Some(value) = xml_attr(tag, "name") {
                pkg = value.replace('.', "/");
            }
        } else if name == "sourcefile" {
            if let Some(value) = xml_attr(tag, "name") {
                source = value;
            }
        } else if name == "line" && !source.is_empty() {
            let Some(number) = xml_attr(tag, "nr").and_then(|value| value.parse::<u32>().ok()) else {
                continue;
            };
            let covered = xml_attr(tag, "ci").and_then(|value| value.parse::<u64>().ok()).unwrap_or(0);
            if covered == 0 {
                continue;
            }
            let rel = if pkg.is_empty() { source.clone() } else { format!("{pkg}/{source}") };
            touch(&mut hits, resolve_suffix(package, &rel), number);
        }
    }
    hits
}

fn xml_attr(tag: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=");
    let mut rest = tag;
    while let Some(index) = rest.find(&needle) {
        let boundary = index == 0 || rest.as_bytes()[index - 1].is_ascii_whitespace();
        if !boundary {
            rest = &rest[index + key.len()..];
            continue;
        }
        let after = &rest[index + needle.len()..];
        let quote = after.chars().next()?;
        if quote != '"' && quote != '\'' {
            return None;
        }
        let value = after[1..].split(quote).next()?;
        return Some(value.to_string());
    }
    None
}

fn resolve_suffix(root: &Path, rel: &str) -> String {
    let rel = rel.replace('\\', "/");
    if root.join(&rel).is_file() {
        return rel;
    }
    let mut found = Vec::new();
    walk_suffix(root, root, &rel, &mut found);
    if found.len() == 1 { found.remove(0) } else { rel }
}

fn walk_suffix(root: &Path, dir: &Path, rel: &str, found: &mut Vec<String>) {
    if found.len() > 1 {
        return;
    }
    let Ok(read) = fs::read_dir(dir) else { return };
    for entry in read.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name == "target" || name == "node_modules" || name == "dist" {
            continue;
        }
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_symlink() {
            continue;
        }
        let path = entry.path();
        if kind.is_dir() {
            walk_suffix(root, &path, rel, found);
        } else if kind.is_file() {
            let Ok(relative) = path.strip_prefix(root) else { continue };
            let relative = relative.to_string_lossy().replace('\\', "/");
            if relative == rel || relative.ends_with(&format!("/{rel}")) {
                found.push(relative);
            }
        }
    }
}

/// Node V8 coverage JSON in `dir`. Offsets become source lines. A count above 0 names no test.
pub fn v8_lines(dir: &Path, package: &Path) -> Option<String> {
    let mut hits = BTreeMap::new();
    let Ok(read) = fs::read_dir(dir) else { return None };
    for entry in read.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else { continue };
        let Ok(value) = json::parse(&text) else { continue };
        let Some(Value::Array(results)) = value.get("result") else { continue };
        for script in results {
            let Some(url) = script.get("url").and_then(Value::as_str) else { continue };
            let Some(source_path) = v8_path(url, package) else { continue };
            let Ok(source) = fs::read_to_string(&source_path) else { continue };
            let rel = match source_path.strip_prefix(package) {
                Ok(relative) => relative.to_string_lossy().replace('\\', "/"),
                Err(_) => source_path.to_string_lossy().replace('\\', "/"),
            };
            let Some(Value::Array(functions)) = script.get("functions") else { continue };
            for function in functions {
                let Some(Value::Array(spans)) = function.get("ranges") else { continue };
                for span in spans {
                    let count = span.get("count").and_then(Value::as_int).unwrap_or(0);
                    if count <= 0 {
                        continue;
                    }
                    let start = span.get("startOffset").and_then(Value::as_int).unwrap_or(0).max(0) as usize;
                    let end = span.get("endOffset").and_then(Value::as_int).unwrap_or(0).max(0) as usize;
                    if end <= start {
                        continue;
                    }
                    let last = end.min(source.len()).saturating_sub(1);
                    let first_line = line_number(&source, start.min(source.len()));
                    let last_line = line_number(&source, last);
                    for number in first_line..=last_line {
                        touch(&mut hits, rel.clone(), number);
                    }
                }
            }
        }
    }
    if hits.is_empty() { None } else { Some(json::stringify(&ranges(hits))) }
}

fn v8_path(url: &str, package: &Path) -> Option<PathBuf> {
    if let Some(rest) = url.strip_prefix("file://") {
        let path = rest.strip_prefix("localhost").unwrap_or(rest);
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let joined = package.join(url);
    if joined.is_file() { Some(joined) } else { None }
}

fn line_number(text: &str, offset: usize) -> u32 {
    let end = offset.min(text.len());
    text.as_bytes()[..end].iter().filter(|byte| **byte == b'\n').count() as u32 + 1
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
    if !test.is_empty() && !names.iter().any(|have| have == &test) {
        names.push(test);
    }
}

fn touch(hits: &mut BTreeMap<String, BTreeMap<u32, Vec<String>>>, file: String, line: u32) {
    hits.entry(file).or_default().entry(line).or_default();
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
    fn a_bare_go_profile_keeps_a_positive_count_and_drops_zero() {
        let bare = "mode: set\nsrc/a.go:1.1,1.4 1 0\nsrc/a.go:2.1,2.4 1 1\n";
        let map = parse_test_map(&normalize(bare).unwrap());
        assert!(matches!(map.line_hit("src/a.go", 1), crate::decide::LineHit::Dark));
        assert!(matches!(map.line_hit("src/a.go", 2), crate::decide::LineHit::Listed(names) if names.is_empty()));
        let marked = "# test TestAdd\nmode: set\nsrc/a.go:1.1,2.4 1 1\n";
        let map = parse_test_map(&normalize(marked).unwrap());
        assert_eq!(map.names_for("src/a.go", 1), vec!["TestAdd".to_string()]);
        assert_eq!(map.names_for("src/a.go", 2), vec!["TestAdd".to_string()]);
    }

    #[test]
    fn lcov_and_jacoco_and_v8_hits_name_no_test() {
        let lcov = "SF:src/a.rs\nDA:1,0\nDA:2,3\nend_of_record\n";
        let map = parse_test_map(&lcov_lines(lcov).unwrap());
        assert!(matches!(map.line_hit("src/a.rs", 1), crate::decide::LineHit::Dark));
        assert!(matches!(map.line_hit("src/a.rs", 2), crate::decide::LineHit::Listed(names) if names.is_empty()));

        let root = std::env::temp_dir().join(format!("probatio-jacoco-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src/main/java/org/example")).unwrap();
        fs::write(root.join("src/main/java/org/example/A.java"), "class A {}\n").unwrap();
        let xml = r#"<report><package name="org/example"><sourcefile name="A.java"><line nr="1" mi="1" ci="0"/><line nr="2" mi="0" ci="1"/></sourcefile></package></report>"#;
        let map = parse_test_map(&jacoco_lines(xml, &root).unwrap());
        assert!(matches!(map.line_hit("src/main/java/org/example/A.java", 1), crate::decide::LineHit::Dark));
        assert!(matches!(map.line_hit("src/main/java/org/example/A.java", 2), crate::decide::LineHit::Listed(names) if names.is_empty()));

        let js = root.join("src/a.js");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(&js, "let a = 1;\nlet b = 2;\n").unwrap();
        let url = format!("file://{}", js.display());
        let v8 = format!(
            r#"{{"result":[{{"url":{url},"functions":[{{"ranges":[{{"startOffset":0,"endOffset":4,"count":0}},{{"startOffset":11,"endOffset":20,"count":1}}]}}]}}]}}"#,
            url = json::stringify_compact(&Value::String(url))
        );
        let dir = root.join("v8");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("coverage.json"), v8).unwrap();
        let map = parse_test_map(&v8_lines(&dir, &root).unwrap());
        assert!(matches!(map.line_hit("src/a.js", 1), crate::decide::LineHit::Dark));
        assert!(matches!(map.line_hit("src/a.js", 2), crate::decide::LineHit::Listed(names) if names.is_empty()));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_range_map_is_not_rewritten() {
        let raw = r#"{"files":{"src/A.java":[{"start":3,"end":5,"tests":["FooTest"]}]}}"#;
        assert!(normalize(raw).is_none());
    }
}
