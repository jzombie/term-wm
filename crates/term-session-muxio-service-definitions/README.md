# term-session-muxio-service-definitions

Shared wire definitions for [term-session](https://crates.io/crates/term-session) IPC over Muxio.

This crate holds the types both the server and client link against, so they never drift out of sync:

- `Spawn` — client requests a PTY on a channel (and the resulting geometry);
- `OnPtyResized` — server pushes a geometry change to all connected clients;
- `SUBSCRIBE_OUTPUT_METHOD_ID` — streaming channel for PTY output broadcast;
- `STREAM_INPUT_METHOD_ID` — streaming channel for client input back into the PTY;
- `ChannelName` and `probe_ipc_endpoint` — parsing of `namespace/name` channels and probing a channel for a live server.
