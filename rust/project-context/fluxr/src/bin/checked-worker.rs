//! Dedicated worker entry point. Build this with pinned Rust 1.99.1 and pass
//! its absolute path and SHA-256 to the supervising coordinator.
use slophawk_fluxr_checked_context::{MAX_JOB_BYTES, execute_wire};
use std::io::{Read, Write};

fn main() {
    let mut request = Vec::new();
    if let Err(error) = std::io::stdin()
        .take((MAX_JOB_BYTES + 1) as u64)
        .read_to_end(&mut request)
    {
        eprintln!("bounded worker input failure: {error}");
        std::process::exit(2);
    }
    match execute_wire(&request) {
        Ok(response) => {
            if let Err(error) = std::io::stdout().write_all(&response) {
                eprintln!("bounded worker output failure: {error}");
                std::process::exit(2);
            }
        }
        Err(error) => {
            eprintln!(
                "checked worker refused: {}",
                error.chars().take(512).collect::<String>()
            );
            std::process::exit(2);
        }
    }
}
