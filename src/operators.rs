//! Text operators. Comments and strings are not mutated.
//! `.ts` and `.tsx` also gain negate-condition, negate-ternary, and drop-not.
//! An id is `m` plus the first 12 hex digits of SHA-256 over file, span, and operator.

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
    Cobol,
    Asm,
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

const PYTHON_BOOL: &[Op] = &[
    Op { original: "and", op: "and-to-or", replacement: "or" },
    Op { original: "or", op: "or-to-and", replacement: "and" },
];

pub fn kind_of(path: &str) -> bool {
    matches!(
        std::path::Path::new(path).extension().and_then(|ext| ext.to_str()).map(|ext| ext.to_ascii_lowercase()),
        Some(ref ext) if matches!(
            ext.as_str(),
            "java" | "go" | "rs" | "py" | "js" | "mjs" | "cjs" | "ts" | "tsx" | "jsx" | "c" | "h" | "cc" | "cpp"
                | "cxx" | "hpp" | "hh" | "hxx" | "cs" | "swift" | "cob" | "cbl" | "cobol" | "s" | "asm" | "nasm" | "yasm"
        )
    )
}

fn language(file: &str) -> Kind {
    match std::path::Path::new(file)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("py") => Kind::Python,
        Some("cob") | Some("cbl") | Some("cobol") => Kind::Cobol,
        Some("s") | Some("asm") | Some("nasm") | Some("yasm") => Kind::Asm,
        _ => Kind::CLike,
    }
}

pub fn mutants_in(file: &str, text: &str) -> Vec<Mutant> {
    scan(file, text, false)
}

/// `wide` adds spaced arithmetic, integer constants, and a dropped statement.
/// Core stays the default, so a generate with no `--operators` flag does not move earlier counts.
pub fn scan(file: &str, text: &str, wide: bool) -> Vec<Mutant> {
    let kind = language(file);
    let dialect = asm_dialect(file, text);
    let code = mask(text, kind, is_rust(file));
    let mut found = Vec::new();
    let mut index = 0;
    while index < text.len() {
        if !text.is_char_boundary(index) || !code[index] {
            index += 1;
            continue;
        }
        if kind == Kind::Cobol {
            if let Some((op, len, replacement)) = cobol_word(text, index) {
                let end = index + len;
                if code[index..end].iter().all(|bit| *bit) {
                    push_mutant(&mut found, file, text, index, end, op, &replacement);
                    index = end;
                    continue;
                }
            }
            index += 1;
            continue;
        }
        if kind == Kind::Asm {
            if let Some((op, len, replacement)) = mnemonic(text, index, dialect) {
                let end = index + len;
                if code[index..end].iter().all(|bit| *bit) {
                    push_mutant(&mut found, file, text, index, end, op, &replacement);
                    index = end;
                    continue;
                }
            }
        }
        if let Some(op) = match_at(text, &code, index, kind) {
            let end = index + op.original.len();
            push_mutant(&mut found, file, text, index, end, op.op, op.replacement);
            index = end;
            continue;
        }
        if wide {
            if let Some((op, len, replacement)) = wide_at(text, &code, index, kind) {
                let end = index + len;
                push_mutant(&mut found, file, text, index, end, op, &replacement);
                index = end;
                continue;
            }
        }
        index += 1;
    }
    if wide {
        found.extend(dropped_statements(file, text, &code));
    }
    if is_typescript(file) {
        for point in crate::ast_ts::points(file, text) {
            if !found.iter().any(|item| item.start == point.start && item.op == point.op) {
                found.push(point);
            }
        }
    }
    found
}

fn is_typescript(file: &str) -> bool {
    let lower = file.to_ascii_lowercase();
    (lower.ends_with(".ts") || lower.ends_with(".tsx") || lower.ends_with(".jsx")) && !lower.ends_with(".d.ts")
}

