use term_wm::window::WindowManager;

#[test]
fn new_window_is_focused() {
    // Create a window manager in managed mode.
    let mut wm = WindowManager::<usize, usize>::new_managed(0);

    // Initially there should be no app-level focused window.
    assert_eq!(wm.wm_focus_app(), None);

    // Tile (create) a new app window with id `1` and ensure it succeeds.
    let created = wm.tile_window(1);
    assert!(created, "tile_window should return true for new windows");

    // The new window should now be the focused app window.
    assert_eq!(wm.wm_focus_app(), Some(1));

    // WM-level focus should report the focused app via `wm_focus_app()` above.
}
