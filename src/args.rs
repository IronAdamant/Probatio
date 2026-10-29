use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct Flags {
    pub values: BTreeMap<String, String>,
    pub positionals: Vec<String>,
}

impl Flags {
    pub fn parse(args: &[String]) -> Flags {
        let mut values = BTreeMap::new();
        let mut positionals = Vec::new();
        let mut index = 0;
        while index < args.len() {
            let arg = &args[index];
            if let Some(body) = arg.strip_prefix("--") {
                if let Some((key, value)) = body.split_once('=') {
                    values.insert(key.to_string(), value.to_string());
                } else if args.get(index + 1).is_some_and(|next| !next.starts_with("--")) {
                    values.insert(body.to_string(), args[index + 1].clone());
                    index += 1;
                } else {
                    values.insert(body.to_string(), "true".into());
                }
            } else {
                positionals.push(arg.clone());
            }
            index += 1;
        }
        Flags { values, positionals }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    pub fn need(&self, key: &str) -> Result<&str, String> {
        self.get(key).filter(|value| !value.is_empty()).ok_or_else(|| format!("missing --{key}"))
    }

    pub fn int(&self, key: &str, default: u64) -> u64 {
        self.get(key).and_then(|value| value.parse().ok()).unwrap_or(default)
    }
}
