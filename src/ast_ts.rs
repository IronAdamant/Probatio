//! Condition, ternary, and `!` mutants for TypeScript and TSX.
//! Strings, comments, and type positions are left alone. The other operators stay in the text scan.

use crate::operators::{self, Mutant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Ident,
    Num,
    Str,
    Punct,
    Keyword,
}

struct Tok {
    kind: Kind,
    start: usize,
    end: usize,
    text: String,
}

pub fn points(file: &str, text: &str) -> Vec<Mutant> {
    if !is_script(file) {
        return Vec::new();
    }
    let tokens = tokenize(text, file.ends_with(".tsx") || file.ends_with(".jsx"));
    let mut found = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        let word = tokens[index].text.as_str();
        if tokens[index].kind == Kind::Keyword && (word == "if" || word == "while") {
            if let Some(span) = paren_inner(&tokens, index + 1, text) {
                push(&mut found, file, text, span.0, span.1, "negate-condition");
            }
        } else if tokens[index].kind == Kind::Keyword && word == "for" {
            if let Some(span) = for_condition(&tokens, index + 1, text) {
                push(&mut found, file, text, span.0, span.1, "negate-condition");
            }
        } else if tokens[index].kind == Kind::Keyword && word == "do" {
            if let Some(span) = do_while(&tokens, index, text) {
                push(&mut found, file, text, span.0, span.1, "negate-condition");
            }
        } else if tokens[index].kind == Kind::Punct && word == "?" {
            if let Some(span) = condition_before(&tokens, index, text) {
                push(&mut found, file, text, span.0, span.1, "negate-ternary");
            }
        } else if tokens[index].kind == Kind::Punct && word == "!" && is_prefix_not(&tokens, index) {
            let end = tokens.get(index + 1).map(|tok| tok.start).unwrap_or(tokens[index].end);
            if end > tokens[index].start {
                push(&mut found, file, text, tokens[index].start, end, "drop-not");
            }
        }
        index += 1;
    }
    found
}

fn is_script(file: &str) -> bool {
    let lower = file.to_ascii_lowercase();
    lower.ends_with(".ts") || lower.ends_with(".tsx") || lower.ends_with(".jsx")
}

fn push(found: &mut Vec<Mutant>, file: &str, text: &str, start: usize, end: usize, op: &str) {
    if start >= end || end > text.len() || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
        return;
    }
    let original = &text[start..end];
    if original.trim().is_empty() {
        return;
    }
    let replacement = if op == "drop-not" {
        String::new()
    } else {
        format!("!({original})")
    };
    let line = text[..start].bytes().filter(|byte| *byte == b'\n').count() as u32 + 1;
    found.push(Mutant {
        id: operators::mutant_id(file, start, end, op),
        file: file.to_string(),
        line,
        start,
        end,
        op: op.to_string(),
        original: original.to_string(),
        replacement,
    });
}

fn is_prefix_not(tokens: &[Tok], index: usize) -> bool {
    let Some(next) = tokens.get(index + 1) else {
        return false;
    };
    if next.text == "!" || next.text == "=" || next.text == "==" {
        return false;
    }
    let Some(prev) = index.checked_sub(1).map(|item| &tokens[item]) else {
        return true;
    };
    !is_operand_end(prev)
}

fn is_operand_end(tok: &Tok) -> bool {
    matches!(tok.kind, Kind::Ident | Kind::Num | Kind::Str)
        || matches!(tok.text.as_str(), ")" | "]" | "++" | "--" | "this" | "true" | "false" | "null" | "undefined")
}

fn paren_inner(tokens: &[Tok], open_at: usize, text: &str) -> Option<(usize, usize)> {
    if tokens.get(open_at).map(|tok| tok.text.as_str()) != Some("(") {
        return None;
    }
    let close = match_close(tokens, open_at, "(", ")")?;
    trimmed(text, tokens[open_at].end, tokens[close].start)
}

fn for_condition(tokens: &[Tok], open_at: usize, text: &str) -> Option<(usize, usize)> {
    if tokens.get(open_at).map(|tok| tok.text.as_str()) != Some("(") {
        return None;
    }
    let close = match_close(tokens, open_at, "(", ")")?;
    let mut depth = 0;
    let mut semis = Vec::new();
    for tok in tokens.iter().take(close).skip(open_at + 1) {
        if tok.text == "(" || tok.text == "[" || tok.text == "{" {
            depth += 1;
        } else if tok.text == ")" || tok.text == "]" || tok.text == "}" {
            depth -= 1;
        } else if tok.text == ";" && depth == 0 {
            semis.push(tok.start);
        }
    }
    if semis.len() < 2 {
        return None;
    }
    trimmed(text, semis[0] + 1, semis[1])
}

