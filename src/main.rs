fn main() {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if let Err(error) = rcdu::session::run(&arguments) {
        eprintln!("rcdu: {error}");
        std::process::exit(1);
    }
}
