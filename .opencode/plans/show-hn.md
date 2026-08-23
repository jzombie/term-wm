# Show HN — Launch Post

> Platform: Hacker News (Show HN). Lead with engineering substance; expect architecture scrutiny. Answer comments same-day; technical depth in replies is the growth mechanism.

## Headline

```
Show HN: term-wm – A zero-prefix spatial desktop compositor for SSH and terminals written in Rust
```

## Post Body

Hi HN — I built term-wm, a Terminal Desktop Environment that runs headless inside any standard terminal, over plain SSH, without a display server: https://github.com/jzombie/term-wm

For years the terminal multiplexer landscape has been bounded by the same design model: rigid split-pane grids governed by modal prefix keys (`Ctrl+B`, `Ctrl+A`) that collide with child applications like Vim or Nano. I wanted the spatial freedom of a graphical desktop — floating windows, drop shadows, snapping, a command palette — but projected through a character grid over SSH. So I built it in Rust on top of Ratatui.

The interesting engineering problems:

**1. Input routing without prefix chords.** The hardest part was eliminating prefix keys without breaking CLI apps. term-wm embeds a `PtyStateTracker` that continuously monitors each PTY byte stream through a VT100 state machine (built on a forked `vt100` parser crate). When a child app requests the alternate screen buffer or enables mouse tracking via CSI escape sequences, term-wm automatically transitions into Direct Input Mode: zero-delay, unbuffered passthrough. Keyboard and mouse are tracked as independent dimensions, so an app on the alternate screen *without* mouse tracking (e.g. nano) still gets native text selection and wheel scrolling.

**2. A hybrid layout compositor on a character-cell grid.** BSP/N-ary tree tiling coexists with a free-floating window layer: mouse dragging, edge/corner snapping with ghost preview outlines and a countdown, and z-ordered drop shadows with depth shading — all rendered into ANSI cells.

**3. Zero-setup persistence.** The single binary (~9 MB) embeds a background session gateway daemon that auto-spawns on first launch. Workspaces, window geometry, and running PTY processes survive SSH drops, terminal restarts, and shell exits — no manual detach/attach choreography.

**4. Multi-viewer collaboration with attribution.** Events route through an RPC pipeline where every keypress and mouse event carries a unique viewer connection ID. Multiple people can attach to the same workspace channel over SSH; a host can evict a single viewer ("Detach Viewer") without terminating the underlying PTYs.

**5. Mobile adaptivity.** Narrow viewports (Blink Shell on iPad, Termux on Android) auto-collapse into Monocle mode; a touch Floating Action Button dynamically pads the viewport when a TUI draws near it, so status bars stay legible.

Honest caveats: the developer-facing library API (layout engine + component library) is exported but unsolidified and subject to breaking changes; visual polish depends on truecolor/Unicode terminal support (degrades gracefully).

Install: `cargo install term-wm` (session persistence and project tasks are compiled in by default).

Repo (screenshots and docs): https://github.com/jzombie/term-wm

Would love feedback on the input-tracking model, the compositor design, and the RPC pipeline — tear the architecture apart.

<!-- INSERT GIF: scenario-1 (spatial drag + ghost snapping) -->
<!-- INSERT GIF: scenario-2 (autonomous Direct Input transition) -->
