//! The MCP Registry listing stays on the crate version.

use std::fs;
use std::path::Path;

use probatio::json;

const REGISTRY_NAME: &str = "io.github.IronAdamant/probatio";
const SCHEMA: &str = "https://static.modelcontextprotocol.io/schemas/2025-12-11/server.schema.json";
const BANNED: &[&str] = &[
    "secure",
    "security",
    "compliant",
    "certified",
    "guaranteed",
    "audit",
    "hack",
];

#[test]
fn server_json_matches_the_npm_package_and_the_crate() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let version = env!("CARGO_PKG_VERSION");
    let server = read_json(&root.join("server.json"));
    assert_eq!(field(&server, "$schema"), SCHEMA);
    assert_eq!(field(&server, "name"), REGISTRY_NAME);
    assert_eq!(field(&server, "version"), version);
    assert_eq!(field(&server, "title"), "Probatio");
    check_description(field(&server, "description"));

    let repository = server.get("repository").expect("repository");
    assert_eq!(
        field(repository, "url"),
        "https://github.com/IronAdamant/Probatio"
    );
    assert_eq!(field(repository, "source"), "github");
    assert_eq!(field(repository, "id"), "1405095626");

    let packages = array(server.get("packages").expect("packages"));
    assert_eq!(packages.len(), 1, "one npm package entry");
    let package = &packages[0];
    assert_eq!(field(package, "registryType"), "npm");
    assert_eq!(
        field(package, "registryBaseUrl"),
        "https://registry.npmjs.org"
    );
    assert_eq!(field(package, "identifier"), "probatio");
    assert_eq!(field(package, "version"), version);
    assert_eq!(field(package, "runtimeHint"), "npx");
    assert_eq!(
        package
            .get("transport")
            .and_then(|value| value.get("type"))
            .and_then(json::Value::as_str),
        Some("stdio")
    );
    assert!(argument_values(package, "runtimeArguments").contains(&"-y"));
    assert!(argument_values(package, "packageArguments").contains(&"mcp"));

    let npm_package = read_json(&root.join("npm/package.json"));
    assert_eq!(field(&npm_package, "name"), "probatio");
    assert_eq!(field(&npm_package, "version"), version);
    assert_eq!(field(&npm_package, "mcpName"), REGISTRY_NAME);
    let optional = npm_package
        .get("optionalDependencies")
        .expect("optionalDependencies");
    for name in [
        "probatio-linux-x64",
        "probatio-darwin-arm64",
        "probatio-darwin-x64",
        "@iron_adamant/probatio-win32-x64",
    ] {
        assert_eq!(field(optional, name), version, "{name}");
    }
    for path in [
        "npm/linux-x64/package.json",
        "npm/darwin-arm64/package.json",
        "npm/darwin-x64/package.json",
        "npm/win32-x64/package.json",
    ] {
        assert_eq!(
            field(&read_json(&root.join(path)), "version"),
            version,
            "{path}"
        );
    }
}

fn check_description(description: &str) {
    assert!(
        !description.is_empty() && description.len() <= 100,
        "{}",
        description.len()
    );
    assert!(description.is_ascii(), "{description}");
    assert!(!description.contains('-'), "{description}");
    let lower = description.to_ascii_lowercase();
    for word in BANNED {
        let hit = lower
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|part| part == *word);
        assert!(!hit, "{word} in {description}");
    }
}

fn read_json(path: &Path) -> json::Value {
    let text =
        fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    json::parse(&text).unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

fn field<'a>(value: &'a json::Value, key: &str) -> &'a str {
    value
        .get(key)
        .and_then(json::Value::as_str)
        .unwrap_or_else(|| panic!("{key} missing"))
}

fn array(value: &json::Value) -> &[json::Value] {
    match value {
        json::Value::Array(items) => items,
        other => panic!("expected array, got {other:?}"),
    }
}

fn argument_values<'a>(package: &'a json::Value, key: &str) -> Vec<&'a str> {
    let Some(json::Value::Array(items)) = package.get(key) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| item.get("value").and_then(json::Value::as_str))
        .collect()
}

/// The docs that state the version say the crate's version, and the changelog has its entry.
#[test]
fn docs_name_the_crate_version() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let version = env!("CARGO_PKG_VERSION");
    for (path, needle) in [
        ("AGENTS.md", format!("The crate version is {version}")),
        ("docs/releasing.md", format!("The crate version is {version}")),
        ("npm/README.md", format!("Version {version} installs")),
        ("MIGRATION.md", format!("Crate {version} on `main`")),
        ("CHANGELOG.md", format!("\n## {version}")),
    ] {
        let text = fs::read_to_string(root.join(path)).unwrap_or_else(|error| panic!("read {path}: {error}"));
        assert!(text.contains(&needle), "{path} does not say {needle:?}");
    }
    let lock = fs::read_to_string(root.join("Cargo.lock")).unwrap();
    assert!(lock.contains(&format!("name = \"probatio\"\nversion = \"{version}\"")) || lock.contains(&format!("name = \"probatio\"\r\nversion = \"{version}\"")));
}
