# Launch checklist

Pre-launch tasks for the term-wm rebrand launch. Copy sources live beside this file (`show-hn.md`, `reddit-rust.md`, `reddit-commandline.md`, `product-hunt.md`).

## Before drafting final copy

- [ ] Re-verify `[features] default` in root `Cargo.toml` includes `session-persistence`, `project-tasks`, and `pty`, so the plain `cargo install term-wm` claim stays accurate.
- [ ] Re-verify crates.io metadata: description uses the colon phrasing ("...Remote Workspaces: floating windows, BSP tiling, persistent sessions...") and goes live only on next publish.
- [ ] Run the punctuation gate over every touched file (zero matches allowed):
      `grep -nP "[\x{2013}\x{2014}]" Cargo.toml README.md docs/DEVELOPMENT.md docs/launch/*.md`
- [ ] Run the doc gate after any README edit:
      `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items`
      Replace any link that emits `unresolved_link` with an absolute GitHub URL, then re-run.

## GIF shot-list (4 scenarios)

1. **Ghost-snap drag.** Float a window, drag by title bar across edge/corner/top targets; capture dashed previews, snap label, z-order shadows. Timeline: 8 to 10 seconds.
2. **Direct Input transition toast.** Open vim, show the "(keyboard and mouse) enabled" toast, edit a line, exit; then open nano and drag-select text natively. Timeline: 10 to 12 seconds.
3. **Monocle/FAB dodge.** Resize the host terminal to narrow; show automatic Monocle and the FAB stepping aside for focused content. Timeline: 6 to 8 seconds.
4. **Directory workspace naming + palette totals.** From `~/projects/term-wm`: menu/FAB adopt the folder name on launch; open the palette showing per-workspace `N windows · M running tasks` lines; switch to a second workspace and back; close the app and reopen to show the workspace persisted. Timeline: 12 to 15 seconds.

Each launch file contains matching `<!-- INSERT GIF: scenario-N -->` markers; keep numbering aligned with this list.

## Sign-off checklist (marketing)

- [ ] Persistence wording keeps the daemon caveat everywhere: tasks survive closing the app because sessions run on the background gateway daemon; they do not survive `--stop-daemon` or a reboot.
- [ ] Screenshots/GIFs use project folders whose names survive sanitization unchanged (alphanumeric, hyphen, underscore).
- [ ] Every launch file carries the feature qualifier line: workspaces, persistence, directory naming, cross-workspace counts, and tasks ship enabled by default (`cargo install term-wm`); `--no-default-features` builds exclude them.
- [ ] Comparison matrix claims match README wording exactly (no stronger claims in social copy than on the README).
- [ ] Terminology check against docs/UI-STYLE.md: workspace (not "session") for the named channel concept; gateway/gateway daemon for the daemon; viewer for connected clients.

## Launch mechanics

- [ ] Product Hunt timing: 12:01 AM PST Tuesday through Thursday; avoid major developer-conference weeks.
- [ ] Maker Comment posted within 15 minutes of going live; every comment answered within 15 minutes for the first hour.
- [ ] Show HN submitted with headline exactly as drafted (ASCII hyphen after "term-wm").
- [ ] Post-launch: triage feedback into GitHub issues within 24 hours and reply with issue links.
