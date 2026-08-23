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

With exactly (deterministic: the refinement analysis below justifies the selection; no implementation-time discretion):
```toml
description = "The Spatial Terminal Desktop Environment for Remote Workspaces: floating windows, BSP tiling, persistent sessions, and zero-prefix input passthrough over SSH."
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

1. `# term-wm` + badge rows: **unchanged**
2. New headline block:
   - `**The Spatial Terminal Desktop Environment for Remote Workspaces.**`
   - Subtitle: `*The Graphical Desktop for SSH.*`
   - Rewritten one-liner replacing line 7 (multiplexer framing → spatial windows, zero-prefix input, persistent multi-viewer workspaces, headless over SSH)
3. **Image `<div>` blocks — byte-for-byte unchanged**, plus an HTML comment placeholder beneath: `<!-- MEDIA-SWAP: replace static PNGs with the 4 launch demo GIFs (shot-list: docs/launch/launch-checklist.md; scenario 4 = directory workspace naming + palette totals) -->`
4. **NEW `## Why term-wm?`** — feature-comparison matrix: term-wm vs tmux vs Zellij vs WezTerm; rows: execution target (headless SSH), layout compositor (hybrid BSP + floating w/ z-order shadows), input routing (auto Direct Input vs prefix chords/modals), session persistence (embedded gateway daemon), multi-viewer collaboration (attributed muxio IPC, evict viewer), mobile adaptivity (Monocle + FAB dodging). Keep honest tone (see claims guardrail below).
5. **NEW `## Feature Highlights`** — condensed versions of the 7 fine-tuned pillars from `README--DRAFT.md:1-38` (spatial compositing over SSH, zero-setup persistence, attributed multiplayer SSH, unified window topology incl. Monocle, context-aware tasks.json, autonomous input routing), plus TWO post-draft pillars pending marketing sign-off in **Step 2a**: directory-based workspace naming, and the cross-workspace windows/tasks overview
6. `## Usage` / Quick Start / keybindings — **keep** (light copyedit only; it's strong end-user content)
7. `## System Requirements & Compatibility` — **keep**; **fix broken link** line 103: `docs/COMPATIBILITY.md` → `docs/compatibility.md` (case mismatch)
8. `## Workspaces & Session Persistence` (+ env vars table), `## No-Conflict Philosophy`, `## Automatic Direct Input Mode`, `## Window Snapping with Preview` — **keep**, reorder ahead of architecture
9. `## Architecture & Core Capabilities` — **condense to a short paragraph + link** to `docs/DEVELOPMENT.md`; move crate table + Window Lifecycle / Tiling Core / Async Threading Model (ASCII diagram) / Draw Pipeline / Testability / Code Coverage subsections out
10. `## Project Origins & Developer API` + `## Declarative Component Trees with view!` — **move** to `docs/DEVELOPMENT.md`; leave a one-paragraph teaser + link in README
11. `## License` — unchanged; link-reference definitions (lines 262–277) — unchanged

⚠️ **Doctest constraint:** README is compiled into lib docs (`src/lib.rs:1` `#![doc = include_str!]`). Any ```` ```rust ```` fence left in README becomes a real doctest: either keep fences annotated `no_run`/`ignore`, or remove them when moving the `view!` example out. (Fences in `docs/DEVELOPMENT.md` are *not* compiled, since only README content is included via `#![doc]`; annotate rust fences there anyway as hygiene.)

**Link style constraint (rustdoc):** the current README already contains relative `.md` links (`CHANGELOG.md`, `./docs/COMPATIBILITY.md`, `./AGENTS.md`, `./Makefile`) and passes CI's `RUSTDOCFLAGS="-D warnings" cargo doc` today: plain file-path markdown links are not intra-doc links and don't trip rustdoc. Reuse this exact proven style for new links (`docs/DEVELOPMENT.md`, `docs/launch/*`). **Enforced gate:** immediately after every README edit, run `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items`; if ANY link emits an `unresolved_link` warning, that link must be replaced with an absolute GitHub URL in the same pass (no "fix later"). This gate is mandatory, not best-effort; Verification step 2 re-runs it as final proof.

### Claims guardrail
`README--DRAFT.md:40` marks the "custom ANSI parser" claim *partially true*. Word it accurately: PtyStateTracker performs VT100 state tracking built on the forked `term-wm-vt100` parser (real: `crates/term-wm-pty-engine/src/pty_state_tracker.rs`). Don't claim a from-scratch parser.

---

## Step 2a — Post-draft feature copy (pending marketing sign-off)

Two shipped features were missing from the draft pillars. Copy below is written for README Feature Highlights and is reused (condensed) across launch assets. All claims verified against current code; caveats noted so marketing does not over-promise.

### Pillar 8: Your project is the workspace

> **Directory-based workspace naming.** Launch term-wm from a project folder and it takes that folder's name: the menu, floating action button, and a new workspace all adopt it automatically. Start tasks inside, close the app when you're done, and they keep running on the background gateway daemon. Launch from anywhere else later (including over SSH), pick that workspace from the Command Palette, and everything is exactly where you left it.

Alternate one-liners for marketing to choose from:
- "The folder you `cd` into becomes your workspace."
- "Every project gets a workspace that names itself."

Accuracy caveats (do not drop from sign-off review):
- The label adoption covers the menu button and FAB (#284 dynamic branding); the bottom panel still shows the package identity.
- Workspace creation happens on first attach to `<folder-name>/main`; invalid characters in folder names are sanitized (falls back to `default`).
- "They keep running" means tasks survive viewer/app exit because sessions live in daemon-hosted PTYs. It does NOT mean surviving an explicit gateway stop (`--stop-daemon`) or machine reboot. Copy must not imply daemon immortality.

### Pillar 9: A glanceable fleet view

> **Windows and tasks across workspaces.** The Command Palette lists every workspace with live counts of open windows and running tasks, so you always know where work is active before you switch. Stopping the gateway warns first, with totals for every session it would take down.

Accuracy caveats:
- Counts come from each running instance reporting to the gateway; a workspace with no running instance shows no counts line (unknown, not zero).
- Counts update on change and refresh with the palette; they are near-real-time, not instantaneous.
- "Running tasks" excludes tasks whose process exited, even though their window stays open.

Placement map (executed in Steps 2 and 4):
- README Feature Highlights: full pillar text for both.
- `show-hn.md`: pillar 8 woven into the narrative as the "why workspaces beat raw tmux sessions" beat; pillar 9 as the collaboration/fleet sentence.
- `reddit-rust.md`: both as feature bullets beside GIF slots.
- `reddit-commandline.md`: pain-point lead reworked to open with `cd my-project && term-wm` naming behavior.
- `product-hunt.md`: Maker Comment gains a "name it after your project" line; gallery list adds the totals screenshot/GIF.
- `launch-checklist.md`: GIF scenario 4 added (see Step 4 notes).

---

## Step 3 — New `docs/DEVELOPMENT.md`

Migrated content, structured as:
1. **Project Origins & Library API Status** (from README §Project Origins, unsolidified-API caveat preserved)
2. **Workspace Architecture**: crate responsibility table + Window Lifecycle, Tiling Core, Async Threading Model diagram, Draw Pipeline, Testability, Code Coverage (Makefile commands, prerequisites) verbatim from README
3. **Declarative `view!` Macro**: full section incl. the `rust,no_run` example + `examples/view_macro_prototype.rs` pointer
4. **Component Design Standards**: pointer to `AGENTS.md`
5. **Further Reading**: links to `docs/ui-style.md`, `docs/tasks.md`, `docs/profiling.md`, `docs/bench.md`, `docs/compatibility.md`

Feature-defaults note (from Step 2a): DEVELOPMENT.md states that workspaces, the gateway daemon, and tasks.json discovery are compiled-in defaults and only absent under custom `--no-default-features` builds.

---

## Step 4 — Launch assets: new `docs/launch/` directory

| File | Content source |
|---|---|
| `show-hn.md` | Headline `"Show HN: term-wm - A zero-prefix spatial desktop compositor for SSH and terminals written in Rust"` + first-person technical narrative (draft §1 + pasted strategy copy: PtyStateTracker CSI interception, ~9 MB gateway daemon, muxio attributed IPC, mobile Monocle/FAB); library-export disclosure |
| `reddit-rust.md` | Headline `term-wm 0.x: A zero-cost terminal desktop compositor and persistent session daemon built with Ratatui`; visual-first body with GIF embed slots; conversational tone |
| `reddit-commandline.md` | Headline `Ditch prefix chords: term-wm brings floating windows, auto-passthrough, and collaborative SSH to your shell`; pain-point-led body with GIF embed slots |
| `product-hunt.md` | Tagline, gallery asset list, Maker Comment draft (homage to tmux/screen → new TDE category), engagement playbook (15-min responses) |
| `launch-checklist.md` | Pre-launch: crates.io metadata live on next publish; GitHub topics list (above); **GIF shot-list** — 4 scenarios w/ timelines from draft §2 plus Step 2a (ghost-snap drag, Direct Input transition toast, Monocle/FAB dodge, directory workspace naming + palette totals); PH timing (12:01 AM PST Tue–Thu, avoid conference weeks, velocity seeding); post-launch follow-ups |

Each Reddit/HN file gets explicit `<!-- INSERT GIF: scenario-N -->` markers tied to the shot-list.

**Install-command accuracy (verified):** `Cargo.toml:148-149` shows `default = ["session-persistence", "project-tasks", "pty"]`: the gateway daemon, workspaces, and `.term-wm/tasks.json` discovery advertised in all launch copy ship in a plain `cargo install term-wm`. No `--features`/`--all-features` flags needed. Add a checklist item in `launch-checklist.md`: *re-verify `[features] default` includes these before publishing copy* (guards against future default drift).

**Feature-gating qualification (required in every launch file):** persistence, directory-based workspace auto-naming, cross-workspace counts, and tasks.json support all depend on the default features (`session-persistence`, `project-tasks`). Each of `show-hn.md`, `reddit-rust.md`, `reddit-commandline.md`, and `product-hunt.md` must carry one unobtrusive qualifier line, e.g.: *"Workspaces, persistent sessions, and project tasks ship enabled by default (`cargo install term-wm`); custom builds using `--no-default-features` exclude them."* README carries the same sentence as a footnote at the end of Feature Highlights. Launch copy must never present these as unconditional in feature-matrix comparisons against tools where they are core.

**Feature-copy integration (from Step 2a; applied when drafting each file):**
- `show-hn.md`: weave pillar 8 into the narrative where session persistence is introduced ("the workspace names itself after the folder you launched from, and it survives closing the app"); close the fleet-view sentence with pillar 9.
- `reddit-rust.md`: two feature bullets with `<!-- INSERT GIF: scenario-4 -->` beside them (workspace rename-on-launch + palette totals).
- `reddit-commandline.md`: open with the pain point "your tmux sessions are all called '0'" then land `cd my-project && term-wm` self-naming; totals line as the collaboration hook.
- `product-hunt.md`: Maker Comment line: name-it-after-your-project; gallery asset list gains a palette-totals capture.
- `launch-checklist.md`: add GIF **scenario 4**: launch from `~/projects/term-wm` showing menu/FAB adopting the folder name, palette totals lines visible, switch to a second workspace and back. Add sign-off checklist items: (a) confirm persistence wording keeps the daemon caveat, (b) confirm screenshots use folders whose names survive sanitization unchanged.

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
1. `cargo sort --workspace` (write mode, then diff) followed by `cargo sort --workspace --check` (CI enforces manifest formatting)
2. `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items` (README is embedded crate docs; this is the enforced link/fence gate from Step 2: any `unresolved_link` on a new relative link means replace that link with an absolute GitHub URL and re-run until clean)
3. If any ```` ```rust ```` fence remains in README: `cargo test --doc -p term-wm`
4. Feature-default audit: `cargo metadata --no-deps --format-version 1 | grep -A2 '"default"'` (or inspect `Cargo.toml [features]`) to confirm `session-persistence`, `project-tasks`, `pty` remain in `default`, keeping both the plain `cargo install term-wm` claim and the feature-gating qualifier lines accurate
5. Punctuation gate: `grep -nP "[\x{2013}\x{2014}]" Cargo.toml README.md docs/DEVELOPMENT.md docs/launch/*.md` must return zero matches (AGENTS.md bans em/en dashes in repo artifacts)
6. `git diff README.md`: visually confirm lines 3-5 badges and lines 9-16 image divs are byte-identical
7. Sanity-render: open README.md preview (GitHub or editor); check matrix/table syntax, that relative links resolve on the filesystem (`docs/compatibility.md`, `docs/DEVELOPMENT.md`, `docs/launch/*`), and that each launch file carries the feature-gating qualifier line
8. No code changes, so clippy/test are expected green; per AGENTS.md run `cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo test` once at the end if time permits

## Explicitly out of scope
Per-crate descriptions, GitHub repo settings/topics UI, GIF production, binary distribution pipelines, community files (CONTRIBUTING etc.), package rename.
