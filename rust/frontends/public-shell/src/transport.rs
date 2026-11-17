use crate::protocol::Transport;
use crate::{Result, failure};

/// The only native transport in this source-only pass is the published Windows
/// named pipe, with its current-user security and bounded single-request ACK.
/// The .NET Unix pipe pathname is not specified by SlopHawk's wire contract.
pub struct NativeTransport {
    instance_name: String,
}

impl NativeTransport {
    pub fn new(instance_name: String) -> Self { Self { instance_name } }
}

impl Transport for NativeTransport {
    fn exchange(&self, request: &[u8]) -> Result<Option<Vec<u8>>> {
        if !(request.len() == 1 && request[0] == 5 ||
             request.len() == 3 && request[0] == 6 && request[1] == 1 && (1..=4).contains(&request[2])) {
            return Err(failure("invalid_request", "Only published version-1 daemon commands are valid."));
        }
        #[cfg(windows)]
        { crate::windows_pipe::exchange(&self.instance_name, request) }
        #[cfg(not(windows))]
        {
            let _ = &self.instance_name;
            Err(failure("unsupported_os", "Native current-user named-pipe transport is Windows-only; use a pinned legacy C# CLI."))
        }
    }
}
