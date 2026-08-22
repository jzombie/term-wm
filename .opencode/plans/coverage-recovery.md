# Coverage Recovery Plan: feat/collaboration-and-quick-actions

## Context

The feature branch has **84.26%** line coverage vs main's **84.49%** -- a **-0.23% gap** (~137 uncovered net-new lines to match main). CI uses `--all-features` and reported a -0.3% drop to 85.095%.

The 7 files responsible for 885 of the net-new missed lines:

| File | Main% | Feat% | Lines+ | Miss+ | Priority |
|---|---|---|---|---|---|
| `src/term_wm_app.rs` | 60.8% | 68.7% | +914 | +226 | HIGH |
| `session_server.rs` | 82.4% | 76.3% | +277 | +182 | MEDIUM (async/IPC) |
| `src/main.rs` | 61.2% | 56.3% | +264 | +157 | HIGH |
| `src/unified_event_source.rs` | 68.0% | 72.7% | +614 | +94 | MEDIUM |
| `runner.rs` | 72.2% | 71.0% | +219 | +92 | MEDIUM |
| `methods.rs` | 74.2% | 62.0% | +96 | +92 | HIGH (quick wins) |
| `command_palette.rs` (ui) | 86.7% | 86.7% | +321 | +42 | LOW |

## Strategy

Work in 4 batches, measuring coverage after each batch. Each batch targets a single file and is independently committable.

---

## Batch 1: `methods.rs` -- RPC roundtrip tests (est. +80-96 covered lines)

**Why first:** Pure data encode/decode -- no mocking, no async, no IPC. Each method needs a ~5-line roundtrip test. The file dropped from 74.2% to 62.0% because 6 new RPC methods were added with zero unit tests.

**File:** `crates/term-session-muxio-service-definitions/src/methods.rs`

**Tests to add (inside existing `#[cfg(test)] mod tests`):**

1. `push_output_round_trips` -- encode/decode `Vec<u8>` passthrough, `()` response
2. `write_input_round_trips` -- encode/decode `(u64, Vec<u8>)` tuple via `WriteInputRequest`
3. `list_users_round_trips` -- encode/decode `ListUsersRequest`/`ListUsersResponse` through wire types (non-trivial structural conversion)
4. `on_user_connected_round_trips` -- encode/decode `UserInfo` with all 8 fields
5. `on_user_disconnected_round_trips` -- encode/decode `usize` via `OnUserDisconnectedRequest`
6. `on_workspace_entered_round_trips` -- encode/decode `String` via `OnWorkspaceEnteredRequest`
7. `attach_round_trips` -- encode/decode `AttachRequest` (now includes ssh_port)
8. `spawn_round_trips` -- encode/decode `SpawnRequest`/`SpawnResponse`
9. `resize_pty_round_trips` -- encode/decode with nested structs
10. `close_session_round_trips` -- encode/decode
11. `write_input_encode_response_is_unit` -- verify `WriteInput::encode_response` returns empty vec
12. `push_output_encode_response_is_unit` -- verify `PushOutput::encode_response` returns empty vec

**Verification:** `cargo test -p term-session-muxio-service-definitions` + `cargo llvm-cov report`

---

## Batch 2: `term_wm_app.rs` -- Thin delegates + accessors (est. +50-80 covered lines)

**Why second:** Many 1-5 line functions that are trivially testable with the existing `TermWmApp::<NoopComponent>::new_custom` constructor.

**File:** `src/term_wm_app.rs`

**Tests to add (inside existing `mod tests`):**