fn push_mutant(found: &mut Vec<Mutant>, file: &str, text: &str, index: usize, end: usize, op: &str, replacement: &str) {
    let line = text[..index].bytes().filter(|byte| *byte == b'\n').count() as u32 + 1;
    let id = mutant_id(file, index, end, op);
    found.push(Mutant {
        id,
        file: file.to_string(),
        line,
        start: index,
        end,
        op: op.to_string(),
        original: text[index..end].to_string(),
        replacement: replacement.to_string(),
    });
}

/// `AND`/`OR` with the source word's case. `AND` is tried before `OR`.
fn cobol_word(text: &str, index: usize) -> Option<(&'static str, usize, String)> {
    let rest = text.get(index..)?;
    let (len, op, upper) = if rest.len() >= 3 && rest.as_bytes()[..3].eq_ignore_ascii_case(b"AND") {
        (3, "and-to-or", "OR")
    } else if rest.len() >= 2 && rest.as_bytes()[..2].eq_ignore_ascii_case(b"OR") {
        (2, "or-to-and", "AND")
    } else {
        return None;
    };
    let end = index + len;
    if !word_edge(text, index, end) {
        return None;
    }
    let source = &text[index..end];
    let replacement = if source.chars().all(|ch| ch.is_ascii_lowercase()) {
        upper.to_ascii_lowercase()
    } else {
        upper.to_string()
    };
    Some((op, len, replacement))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dialect {
    X86,
    Arm,
    Riscv,
    None,
}

fn asm_dialect(file: &str, text: &str) -> Dialect {
    let ext = std::path::Path::new(file).extension().and_then(|ext| ext.to_str()).unwrap_or("");
    let lower = ext.to_ascii_lowercase();
    if matches!(lower.as_str(), "asm" | "nasm" | "yasm") {
        return Dialect::X86;
    }
    if lower != "s" {
        return Dialect::None;
    }
    let riscv = has_token(text, &["beq", "bne", "blt", "bge", "bltu", "bgeu", "ecall"]);
    let arm = text.contains("b.eq")
        || text.contains("b.ne")
        || text.contains("b.lt")
        || text.contains("b.le")
        || text.contains("b.gt")
        || text.contains("b.ge")
        || text.contains("b.hi")
        || text.contains("b.hs")
        || text.contains("b.lo")
        || text.contains("b.ls")
        || has_token(text, &["adrp", "svc"]);
    // Only RISC-V signs: RISC-V. Only ARM signs, or neither and a lowercase `.s`: ARM.
    if riscv && !arm {
        Dialect::Riscv
    } else if (arm && !riscv) || ext == "s" {
        Dialect::Arm
    } else {
        Dialect::Riscv
    }
}

fn has_token(text: &str, words: &[&str]) -> bool {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_' {
            let start = index;
            while index < bytes.len() && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_') {
                index += 1;
            }
            let word = &text[start..index];
            if words.contains(&word) {
                return true;
            }
            continue;
        }
        index += 1;
    }
    false
}

fn mnemonic(text: &str, index: usize, dialect: Dialect) -> Option<(&'static str, usize, String)> {
    let words: &[Op] = match dialect {
        Dialect::X86 => X86,
        Dialect::Arm => ARM,
        Dialect::Riscv => RISCV,
        Dialect::None => return None,
    };
    let rest = text.get(index..)?;
    for word in words {
        if rest.len() < word.original.len() {
            continue;
        }
        let slice = &rest[..word.original.len()];
        let same = if dialect == Dialect::X86 {
            slice.eq_ignore_ascii_case(word.original)
        } else {
            slice == word.original
        };
        if !same {
            continue;
        }
        let end = index + word.original.len();
        if !mnemonic_edge(text, index, end) || text[end..].starts_with(':') {
            continue;
        }
        return Some((word.op, word.original.len(), mirror_case(slice, word.replacement)));
    }
    None
}

