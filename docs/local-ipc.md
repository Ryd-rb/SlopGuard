# Local daemon IPC (SG-07)

This is an internal **current-user** named-pipe protocol, not the public MCP
stdio `initialize` lifecycle. Both peers open the workspace-hashed pipe with
`PipeOptions.Asynchronous | PipeOptions.CurrentUserOnly`. Connections carry one
request and one response. The daemon accepts at most eight connected requests,
with a 750 ms request-read deadline, a five-second operation deadline and a
one-second response acknowledgement deadline. The listener accepts a valid
client while another is reading slowly; a full set of stalled clients is released
by the read deadline. Pending client connects and whole client operations also
have finite deadlines.

## Wire

| Request | Bytes | Behavior |
| --- | --- | --- |
| Legacy status | `01` | Current workspace status |
| Legacy results | `02` | Validated report or `no_current_report` |
| Legacy recheck | `03` | Request an inventory reconciliation |
| Legacy stop | `04` | Acknowledged daemon shutdown |
| Hello | `05` | Advertises `wireVersion: 1`, `reportSchemaVersion: 2`, command capabilities and the preview `list_checkers` capability |
| Versioned command | `06 01 <command>` | Wire version `01` followed by command `01`–`04` |

All responses use the existing little-endian signed 32-bit byte length followed
by a UTF-8 JSON `DaemonReply` (`ok`, `payload`, `error`, optional `code`). A
response must not exceed `ReportFile.MaximumBytes + 4096` bytes. The client
reads the entire response and sends `01` as an ACK before disconnecting, which
retains the required Windows pipe handoff. Invalid command or wire version,
unsupported feature, oversized response and unavailable report produce bounded
errors with codes. The sender enforces the byte budget during serialization,
and the client checks the response length before allocating the payload buffer.

Current CLI clients issue `05`, validate the daemon version, schema and reported
capabilities, then send the versioned command on a separate connection. They do
not infer support from their own binary. A pre-handshake daemon returns an
unknown-command reply to `05`; new CLI/MCP clients translate it to the typed
`upgrade_required` response instructing the user to stop that daemon with the
matching older CLI. They do not stop or replace it automatically. A newer daemon
continues to accept the existing single-byte legacy commands and response/ACK
contract so older CLI clients can use the four existing operations. The legacy
path exposes no new optional features. MCP tool registration reflects only the
capabilities advertised by the daemon at bridge creation; subsequent operations
re-negotiate to catch a replaced daemon.

Local wire version 1 is independent from MCP's public protocol version. MCP
initialization and tool lifecycles remain under the SDK's stdio transport; no
local handshake bytes or logs are written to stdout. No report schema, scheduler
or execution authority changes are made by SG-07.
