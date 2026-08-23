# Rebranding Phase 1: Positioning, Metadata & Launch Assets for term-wm

## Goal
Reposition term-wm from "terminal multiplexer" to **"The Spatial Terminal Desktop Environment for Remote Workspaces"** across crate metadata, README, developer docs, and ready-to-use launch copy.

## Standing constraints
- ⚠️ **MEDIA REMINDER (for jeremy):** The two static PNG screenshot `<div>` blocks in `README.md` (lines 9–16, hotlinked from `jzombie/live-assets`) are **NOT to be touched**. You will swap them for demo GIFs yourself later. All other README copy may change around them.
- Root `Cargo.toml` metadata changes only go live on the **next crates.io publish** — nothing to run now, just be aware.
- Per-crate descriptions ("...for term-wm.") are **out of scope**.
- No renaming of the binary/package; categorical language carries the rebrand.

---

## Step 1 — Root `Cargo.toml` metadata (`Cargo.toml:3-4`)

Replace:
```toml
description = "A cross-platform terminal multiplexer, window manager, Ratatui component library, and runtime."
keywords = ["window-manager", "multiplexer", "terminal", "tui", "cross-platform"]
```

With exactly (deterministic — refinement analysis below justifies the selection; no implementation-time discretion):
```toml
description = "The Spatial Terminal Desktop Environment for Remote Workspaces — floating windows, BSP tiling, persistent sessions, and zero-prefix input passthrough over SSH."
keywords = ["terminal", "tui", "window-manager", "ssh", "workspace"]
```

Manifest formatting: preserve the existing key ordering style (`name`, `description`, `keywords`, `categories`, inherited keys). CI runs `cargo sort --workspace --check`; run `cargo sort --workspace` (write mode) after editing and inspect the diff before committing.

### Keywords — refinement analysis (selection is final; retained as justification)

crates.io rules: **max 5**, each ≤20 chars, lowercase ASCII + hyphens. Several strategy-doc phrases are **invalid**: `terminal-desktop-environment` (28 chars), `terminal-window-manager` (23 chars) → those belong in **GitHub topics** only.

| Candidate | Len | Valid | Search vol. | Role |
|---|---|---|---|---|
| `terminal` | 8 | ✅ | very high | must-have volume |
| `tui` | 3 | ✅ | high | must-have (ratatui ecosystem) |
| `window-manager` | 14 | ✅ | med-high | category fit; current holder |
| `ssh` | 3 | ✅ | high | primary GTM use case |
| `workspace` | 9 | ✅ | med | remote-workspace narrative |
| `multiplexer` | 11 | ✅ | med | old positioning → recommend dropping |
| `cross-platform` | 14 | ✅ | med | generic; weak differentiation |
| `desktop-environment` | 19 | ✅ | med | TDE category hook |
| `tmux-alternative` | 16 | ✅ | low-med | high-intent switcher capture |
| `compositor` | 10 | ✅ | low | rebrand-aligned, low traffic |
| `spatial-terminal` | 16 | ✅ | low | invented term, low traffic |
| `pty-daemon` | 10 | ✅ | low | architectural, low traffic |

Candidate sets considered (**set A is mandated** in Step 1 — analysis retained as justification only):
- **A (balanced, SELECTED):** `terminal, tui, window-manager, ssh, workspace`
- **B (category-defining):** `terminal, tui, desktop-environment, ssh, workspace` — rejected: trades proven category-search volume for a hook better carried by GitHub topics + description
- **C (migration capture):** `terminal, tui, window-manager, tmux-alternative, ssh` — rejected: drops `workspace`, which anchors the remote-workspaces narrative central to the rebrand

GitHub topics (set manually in repo settings; documented in launch checklist):
`terminal-desktop-environment`, `spatial-terminal`, `ratatui`, `ssh-collaboration`, `tmux-alternative`, `terminal-window-manager`, `pty-daemon`.

---

## Step 2 — README.md rewrite (`README.md`)

Target outline (end-user value first; dev/library depth moves out):

1. `# term-wm` + badge rows — **unchanged**
2. New headline block:
   - `**The Spatial Terminal Desktop Environment for Remote Workspaces.**`
   - Subtitle: `*The Graphical Desktop for SSH.*`
   - Rewritten one-liner replacing line 7 (multiplexer framing → spatial windows, zero-prefix input, persistent multi-viewer workspaces, headless over SSH)