const X86: &[Op] = &[
    Op { original: "jne", op: "neq-to-eq", replacement: "je" },
    Op { original: "jge", op: "ge-to-gt", replacement: "jg" },
    Op { original: "jle", op: "le-to-lt", replacement: "jl" },
    Op { original: "jae", op: "ge-to-gt", replacement: "ja" },
    Op { original: "jbe", op: "le-to-lt", replacement: "jb" },
    Op { original: "jnz", op: "neq-to-eq", replacement: "jz" },
    Op { original: "and", op: "and-to-or", replacement: "or" },
    Op { original: "je", op: "eq-to-neq", replacement: "jne" },
    Op { original: "jz", op: "eq-to-neq", replacement: "jnz" },
    Op { original: "jg", op: "gt-to-ge", replacement: "jge" },
    Op { original: "jl", op: "lt-to-le", replacement: "jle" },
    Op { original: "ja", op: "gt-to-ge", replacement: "jae" },
    Op { original: "jb", op: "lt-to-le", replacement: "jbe" },
    Op { original: "or", op: "or-to-and", replacement: "and" },
];

const ARM: &[Op] = &[
    Op { original: "b.eq", op: "eq-to-neq", replacement: "b.ne" },
    Op { original: "b.ne", op: "neq-to-eq", replacement: "b.eq" },
    Op { original: "b.gt", op: "gt-to-ge", replacement: "b.ge" },
    Op { original: "b.ge", op: "ge-to-gt", replacement: "b.gt" },
    Op { original: "b.lt", op: "lt-to-le", replacement: "b.le" },
    Op { original: "b.le", op: "le-to-lt", replacement: "b.lt" },
    Op { original: "b.hi", op: "gt-to-ge", replacement: "b.hs" },
    Op { original: "b.hs", op: "ge-to-gt", replacement: "b.hi" },
    Op { original: "b.lo", op: "lt-to-le", replacement: "b.ls" },
    Op { original: "b.ls", op: "le-to-lt", replacement: "b.lo" },
    Op { original: "cbnz", op: "neq-to-eq", replacement: "cbz" },
    Op { original: "cbz", op: "eq-to-neq", replacement: "cbnz" },
];

const RISCV: &[Op] = &[
    Op { original: "bgeu", op: "le-to-lt", replacement: "bltu" },
    Op { original: "bltu", op: "lt-to-le", replacement: "bgeu" },
    Op { original: "beq", op: "eq-to-neq", replacement: "bne" },
    Op { original: "bne", op: "neq-to-eq", replacement: "beq" },
    Op { original: "bge", op: "le-to-lt", replacement: "blt" },
    Op { original: "blt", op: "lt-to-le", replacement: "bge" },
];

fn mnemonic_edge(text: &str, start: usize, end: usize) -> bool {
    let ident = |ch: char| ch.is_ascii_alphanumeric() || ch == '_' || ch == '.';
    let before = if start == 0 { None } else { text[..start].chars().next_back() };
    let after = text[end..].chars().next();
    !before.is_some_and(ident) && !after.is_some_and(ident)
}

fn mirror_case(source: &str, replacement: &str) -> String {
    if source.chars().all(|ch| !ch.is_ascii_lowercase()) {
        replacement.to_ascii_uppercase()
    } else if source.chars().all(|ch| !ch.is_ascii_uppercase()) {
        replacement.to_ascii_lowercase()
    } else {
        let mut chars = replacement.chars();
        match chars.next() {
            Some(first) => first.to_ascii_uppercase().to_string() + &chars.as_str().to_ascii_lowercase(),
            None => String::new(),
        }
    }
}

