//! A JSON subset reader and writer for envelopes and out-dir files.
//! Objects keep the order they were built in.

use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    pub fn object(entries: Vec<(&str, Value)>) -> Value {
        Value::Object(entries.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(text) => Some(text),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(value) => Some(*value),
            Value::Float(value) if value.fract() == 0.0 => Some(*value as i64),
            _ => None,
        }
    }
}

pub fn stringify(value: &Value) -> String {
    let mut out = String::new();
    write_pretty(&mut out, value, 0);
    out.push('\n');
    out
}

/// One JSON value with no extra whitespace. MCP writes one of these per line.
pub fn stringify_compact(value: &Value) -> String {
    let mut out = String::new();
    write_compact(&mut out, value);
    out
}

fn write_compact(out: &mut String, value: &Value) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Int(number) => out.push_str(&number.to_string()),
        Value::Float(number) => out.push_str(&format_float(*number)),
        Value::String(text) => push_string(out, text),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_compact(out, item);
            }
            out.push(']');
        }
        Value::Object(entries) => {
            out.push('{');
            for (index, (key, item)) in entries.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                push_string(out, key);
                out.push(':');
                write_compact(out, item);
            }
            out.push('}');
        }
    }
}

fn write_pretty(out: &mut String, value: &Value, indent: usize) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Int(number) => out.push_str(&number.to_string()),
        Value::Float(number) => out.push_str(&format_float(*number)),
        Value::String(text) => push_string(out, text),
        Value::Array(items) if items.is_empty() => out.push_str("[]"),
        Value::Array(items) => {
            out.push_str("[\n");
            for (index, item) in items.iter().enumerate() {
                push_indent(out, indent + 1);
                write_pretty(out, item, indent + 1);
                if index + 1 != items.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            push_indent(out, indent);
            out.push(']');
        }
        Value::Object(entries) if entries.is_empty() => out.push_str("{}"),
        Value::Object(entries) => {
            out.push_str("{\n");
            for (index, (key, item)) in entries.iter().enumerate() {
                push_indent(out, indent + 1);
                push_string(out, key);
                out.push_str(": ");
                write_pretty(out, item, indent + 1);
                if index + 1 != entries.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            push_indent(out, indent);
            out.push('}');
        }
    }
}

fn push_indent(out: &mut String, indent: usize) {
    for _ in 0..indent {
        out.push_str("  ");
    }
}

fn push_string(out: &mut String, text: &str) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out.push('"');
}

fn format_float(number: f64) -> String {
    if number.fract() == 0.0 && number.abs() < 1e15 {
        format!("{number:.1}")
    } else {
        let text = format!("{number}");
        if text.contains('.') || text.contains('e') || text.contains('E') {
            text
        } else {
            format!("{text}.0")
        }
    }
}

pub fn parse(input: &str) -> Result<Value, String> {
    let mut parser = Parser { bytes: input.as_bytes(), index: 0 };
    parser.skip();
    let value = parser.value()?;
    parser.skip();
    if parser.index != parser.bytes.len() {
        return Err("trailing input after the JSON value".into());
    }
    Ok(value)
}

struct Parser<'a> {
    bytes: &'a [u8],
    index: usize,
}

impl<'a> Parser<'a> {
    fn skip(&mut self) {
        while let Some(byte) = self.bytes.get(self.index) {
            if byte.is_ascii_whitespace() {
                self.index += 1;
            } else {
                break;
            }
        }
    }

    fn value(&mut self) -> Result<Value, String> {
        self.skip();
        match self.bytes.get(self.index).copied() {
            Some(b'n') => self.literal("null", Value::Null),
            Some(b't') => self.literal("true", Value::Bool(true)),
            Some(b'f') => self.literal("false", Value::Bool(false)),
            Some(b'"') => Ok(Value::String(self.string()?)),
            Some(b'[') => self.array(),
            Some(b'{') => self.object(),
            Some(b'-') | Some(b'0'..=b'9') => self.number(),
            Some(other) => Err(format!("unexpected byte {other} at {}", self.index)),
            None => Err("unexpected end".into()),
        }
    }

    fn literal(&mut self, text: &str, value: Value) -> Result<Value, String> {
        if self.bytes[self.index..].starts_with(text.as_bytes()) {
            self.index += text.len();
            Ok(value)
        } else {
            Err(format!("expected {text}"))
        }
    }