3. **Image `<div>` blocks — byte-for-byte unchanged**, plus an HTML comment placeholder beneath: `<!-- MEDIA-SWAP: replace static PNGs with the 3 launch demo GIFs (shot-list: docs/launch/launch-checklist.md) -->`
4. **NEW `## Why term-wm?`** — feature-comparison matrix: term-wm vs tmux vs Zellij vs WezTerm; rows: execution target (headless SSH), layout compositor (hybrid BSP + floating w/ z-order shadows), input routing (auto Direct Input vs prefix chords/modals), session persistence (embedded gateway daemon), multi-viewer collaboration (attributed muxio IPC, evict viewer), mobile adaptivity (Monocle + FAB dodging). Keep honest tone — see claims guardrail below.
5. **NEW `## Feature Highlights`** — condensed versions of the 7 fine-tuned pillars from `README--DRAFT.md:1-38` (spatial compositing over SSH, zero-setup persistence, attributed multiplayer SSH, unified window topology incl. Monocle, context-aware tasks.json, autonomous input routing)
6. `## Usage` / Quick Start / keybindings — **keep** (light copyedit only; it's strong end-user content)
7. `## System Requirements & Compatibility` — **keep**; **fix broken link** line 103: `docs/COMPATIBILITY.md` → `docs/compatibility.md` (case mismatch)
8. `## Workspaces & Session Persistence` (+ env vars table), `## No-Conflict Philosophy`, `## Automatic Direct Input Mode`, `## Window Snapping with Preview` — **keep**, reorder ahead of architecture
9. `## Architecture & Core Capabilities` — **condense to a short paragraph + link** to `docs/DEVELOPMENT.md`; move crate table + Window Lifecycle / Tiling Core / Async Threading Model (ASCII diagram) / Draw Pipeline / Testability / Code Coverage subsections out
10. `## Project Origins & Developer API` + `## Declarative Component Trees with view!` — **move** to `docs/DEVELOPMENT.md`; leave a one-paragraph teaser + link in README
11. `## License` — unchanged; link-reference definitions (lines 262–277) — unchanged

⚠️ **Doctest constraint:** README is compiled into lib docs (`src/lib.rs:1` `#![doc = include_str!]`). Any ```` ```rust ```` fence left in README becomes a real doctest — either keep fences annotated `no_run`/`ignore` or remove them when moving the `view!` example out. (Fences in `docs/DEVELOPMENT.md` are *not* compiled — only README content is included via `#![doc]` — but annotate rust fences there anyway as hygiene.)

**Link style constraint (rustdoc):** the current README already contains relative `.md` links (`CHANGELOG.md`, `./docs/COMPATIBILITY.md`, `./AGENTS.md`, `./Makefile`) and passes CI's `RUSTDOCFLAGS="-D warnings" cargo doc` today — plain file-path markdown links are not intra-doc links and don't trip rustdoc. Reuse this exact proven style for new links (`docs/DEVELOPMENT.md`, `docs/launch/*`). If any new link emits a rustdoc warning locally, fall back to absolute GitHub URLs.

### Claims guardrail
`README--DRAFT.md:40` marks the "custom ANSI parser" claim *partially true*. Word it accurately: PtyStateTracker performs VT100 state tracking built on the forked `term-wm-vt100` parser (real: `crates/term-wm-pty-engine/src/pty_state_tracker.rs`). Don't claim a from-scratch parser.

---

## Step 3 — New `docs/DEVELOPMENT.md`

Migrated content, structured as:
1. **Project Origins & Library API Status** (from README §Project Origins — unsolidified-API caveat preserved)
2. **Workspace Architecture** — crate responsibility table + Window Lifecycle, Tiling Core, Async Threading Model diagram, Draw Pipeline, Testability, Code Coverage (Makefile commands, prerequisites) verbatim from README
3. **Declarative `view!` Macro** — full section incl. the `rust,no_run` example + `examples/view_macro_prototype.rs` pointer
4. **Component Design Standards** — pointer to `AGENTS.md`
5. **Further Reading** — links to `docs/ui-style.md`, `docs/tasks.md`, `docs/profiling.md`, `docs/bench.md`, `docs/compatibility.md`

---

## Step 4 — Launch assets: new `docs/launch/` directory

| File | Content source |
|---|---|
| `show-hn.md` | Headline `"Show HN: term-wm – A zero-prefix spatial desktop compositor for SSH and terminals written in Rust"` + first-person technical narrative (draft §1 + pasted strategy copy: PtyStateTracker CSI interception, ~9 MB gateway daemon, muxio attributed IPC, mobile Monocle/FAB); library-export disclosure |
| `reddit-rust.md` | Headline `term-wm 0.x: A zero-cost terminal desktop compositor and persistent session daemon built with Ratatui`; visual-first body with GIF embed slots; conversational tone |
| `reddit-commandline.md` | Headline `Ditch prefix chords: term-wm brings floating windows, auto-passthrough, and collaborative SSH to your shell`; pain-point-led body with GIF embed slots |
| `product-hunt.md` | Tagline, gallery asset list, Maker Comment draft (homage to tmux/screen → new TDE category), engagement playbook (15-min responses) |
| `launch-checklist.md` | Pre-launch: crates.io metadata live on next publish; GitHub topics list (above); **GIF shot-list** — 3 scenarios w/ timelines from draft §2 (ghost-snap drag, Direct Input transition toast, Monocle/FAB dodge); PH timing (12:01 AM PST Tue–Thu, avoid conference weeks, velocity seeding); post-launch follow-ups |

Each Reddit/HN file gets explicit `<!-- INSERT GIF: scenario-N -->` markers tied to the shot-list.

**Install-command accuracy (verified):** `Cargo.toml:148-149` shows `default = ["session-persistence", "project-tasks", "pty"]` — the gateway daemon, workspaces, and `.term-wm/tasks.json` discovery advertised in all launch copy ship in a plain `cargo install term-wm`. No `--features`/`--all-features` flags needed. Add a checklist item in `launch-checklist.md`: *re-verify `[features] default` includes these before publishing copy* (guards against future default drift).

---

## Step 5 — Retire `README--DRAFT.md`

Delete after absorbing its content into README + launch files (history preserves it).

**Reference audit (required before deletion):** run a repo-wide search for the path across all tracked and untracked files —
`grep -rn --exclude-dir=.git --exclude-dir=target "README--DRAFT" .`
Any hit in build tooling (`Makefile`), CI (`.github/workflows/*`), scripts, or manifests must be removed in the same commit. **Audit already performed during planning: zero references exist** outside this plan file, so plain deletion is safe as of today; re-run the grep at execution time in case anything landed since.

Its stale companion `.opencode/plans/term-wm-description--draft.md` stays untouched (out of scope).

---

## Files touched
- `Cargo.toml` — description + keywords (edit)
- `README.md` — rewrite per outline (edit)
- `docs/DEVELOPMENT.md` — new
- `docs/launch/{show-hn, reddit-rust, reddit-commandline, product-hunt, launch-checklist}.md` — new
- `README--DRAFT.md` — deleted

## Verification
1. `cargo sort --workspace` (write mode, then diff) followed by `cargo sort --workspace --check` — CI enforces manifest formatting
2. `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items` — README is embedded crate docs; this catches doc-build breakage including any link/fence issues
3. If any ```` ```rust ```` fence remains in README: `cargo test --doc -p term-wm`
4. Feature-default audit: `cargo metadata --no-deps --format-version 1 | grep -A2 '"default"'` (or inspect `Cargo.toml [features]`) — confirm `session-persistence`, `project-tasks`, `pty` remain in `default` so launch copy's plain `cargo install term-wm` is accurate
5. `git diff README.md` — visually confirm lines 3–5 badges and lines 9–16 image divs are byte-identical
6. Sanity-render: open README.md preview (GitHub or editor) — check matrix/table syntax and that relative links resolve on the filesystem (`docs/compatibility.md`, `docs/DEVELOPMENT.md`, `docs/launch/*`)
7. No code changes → clippy/test expected green, but per AGENTS.md run `cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo test` once at the end if time permits

## Explicitly out of scope
Per-crate descriptions, GitHub repo settings/topics UI, GIF production, binary distribution pipelines, community files (CONTRIBUTING etc.), package rename.
