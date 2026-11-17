use std::env;
use std::process;

fn main() {
    let args = env::args().skip(1).collect::<Vec<_>>();
    match slophawk_scalar_foreground::run(&args) {
        Ok((value, exit)) => {
            println!("{}", serde_json::to_string(&value).expect("JSON result"));
            process::exit(exit);
        }
        Err(reason) => {
            eprintln!("scalar foreground rejected: {}", reason.chars().take(512).collect::<String>());
            process::exit(2);
        }
    }
}
