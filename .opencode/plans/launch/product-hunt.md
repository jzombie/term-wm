# Product Hunt launch (draft)

## Tagline

The Graphical Desktop for SSH: floating windows, zero-prefix input, and workspaces that persist.

## Gallery asset list

1. Hero GIF: scenario-4 (directory workspace naming + palette totals). First impression = "it named itself after my folder".
2. Ghost-snap drag (scenario-1) showing dashed preview and z-order shadows.
3. Direct Input transition toast (scenario-2) inside vim, then native selection in nano.
4. Monocle + FAB dodge on a narrow viewport (scenario-3).
5. Static: Why term-wm comparison matrix from the README.

<!-- INSERT GIF per gallery item; shot-list and timelines live in launch-checklist.md -->

## Maker Comment

Hi HN/Product Hunt! I grew up on window managers that let me throw windows around with the mouse, then spent a decade inside tmux treating my terminal like a 1980s spreadsheet.

term-wm is my attempt to give the terminal a real desktop: floating, draggable windows with drop shadows over SSH, BSP tiling when you want order, and automatic passthrough so apps like vim and nano behave exactly like they do locally (no prefix chords to memorize).

The part I am proudest of: launch it from a project folder and it names itself after your project. Tasks from `.term-wm/tasks.json` keep running on a tiny background daemon even after you close the app, and the command palette shows live counts of windows and running tasks across every workspace so you always know where things stand. It is an homage to tmux and GNU screen, which taught us sessions should outlive terminals; term-wm adds the desktop metaphors we actually use to navigate them.

It is one Rust binary, works over plain SSH with no server-side setup, ships these features by default (`cargo install term-wm`; `--no-default-features` builds exclude them), and doubles as an embeddable Ratatui component library if you want to build your own.

Happy to answer anything about the architecture (custom muxio IPC protocol, PTY state tracking, drain-synchronized resizes).

## Engagement playbook

- Launch window: 12:01 AM PST Tuesday through Thursday; avoid major conference weeks.
- First hour: respond to every comment within 15 minutes; pin the Maker Comment.
- Prepare short answers for the predictable questions: tmux comparison (README matrix), security model of the daemon socket (user-scoped namespace), why not just use Zellij (input routing philosophy), Windows support status (CI-tested; ConPTY).
- After day 1: collect feature requests into GitHub issues and reply with links.
