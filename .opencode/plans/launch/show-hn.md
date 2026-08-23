# Show HN post (draft)

## Headline

Show HN: term-wm - A zero-prefix spatial desktop compositor for SSH and terminals written in Rust

<!-- INSERT GIF: scenario-1 (ghost-snap drag) -->

## Body

I spent the last year building term-wm because I wanted my tmux workflow to behave like a desktop window manager: overlapping windows I can drag, snap, and shade; a command palette instead of memorized prefix chords; and sessions that survive my laptop lid closing.

What came out is a single Rust binary that runs entirely inside any terminal over plain SSH:

- **Spatial compositing on the character grid.** Hybrid BSP tiling plus a free-floating layer with z-order drop shadows, ghost-preview edge/corner snapping, and maximize. All rendered with Ratatui, no display server anywhere.
- **Zero-prefix input.** A PTY state tracker (`PtyStateTracker`, built on our forked vt100 parser) watches the child process's escape-sequence behavior. When vim takes the alternate screen or requests mouse tracking, term-wm steps aside into unbuffered key/mouse passthrough automatically. Nano keeps native text selection because it never asked for the mouse.
- **Persistent multi-viewer workspaces.** First launch auto-spawns a ~9 MB gateway daemon over a local IPC socket. Workspaces are named channels (`<name>/main`); launch from a project folder and the workspace names itself after it. Close the terminal app and everything keeps running; come back from another machine and reattach.
- **Attributed multi-viewer sessions.** Every input event carries its viewer's connection ID over a muxio RPC pipeline, so you can share a workspace over SSH and evict one viewer without killing anyone's processes.

The workspace feature is the one I use most: `cd ~/projects/foo && term-wm` gives me a workspace called `foo` with my tasks from `.term-wm/tasks.json` one palette-search away, live counts of windows and running tasks across every other workspace, and a confirmation dialog that tells me exactly what terminates if I stop the daemon.

Honest caveats: this is pre-1.0 software with an intentionally unsolidified embedding API (it doubles as a Ratatui component library), and truecolor makes it much prettier than 256-color.

Repo: https://github.com/jzombie/term-wm

Docs worth reading even if you skip the install: [compatibility notes](https://github.com/jzombie/term-wm/blob/main/docs/compatibility.md) and the [tasks.json spec](https://github.com/jzombie/term-wm/blob/main/docs/tasks.md).

Ask me anything about the compositor, the IPC protocol, or why the gateway daemon exists at all.

---

Qualifier for accuracy: workspaces, persistent sessions, directory-based naming, cross-workspace counts, and tasks.json ship enabled by default (`cargo install term-wm`); custom builds using `--no-default-features` exclude them.

Library disclosure: term-wm is also published as a set of crates (core engine, layout engine, UI components) intended for embedding into other Ratatui apps; the API is explicitly unstable.
