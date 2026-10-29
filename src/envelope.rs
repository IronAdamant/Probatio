//! Short JSON envelope. Kill lists and full id lists stay off stdout.

use std::io::{self, Write};
use std::path::Path;

use crate::json::{self, Value};

pub const SCHEMA_VERSION: i64 = 2;

#[derive(Clone, Debug)]
pub struct Envelope {
    pub ok: bool,
    pub command: String,
    pub summary: String,
    pub next: String,
    pub next_call: Option<Vec<String>>,
    pub extra: Vec<(String, Value)>,
}

impl Envelope {
    pub fn new(ok: bool, command: &str, summary: impl Into<String>, next: impl Into<String>) -> Envelope {
        Envelope {
            ok,
            command: command.to_string(),
            summary: summary.into(),
            next: next.into(),
            next_call: None,
            extra: Vec::new(),
        }
    }

    pub fn with_extra(mut self, key: &str, value: Value) -> Envelope {
        self.extra.push((key.to_string(), value));
        self
    }

    pub fn to_value(&self, home: &str) -> Value {
        let mut entries = vec![
            ("schemaVersion".to_string(), Value::Int(SCHEMA_VERSION)),
            ("ok".to_string(), Value::Bool(self.ok)),
            ("command".to_string(), Value::String(self.command.clone())),
            ("summary".to_string(), Value::String(scrub(&self.summary, home))),
            ("next".to_string(), Value::String(scrub(&self.next, home))),
            (
                "nextCall".to_string(),
                match &self.next_call {
                    None => Value::Null,
                    Some(argv) => Value::object(vec![(
                        "argv",
                        Value::Array(argv.iter().cloned().map(Value::String).collect()),
                    )]),
                },
            ),
        ];
        for (key, value) in &self.extra {
            entries.push((key.clone(), scrub_value(value, home, key == "nextCall")));
        }
        Value::Object(entries)
    }

    pub fn render(&self, home: &str) -> String {
        json::stringify(&self.to_value(home))
    }
}

pub fn scrub(text: &str, home: &str) -> String {
    if home.is_empty() || !text.contains(home) {
        return text.to_string();
    }
    text.replace(home, "~")
}

fn scrub_value(value: &Value, home: &str, keep: bool) -> Value {
    if keep {
        return value.clone();
    }
    match value {
        Value::String(text) => Value::String(scrub(text, home)),
        Value::Array(items) => Value::Array(items.iter().map(|item| scrub_value(item, home, false)).collect()),
        Value::Object(entries) => Value::Object(
            entries
                .iter()
                .map(|(key, item)| {
                    let keep_child = key == "nextCall";
                    (key.clone(), scrub_value(item, home, keep_child))
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Write every byte, then flush. Callers exit only after this returns.
pub fn write_all_flush(mut writer: impl Write, bytes: &[u8]) -> io::Result<()> {
    writer.write_all(bytes)?;
    writer.flush()
}

pub fn emit(envelope: &Envelope) -> i32 {
    let home = std::env::var("HOME").unwrap_or_default();
    let rendered = envelope.render(&home);
    let mut stdout = io::stdout().lock();
    if write_all_flush(&mut stdout, rendered.as_bytes()).is_err() {
        return 1;
    }
    drop(stdout);
    if envelope.ok { 0 } else { 1 }
}

pub fn read_text(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Read;
    use std::os::unix::io::FromRawFd;
    use std::thread;

    unsafe extern "C" {
        fn pipe(fds: *mut i32) -> i32;
    }

    #[test]
    fn large_envelope_survives_a_pipe() {
        let mut envelope = Envelope::new(true, "schema", "short", "x".repeat(200_000));
        envelope.extra.push(("finished".into(), Value::Int(1)));
        let bytes = envelope.render("");
        let mut fds = [0i32; 2];
        assert_eq!(unsafe { pipe(fds.as_mut_ptr()) }, 0);
        let mut reader = unsafe { File::from_raw_fd(fds[0]) };
        let mut writer = unsafe { File::from_raw_fd(fds[1]) };
        let handle = thread::spawn(move || {
            let mut got = String::new();
            reader.read_to_string(&mut got).unwrap();
            got
        });
        write_all_flush(&mut writer, bytes.as_bytes()).unwrap();
        drop(writer);
        let got = handle.join().unwrap();
        let value = json::parse(&got).unwrap();
        let (version, ok, summary, next, next_call) = json::envelope_fields(&value).unwrap();
        assert_eq!(version, 2);
        assert!(ok);
        assert_eq!(summary, "short");
        assert_eq!(next.len(), 200_000);
        assert_eq!(next_call, Value::Null);
        assert!(value.get("notStarted").is_none());
        assert!(value.get("kills").is_none());
    }

    #[test]
    fn home_is_scrubbed_outside_next_call() {
        let mut envelope = Envelope::new(false, "mutate.run", "see /Users/ada/proj", "next");
        envelope.next_call = Some(vec!["mutate".into(), "run".into(), "--package".into(), "/Users/ada/proj".into()]);
        let rendered = envelope.render("/Users/ada");
        assert!(rendered.contains("~/proj"));
        assert!(rendered.contains("\"/Users/ada/proj\""));
        assert!(!rendered.contains("see /Users/ada/proj"));
    }
}
