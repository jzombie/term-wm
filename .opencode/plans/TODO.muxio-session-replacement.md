Contigent upon https://github.com/jzombie/rust-muxio/issues/64

It could replace the current code — and it's a surprisingly natural fit
Current architecture vs muxio mapping
Current (term-session-server/client)	Muxio equivalent
Custom send_msg/recv_msg wire format (5-byte header)	Frame format (21-byte header) — or skip frames and use RPC directly
SessionServerRequest / SessionServerResponse	RpcMessageType::Call (0) / Response (1)
SessionServerPush (Welcome, RawOutput, Snapshot, SessionExited, TitleChanged)	RpcMessageType::Event (2) — push is a first-class concept
Session ID multiplexed in message payload	stream_id per logical stream — true multiplexing
Background net thread + MPSC channels + 4ms poll	RpcEmit callback (output) + dispatcher.read_bytes() (input) — callback-driven
bitcode serialization	Raw bytes — you can still use bitcode in the payload
What gets eliminated
protocol.rs — the entire file (57 lines of custom frame encoding/decoding) disappears
connection.rs — SessionServerConnection/SessionServerReceiver (~191 lines) becomes a thin wrapper around RpcDispatcher
The hand-coded send_msg/recv_msg with length-prefix framing — muxio's FrameCodec handles this
The background thread + MPSC channel dance — muxio's callback model replaces it
What stays
remote_pane.rs — mostly stays, but send_write() / send_request() calls change to muxio RPC calls
session_server.rs (SessionServer) — session management logic stays, but process_client() / push_session_updates() become muxio handler registrations
Clipboard, input_encoding — all the TUI-specific logic is orthogonal
Key upsides
Push is already a protocol primitive (RpcMessageType::Event) — your Welcome, RawOutput, SessionExited, TitleChanged map cleanly
True per-session multiplexing — each PTY session gets its own stream_id, so output from session A can't block session B
IPC transport (what you'd add in issue #64) — Unix domain sockets are 2-5x faster than localhost TCP for the small-frame workload you have
Same code works locally and remotely — TCP/WS transport for remote, IPC for local, identical RpcDispatcher API
Cancel/error propagation — FrameKind::Cancel maps well to session termination
Challenges to consider
I/O model inversion — current code uses explicit sleep(4ms) polling; muxio is callback-driven. The event loop in lib.rs:run_session() would become a set of handler registrations + an stdin event source. The 8ms frame pacing would need a different approach (maybe an interval timer that triggers render even without network data).
Alpha stability — muxio is 0.10.0-alpha. Breaking API changes are expected.
Frame overhead — muxio uses 21 bytes per frame vs your current 5 bytes. For small writes like keystrokes (key_to_bytes often returns 1-3 bytes), that's significant overhead. Not a problem for PTY output (which is chunky), but worth measuring for the input direction.
RpcDispatch is not Send — the core dispatcher isn't thread-safe, which means you can't just drop it into a background thread like the current SessionServerConnection. You'd need a different threading model — likely Arc<Mutex<RpcDispatcher>> or channel-based dispatch.
Summary
The fit is natural enough that I'd say: yes, this is exactly the right kind of use case for muxio. The IPC transport + event-driven RPC model would replace the custom wire protocol cleanly. The main question is whether it's worth the refactoring effort now vs after muxio stabilizes past alpha.

----

That's a compelling vision. The architecture practically builds itself:
┌─────────────────────────────────────────────┐
│  term-wm (GUI)                              │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐   │
│  │Terminal① │ │Terminal② │ │Terminal③ │   │
│  │  Pane    │ │  Pane    │ │  Pane    │   │
│  └────┬─────┘ └────┬─────┘ └────┬─────┘   │
│       │            │            │          │
│       └──────┬─────┴─────┬──────┘          │
│              │ muxio IPC │                 │
├──────────────┼───────────┼─────────────────┤
│              │           │                 │
│   daemon controller (muxio IPC server)      │
│   ┌──────────┬──────────┬──────────┐       │
│   │Session ① │Session ② │Session ③ │       │
│   │  PTY     │  PTY     │  PTY     │       │
│   └──────────┴──────────┴──────────┘       │
│                                            │
│   Sessions outlive the GUI — reattach on    │
│   restart, manage via WM controls           │
└─────────────────────────────────────────────┘
Key insight: you already have the Pane trait as the abstraction boundary (term-wm-pty-engine/src/pane.rs). TerminalComponent stores Box<dyn Pane> — it doesn't care whether the pane is a local child process (Pty) or a daemon-backed session (RemotePane). So the GUI integration is already wired up.
The missing pieces muxio IPC would unlock:
1. Session persistence — close term-wm, the daemon keeps running. Reopen term-wm, reattach to the same sessions. This is the tmux/Screen model.
2. Session management inside the WM — a WmSessionManager component listing running sessions with attach/detach/kill actions, reusing the existing WmMenuOverlay pattern.
3. Shared sessions across WM instances — multiple term-wm instances could attach to the same daemon and share sessions (useful for pairing/debugging).
4. Graceful detachment — closing a terminal window in the WM doesn't kill the PTY; it just detaches the pane. The session lives on.
term-session-server and term-session-client already prove the concept works. Muxio IPC just replaces the raw TCP transport and gives you per-session multiplexing for free — each session gets its own stream_id, so output from one can't block another, and the daemon doesn't need to interleave messages manually.
