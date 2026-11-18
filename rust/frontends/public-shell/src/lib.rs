//! CLI and MCP projections for the native SlopHawk daemon.

pub mod cli;
pub mod identity;
pub mod mcp;
pub mod protocol;
pub mod registry;

use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShellError {
    pub code: String,
    pub message: String,
}

impl ShellError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self { code: code.into(), message: message.into() }
    }
}

impl fmt::Display for ShellError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ShellError {}

pub type Result<T> = std::result::Result<T, ShellError>;

pub fn failure(code: &'static str, message: impl fmt::Display) -> ShellError {
    ShellError::new(code, message.to_string())
}
