fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "mcp") {
        std::process::exit(probatio::mcp::serve());
    }
    let human = probatio::args::Flags::parse(&args).human();
    let envelope = probatio::dispatch(&args);
    let code = probatio::envelope::emit_with(&envelope, human);
    std::process::exit(code);
}
