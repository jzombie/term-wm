# Plan: Clipboard temp-store + OSC 52 relay hardening (Option A)

Status: **Approved, amended per code review (AMEND verdict)** — implement per this
plan.

## Goal

Harden the temp-file clipboard backing store and OSC 52 relay in
`term-wm-pty-engine`:

1. Test isolation (no test ever touches the real user store).
2. Reliable `set()` ordering (internal paste guaranteed first).
3. 1 MB OSC 52 emission cap, applied via **safe UTF-8 char-boundary
   truncation** (local file cache uncapped).
4. No handle-lifetime coupling to store lifetime (no unlink-on-Drop).

Preserves the tree's deliberate **OSC 52-last** design (`525a3eb`, X11
final-clipboard-owner rationale) and the **temp-store-only-when-headless**
privacy property.

## Amendments from code review

- **No `Drop` unlink.** Removed the proposed `impl Drop` + `wrote_store`.
  A clipboard cache is session-scoped, not handle-scoped; unlinking on handle
  drop would break concurrent/subsequent readers. Cleanup is handled by
  consume-on-read (`get()`), and by OS-level cleanup (`XDG_RUNTIME_DIR` is
  tmpfs, wiped on logout; `/tmp` fallback cleaned by the OS).
- **No atomic path counter in `Clipboard::new()`.** `new()` resolves
  deterministically to `default_temp_path()`. Test isolation is bound
  explicitly per-test via `with_options(test_path, limit)` /
  `with_temp_path`. (No pty/session test feeds OSC 52 through the reader
  loop, so `Clipboard::new()` is never exercised in tests.)
- **Char-boundary truncation, not suppression.** Oversized OSC 52 payloads
  are truncated with `str::floor_char_boundary` (stable, no panic) rather
  than dropped entirely, so the host terminal still receives output up to
  the cap. File cache + arboard receive the full untruncated text.
- **Trailing-edge debounce, not naive skip.** The 100 ms relay debounce in
  `pty.rs` buffers the latest payload and flushes it after the window has
  elapsed (and on EOF), so the final payload of a burst is never dropped.

## Files changed

- `crates/term-wm-pty-engine/src/clipboard.rs`
- `crates/term-wm-pty-engine/src/pty.rs`
- (no Cargo.toml / dependency changes)

## 1. `clipboard.rs` — Option A API

- `pub const DEFAULT_MAX_OSC52_BYTES: usize = 1024 * 1024;` (1 MB).
- Add field `osc52_limit: usize` to `Clipboard`.
- Add `pub fn with_options(cache_path: PathBuf, osc52_limit: usize) -> Self`;
  `with_temp_path(path)` delegates with the default limit; `new()` ->
  `with_options(default_temp_path(), DEFAULT_MAX_OSC52_BYTES)`. Constructor
  param reserved for future config wiring.

## 2. `clipboard.rs` — test isolation

- All clipboard tests bind explicit isolated paths via `with_temp_path` /
  `with_options` + `tempfile::tempdir()`; `clipboard_set_emits_osc52` no
  longer uses `Clipboard::new()`. No test touches the real store.
- `Clipboard::new()` stays deterministic (always `default_temp_path()`).

## 3. `clipboard.rs` — `set()` ordering

Reorder to: **(1) temp store** (if `temp_store_enabled`, uncapped) ->
**(2) arboard** (with `StderrSuppressGuard`) -> **(3) OSC 52 last**,
truncated to `self.osc52_limit` at a char boundary via
`text.floor_char_boundary(limit)` (applied to both stdout and the
`#[cfg(test)]` capture). Full text always goes to file cache + arboard.
Doc comment updated.

## 4. `clipboard.rs` — store lifecycle

No `Drop` impl. `get()` keeps its consume-on-read semantics; the store is
otherwise cleaned by OS-level mechanisms (`XDG_RUNTIME_DIR` tmpfs, OS temp
cleanup). Documented in the module/struct docs.

## 5. `pty.rs` — relay hardening

- Hoist one `Clipboard::new()` above the reader loop (removes per-sequence
  arboard init).
- Add `const OSC52_RELAY_DEBOUNCE: Duration = Duration::from_millis(100);`
  and a `pending_osc52: Option<String>` buffer. On each OSC 52 payload:
  always record the `osc52_text` test hook, then store the latest text into
  `pending_osc52`. Flush `pending_osc52` (via `clipboard.set`) once the
  debounce window has elapsed since the last flush, and flush on EOF before
  breaking the loop. Trailing-edge debounce: rapid bursts coalesce and the
  final payload is never lost.

## Tests (added/changed in `clipboard.rs`)

- `osc52_emission_truncated_over_limit` — small `osc52_limit` via
  `with_options`; oversized text -> `osc52_output` decodes to the truncated
  (<= limit) text, while the temp store retains the full text.
- `osc52_truncation_respects_utf8_boundary` — limit landing mid-multibyte
  char truncates at a valid boundary (no panic, valid UTF-8).
- `clipboard_set_emits_osc52` — moved to an isolated `with_temp_path`.
- Existing 33 clipboard tests and pty tests keep passing (`osc52_text` hook
  ungated; pty loop tests feed no OSC 52).

## Verification

```
cargo test
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

## Notes

- All unix-only calls stay `#[cfg(unix)]`-guarded.
- No new dependencies; no public API removals (`with_temp_path` retained as a
  delegating constructor).
