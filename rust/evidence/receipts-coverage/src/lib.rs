//! SG-MIG-13: typed, bounded receipt validation and run-level Cobertura attribution.
//!
//! This crate does not discover targets, issue execution grants, run tests, or assert a
//! read-scope lease. `capture` requires independently supplied scope facts and fails
//! closed when the platform cannot establish a stable, unlinked artifact identity.
//! The verification bridge never turns aggregate lines/branches into an exact arm.

pub mod capture;
pub mod cobertura;
pub mod identity;
pub mod input_audit;
pub mod receipt;
pub mod verification;

use sha2::{Digest, Sha256};
use serde::{Deserialize, Serialize};

pub const MAX_COVERAGE_BYTES: usize = 1024 * 1024;
pub const MAX_TRX_BYTES: usize = 2 * 1024 * 1024;

pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:X}", Sha256::digest(bytes))
}

pub fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|v| v.is_ascii_hexdigit())
}

pub fn digest_matches(expected: &str, actual: &str) -> bool {
    // Input digests are never secrets. Still compare without early-exit byte leaks.
    expected.len() == 64 && actual.len() == 64 &&
        expected.bytes().zip(actual.bytes()).fold(0_u8, |diff, (a,b)| {
            diff | (a.to_ascii_uppercase() ^ b.to_ascii_uppercase())
        }) == 0
}

pub fn field(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max.saturating_mul(4) &&
        value.encode_utf16().count() <= max && !value.trim().is_empty() && !value.chars().any(char::is_control)
}

pub fn logical_path(value: &str, max: usize) -> Option<String> {
    let p = value.replace('\\', "/");
    if !field(&p, max) || p.starts_with('/') || p.ends_with('/') || p.contains(':') ||
        p.split('/').any(|s| s.is_empty() || s == "." || s == ".." ||
            s.ends_with(' ') || s.ends_with('.')) { return None; }
    Some(p)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status { Complete, Unknown, Mismatch, Malformed, Expired, Unsupported, Missing }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Problem {
    pub status: Status,
    pub code: String,
}

impl Problem {
    pub fn new(status: Status, code: &'static str) -> Self {
        Self { status, code: code.into() }
    }
}

pub type Check<T> = Result<T, Problem>;
