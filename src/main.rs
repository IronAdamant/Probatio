fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    probatio::process::watch_parent();
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
