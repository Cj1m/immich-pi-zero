fn main() {
    if let Err(error) = immich_server::run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}
