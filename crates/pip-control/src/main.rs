fn main() {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments.first().map(String::as_str) == Some("webhook-serve") {
        if let Err(error) = pip_control::run_webhook_ingress_cli(&arguments[1..]) {
            eprintln!("pip-control webhook ingress: {error}");
            std::process::exit(1);
        }
        return;
    }
    if std::env::var("PIP_GIT_ASKPASS").as_deref() == Ok("1") {
        match pip_control::run_git_askpass(arguments) {
            Ok(value) => println!("{value}"),
            Err(error) => {
                eprintln!("pip-control askpass: {error}");
                std::process::exit(1);
            }
        }
        return;
    }
    match pip_control::run_cli(arguments) {
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