fn do_while(tokens: &[Tok], do_at: usize, text: &str) -> Option<(usize, usize)> {
    let mut index = do_at + 1;
    while index < tokens.len() && tokens[index].text != "{" && tokens[index].text != "while" {
        index += 1;
    }
    if tokens.get(index).map(|tok| tok.text.as_str()) == Some("{") {
        index = match_close(tokens, index, "{", "}")? + 1;
    }
    if tokens.get(index).map(|tok| tok.text.as_str()) != Some("while") {
        return None;
    }
    paren_inner(tokens, index + 1, text)
}

fn condition_before(tokens: &[Tok], question: usize, text: &str) -> Option<(usize, usize)> {
    if question == 0 {
        return None;
    }
    let start = expression_start(tokens, question - 1)?;
    trimmed(text, tokens[start].start, tokens[question - 1].end)
}

fn expression_start(tokens: &[Tok], mut end: usize) -> Option<usize> {
    loop {
        if tokens[end].text == ")" || tokens[end].text == "]" {
            let open = if tokens[end].text == ")" { "(" } else { "[" };
            let close = tokens[end].text.as_str();
            let mut depth = 0;
            let mut cursor = end;
            loop {
                if tokens[cursor].text == close {
                    depth += 1;
                } else if tokens[cursor].text == open {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                if cursor == 0 {
                    return None;
                }
                cursor -= 1;
            }
            end = cursor;
        }
        while end > 0 && is_prefix_op(&tokens[end - 1]) && !is_operand_end(&tokens[end - 1]) {
            end -= 1;
        }
        if end > 0 && (tokens[end - 1].text == "." || tokens[end - 1].text == "?.") {
            end -= 2;
            continue;
        }
        break;
    }
    let mut start = end;
    let mut guard = 0;
    while start > 1 && is_binary(&tokens[start - 1]) && guard < 32 {
        let Some(left) = expression_start(tokens, start - 2) else {
            break;
        };
        if left >= start {
            break;
        }
        start = left;
        guard += 1;
    }
    Some(start)
}

fn is_prefix_op(tok: &Tok) -> bool {
    matches!(tok.text.as_str(), "!" | "~" | "+" | "-" | "++" | "--" | "typeof" | "void" | "await" | "delete" | "new")
}

fn is_binary(tok: &Tok) -> bool {
    matches!(
        tok.text.as_str(),
        "&&" | "||"
            | "==="
            | "!=="
            | "=="
            | "!="
            | "<="
            | ">="
            | "<"
            | ">"
            | "+"
            | "-"
            | "*"
            | "/"
            | "%"
            | "??"
            | "&"
            | "|"
            | "^"
            | "<<"
            | ">>"
            | ">>>"
            | "instanceof"
            | "in"
            | "as"
    )
}

fn match_close(tokens: &[Tok], open_at: usize, open: &str, close: &str) -> Option<usize> {
    if tokens.get(open_at).map(|tok| tok.text.as_str()) != Some(open) {
        return None;
    }
    let mut depth = 0;
    for (index, tok) in tokens.iter().enumerate().skip(open_at) {
        if tok.text == open {
            depth += 1;
        } else if tok.text == close {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

fn trimmed(text: &str, start: usize, end: usize) -> Option<(usize, usize)> {
    if start > end || end > text.len() {
        return None;
    }
    let slice = text.get(start..end)?;
    let lead = slice.len() - slice.trim_start().len();
    let trail = slice.len() - slice.trim_end().len();
    let from = start + lead;
    let to = end - trail;
    if from >= to { None } else { Some((from, to)) }
}

fn tokenize(text: &str, tsx: bool) -> Vec<Tok> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;
    let mut brace = 0i32;
    let mut type_skip: i32 = 0;
    while index < bytes.len() {
        if !text.is_char_boundary(index) {
            index += 1;
            continue;
        }
        let rest = &text[index..];
        if rest.starts_with("//") {
            index = text[index..].find('\n').map(|at| index + at).unwrap_or(text.len());
            continue;
        }
        if rest.starts_with("/*") {
            index = text[index + 2..].find("*/").map(|at| index + 2 + at + 2).unwrap_or(text.len());
            continue;
        }
        let byte = bytes[index];
        if byte.is_ascii_whitespace() {
            index += 1;
            continue;
        }
        if byte == b'"' || byte == b'\'' || byte == b'`' {
            let end = scan_string(text, index);
            if type_skip == 0 {
                tokens.push(Tok { kind: Kind::Str, start: index, end, text: text[index..end].to_string() });
            }
            index = end;
            continue;
        }
        if byte == b'{' {
            brace += 1;
            if type_skip == 0 {
                tokens.push(punct(text, index, 1));
            }
            index += 1;
            continue;
        }
        if byte == b'}' {
            brace -= 1;
            if type_skip > 0 && brace < type_skip {
                type_skip = 0;
            }
            if type_skip == 0 {
                tokens.push(punct(text, index, 1));
            }
            index += 1;
            continue;
        }
        if text[index..].starts_with("?.") || text[index..].starts_with("??") || text[index..].starts_with("?.") {
            let len = 2;
            if type_skip == 0 {
                tokens.push(punct(text, index, len));
            }
            index += len;
            continue;
        }
        if is_ident_start(byte) {
            let mut end = index + 1;
            while end < bytes.len() && is_ident_cont(bytes[end]) {
                end += 1;
            }
            let word = &text[index..end];
            if type_skip == 0 && (word == "type" || word == "interface") && looks_like_type_decl(&tokens) {
                if word == "type" {
                    index = skip_type_alias(text, end);
                    continue;
                }
                index = skip_interface(text, end);
                continue;
            }
            if type_skip == 0 {
                let kind = if is_keyword(word) { Kind::Keyword } else { Kind::Ident };
                tokens.push(Tok { kind, start: index, end, text: word.to_string() });
            }
            index = end;
            continue;
        }
        if byte.is_ascii_digit() {
            let mut end = index + 1;
            while end < bytes.len() && (bytes[end].is_ascii_digit() || bytes[end] == b'_' || bytes[end] == b'.') {
                end += 1;
            }
            if type_skip == 0 {
                tokens.push(Tok { kind: Kind::Num, start: index, end, text: text[index..end].to_string() });
            }
            index = end;
            continue;
        }
        let len = punct_len(&text[index..]);
        if type_skip == 0 {
            let tok = punct(text, index, len);
            if tsx && tok.text == "<" && looks_jsx(&tokens) {
                index = skip_jsx(text, index);
                continue;
            }
            tokens.push(tok);
        }
        index += len;
    }
    tokens
}

fn looks_like_type_decl(tokens: &[Tok]) -> bool {
    let Some(prev) = tokens.last() else {
        return true;
    };
    matches!(prev.text.as_str(), ";" | "}" | "{" | "export" | "declare" | "abstract")
        || prev.kind == Kind::Keyword && prev.text == "export"
}

fn looks_jsx(tokens: &[Tok]) -> bool {
    let Some(prev) = tokens.last() else {
        return true;
    };
    matches!(prev.text.as_str(), "(" | "[" | "{" | "=" | "," | ";" | "return" | "?" | ":" | "&&" | "||" | "=>")
}

fn skip_jsx(text: &str, start: usize) -> usize {
    let bytes = text.as_bytes();
    let mut index = start + 1;
    let mut depth = 1;
    while index < bytes.len() {
        if text[index..].starts_with("//") {
            index = text[index..].find('\n').map(|at| index + at).unwrap_or(text.len());
            continue;
        }
        let byte = bytes[index];
        if byte == b'"' || byte == b'\'' || byte == b'`' {
            index = scan_string(text, index);
            continue;
        }
        if byte == b'<' {
            depth += 1;
        } else if byte == b'>' {
            depth -= 1;
            if depth == 0 {
                return index + 1;
            }
        }
        index += 1;
    }
    text.len()
}

fn skip_interface(text: &str, from: usize) -> usize {
    let bytes = text.as_bytes();
    let mut index = from;
    while index < bytes.len() && bytes[index] != b'{' && bytes[index] != b';' {
        index += 1;
    }
    if index < bytes.len() && bytes[index] == b'{' {
        let mut depth = 0i32;
        while index < bytes.len() {
            if bytes[index] == b'"' || bytes[index] == b'\'' || bytes[index] == b'`' {
                index = scan_string(text, index);
                continue;
            }
            if bytes[index] == b'{' {
                depth += 1;
            } else if bytes[index] == b'}' {
                depth -= 1;
                if depth == 0 {
                    return index + 1;
                }
            }
            index += 1;
        }
    }
    index
}

fn skip_type_alias(text: &str, from: usize) -> usize {
    let bytes = text.as_bytes();
    let mut index = from;
    let mut depth = 0i32;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'"' || byte == b'\'' || byte == b'`' {
            index = scan_string(text, index);
            continue;
        }
        if byte == b'{' || byte == b'(' || byte == b'<' {
            depth += 1;
        } else if byte == b'}' || byte == b')' || byte == b'>' {
            depth -= 1;
        } else if byte == b';' && depth <= 0 {
            return index + 1;
        } else if byte == b'\n' && depth <= 0 {
            return index + 1;
        }
        index += 1;
    }
    text.len()
}

fn scan_string(text: &str, start: usize) -> usize {
    let bytes = text.as_bytes();
    let quote = bytes[start];
    let mut index = start + 1;
    if quote == b'`' {
        while index < bytes.len() {
            if bytes[index] == b'\\' {
                index += 2;
                continue;
            }
            if bytes[index] == b'`' {
                return index + 1;
            }
            if text[index..].starts_with("${") {
                let mut depth = 1;
                index += 2;
                while index < bytes.len() && depth > 0 {
                    if bytes[index] == b'{' {
                        depth += 1;
                    } else if bytes[index] == b'}' {
                        depth -= 1;
                    } else if bytes[index] == b'\\' {
                        index += 1;
                    }
                    index += 1;
                }
                continue;
            }
            index += 1;
        }
        return text.len();
    }
    while index < bytes.len() && bytes[index] != quote && bytes[index] != b'\n' {
        if bytes[index] == b'\\' {
            index += 2;
            continue;
        }
        index += 1;
    }
    if index < bytes.len() && bytes[index] == quote {
        index + 1
    } else {
        index
    }
}

fn punct(text: &str, start: usize, len: usize) -> Tok {
    Tok { kind: Kind::Punct, start, end: start + len, text: text[start..start + len].to_string() }
}

fn punct_len(text: &str) -> usize {
    let candidates = [">>>=", ">>=", "<<=", ">>>", "===", "!==", "==", "!=", "<=", ">=", "&&", "||", "??", "?.", "=>", "++", "--", "<<", ">>", "**", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^="];
    for candidate in candidates {
        if text.starts_with(candidate) {
            return candidate.len();
        }
    }
    1
}

fn is_keyword(word: &str) -> bool {
    matches!(
        word,
        "if" | "else" | "while" | "for" | "do" | "return" | "function" | "class" | "const" | "let" | "var"
            | "export" | "import" | "from" | "new" | "typeof" | "void" | "await" | "delete" | "in"
            | "instanceof" | "true" | "false" | "null" | "undefined" | "this" | "as" | "type" | "interface"
            | "switch" | "case" | "throw" | "try" | "catch" | "finally"
    )
}

fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte == b'$'
}

fn is_ident_cont(byte: u8) -> bool {
    is_ident_start(byte) || byte.is_ascii_digit()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_typescript_points_match_the_three_ast_operators() {
        let text = "function f(a: boolean, n: number) {\n  if (a && n > 0) return !a ? \"x\" : \"y\";\n  return n === 1 ? \"a\" : \"b\";\n}\n";
        let found = points("src/gate.ts", text);
        let ops: Vec<_> = found.iter().map(|item| item.op.as_str()).collect();
        assert!(ops.contains(&"negate-condition"), "{ops:?}");
        assert!(ops.contains(&"drop-not"), "{ops:?}");
        assert_eq!(ops.iter().filter(|op| **op == "negate-ternary").count(), 2, "{ops:?}");
        let dropped = found.iter().find(|item| item.op == "drop-not").unwrap();
        assert_eq!(dropped.original, "!");
        assert_eq!(dropped.replacement, "");
        let negated = found.iter().find(|item| item.op == "negate-condition").unwrap();
        assert_eq!(negated.original, "a && n > 0");
        assert_eq!(negated.replacement, "!(a && n > 0)");
    }

    #[test]
    fn a_comment_and_a_string_are_not_conditions() {
        let text = "const s = \"if (a)\";\n// if (b)\nfunction f(a: boolean) { if (a) return 1; }\n";
        let found = points("src/gate.ts", text);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].original, "a");
    }
}
