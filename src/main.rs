fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    probatio::process::watch_parent();
    if std::env::var_os(probatio::process::PARENT_ENV).is_some() {
        // A call the MCP server started gets its own kill-on-close job (Windows), so stopping this
        // process for a cancel also ends every process under it, including MSYS ones (Git Bash's
        // `sleep`) that `taskkill /T` does not reach, and nothing holds the mutant tree after.
        probatio::process::contain_descendants();
    }
    match args.first().map(String::as_str) {
        Some("mcp") => std::process::exit(probatio::mcp::serve()),
        Some("--version" | "-V") => std::process::exit(print_version()),
        _ => {}
    }
    let human = probatio::args::Flags::parse(&args).human();
    let envelope = probatio::dispatch(&args);
    let code = probatio::envelope::emit_with(&envelope, human);
    std::process::exit(code);
}

fn print_version() -> i32 {
    let envelope = probatio::envelope::Envelope::new(
        true,
        "version",
        env!("CARGO_PKG_VERSION"),
        "This is the Probatio binary.",
    );
    probatio::envelope::emit(&envelope)
}
