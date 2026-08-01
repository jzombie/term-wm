# term-session

A generic terminal session reproducer: run one PTY in a detached server process and attach as many terminals to it as you like — on the same machine or over SSH.

> _[term-wm](https://crates.io/crates/term-wm) uses `term-session` as the backbone of its terminal session persistence. The crate is not tied to term-wm, though: any terminal program can use it to persist or duplicate a live session._

## What it does

One process runs the server (`--server`). It owns the PTY and the child process that runs inside it. Any number of client processes attach to the server over IPC, receive a live broadcast of the PTY's output, and forward keystrokes, mouse events, and pastes back into the shared session. Because every client streams from the same server, all attached terminals show the same live session — duplicates in real time.

Session duplication extends across computers: a session lives on the machine that hosts the server, so a client that can reach that machine can attach. `ssh` into the host and run `term-session --channel <name>` and you have the same session on a different machine.

## Channels

Sessions are addressed by a channel name (`namespace/name`, e.g. `default/main`). Channels resolve to a platform-local socket (`GenericNamespaced`: `$XDG_RUNTIME_DIR` / `~/.term-wm` on Linux, `/tmp` on macOS, named pipes on Windows), so no filesystem path is ever involved.

The server binds to a channel and detaches from the launching terminal (its own session/process group on Unix, `DETACHED_PROCESS` on Windows), so it keeps running — and keeps the session alive — even after every client disconnects. Reconnecting, or attaching a second terminal, picks up the same session.

## Usage

```sh
term-session                     # attach to default/main (spawning a server if needed)
term-session --channel work/dev  # attach to (or spawn) work/dev
term-session --server --channel work/dev -- vim foo.txt   # start a server running a program
```

In client mode `term-session` first probes the channel for a live server and, if none is running, spawns a detached one automatically (`connect_or_spawn_server`). The program to run and the PTY size are forwarded to the server at spawn time. The channel can also be set via the `TERM_WM_CHANNEL` environment variable.

## Crate layout

- `term-session` — this crate: the CLI, the `auto_spawn` helper, and the shared session entry points.
- `term-session-server` — the detached daemon that hosts one PTY per channel and broadcasts it to every subscriber.
- `term-session-client` — attaches a local terminal to a server session, renders it with a `vt100` parser, and forwards local input back into the shared session.
- `term-session-muxio-service-definitions` — the shared Muxio wire types (`Spawn`, subscribe output, stream input, channel handling).