    fn array(&mut self) -> Result<Value, String> {
        self.index += 1;
        let mut items = Vec::new();
        loop {
            self.skip();
            if self.bytes.get(self.index) == Some(&b']') {
                self.index += 1;
                return Ok(Value::Array(items));
            }
            if !items.is_empty() {
                if self.bytes.get(self.index) != Some(&b',') {
                    return Err("expected comma in array".into());
                }
                self.index += 1;
            }
            items.push(self.value()?);
        }
    }

    fn object(&mut self) -> Result<Value, String> {
        self.index += 1;
        let mut entries = Vec::new();
        loop {
            self.skip();
            if self.bytes.get(self.index) == Some(&b'}') {
                self.index += 1;
                return Ok(Value::Object(entries));
            }
            if !entries.is_empty() {
                if self.bytes.get(self.index) != Some(&b',') {
                    return Err("expected comma in object".into());
                }
                self.index += 1;
                self.skip();
            }
            if self.bytes.get(self.index) != Some(&b'"') {
                return Err("expected object key".into());
            }
            let key = self.string()?;
            self.skip();
            if self.bytes.get(self.index) != Some(&b':') {
                return Err("expected colon".into());
            }
            self.index += 1;
            entries.push((key, self.value()?));
        }
    }

    fn number(&mut self) -> Result<Value, String> {
        let start = self.index;
        if self.bytes.get(self.index) == Some(&b'-') {
            self.index += 1;
        }
        while matches!(self.bytes.get(self.index), Some(b'0'..=b'9')) {
            self.index += 1;
        }
        let mut float = false;
        if self.bytes.get(self.index) == Some(&b'.') {
            float = true;
            self.index += 1;
            while matches!(self.bytes.get(self.index), Some(b'0'..=b'9')) {
                self.index += 1;
            }
        }
        if matches!(self.bytes.get(self.index), Some(b'e' | b'E')) {
            float = true;
            self.index += 1;
            if matches!(self.bytes.get(self.index), Some(b'+' | b'-')) {
                self.index += 1;
            }
            while matches!(self.bytes.get(self.index), Some(b'0'..=b'9')) {
                self.index += 1;
            }
        }
        let text = std::str::from_utf8(&self.bytes[start..self.index]).map_err(|_| "bad number".to_string())?;
        if float {
            text.parse::<f64>().map(Value::Float).map_err(|_| "bad number".into())
        } else {
            text.parse::<i64>().map(Value::Int).map_err(|_| "bad number".into())
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.index += 1;
        let mut raw = Vec::new();
        while let Some(byte) = self.bytes.get(self.index).copied() {
            self.index += 1;
            match byte {
                b'"' => return String::from_utf8(raw).map_err(|_| "bad utf-8".into()),
                b'\\' => {
                    let escaped = self.bytes.get(self.index).copied().ok_or("bad escape")?;
                    self.index += 1;
                    match escaped {
                        b'"' => raw.push(b'"'),
                        b'\\' => raw.push(b'\\'),
                        b'/' => raw.push(b'/'),
                        b'n' => raw.push(b'\n'),
                        b'r' => raw.push(b'\r'),
                        b't' => raw.push(b'\t'),
                        b'u' => {
                            let hex = self.bytes.get(self.index..self.index + 4).ok_or("bad unicode")?;
                            self.index += 4;
                            let text = std::str::from_utf8(hex).map_err(|_| "bad unicode")?;
                            let code = u32::from_str_radix(text, 16).map_err(|_| "bad unicode")?;
                            let ch = char::from_u32(code).ok_or("bad unicode")?;
                            let mut buf = [0u8; 4];
                            raw.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                        }
                        _ => return Err("bad escape".into()),
                    }
                }
                byte => raw.push(byte),
            }
        }
        Err("unterminated string".into())
    }
}

/// Read the five envelope fields. Extra keys are ignored by the caller.
pub fn envelope_fields(value: &Value) -> Result<(i64, bool, String, String, Value), String> {
    let version = value.get("schemaVersion").and_then(Value::as_int).ok_or("missing schemaVersion")?;
    let ok = value.get("ok").and_then(Value::as_bool).ok_or("missing ok")?;
    let summary = value.get("summary").and_then(Value::as_str).ok_or("missing summary")?.to_string();
    let next = value.get("next").and_then(Value::as_str).ok_or("missing next")?.to_string();
    let next_call = value.get("nextCall").cloned().ok_or("missing nextCall")?;
    Ok((version, ok, summary, next, next_call))
}

pub fn object_map(value: &Value) -> BTreeMap<String, Value> {
    match value {
        Value::Object(entries) => entries.iter().cloned().collect(),
        _ => BTreeMap::new(),
    }
}