1. `request_quit_sets_flag` -- call `request_quit()`, assert `quit_requested()` returns true
2. `quit_requested_defaults_false` -- assert `quit_requested()` is false on fresh app
3. `set_window_title_delegates` -- call `set_window_title(key, "test")`, assert no panic
4. `engine_returns_mut_reference` -- call `engine()`, verify non-null
5. `draw_renderer_returns_mut_reference` -- call `draw_renderer()`, verify non-null
6. `wm_returns_mut_reference` -- already tested via `wm()` calls but add explicit test
7. `on_panic_shows_debug_log` -- call `on_panic()` on `TermWmApp`, verify debug log window transitions to Mapped. **Note:** `TermWmApp::on_panic` (line 614) only calls `wm.transition_window` + `wm.focus_window_key` -- no global panic hook mutation. Safe for parallel tests.
8. `toggle_debug_window_shows_when_hidden` -- call twice: first shows, second hides
9. `toggle_system_panel_shows_when_hidden` -- same pattern
10. `open_help_overlay_creates_window` -- call `open_help_overlay()`, verify window exists
11. `open_exit_confirm_creates_window` -- call `open_exit_confirm()`, verify window exists
12. `handle_app_event_records_key` -- send a key event, verify no panic
13. `spawn_project_task_success_path` -- register a task with valid command, call `spawn_project_task`, verify returns Ok and window created

**Verification:** `cargo test -p term-wm` + `cargo llvm-cov report`

---

## Batch 3: `runner.rs` -- dispatch_action untested arms (est. +30-50 covered lines)

**Why third:** Medium effort -- needs WM setup but no async/IPC.

**File:** `crates/term-wm-core/src/runner.rs`

**Tests to add (inside existing `mod tests`):**

1. `dispatch_action_request_keyboard_focus_sets_focus` -- create WM with a window, dispatch `RequestKeyboardFocus(id)`, verify `keyboard_focus_id()` changed
2. `dispatch_action_run_project_task_forwards_to_host` -- dispatch `RunProjectTask("task".into())`, verify `handle_custom_action` is called
3. `dispatch_action_help_opens_overlay` -- dispatch `Help`, verify `open_help_overlay()` called
4. `dispatch_action_open_help_opens_overlay` -- dispatch `OpenHelp`, verify same
5. `dispatch_action_paste_clipboard_noop_when_no_clipboard` -- dispatch `PasteClipboard` when WM has no clipboard set (`clipboard_mut()` returns None), verify no `ClipboardPaste` action is queued. **Avoids system clipboard access** which panics on headless CI.
6. `handle_focused_app_event_focus_lost_clears_hover` -- send `FocusLost`, verify hover is cleared

**Verification:** `cargo test -p term-wm-core` + `cargo llvm-cov report`

---

## Batch 4 (optional): `unified_event_source.rs` -- next_mouse (est. +20-30 covered lines)

**File:** `src/unified_event_source.rs`

**Tests to add:**

1. `next_mouse_surfaces_mouse_event_and_owner` -- send a `MouseEvent` through the unified channel, call `next_mouse`, verify it returns the event with correct owner
2. `next_mouse_accumulates_non_mouse_events` -- send key events then a mouse event, verify key events are buffered
3. `set_pending_work_toggle` -- call `set_pending_work(true)`, verify `current_profile` is Streaming

**Verification:** `cargo test -p term-wm` + `cargo llvm-cov report`

---

## Final Verification

After all batches:
```bash
cargo llvm-cov clean --workspace
cargo llvm-cov --workspace --all-features --release --no-report
cargo llvm-cov report --workspace --release
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

**Known issue:** `term-session-server` has a pre-existing test failure on main (`spawn_leaves_channel_var_unset_when_no_channel`). This is an environment-dependent test that may pass on CI. If it blocks local coverage measurement, note it as a pre-existing issue unrelated to this branch.

Target: **>=84.49%** line coverage (match main), ideally **>=85.0%**.

## Risk Notes

- **Pre-existing CI issue:** `term-session-server::session::tests::spawn_leaves_channel_var_unset_when_no_channel` fails on main (expects `TERM_SESSION_CHANNEL=` but gets `TERM_SESSION_CHANNEL=default/main`). This is environment-dependent and unrelated to this branch. If it blocks local coverage measurement, exclude it with a note.
- `session_server.rs` (+182 missed lines) is the second-biggest gap but requires async runtime + IPC -- defer to integration tests or a follow-up PR.
- `main.rs` `run()` function (285 lines) is inherently untestable as a unit test -- it orchestrates the entire app lifecycle. The thin delegates and `handle_custom_action` branches are the testable parts.
- `runner.rs` `run_event_loop()` (618 lines) is the event loop -- testing individual arms requires the full event-loop infrastructure which only integration tests (`panic_debug_log.rs`) exercise.
