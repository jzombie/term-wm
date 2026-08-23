# r/rust post (draft)

## Title

term-wm 0.x: A zero-cost terminal desktop compositor and persistent session daemon built with Ratatui

<!-- INSERT GIF: scenario-1 (ghost-snap drag) -->

## Body

I built a desktop window manager that runs inside your terminal. Not "a tiling multiplexer": an actual compositor for the character grid, with floating windows, drop shadows, and mouse dragging that survives SSH.

**The short tour:**

- Drag floating windows by their title bar; edge and corner targets show dashed ghost previews, and the top edge maximizes.
  <!-- INSERT GIF: scenario-1 -->
- Zero prefix chords. The WM watches each PTY for escape-sequence behavior (`PtyStateTracker` on our forked vt100 parser) and hands the keyboard/mouse to apps like vim automatically, then takes them back when they exit. Nano keeps native selection because it only ever asked for the keyboard.
  <!-- INSERT GIF: scenario-2 (Direct Input transition toast) -->
- Workspaces persist in a small gateway daemon that auto-spawns on first launch. Launch from `~/projects/foo` and the workspace names itself `foo`; close the terminal and your tasks keep running; reattach from anywhere over SSH.
- The Command Palette shows live window/task counts per workspace, so you can see where things are running before you switch.
  <!-- INSERT GIF: scenario-4 (directory naming + palette totals) -->
- Multiple viewers can share one workspace over SSH with per-viewer attribution; a host can evict one viewer without touching anyone else's processes.
- Mobile-ish terminals get automatic Monocle mode and a Floating Action Button that dodges content.
  <!-- INSERT GIF: scenario-3 (Monocle/FAB dodge) -->

**Under the hood:** a multi-crate workspace (core state engine, layout engine, PTY engine with drain-synchronized resizes, crossterm console backend) wired together with a custom muxio RPC/IPC protocol for attributed events between the daemon and attached viewers. It doubles as an embeddable Ratatui component library (`view!` macro included), though that API is explicitly unstable.

Everything ships enabled by default (`cargo install term-wm`); builds using `--no-default-features` exclude workspaces/persistence/tasks.

Repo and docs: https://github.com/jzombie/term-wm

Feedback wanted most on: the Direct Input Mode heuristics (what apps does it get wrong?), the palette UX, and whether the workspace/channel model maps onto how you actually work.
