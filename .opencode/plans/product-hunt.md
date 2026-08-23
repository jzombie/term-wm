# Product Hunt — Launch Kit

> Platform: Product Hunt. Audience rewards polish, out-of-the-box UX, and the indie-maker story over raw systems detail. Gallery quality matters as much as copy.

## Listing

**Name:** term-wm

**Tagline (≤60 chars):**

```
The Graphical Desktop for SSH
```

**Description:**

term-wm is a Spatial Terminal Desktop Environment. It brings floating windows with z-ordered shadows, tiling, zero-prefix input passthrough, and persistent multiplayer workspaces to any terminal — headless, over plain SSH, on Linux, macOS, and Windows. One ~9 MB Rust binary; no display server required.

**Topics:** #developer-tools #productivity #open-source #terminal #remote-work

## Gallery Asset List

| # | Asset | Source |
|---|---|---|
| 1 | GIF: spatial drag + ghost snapping (hero) | scenario-1 |
| 2 | GIF: autonomous Direct Input transition in vim | scenario-2 |
| 3 | GIF: tablet viewport → Monocle + FAB dodging | scenario-3 |
| 4 | Static: wide desktop screenshot | existing README PNG (Linux) |
| 5 | Static: macOS screenshot | existing README PNG (macOS) |

## Maker Comment (post immediately at launch)

Terminal tools have barely changed their mental model in decades: a rigid grid of panes, controlled through prefix key chords you have to memorize. tmux and screen are legendary — they solved persistence for generations of developers — but they treat the character grid as a flat 2D matrix and your keyboard as something to be intercepted.

I kept wanting the opposite bargain: the spatial freedom my desktop gives me — floating windows I can drag, snap, and stack — projected into the terminal I'm already using over SSH. So I built term-wm.

The part I'm proudest of is what you *don't* notice: there are no mode switches to learn. term-wm watches the byte stream of every app you run, and when Neovim or htop takes over the screen, it silently steps out of the way — raw input flows straight through, no chords required. When you're back at the shell, the full desktop is yours again: command palette, floating windows, workspaces that survive disconnects.

It's one ~9 MB binary written in Rust, free and open source (MIT/Apache-2.0): `cargo install term-wm`

Huge thanks to the maintainers of tmux, GNU screen, and Ratatui — this project stands on their shoulders. I'd love your feedback, feature ideas, and bug reports right here in the comments.

— Jeremy

## Execution Playbook

1. **Timing:** Launch 12:01 AM PST on a Tuesday, Wednesday, or Thursday. Avoid major tech conference weeks (WWDC, re:Invent) when developer attention fragments.
2. **Velocity seeding:** Convert GitHub stargazers/community members into a launch-day notify list beforehand; first-four-hours upvote velocity drives the recommendation engine.
3. **First hour:** Post the Maker Comment immediately. Prepare answers in advance for expected questions (How does passthrough detect apps? How does persistence work? Windows support status?).
4. **Engagement window:** Commit to 12–16 hours of rapid responses; target <15-minute reply latency to keep momentum.
5. **Cross-links:** Reply threads with the repo link, docs (`docs/DEVELOPMENT.md`), and the three demo GIFs; pin the Maker Comment.
6. **Assets:** All gallery items uploaded and ordered per the table above before launch day; verify GIF loop points look clean at 3–5 seconds.
