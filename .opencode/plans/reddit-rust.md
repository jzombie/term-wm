# Reddit r/rust — Launch Post

> Platform: r/rust. Visual-first: GIFs embedded in the post body. Conversational tone; Rust implementation details welcome here (Ratatui, workspace structure, PTY handling). Check subreddit rules on version-numbered release titles before posting.

## Headline

```
term-wm: A terminal desktop compositor and persistent session daemon built with Ratatui
```

## Post Body

Over the past while I've been building **term-wm** — a Terminal Desktop Environment written in Rust on Ratatui. It runs headless in any standard terminal over plain SSH, with no display server.

Most terminal tools force a choice between rigid 2D grid tiling (tmux) or a local GUI app (WezTerm). term-wm brings spatial desktop window management into the character-cell grid instead:

<!-- INSERT GIF: scenario-1 (spatial drag + ghost snapping) -->

**What's in it:**

- **Hybrid layout engine** — BSP/N-ary tree tiling plus a free-floating layer with z-order drop shadows, mouse dragging, and edge-snapping with ghost previews
- **Zero-prefix input** — a `PtyStateTracker` watches each PTY's byte stream (via a forked `vt100` parser); when Neovim or htop requests alt-screen/mouse tracking, term-wm automatically drops into zero-delay Direct Input Mode. Keyboard and mouse are yielded independently, so `nano` keeps native text selection
- **Persistent gateway daemon** — one ~9 MB binary embeds a background session daemon that auto-spawns on first launch; workspaces, layouts, and running PTYs survive SSH drops and restarts
- **Attributed multi-viewer IPC** — events carry per-viewer connection IDs over an RPC pipeline, so you can attach multiple viewers to one workspace and evict a single viewer without killing processes
- **Mobile-friendly** — narrow viewports auto-collapse into Monocle mode; a touch FAB dodges TUI content so status bars stay visible (iPad/Blink Shell, Termux)
- **`.term-wm/tasks.json`** — project tasks discovered automatically, launched as searchable Command Palette entries in dedicated PTY windows with explicit exit markers

<!-- INSERT GIF: scenario-2 (autonomous Direct Input transition) -->

Implementation notes for the curious:

- Pure-Rust workspace of ~20 crates: layout math (`term-wm-layout-engine`), PTY engine with Unix PTY + Windows ConPTY support (`portable-pty`), crossterm backend, component library
- Single-threaded UI loop fed by a `crossbeam-channel` event fan-in; PTY reads and network IPC run off-thread, so rendering never blocks on I/O
- Frame pacer with power-aware throttling — interactive frame rates when active, scaled-down polling at idle
- The layout engine and component library are exported as libraries (API still unsolidified — expect breaking changes)

Try it:

```sh
cargo install term-wm
```

Repo + docs: https://github.com/jzombie/term-wm

Feedback very welcome — especially on the input-tracking model and the compositor.