fn wide_at(text: &str, code: &[bool], index: usize, kind: Kind) -> Option<(&'static str, usize, String)> {
    if kind == Kind::Cobol || kind == Kind::Asm {
        return None;
    }
    let bytes = text.as_bytes();
    let ch = bytes.get(index).copied()?;
    if matches!(ch, b'+' | b'-' | b'*' | b'/' | b'%') {
        let prev = if index == 0 { b' ' } else { bytes[index - 1] };
        let next = bytes.get(index + 1).copied().unwrap_or(b' ');
        if !matches!(prev, b' ' | b'\t') || !matches!(next, b' ' | b'\t') || !code[index] {
            return None;
        }
        let before = text[..index].trim_end().chars().next_back().unwrap_or(' ');
        if before == ' ' || "=(,[{:?!<>&|+-*/%^~".contains(before) {
            return None;
        }
        let (op, replacement) = match ch {
            b'+' if string_beside(text, code, index) => return None,
            b'+' => ("add-to-sub", "-"),
            b'-' => ("sub-to-add", "+"),
            b'*' => ("mul-to-div", "/"),
            b'/' => ("div-to-mul", "*"),
            _ => ("mod-to-mul", "*"),
        };
        return Some((op, 1, replacement.to_string()));
    }
    if !ch.is_ascii_digit() {
        return None;
    }
    let prev = if index == 0 { b' ' } else { bytes[index - 1] };
    if prev.is_ascii_alphanumeric() || matches!(prev, b'_' | b'.' | b'$' | b'#') {
        return None;
    }
    let mut end = index;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    let digits = &text[index..end];
    let after = bytes.get(end).copied().unwrap_or(b' ');
    if after.is_ascii_alphanumeric() || matches!(after, b'_' | b'.') || !code[index..end].iter().all(|bit| *bit) {
        return None;
    }
    let replacement = if digits == "0" {
        "1".to_string()
    } else if digits == "1" {
        "0".to_string()
    } else if digits.len() > 15 || digits.starts_with('0') {
        return None;
    } else {
        let number: u128 = digits.parse().ok()?;
        (number + 1).to_string()
    };
    let op = if digits == "0" {
        "const-zero-to-one"
    } else if digits == "1" {
        "const-one-to-zero"
    } else {
        "const-inc"
    };
    Some((op, digits.len(), replacement))
}

fn string_beside(text: &str, code: &[bool], index: usize) -> bool {
    let bytes = text.as_bytes();
    for step in [usize::MAX, 1] {
        let mut at = if step == usize::MAX { index.wrapping_sub(1) } else { index + 1 };
        if step == usize::MAX && index == 0 {
            continue;
        }
        while at < bytes.len() && matches!(bytes[at], b' ' | b'\t') {
            if step == usize::MAX {
                if at == 0 {
                    break;
                }
                at -= 1;
            } else {
                at += 1;
            }
        }
        if at < bytes.len() && !code[at] && matches!(bytes[at], b'"' | b'\'' | b'`') {
            return true;
        }
    }
    false
}

fn dropped_statements(file: &str, text: &str, code: &[bool]) -> Vec<Mutant> {
    if !drops_statements(file) {
        return Vec::new();
    }
    let mut found = Vec::new();
    let mut offset = 0;
    for raw in text.split('\n') {
        let trimmed = raw.trim();
        if let Some(start_in_raw) = raw.find(trimmed) {
            let start = offset + start_in_raw;
            let end = start + trimmed.len();
            if trimmed.ends_with(';')
                && end <= code.len()
                && code[start..end].iter().all(|bit| *bit)
                && droppable(trimmed)
            {
                push_mutant(&mut found, file, text, start, end, "drop-statement", ";");
            }
        }
        offset += raw.len() + 1;
    }
    found
}

fn drops_statements(file: &str) -> bool {
    matches!(
        std::path::Path::new(file).extension().and_then(|ext| ext.to_str()).map(|ext| ext.to_ascii_lowercase()).as_deref(),
        Some("c" | "h" | "cc" | "cpp" | "hpp" | "hh" | "cxx" | "java" | "cs" | "js" | "mjs" | "cjs" | "rs")
    )
}

fn droppable(statement: &str) -> bool {
    const KEPT: &[&str] = &[
        "return", "throw", "break", "continue", "goto", "case", "default", "yield", "else", "if", "for", "while", "do",
        "switch", "try", "catch", "finally", "new", "package", "import", "using", "let", "const", "var", "val", "super",
        "assert",
    ];
    let head = statement.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_').next().unwrap_or("");
    if KEPT.contains(&head) {
        return false;
    }
    is_call(statement) || is_assign(statement) || is_step(statement)
}

fn is_call(statement: &str) -> bool {
    let body = statement.trim_end_matches(';').trim();
    let rest = body.strip_prefix("this.").unwrap_or(body);
    let Some(open) = rest.find('(') else { return false };
    let Some(close) = rest.rfind(')') else { return false };
    close > open && rest[close + 1..].trim().is_empty() && is_dotted_name(rest[..open].trim())
}

