//! Shell-suite lines. `testLine` and `passLine` are the project's regular expressions.
//! Without `testLine`, TAP (`ok 1 - name`) is read, and anything else is one command.

use crate::json::Value;

#[derive(Clone, Debug)]
enum Atom {
    Empty,
    Lit(char),
    Any,
    Seq(Vec<Atom>),
    Alt(Vec<Atom>),
    Repeat { inner: Box<Atom>, min: u8, open: bool },
    Capture(Box<Atom>),
    Start,
    End,
}

pub fn compile(source: &str) -> Result<(), String> {
    let _ = parse(source)?;
    Ok(())
}

pub struct ScriptConfig {
    pub test_line: Option<String>,
    pub pass_line: Option<String>,
}

pub enum ScriptHit {
    /// `testLine` was set and no line matched. That is not a test report.
    Silent,
    /// Failed names. An empty list passed.
    Named(Vec<String>),
}

/// `None` means the output is not a named report, so the caller keeps the process exit.
pub fn report(stdout: &str, stderr: &str, code: Option<i32>, config: &ScriptConfig) -> Option<ScriptHit> {
    let text = format!("{stdout}\n{stderr}");
    if let Some(pattern) = &config.test_line {
        let names = named_lines(&text, pattern);
        if names.is_empty() {
            return Some(ScriptHit::Silent);
        }
        let passed = code == Some(0) || config.pass_line.as_ref().is_some_and(|pattern| line_hits(&text, pattern));
        let failed = if passed { Vec::new() } else { vec![names[names.len() - 1].clone()] };
        return Some(ScriptHit::Named(failed));
    }
    tap_report(&text)
}

pub fn config_of(value: &Value) -> ScriptConfig {
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_string);
    ScriptConfig { test_line: text("testLine"), pass_line: text("passLine") }
}

fn named_lines(text: &str, pattern: &str) -> Vec<String> {
    let Ok(atom) = parse(pattern) else { return Vec::new() };
    let mut names = Vec::new();
    for line in text.lines().map(clean_line) {
        if let Some(name) = find_match(&atom, &line) {
            let name = name.trim();
            if !name.is_empty() && !names.iter().any(|seen| seen == name) {
                names.push(name.to_string());
            }
        }
    }
    names
}

fn line_hits(text: &str, pattern: &str) -> bool {
    let Ok(atom) = parse(pattern) else { return false };
    text.lines().map(clean_line).any(|line| find_match(&atom, &line).is_some())
}

fn tap_report(text: &str) -> Option<ScriptHit> {
    let mut names = Vec::new();
    let mut failed = Vec::new();
    for line in text.lines().map(clean_line) {
        let Some((name, bad)) = tap_line(&line) else { continue };
        if names.iter().any(|seen| seen == &name) {
            continue;
        }
        names.push(name.clone());
        if bad {
            failed.push(name);
        }
    }
    if names.is_empty() { None } else { Some(ScriptHit::Named(failed)) }
}

fn tap_line(line: &str) -> Option<(String, bool)> {
    let rest = line.strip_prefix("not ok").map(|rest| (rest, true)).or_else(|| line.strip_prefix("ok").map(|rest| (rest, false)))?;
    let (body, failed) = rest;
    if !body.is_empty() && !body.starts_with(|ch: char| ch.is_whitespace()) {
        return None;
    }
    let body = body.trim();
    let body = body.trim_start_matches(|ch: char| ch.is_ascii_digit()).trim();
    let body = body.strip_prefix('-').unwrap_or(body).trim();
    let (name, trailer) = body.split_once('#').map(|(name, rest)| (name.trim(), rest.trim())).unwrap_or((body, ""));
    if name.is_empty() {
        return None;
    }
    let skipped = trailer.starts_with("SKIP") || trailer.starts_with("TODO");
    Some((name.to_string(), failed && !skipped))
}

fn clean_line(raw: &str) -> String {
    raw.chars().filter(|ch| !ch.is_control() || *ch == '\t').collect::<String>().trim().to_string()
}

fn parse(source: &str) -> Result<Atom, String> {
    let mut chars = source.chars().peekable();
    let atom = parse_alt(&mut chars)?;
    if chars.peek().is_some() {
        return Err("unbalanced pattern".into());
    }
    Ok(atom)
}

