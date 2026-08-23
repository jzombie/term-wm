# README TODOs

Outstanding items for the rebranded README. Tracked here (rather than as comments in `README.md`) so the published/docs.rs-rendered file stays clean.

## Media Swap

- [ ] Replace the two static PNG screenshot `<div>` blocks near the top of `README.md` with the four launch demo GIFs once recorded.
  - The blocks to replace are the `<div align="center">` image sections directly under the headline (hotlinked from `jzombie/live-assets`, currently Linux v0.9.28-alpha + macOS v0.9.0-alpha).
  - Shot list and recording scenarios: [.opencode/plans/launch/launch-checklist.md](.opencode/plans/launch/launch-checklist.md)
    - Scenario 1: spatial drag + ghost snapping
    - Scenario 2: autonomous Direct Input transition
    - Scenario 3: mobile Monocle + FAB dodging
    - Scenario 4: directory workspace naming + palette totals

## Post-Swap Follow-ups

- [ ] After swapping, update the image captions (`<em>pictured: ...</em>`) to describe each GIF scenario.
- [ ] Re-verify GitHub render of `README.md` (GIF loop points, sizing) and docs.rs rendering (README is embedded via `#![doc = include_str!]` in `src/lib.rs`; HTML divs pass through, but confirm).
