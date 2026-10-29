//! Text operators. Comments and strings are not mutated. JS and TS use the same set as Java.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mutant {
    pub id: String,
    pub file: String,
    pub line: u32,
    pub start: usize,
    pub end: usize,
    pub op: String,
    pub original: String,
    pub replacement: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    CLike,
    Python,
}

struct Op {
    original: &'static str,
    op: &'static str,
    replacement: &'static str,
}

const OPS: &[Op] = &[
    Op { original: "&&", op: "and-to-or", replacement: "||" },
    Op { original: "||", op: "or-to-and", replacement: "&&" },
    Op { original: "===", op: "eq-to-neq", replacement: "!==" },
    Op { original: "!==", op: "neq-to-eq", replacement: "===" },
    Op { original: "<=", op: "le-to-lt", replacement: "<" },
    Op { original: ">=", op: "ge-to-gt", replacement: ">" },
    Op { original: "==", op: "eq-to-neq", replacement: "!=" },
    Op { original: "!=", op: "neq-to-eq", replacement: "==" },
    Op { original: "<", op: "lt-to-le", replacement: "<=" },
    Op { original: ">", op: "gt-to-ge", replacement: ">=" },
];

const WORDS: &[Op] = &[
    Op { original: "true", op: "true-to-false", replacement: "false" },
    Op { original: "false", op: "false-to-true", replacement: "true" },
];

pub fn kind_of(path: &str) -> bool {
    matches!(
        std::path::Path::new(path).extension().and_then(|ext| ext.to_str()).map(|ext| ext.to_ascii_lowercase()),
        Some(ref ext) if matches!(
            ext.as_str(),
            "java" | "go" | "rs" | "py" | "js" | "ts" | "tsx" | "jsx" | "c" | "h" | "cc" | "cpp" | "cs" | "swift" | "cob" | "cbl"
        )
    )
}

pub fn mutants_in(file: &str, text: &str) -> Vec<Mutant> {
    let kind = if file.ends_with(".py") { Kind::Python } else { Kind::CLike };
    let code = mask(text, kind);
    let mut found = Vec::new();
    let mut index = 0;
    while index < text.len() {
        if !text.is_char_boundary(index) || !code[index] {
            index += 1;
            continue;
        }
        if let Some(op) = match_at(text, &code, index, kind) {
            let end = index + op.original.len();
            let line = text[..index].bytes().filter(|byte| *byte == b'\n').count() as u32 + 1;
            let id = mutant_id(file, index, &op.op, &op.replacement);
            found.push(Mutant {
                id,
                file: file.to_string(),
                line,
                start: index,
                end,
                op: op.op.to_string(),
                original: text[index..end].to_string(),
                replacement: op.replacement.to_string(),
            });
            index = end;
            continue;
        }
        index += 1;
    }
    found
}

fn match_at<'a>(text: &str, code: &[bool], index: usize, kind: Kind) -> Option<&'a Op> {
    let ops = if kind == Kind::Python {
        &OPS[4..]
    } else {
        OPS
    };
    for op in ops {
        if !text[index..].starts_with(op.original) {
            continue;
        }
        let end = index + op.original.len();
        if !code[index..end].iter().all(|bit| *bit) {
            continue;
        }
        if blocked(text, index, op.original) {
            continue;
        }
        return Some(op);
    }
    for word in WORDS {
        if !text[index..].starts_with(word.original) {
            continue;
        }
        let end = index + word.original.len();
        if !word_edge(text, index, end) || !code[index..end].iter().all(|bit| *bit) {
            continue;
        }
        return Some(word);
    }
    None
}

fn blocked(text: &str, index: usize, original: &str) -> bool {
    let prev = if index == 0 { ' ' } else { text[index - 1..].chars().next().unwrap_or(' ') };
    let next = text[index + original.len()..].chars().next().unwrap_or(' ');
    match original {
        "<" => next == '<' || next == '=' || next == '-' || prev == '<' || !(prev.is_whitespace() && next.is_whitespace()),
        ">" => next == '>' || next == '=' || prev == '>' || prev == '-' || prev == '=' || !(prev.is_whitespace() && next.is_whitespace()),
        "<=" => prev == '<' || next == '>' || next == '=',
        ">=" => prev == '>' || next == '=',
        "==" | "!=" | "===" | "!==" => next == '=',
        "&&" => next == '&' || prev == '&',
        "||" => next == '|' || prev == '|',
        _ => false,
    }
}

fn word_edge(text: &str, start: usize, end: usize) -> bool {
    let ident = |ch: char| ch.is_ascii_alphanumeric() || ch == '_';
    let before = if start == 0 { None } else { text[..start].chars().next_back() };
    let after = text[end..].chars().next();
    !before.is_some_and(ident) && !after.is_some_and(ident)
}

fn mask(text: &str, kind: Kind) -> Vec<bool> {
    let bytes = text.as_bytes();
    let mut code = vec![false; bytes.len()];
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'#' && kind == Kind::Python {
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        if kind == Kind::CLike && bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'/') {
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        if kind == Kind::CLike && bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'*') {
            index += 2;
            while index + 1 < bytes.len() && !(bytes[index] == b'*' && bytes[index + 1] == b'/') {
                index += 1;
            }
            index = (index + 2).min(bytes.len());
            continue;
        }
        if bytes[index] == b'"' || bytes[index] == b'\'' {
            let quote = bytes[index];
            index += 1;
            while index < bytes.len() && bytes[index] != quote {
                if bytes[index] == b'\\' {
                    index += 1;
                }
                index += 1;
            }
            index = (index + 1).min(bytes.len());
            continue;
        }
        code[index] = true;
        index += 1;
    }
    code
}

pub fn mutant_id(file: &str, start: usize, op: &str, replacement: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in format!("{file}\0{start}\0{op}\0{replacement}").bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("m{hash:012x}")
}

pub fn apply(text: &str, start: usize, end: usize, replacement: &str) -> String {
    let mut out = String::new();
    out.push_str(&text[..start]);
    out.push_str(replacement);
    out.push_str(&text[end..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_and_is_a_mutant_and_a_comment_is_not() {
        let text = "class A {\n  boolean f(boolean a, boolean b) { return a && b; }\n  // a && b\n}\n";
        let found = mutants_in("src/A.java", text);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].op, "and-to-or");
        assert_eq!(found[0].replacement, "||");
        let applied = apply(text, found[0].start, found[0].end, &found[0].replacement);
        assert!(applied.contains("a || b"));
        assert!(applied.contains("// a && b"));
    }
}
