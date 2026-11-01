use ulo::StartupError;

fn main() {
    match ex_11_wiring_report::builder().wire() {
        Ok(_) => println!("wired"),
        Err(StartupError::Wiring(errors)) => {
            eprintln!("{errors}");
            std::process::exit(1);
        }
        Err(other) => panic!("{other}"),
    }
}
