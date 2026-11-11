//! Test code is not mutated by default. A mutant in a test assertion only asks whether the test
//! tests itself, and Specula's lanes found most of their survivors there. `--include-tests`
//! (MCP `includeTests`) turns this off.
//!
//! A whole file is test code when its path says so (`tests/`, `benches/`, `*_test.go`,
//! `test_*.py`, `*.test.ts`, `FooTest.java`, ...). Inside a Rust source file, an item under
//! `#[cfg(test)]`, `#[test]`, or another test attribute (`#[tokio::test]`, `#[bench]`) is test code
//! from its attribute line to its closing brace.

/// Directory names that hold tests or benchmarks, in any supported language.
const TEST_DIRS: &[&str] = &["tests", "test", "benches", "__tests__", "__mocks__"];

/// True when `rel` (a path inside the package, `/` or `\` separated) is a test or bench file.
pub fn is_test_path(rel: &str) -> bool {
    let parts: Vec<&str> = rel.split(['/', '\\']).filter(|part| !part.is_empty() && *part != ".").collect();
    let Some((name, dirs)) = parts.split_last() else { return false };
    if dirs.iter().any(|dir| TEST_DIRS.contains(dir)) {
        return true;
    }
    let lower = name.to_ascii_lowercase();
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) => (stem, ext.to_ascii_lowercase()),
        None => (*name, String::new()),
    };
    let stem_lower = stem.to_ascii_lowercase();
    match ext.as_str() {
        "rs" => stem == "tests" || stem_lower.ends_with("_test") || stem_lower.ends_with("_tests"),
        "go" => stem_lower.ends_with("_test"),
        "py" => stem_lower.starts_with("test_") || stem_lower.ends_with("_test") || stem_lower == "conftest",
        "js" | "mjs" | "cjs" | "ts" | "tsx" | "jsx" => lower.contains(".test.") || lower.contains(".spec."),
        "java" | "cs" | "swift" | "kt" => stem.ends_with("Test") || stem.ends_with("Tests") || stem.ends_with("IT"),
        "c" | "cc" | "cpp" | "cxx" | "h" | "hpp" => {
            stem_lower.starts_with("test_") || stem_lower.ends_with("_test") || stem_lower.ends_with("_tests")
        }
        _ => false,
    }
}

/// Inclusive 1-based line ranges of test code inside a file that is not itself a test file.
/// Only Rust marks test items in place; other languages keep tests in their own files.
pub fn test_ranges(rel: &str, text: &str) -> Vec<(u32, u32)> {
    if !rel.to_ascii_lowercase().ends_with(".rs") {
        return Vec::new();
    }
    let code = crate::operators::code_mask(rel, text);
    let bytes = text.as_bytes();
    let line_of = |offset: usize| text[..offset.min(text.len())].bytes().filter(|byte| *byte == b'\n').count() as u32 + 1;
    let mut ranges: Vec<(u32, u32)> = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if !code[index] || bytes[index] != b'#' {
            index += 1;
            continue;
        }
        let inner = bytes.get(index + 1) == Some(&b'!');
        let open = if inner { index + 2 } else { index + 1 };
        if bytes.get(open) != Some(&b'[') {
            index += 1;
            continue;
        }
        let Some(close) = matching(bytes, &code, open, b'[', b']') else { break };
        let attribute: String = text[open + 1..close].chars().filter(|ch| !ch.is_whitespace()).collect();
        if !is_test_attribute(&attribute) {
            index = close + 1;
            continue;
        }
        if inner {
            // `#![cfg(test)]` at the top of a module file: the whole file is test code.
            return vec![(1, line_of(text.len()).max(1))];
        }
        let end = item_end(bytes, &code, close + 1);
        ranges.push((line_of(index), line_of(end)));
        index = end + 1;
    }
    ranges
}

/// How many lines the ranges cover.
pub fn line_count(ranges: &[(u32, u32)]) -> usize {
    ranges.iter().map(|(start, end)| (end - start + 1) as usize).sum()
}

pub fn in_ranges(ranges: &[(u32, u32)], line: u32) -> bool {
    ranges.iter().any(|(start, end)| line >= *start && line <= *end)
}

