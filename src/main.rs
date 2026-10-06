fn main() {
    if let Err(error) = rcdu::run(std::env::args_os().skip(1).collect()) {
        eprintln!("rcdu: {error}");
        std::process::exit(1);
    }
}
