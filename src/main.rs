fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let envelope = probatio::dispatch(&args);
    let code = probatio::envelope::emit(&envelope);
    std::process::exit(code);
}
