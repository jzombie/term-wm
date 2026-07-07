use std::sync::Arc;

use ratatui::layout::Rect;

#[test]
fn default_shell_nonempty() {
    let s = term_wm::default_shell();
    assert!(!s.is_empty());
    let _ = term_wm::default_shell_command();
}

#[test]
fn mouse_capture_flow_through_window_manager() {
    let ctx = Arc::new(term_wm::AppContext::new("test", "0.0.0"));
    let top: Box<dyn term_wm_core::components::WmComponent> = Box::new(
        term_wm_sys_ui_components::WmTopPanelComponent::new(&ctx.app_name),
    );
    let bottom: Box<dyn term_wm_core::components::WmComponent> =
        Box::new(term_wm_sys_ui_components::WmBottomPanelComponent::new(
            &ctx.app_name,
            &ctx.app_version,
            None,
        ));
    let menu: Box<dyn term_wm_core::components::WmComponent> =
        Box::new(term_wm_sys_ui_components::WmMenuOverlay::new());
    let mut wm: term_wm::window::WindowManager = term_wm::window::WindowManager::with_config(
        term_wm::wm_config::WmConfig::standalone(),
        ctx,
        Some(top),
        Some(bottom),
        Some(menu),
    );
    // default starts enabled (from config)
    assert!(wm.mouse_capture_enabled());
    // setting the same value shouldn't mark change
    wm.set_mouse_capture_enabled(true);
    assert!(wm.take_mouse_capture_change().is_none());
    // flip it and observe the change
    wm.set_mouse_capture_enabled(false);
    assert_eq!(wm.take_mouse_capture_change(), Some(false));
    // consumed
    assert!(wm.take_mouse_capture_change().is_none());
}

#[test]
fn top_panel_split_area_basic() {
    let mut p = term_wm_sys_ui_components::WmTopPanelComponent::new("test");
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
fn bottom_panel_split_area_basic() {
    let mut p =
        term_wm_sys_ui_components::WmBottomPanelComponent::new("test", "0.0.0", Some("host"));
    let area = Rect {
        x: 0,
        y: 0,
        width: 12,
        height: 6,
    };
    let (bottom_rect, managed) = p.split_bottom_area(area, 1);
    assert_eq!(bottom_rect.width, area.width);
    assert_eq!(managed.width, area.width);
}

#[test]
fn sanity_list_behavior() {
    let mut list = term_wm::list::ListComponent::new("t");
    list.set_items(vec!["a".into(), "b".into(), "c".into()]);
    assert_eq!(list.items().len(), 3);
    list.move_selection(1);
    assert_eq!(list.selected(), 1);
    list.move_selection(-1);
    assert_eq!(list.selected(), 0);
}
