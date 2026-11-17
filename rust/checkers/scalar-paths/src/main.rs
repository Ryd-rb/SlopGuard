use std::io::{self, Read};

fn main() {
    let mut input = String::new();
    if let Err(error) = io::stdin().read_to_string(&mut input) {
        eprintln!("Scalar fact read failed: {error}");
        std::process::exit(2);
    }
    match slophawk_scalar_paths::parse_wire(&input) {
        Ok(problem) => {
            let result = slophawk_scalar_paths::search(&problem);
            print!("{}", slophawk_scalar_paths::result_to_wire(&result));
        }
        Err(error) => {
            eprintln!("Invalid scalar facts: {error}");
            std::process::exit(2);
        }
    }
}