fn is_assign(statement: &str) -> bool {
    let body = statement.trim_end_matches(';').trim();
    let Some(eq) = body.find('=') else { return false };
    if body.as_bytes().get(eq + 1) == Some(&b'=') {
        return false;
    }
    let left = body[..eq].trim_end();
    let left = left
        .trim_end_matches(['+', '-', '*', '/', '%', '|', '&', '^'])
        .trim_end_matches("<<")
        .trim_end_matches(">>")
        .trim();
    !left.is_empty() && !body[eq + 1..].trim().is_empty() && is_dotted_name(left.split('[').next().unwrap_or(left).trim())
}

fn is_step(statement: &str) -> bool {
    let body = statement.trim_end_matches(';').trim();
    if let Some(name) = body.strip_prefix("++").or_else(|| body.strip_prefix("--")) {
        return is_dotted_name(name.trim());
    }
    if let Some(name) = body.strip_suffix("++").or_else(|| body.strip_suffix("--")) {
        return is_dotted_name(name.trim());
    }
    false
}

fn is_dotted_name(text: &str) -> bool {
    !text.is_empty()
        && text.split('.').all(|part| {
            let part = part.trim();
            let mut chars = part.chars();
            matches!(chars.next(), Some(ch) if ch.is_ascii_alphabetic() || ch == '_' || ch == '$')
                && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '$')
        })
}

fn match_at<'a>(text: &str, code: &[bool], index: usize, kind: Kind) -> Option<&'a Op> {
    if kind == Kind::Python {
        for word in PYTHON_BOOL {
            if !text[index..].starts_with(word.original) {
                continue;
            }
            let end = index + word.original.len();
            if word_edge(text, index, end) && code[index..end].iter().all(|bit| *bit) {
                return Some(word);
            }
        }
    }
    let ops = if kind == Kind::Python { &OPS[4..] } else { OPS };
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
    let prev = text[..index].chars().next_back().unwrap_or(' ');
    let next = text[index + original.len()..].chars().next().unwrap_or(' ');
    match original {
        "<" => next == '<' || next == '=' || next == '-' || prev == '<' || !(prev.is_whitespace() && next.is_whitespace()),
        ">" => next == '>' || next == '=' || prev == '>' || prev == '-' || prev == '=' || !(prev.is_whitespace() && next.is_whitespace()),
        "<=" => prev == '<' || next == '>' || next == '=',
        ">=" => prev == '>' || next == '=',
        "==" | "!=" | "===" | "!==" => next == '=',
        "&&" => next == '&' || prev == '&' || !has_left_operand(text, index),
        "||" => next == '|' || prev == '|' || !has_left_operand(text, index),
        _ => false,
    }
}

