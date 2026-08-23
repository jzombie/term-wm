# r/commandline post (draft)

## Title

Ditch prefix chords: term-wm brings floating windows, auto-passthrough, and collaborative SSH to your shell

<!-- INSERT GIF: scenario-2 (Direct Input transition toast) -->

## Body

Your tmux sessions are all called "0". Your windows are locked to a grid. And every time you want to scroll, you pray you remembered the right prefix sequence.

term-wm tries something different: it treats your terminal like a desktop.

- **Launch it from a project folder and it becomes that project.** `cd ~/projects/foo && term-wm` gives you a workspace named `foo`, a menu and quick-launch button labeled `foo`, and your `.term-wm/tasks.json` entries one palette search away. Close the app; the tasks keep running in the background daemon. Next time you launch (from anywhere, SSH included), `foo` is right there in the palette.
  <!-- INSERT GIF: scenario-4 (directory naming + palette totals) -->
- **Floating windows with real mouse behavior.** Drag by title bar, snap to edges/corners with dashed ghost previews, maximize by dragging to the top. Or use BSP tiling when you want order.
- **No prefix chords.** The window manager watches each child process's terminal behavior and gets out of the way automatically: vim gets raw keyboard/mouse passthrough the moment it takes the alternate screen; nano keeps native text selection because it never captured the mouse. Scrollback is on PageUp/PageDown like a normal person expects.
- **Sessions survive disconnects.** A small gateway daemon persists your workspaces and running tasks across terminal restarts, network drops, and machine sleeps.
- **See everything at a glance.** The command palette lists every workspace with live counts of open windows and running tasks before you commit to switching.
- **Bring a friend.** Multiple SSH viewers can share a workspace; evicting one doesn't kill anyone's session.

Works in any terminal on Linux, macOS, and Windows. Truecolor recommended.

Everything described ships enabled by default (`cargo install term-wm`); builds using `--no-default-features` exclude workspaces/persistence/tasks.

Repo: https://github.com/jzombie/term-wm
