# term-wm Launch Checklist

Master checklist for the repositioned launch ("The Spatial Terminal Desktop Environment for Remote Workspaces"). Companion copy lives in this directory: [show-hn.md](show-hn.md), [reddit-rust.md](reddit-rust.md), [reddit-commandline.md](reddit-commandline.md), [product-hunt.md](product-hunt.md).

## Phase 0 — Metadata & Repository (pre-launch)

- [ ] **crates.io metadata:** new description + keywords (`terminal`, `tui`, `window-manager`, `ssh`, `workspace`) go live on the next `cargo publish` of the root crate — verify after release on https://crates.io/crates/term-wm
- [ ] **Feature-default audit:** confirm `[features] default` in the root `Cargo.toml` still includes `session-persistence`, `project-tasks`, and `pty` so the plain `cargo install term-wm` command used in all launch copy ships the full feature set
- [ ] **GitHub topics** (repo Settings → Topics — set manually):
  - `terminal-desktop-environment`
  - `spatial-terminal`
  - `ratatui`
  - `ssh-collaboration`
  - `tmux-alternative`
  - `terminal-window-manager`
  - `pty-daemon`
- [ ] **README media swap:** replace the two static PNG screenshot `<div>` blocks near the top of `README.md` with demo GIFs (see shot-list below). The placeholder comment `<!-- MEDIA-SWAP ... -->` marks the spot.
- [ ] Verify all links in README resolve (GitHub render check), especially `docs/compatibility.md` (case-sensitive) and `docs/DEVELOPMENT.md`

## Demo GIF Shot-List (3–5 second loops each)

| Scenario | Beats | Proves |
|---|---|---|
| **1. Spatial drag + ghost snapping** | 0–1s: tiled panes, command palette summons a floating window with lerped shadow → 1–2s: mouse-drags it across the tiling layer, z-order shadow glides over cells → 2–3s: ghost outline appears at screen edge, countdown ticks, window snaps cleanly | Desktop-class compositing inside a character grid over SSH |
| **2. Autonomous Direct Input transition** | 0–1s: typing at shell prompt, status bar shows managed mode → 1–2s: launch `nvim main.rs`; tracker catches alt-screen request → 2–3s: toast/status flips to "Direct Input Mode"; vim modal editing works with zero chords | Zero-prefix input routing; WM never fights your tools |
| **3. Mobile Monocle + FAB dodging** | 0–1s: wide layout shrinks to tablet aspect → 1–2s: auto-collapse to Monocle; touch FAB appears bottom-right → 2–3s: child TUI draws a status bar near the FAB; viewport auto-pads one row so nothing is obscured | Adaptive viewports for iPad/Termux SSH sessions |

Recording notes: truecolor terminal, UTF-8 box-drawing font, clean theme, tight loop points.

## Phase 1 — Community Launch

- [ ] **Show HN:** post per [show-hn.md](show-hn.md); commit to same-day comment coverage; lead replies with architecture depth
- [ ] **r/rust:** post per [reddit-rust.md](reddit-rust.md); GIFs inline; respond to implementation questions
- [ ] **r/commandline:** post per [reddit-commandline.md](reddit-commandline.md); pain-point framing
- [ ] Space HN/Reddit posts apart (same day is fine; stagger by several hours)
- [ ] **Product Hunt:** schedule per [product-hunt.md](product-hunt.md) — 12:01 AM PST Tue/Wed/Thu; avoid conference weeks; Maker Comment ready; velocity-seeding list notified 24h ahead
- [ ] **Influencer/reviewer outreach:** send pre-configured demo scripts highlighting automatic Direct Input Mode to Rust/terminal ecosystem reviewers
- [ ] **Mobile communities:** share scenario-3 material with iPad/Android dev groups (r/ipaddev, Blink Shell Discord, Termux user groups)

## Phase 2 — Post-Launch

- [ ] Monitor threads: reply latency targets (<15 min PH launch day; same-day HN/Reddit)
- [ ] Collect recurring questions → FAQ section in README or docs/
- [ ] Triage bug reports from launch into issues with repro labels
- [ ] After traffic settles: publish benchmark numbers (render throughput, idle footprint) as a follow-up post
- [ ] Retrospective: note which messaging angle (spatial / zero-prefix / persistence / mobile) drove the most engagement for future positioning
