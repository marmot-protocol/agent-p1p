fn main() {
    match pip_control::run_cli(std::env::args().skip(1)) {
        Ok(value) => {
            if let Err(error) = serde_json::to_writer(std::io::stdout().lock(), &value) {
                eprintln!("pip-control failed to write output: {error}");
                std::process::exit(1);
            }
            println!();
        }
        Err(error) => {
            eprintln!("pip-control: {error}");
            std::process::exit(1);
        }
    }
}
