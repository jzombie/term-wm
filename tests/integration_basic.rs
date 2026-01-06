use ratatui::layout::Rect;

#[test]
fn default_shell_nonempty() {
    let s = term_wm::components::default_shell();
    assert!(!s.is_empty());
    // ensure the command builder can be constructed without panicking
    let _ = term_wm::components::default_shell_command();
}

#[test]
fn app_state_mouse_capture_flow() {
    let mut s = term_wm::state::AppState::new();
    // default starts enabled
    assert!(s.mouse_capture_enabled());
    // setting the same value shouldn't mark change
    s.set_mouse_capture_enabled(true);
    assert!(s.take_mouse_capture_change().is_none());
    // flip it and observe the change
    s.set_mouse_capture_enabled(false);
    assert_eq!(s.take_mouse_capture_change(), Some(false));
    // consumed
    assert!(s.take_mouse_capture_change().is_none());
}

#[test]
fn panel_split_area_basic() {
    let mut p: term_wm::panel::Panel<u8> = term_wm::panel::Panel::new();
    let area = Rect {
        x: 0,
        y: 0,
        width: 12,
        height: 6,
    };
    let (panel_rect, managed) = p.split_area(true, area);
    assert_eq!(panel_rect.width, area.width);
    assert_eq!(managed.width, area.width);
}

#[test]
fn sanity_list_behavior() {
    let mut list = term_wm::components::list::ListComponent::new("t");
    list.set_items(vec!["a".into(), "b".into(), "c".into()]);
    assert_eq!(list.items().len(), 3);
    list.move_selection(1);
    assert_eq!(list.selected(), 1);
    list.move_selection(-1);
    assert_eq!(list.selected(), 0);
}