/// `test`, `tokio::test`, `bench`, `rstest`, `test_case(..)`, `wasm_bindgen_test`, `cfg(test)`, and
/// `cfg(all(test, ..))`. Not `cfg(not(test))`, `cfg(any(test, ..))` (also built outside tests), or
/// `cfg_attr(test, ..)`.
fn is_test_attribute(attribute: &str) -> bool {
    let (path, args) = match attribute.split_once('(') {
        Some((path, rest)) => (path, rest.strip_suffix(')').unwrap_or(rest)),
        None => (attribute, ""),
    };
    if path == "cfg" {
        if args == "test" {
            return true;
        }
        if let Some(list) = args.strip_prefix("all(").and_then(|rest| rest.strip_suffix(')')) {
            return top_level(list).contains(&"test");
        }
        return false;
    }
    let last = path.rsplit("::").next().unwrap_or(path);
    last == "test" || last == "bench" || last == "rstest" || last.starts_with("test_") || last.ends_with("_test")
}

/// Comma-separated items of a cfg list, split only at depth 0.
fn top_level(list: &str) -> Vec<&str> {
    let mut items = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (at, ch) in list.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                items.push(&list[start..at]);
                start = at + 1;
            }
            _ => {}
        }
    }
    items.push(&list[start..]);
    items
}

/// The index of the bracket that closes the one at `open`, counting only code bytes.
fn matching(bytes: &[u8], code: &[bool], open: usize, left: u8, right: u8) -> Option<usize> {
    let mut depth = 0usize;
    for at in open..bytes.len() {
        if !code[at] {
            continue;
        }
        if bytes[at] == left {
            depth += 1;
        } else if bytes[at] == right {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(at);
            }
        }
    }
    None
}

/// The last byte of the item that follows an attribute: its closing `}`, or the `;` of an item
/// with no body (`mod tests;`, `use x;`). Other attributes in between are skipped.
fn item_end(bytes: &[u8], code: &[bool], from: usize) -> usize {
    let mut depth = 0i32;
    let mut at = from;
    while at < bytes.len() {
        if !code[at] {
            at += 1;
            continue;
        }
        match bytes[at] {
            b'#' if depth == 0 && bytes.get(at + 1) == Some(&b'[') => {
                if let Some(close) = matching(bytes, code, at + 1, b'[', b']') {
                    at = close + 1;
                    continue;
                }
            }
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth -= 1,
            b';' if depth == 0 => return at,
            b'{' if depth == 0 => return matching(bytes, code, at, b'{', b'}').unwrap_or(bytes.len().saturating_sub(1)),
            _ => {}
        }
        at += 1;
    }
    bytes.len().saturating_sub(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_and_bench_paths_are_test_code() {
        for path in [
            "tests/cli.rs",
            "crates/x/tests/a.rs",
            "benches/speed.rs",
            "src/foo/tests.rs",
            "src/parse_tests.rs",
            "pkg/a_test.go",
            "tests/test_x.py",
            "test_y.py",
            "pkg/conftest.py",
            "src/a.test.ts",
            "src/__tests__/b.js",
            "lib/c.spec.mjs",
            "src/test/java/a/FooTest.java",
            "Lib/FooTests.cs",
        ] {
            assert!(is_test_path(path), "{path}");
        }
        for path in ["src/lib.rs", "src/testing.rs", "src/contest.py", "src/attest.go", "src/Contest.java", "src/latest.ts", "src/protest.rs"] {
            assert!(!is_test_path(path), "{path}");
        }
    }

    #[test]
    fn rust_test_items_are_ranged_from_attribute_to_brace() {
        let text = "\
pub fn add(a: u32, b: u32) -> u32 {
    a + b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds() {
        assert!(add(1, 2) == 3 && true);
    }
}

#[test]
fn outside() { assert!(add(0, 0) == 0); }

#[tokio::test]
#[should_panic(expected = \"}\")]
async fn async_one() -> Result<(), ()> { Ok(()) }

#[cfg(not(test))]
fn real() -> bool { 1 > 0 }

#[cfg(all(test, feature = \"x\"))]
mod more;
";
        let ranges = test_ranges("src/lib.rs", text);
        assert_eq!(ranges, vec![(5, 13), (15, 16), (18, 20), (25, 26)]);
        assert_eq!(line_count(&ranges), 9 + 2 + 3 + 2);
        assert!(!in_ranges(&ranges, 2) && in_ranges(&ranges, 11) && !in_ranges(&ranges, 23));
        assert_eq!(test_ranges("src/lib.py", text), vec![]);
        assert_eq!(test_ranges("src/m.rs", "#![cfg(test)]\nfn a() {}\nfn b() {}\n"), vec![(1, 4)]);
    }
}