/// A binary `&&` or `||` has an operand on its left. Without one it is something else: a Rust
/// closure with no arguments (`move || x`, `from_fn(|| next())`) or a double reference
/// (`x: &&str`, `= &&y`). Swapping it would only give a mutant that does not build.
fn has_left_operand(text: &str, index: usize) -> bool {
    let before = text[..index].trim_end();
    let Some(last) = before.chars().next_back() else { return false };
    if "(,={;[:!|&<>+-*/%^~".contains(last) {
        return false;
    }
    if last.is_ascii_alphanumeric() || last == '_' {
        let start = before.rfind(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_')).map(|at| at + 1).unwrap_or(0);
        return !matches!(&before[start..], "move" | "return" | "async" | "yield" | "in" | "else" | "break" | "match" | "if" | "while");
    }
    true
}

fn word_edge(text: &str, start: usize, end: usize) -> bool {
    let ident = |ch: char| ch.is_ascii_alphanumeric() || ch == '_';
    let before = if start == 0 { None } else { text[..start].chars().next_back() };
    let after = text[end..].chars().next();
    !before.is_some_and(ident) && !after.is_some_and(ident)
}

/// Which bytes of `file` are code: not a comment, not a string or char literal.
pub fn code_mask(file: &str, text: &str) -> Vec<bool> {
    mask(text, language(file), is_rust(file))
}

fn is_rust(file: &str) -> bool {
    file.to_ascii_lowercase().ends_with(".rs")
}

/// Rust: a raw string `r"..."`, `r#"..."#`, or `br##"..."##` at `index`. Returns its end.
fn rust_raw_string(bytes: &[u8], index: usize) -> Option<usize> {
    if index > 0 && (bytes[index - 1].is_ascii_alphanumeric() || bytes[index - 1] == b'_') {
        return None;
    }
    let mut at = index;
    if bytes.get(at) == Some(&b'b') {
        at += 1;
    }
    if bytes.get(at) != Some(&b'r') {
        return None;
    }
    at += 1;
    let mut hashes = 0;
    while bytes.get(at) == Some(&b'#') {
        hashes += 1;
        at += 1;
    }
    if bytes.get(at) != Some(&b'"') {
        return None;
    }
    at += 1;
    while at < bytes.len() {
        if bytes[at] == b'"' && (0..hashes).all(|step| bytes.get(at + 1 + step) == Some(&b'#')) {
            return Some(at + 1 + hashes);
        }
        at += 1;
    }
    Some(bytes.len())
}

/// Rust: `'a'`, `'\n'`, and `'\u{1F600}'` are char literals; `'a` and `'static` are lifetimes or
/// labels, which are code. Returns the end of a char literal that starts at `index`.
fn rust_char_literal(text: &str, index: usize) -> Option<usize> {
    let rest = &text[index + 1..];
    let mut chars = rest.char_indices();
    let (_, first) = chars.next()?;
    if first == '\\' {
        // The escaped character can itself be a quote: `'\''`.
        let close = rest.get(2..)?.find('\'')?;
        return Some(index + 1 + 2 + close + 1);
    }
    let (at, second) = chars.next()?;
    (second == '\'').then_some(index + 1 + at + 1)
}

fn mask(text: &str, kind: Kind, rust: bool) -> Vec<bool> {
    if kind == Kind::Cobol {
        return mask_cobol(text);
    }
    let bytes = text.as_bytes();
    let mut code = vec![false; bytes.len()];
    let mut index = 0;
    while index < bytes.len() {
        if (kind == Kind::Python && bytes[index] == b'#')
            || (kind == Kind::Asm && (bytes[index] == b'#' || bytes[index] == b';'))
        {
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        if (kind == Kind::CLike || kind == Kind::Asm) && bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'/') {
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        if (kind == Kind::CLike || kind == Kind::Asm) && bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'*') {
            index += 2;
            while index + 1 < bytes.len() && !(bytes[index] == b'*' && bytes[index + 1] == b'/') {
                index += 1;
            }
            index = (index + 2).min(bytes.len());
            continue;
        }
        if rust && (bytes[index] == b'r' || bytes[index] == b'b') {
            if let Some(end) = rust_raw_string(bytes, index) {
                index = end;
                continue;
            }
        }
        if rust && bytes[index] == b'\'' {
            match rust_char_literal(text, index) {
                Some(end) => index = end,
                None => {
                    code[index] = true;
                    index += 1;
                }
            }
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

fn mask_cobol(text: &str) -> Vec<bool> {
    let bytes = text.as_bytes();
    let mut code = vec![false; bytes.len()];
    let mut index = 0;
    let mut column = 0usize;
    let mut comment_line = false;
    while index < bytes.len() {
        if bytes[index] == b'\n' {
            index += 1;
            column = 0;
            comment_line = false;
            continue;
        }
        if column == 6 && bytes[index] == b'*' {
            comment_line = true;
        }
        if comment_line {
            index += 1;
            column += 1;
            continue;
        }
        if bytes[index] == b'*' && bytes.get(index + 1) == Some(&b'>') {
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        if bytes[index] == b'\'' || bytes[index] == b'"' {
            let quote = bytes[index];
            index += 1;
            column += 1;
            while index < bytes.len() && bytes[index] != quote && bytes[index] != b'\n' {
                index += 1;
                column += 1;
            }
            if index < bytes.len() && bytes[index] == quote {
                index += 1;
                column += 1;
            }
            continue;
        }
        code[index] = true;
        index += 1;
        column += 1;
    }
    code
}

pub fn mutant_id(file: &str, start: usize, end: usize, op: &str) -> String {
    format!("m{}", crate::hash::hex12(format!("{file}\0{start}\0{end}\0{op}").as_bytes()))
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

    #[test]
    fn nbsp_before_and_does_not_panic() {
        let text = "class A {\n  boolean f(boolean a, boolean b) { return a\u{00A0}&& b; }\n}\n";
        let found = mutants_in("src/A.java", text);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].op, "and-to-or");
    }

    fn one_live(file: &str, text: &str, op: &str) -> Mutant {
        let found = mutants_in(file, text);
        assert_eq!(found.len(), 1, "{file}: {found:?}");
        assert_eq!(found[0].op, op);
        let applied = apply(text, found[0].start, found[0].end, &found[0].replacement);
        assert!(applied.contains(&found[0].replacement), "{applied}");
        found.into_iter().next().unwrap()
    }

    #[test]
    fn go_rust_c_and_csharp_skip_comments() {
        one_live("a.go", "package p\nfunc f(a, b bool) bool { return a && b }\n// a && b\n", "and-to-or");
        one_live("a.rs", "fn f(a: bool, b: bool) -> bool { a && b }\n// a && b\n", "and-to-or");
        one_live("a.c", "int f(int a, int b) { return a && b; }\n/* a && b */\n", "and-to-or");
        one_live("a.cs", "class A { bool F(bool a, bool b) { return a && b; } }\n// a && b\n", "and-to-or");
    }

    #[test]
    fn python_and_skips_a_hash_comment_and_an_identifier() {
        let found = one_live("a.py", "def f(a, b):\n    return a and b  # a and b\n", "and-to-or");
        assert_eq!(found.replacement, "or");
        let applied = apply("def f(a, b):\n    return a and b  # a and b\n", found.start, found.end, &found.replacement);
        assert!(applied.contains("# a and b"));
        assert!(mutants_in("a.py", "land = 1\n").is_empty());
    }

    #[test]
    fn js_and_ts_ids_are_the_span_hash() {
        for file in ["a.js", "a.ts"] {
            let text = "function f(a, b) { return a && b; }\n// a && b\n";
            let found = one_live(file, text, "and-to-or");
            assert!(found.id.starts_with('m'), "{}", found.id);
            assert_eq!(found.id, mutant_id(file, found.start, found.end, "and-to-or"));
            assert_eq!(found.id.len(), 13);
        }
    }

    #[test]
    fn typescript_gains_the_ast_operators() {
        let text = "function f(a: boolean, n: number) {\n  if (a && n > 0) return !a ? \"x\" : \"y\";\n  return n === 1 ? \"a\" : \"b\";\n}\n";
        let found = mutants_in("src/gate.ts", text);
        let ops: Vec<_> = found.iter().map(|item| item.op.as_str()).collect();
        for op in ["and-to-or", "gt-to-ge", "drop-not", "eq-to-neq", "negate-condition"] {
            assert!(ops.contains(&op), "{op} missing from {ops:?}");
        }
        assert_eq!(ops.iter().filter(|op| **op == "negate-ternary").count(), 2, "{ops:?}");
    }

    #[test]
    fn extra_source_extensions_are_recognised() {
        for file in ["a.mjs", "a.cjs", "a.cxx", "a.hpp", "a.hh", "a.hxx"] {
            assert!(kind_of(file), "{file}");
        }
    }

    #[test]
    fn cobol_and_keeps_case_and_skips_comments() {
        let found = one_live("a.cob", "       IF A AND B\n", "and-to-or");
        assert_eq!(found.original, "AND");
        assert_eq!(found.replacement, "OR");
        let lower = one_live("a.cbl", "       if a and b\n", "and-to-or");
        assert_eq!(lower.replacement, "or");
        assert!(mutants_in("a.cobol", "       IF A AND B\n      * AND stays\n       IF C *> AND no\n").len() == 1);
    }

    #[test]
    fn asm_and_skips_semicolon_and_hash_comments() {
        let found = one_live("a.s", "        and_ok && 1\n; && no\n# && no\n", "and-to-or");
        let applied = apply("        and_ok && 1\n; && no\n# && no\n", found.start, found.end, &found.replacement);
        assert!(applied.contains("; && no"));
        assert!(applied.contains("# && no"));
        assert!(kind_of("src/a.s") && kind_of("src/a.S") && kind_of("src/a.asm") && kind_of("src/a.cobol"));
    }

    #[test]
    fn wide_is_off_unless_asked_and_skips_a_comment() {
        let text = "int f(int a, int b) { return a + b; }\n// a + b\n";
        assert!(mutants_in("A.java", text).iter().all(|item| item.op != "add-to-sub"));
        let found = scan("A.java", text, true);
        let add = found.iter().find(|item| item.op == "add-to-sub").unwrap();
        assert_eq!(add.replacement, "-");
        assert_eq!(found.iter().filter(|item| item.op == "add-to-sub").count(), 1);
    }

    #[test]
    fn wide_drops_a_call_and_bumps_a_constant() {
        let found = scan("A.java", "void f() {\n    foo();\n}\nint n = 2;\nreturn 1;\n", true);
        assert!(found.iter().any(|item| item.op == "drop-statement" && item.original == "foo();"));
        assert!(found.iter().any(|item| item.op == "const-inc" && item.original == "2" && item.replacement == "3"));
        assert!(found.iter().any(|item| item.op == "const-one-to-zero"));
        assert!(!found.iter().any(|item| item.original.starts_with("return")));
    }

    #[test]
    fn asm_mnemonics_follow_the_dialect() {
        let x86 = mutants_in("a.asm", "  je target\n; je no\n");
        assert_eq!(x86.len(), 1);
        assert_eq!(x86[0].op, "eq-to-neq");
        assert_eq!(x86[0].replacement, "jne");
        let arm = mutants_in("a.s", "  b.eq label\n");
        assert_eq!(arm[0].replacement, "b.ne");
        let riscv = mutants_in("a.S", "  beq a0, a1, label\n  ecall\n");
        assert!(riscv.iter().any(|item| item.original == "beq" && item.replacement == "bne"));
    }

    /// Specula M5 (renderfollow: 5 of 15 mutants did not build). A Rust closure with no arguments
    /// and a double reference are not boolean operators, so no mutant swaps them.
    #[test]
    fn a_rust_closure_or_double_reference_is_not_a_boolean_operator() {
        let text = "fn f(x: &&str, a: bool, b: bool) -> bool {\n    let g = || a;\n    let h = move || b;\n    let n: Vec<u32> = std::iter::from_fn(|| None).collect();\n    let r = &&x;\n    a && b || g()\n}\n";
        let found = mutants_in("src/lib.rs", text);
        let ops: Vec<(u32, &str)> = found.iter().map(|item| (item.line, item.original.as_str())).collect();
        assert_eq!(ops, vec![(6, "&&"), (6, "||")], "{found:?}");
        assert!(mutants_in("a.js", "const f = () => a || b;\n").iter().any(|item| item.original == "||"));
    }

    /// A lifetime is not the start of a char literal, and a raw string is a string.
    #[test]
    fn rust_lifetimes_and_raw_strings_are_masked_right() {
        let text = "fn f<'a>(x: &'a str) -> bool { x == \"a > b\" }\nconst Q: char = '\\'';\nfn g(s: &str) -> bool { s == r#\"x \"|| y\"#  }\nfn h(n: u8) -> bool { n > 2 && b'x' != 0 }\n";
        let found = mutants_in("src/lib.rs", text);
        let ops: Vec<(u32, &str)> = found.iter().map(|item| (item.line, item.original.as_str())).collect();
        assert_eq!(ops, vec![(1, "=="), (3, "=="), (4, ">"), (4, "&&"), (4, "!=")], "{found:?}");
    }
}