fn parse_alt(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Result<Atom, String> {
    let mut items = vec![parse_seq(chars)?];
    while chars.peek() == Some(&'|') {
        chars.next();
        items.push(parse_seq(chars)?);
    }
    if items.len() == 1 { Ok(items.pop().unwrap()) } else { Ok(Atom::Alt(items)) }
}

fn parse_seq(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Result<Atom, String> {
    let mut items = Vec::new();
    while let Some(ch) = chars.peek().copied() {
        if ch == '|' || ch == ')' {
            break;
        }
        items.push(parse_piece(chars)?);
    }
    if items.is_empty() { Ok(Atom::Empty) } else if items.len() == 1 { Ok(items.pop().unwrap()) } else { Ok(Atom::Seq(items)) }
}

fn parse_piece(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Result<Atom, String> {
    let atom = parse_atom(chars)?;
    let (min, open) = match chars.peek().copied() {
        Some('+') => {
            chars.next();
            (1, true)
        }
        Some('*') => {
            chars.next();
            (0, true)
        }
        Some('?') => {
            chars.next();
            (0, false)
        }
        _ => return Ok(atom),
    };
    Ok(Atom::Repeat { inner: Box::new(atom), min, open })
}

fn parse_atom(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Result<Atom, String> {
    match chars.next() {
        Some('^') => Ok(Atom::Start),
        Some('$') => Ok(Atom::End),
        Some('.') => Ok(Atom::Any),
        Some('\\') => chars.next().map(Atom::Lit).ok_or_else(|| "trailing escape".into()),
        Some('(') => {
            let inner = parse_alt(chars)?;
            if chars.next() != Some(')') {
                return Err("unclosed group".into());
            }
            Ok(Atom::Capture(Box::new(inner)))
        }
        Some('[') => Err("unclosed character class".into()),
        Some(ch) => Ok(Atom::Lit(ch)),
        None => Err("empty pattern".into()),
    }
}

fn find_match(atom: &Atom, line: &str) -> Option<String> {
    let chars: Vec<char> = line.chars().collect();
    let anchored = matches!(atom, Atom::Start) || matches!(atom, Atom::Seq(items) if items.first().is_some_and(|item| matches!(item, Atom::Start)));
    for start in 0..=chars.len() {
        let mut capture = None;
        if let Some(end) = matches_at(atom, &chars, start, &mut capture) {
            return Some(capture.unwrap_or_else(|| chars[start..end].iter().collect()));
        }
        if anchored {
            break;
        }
    }
    None
}

fn matches_at(atom: &Atom, chars: &[char], index: usize, capture: &mut Option<String>) -> Option<usize> {
    match atom {
        Atom::Empty => Some(index),
        Atom::Start => (index == 0).then_some(index),
        Atom::End => (index == chars.len()).then_some(index),
        Atom::Lit(ch) => chars.get(index).filter(|have| *have == ch).map(|_| index + 1),
        Atom::Any => (index < chars.len()).then_some(index + 1),
        Atom::Seq(items) => {
            let mut at = index;
            for item in items {
                at = matches_at(item, chars, at, capture)?;
            }
            Some(at)
        }
        Atom::Alt(items) => items.iter().find_map(|item| matches_at(item, chars, index, capture)),
        Atom::Capture(inner) => {
            let end = matches_at(inner, chars, index, capture)?;
            *capture = Some(chars[index..end].iter().collect());
            Some(end)
        }
        Atom::Repeat { inner, min, open } => {
            let mut at = index;
            let mut hits = 0u8;
            while hits < *min {
                at = matches_at(inner, chars, at, capture)?;
                hits += 1;
            }
            if *open {
                let mut furthest = at;
                loop {
                    match matches_at(inner, chars, furthest, capture) {
                        Some(next) if next > furthest => furthest = next,
                        _ => break,
                    }
                }
                Some(furthest)
            } else if let Some(next) = matches_at(inner, chars, at, capture) {
                Some(next)
            } else {
                Some(at)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_test_line_names_the_last_failure_and_a_pass_line_clears_it() {
        let config = ScriptConfig { test_line: Some("^Testing .+$".into()), pass_line: Some("All done".into()) };
        let ScriptHit::Named(failed) = report("Testing one\nTesting two\n", "", Some(1), &config).unwrap() else { panic!("named") };
        assert_eq!(failed, ["Testing two"]);
        let ScriptHit::Named(passed) = report("Testing one\nAll done\n", "", Some(1), &config).unwrap() else { panic!("named") };
        assert!(passed.is_empty());
        assert!(matches!(report("no tests\n", "", Some(0), &config), Some(ScriptHit::Silent)));
        assert!(compile("( [").is_err());
    }

    #[test]
    fn tap_names_a_failure_and_skips_a_bare_ok() {
        let config = ScriptConfig { test_line: None, pass_line: None };
        assert!(report("ok\n", "", Some(0), &config).is_none());
        let ScriptHit::Named(failed) = report("ok 1 - stays shut\nnot ok 2 - opens\n", "", Some(1), &config).unwrap() else { panic!("named") };
        assert_eq!(failed, ["opens"]);
    }
}
