# Reddit r/commandline — Launch Post

> Platform: r/commandline. Pain-point-led, practical tone. Terminal enthusiasts: lead with what changes about their daily shell workflow, not with implementation detail. GIFs embedded in the post body.

## Headline

```
Ditch prefix chords: term-wm brings floating windows, auto-passthrough, and collaborative SSH to your shell
```

## Post Body

I got tired of two things in every terminal multiplexer I've used:

1. Memorizing prefix chords (`Ctrl+B`, `Ctrl+A`) that collide with my editor and shell
2. Panes locked into a rigid grid that turns into unreadable slivers the moment I attach from a tablet

So I built **term-wm** — a desktop-compositor-style window manager that runs inside any terminal over plain SSH. No display server, no local GUI, one ~9 MB binary.

<!-- INSERT GIF: scenario-1 (spatial drag + ghost snapping) -->

**How it feels different:**

- **Floating windows in your terminal.** Drag them with the mouse, snap them to edges/corners with a ghost preview, watch z-ordered drop shadows glide over the panes underneath. Tiling (BSP) is still there when you want it — they coexist.

- **No prefix chords, ever.** When you launch `vim`, `htop`, or `tmux` inside a window, term-wm notices the app taking over (it watches the PTY stream for alternate-screen/mouse-tracking requests) and automatically hands over raw, zero-delay input. The only key it keeps is a single super key (`Ctrl+A`) for the command palette — and pressing it again forwards `Ctrl+A` to the focused app.

  <!-- INSERT GIF: scenario-2 (autonomous Direct Input transition) -->

- **Sessions that survive everything.** A background gateway daemon auto-spawns on first launch. Kill your SSH connection, close the laptop, restart the terminal — reconnect and your workspaces, layouts, and running processes are exactly where you left them. No detach/attach dance.

- **Actually usable on tablets/phones.** Attach from an iPad (Blink Shell) or Termux and narrow viewports auto-collapse into Monocle mode; a touch button gets out of the way of TUI status bars instead of covering them.

  <!-- INSERT GIF: scenario-3 (mobile Monocle + FAB dodging) -->

- **Share a session without socket gymnastics.** Multiple people can attach to the same workspace over plain SSH. Every event is tagged with who sent it — so you can kick one viewer without nuking the session or everyone else's work.

- **Project tasks built in.** Drop a `.term-wm/tasks.json` in your repo and its commands show up as searchable entries in the command palette, running in their own windows with visible exit markers.

Install:

```sh
cargo install term-wm
```

Repo + screenshots: https://github.com/jzombie/term-wm

Happy to answer questions about how the input passthrough works or how sessions persist across disconnects.
