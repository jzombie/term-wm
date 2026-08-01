# Changelog
All notable changes to this project will be documented in this file.

The format is based on Keep a Changelog and this project adheres to
(or is loosely based on) Semantic Versioning.

## [0.9.1-alpha] - 2026-07-31

### Added

- **Unified window management, Command Palette, and mobile targeting** on top of a refactored layout engine (#123, #109).
- **Channel-based term sessions** with a consolidated `term-session` CLI; the shared input/event model is extracted into the `term-wm-events` crate (#186).
- **Command Palette enhancements:** dynamic titles, SUPER key forwarding, and unicode-safe rendering (#170); menu icons searchable in the palette (#160); separator support, `TypeId` registry, auto-scroll, and group reordering (#162).
- **Spatial outside-click dismissal** for the Command Palette and help overlay (#154).
- **Key monitor applet** in the system panel (#167).
- **Notifications** for Direct Mode and Monocle Mode transitions (#140).
- **`AppRootComponent`** made extensible via a generic parameter (#152).
- **DECCKM state tracking** with conditional SS3 arrow-key encoding (#150).
- **Per-window actions parameterized by `WindowKey`**, with scroll-sync fixes for Direct/alt-screen (#145).
- **Progressive degradation** for keybinding hints in the bottom panel (#177).
- **Hardened event pipeline:** media keys, key-repeat handling, exhaustive matching, and removal of the Esc-key fallback (#139).
- **Debug launch command** and expanded profiling documentation.

### Changed

- **Session transport** rewritten from custom stream framing to RPC-native geometry sync with row-by-row rendering (#173).
- **Keyboard event translation** unified across the codebase (#138).
- **Window internals** encapsulated behind getters/setters (#146).
- **Terminal resize indicator** simplified (#176).
- **Window chrome polish:** header buttons bold with `REVERSED` hover inversion (#161); control-button position adjusted for tiled vs. floating windows (#137); FAB text updated to `[≡]` (#143).
- **Command Palette ordering:** `New Window` moved below `Resume` (#144).
- **Documentation overhaul:** READMEs rewritten, `docs/COMPATIBILITY.md` added, and "Direct Mode" naming standardized (#188).
- **Dependency bumps:** Dependabot rollup (#134) and general bump (#187).

### Removed

- Esc-key fallback from the input event pipeline (#139).
- Custom stream framing in favor of RPC-native session geometry sync (#173).

### Fixed

- Window layout state-erasure, void lifecycle, and insertion issues (#178).
- Tiling auto-unmaximize on focus shift; tile position preservation on unmaximize (#141).
- Floating-rect geometry desync on tiled-to-floating drag (#148).
- Tiling split-handle hover firing while panels, overlays, or floating windows are active (#164).
- Scrollbar thumb drift and bounce via ratatui-matching track math (#165).
- Window titles not truncating with an ellipsis (#175).
- Help overlay arrow-key navigation (#174).
- Minimized windows not restoring to top in float mode (#172).
- Cursor-bounded SU injection on shrink; ScrollView persistent state (#171).
- PTY child exit undetected when no subscriber is attached (#159).
- Scrollbar drag dead-zone from viewport/layout misalignment (#156).
- Monocle chrome rules not respected for floating windows (#155).
- Initial frame not rendered before the event loop; FramePacer clock capture fix (#136).
- Double-fire of the `PtyStatus::Exited` callback (#133).

### Performance

- Coalesced rapid mouse-motion events; server input channel bounded (#180).
- Eliminated idle wakeups and heap allocations in the session client and server (#135).
- FramePacer wired into the render loop with an EventSource redraw flag; hitbox/dirty-state fixes (#132).
- Row-slice BCE iterators and persistent SoA mask buffers for the render path (#125).

### Security

- No security-relevant changes in this release.
