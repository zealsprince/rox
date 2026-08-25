# MCP

rox speaks MCP through `rox-mcp`, a small binary that ships beside the rox
executable: MCP over stdio on one side, the [control socket](README_IPC.md) on the
other. Every tool is a straight proxy of one socket method.

## Turning it on

Two switches, both off by default:

1. **Enable AI Features** at the top of Settings > Application. Reveals the MCP and
   ML Models pages.
2. **Enable MCP Server** on Settings > MCP.

The proxy checks both on every tool call, so a flip applies to the next call without
restarting rox or the client. A switched-off toggle, like a rox that isn't running,
comes back as a tool error naming the reason.

## Pointing a client at it

Settings > MCP holds a copy-ready snippet in the `mcpServers` shape most clients
read:

```json
{
  "mcpServers": {
    "rox": {
      "command": "/path/to/rox-mcp"
    }
  }
}
```

Claude Code takes the same thing as `claude mcp add rox /path/to/rox-mcp`; in Zed
it's a custom context server with that command. rox has to be running: the proxy
connects to the socket on the first tool call and reconnects by itself when rox
restarts.

Two flags cover the non-default socket:

| Flag                | Use                                                          |
| ------------------- | ------------------------------------------------------------ |
| `--data-dir <path>` | derive the socket for this data directory (a `--portable` rox) |
| `--socket <path>`   | name the socket outright                                     |

## Tools

| Tool             | Arguments                                                     | Answers                                                              |
| ---------------- | ------------------------------------------------------------- | -------------------------------------------------------------------- |
| `now_playing`    |                                                               | the playing track's tags, where its clock sits, whether audio moves   |
| `transport`      | `action`: `toggle` `play` `pause` `next` `prev` `stop`        | the resulting player state                                           |
| `search_library` | `query`, optional `limit` (1..500)                            | matching tracks with tags; pins like `artist:name` narrow one field   |
| `get_queue`      |                                                               | the play order with each entry's stable id and the one playing        |

Everything a tool can do, the socket can do; the reverse doesn't hold. Queue edits,
seeking, volume, artwork, and the debug scope stay socket-only, reachable through
`roxctl` or any JSON-RPC client.

## Protocol

MCP is JSON-RPC 2.0, one object per line on stdio, the same framing as the socket
itself. The proxy answers `initialize`, `ping`, `tools/list`, and `tools/call`, and
reads and drops notifications. It speaks the `2024-11-05`, `2025-03-26`, and
`2025-06-18` revisions verbatim, agreeing to the client's own dialect and offering
`2025-06-18` when asked for one it doesn't know.
