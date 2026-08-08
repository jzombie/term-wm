//! Top status panel — composed of small applets (menu, window strip, status
//! line, tiling indicator) that each own a bounded region of the single row.
//!
//! The parent runs a small layout pass each frame: it reserves the menu on the
//! left and the tiling indicator on the right, then hands the middle region to
//! either the status line or the scrollable, draggable window strip. Because
//! each applet renders and interacts strictly inside its own allocated rect,
//! the strip's `◀`/`▶` overflow indicators are always contained and never
//! overwritten by the menu or the tiling label.

mod menu;
mod status;
mod tiling;
mod window_strip;

use std::collections::BTreeMap;

use term_wm_core::{
    actions::{EventResult, TermWmAction},
    components::{
        Component, ComponentAction, ComponentContext, ComponentQuery, ComponentResponse,
        WmComponent,
    },
    events::{Event, MouseEventKind},
    hitbox_registry::HitboxId,
    layout::rect_contains,
    window::WindowKey,
};
use term_wm_layout_engine::LayoutRect;
use term_wm_ui_components::helpers::{color_to_ratatui, layout_rect_to_clipped_rect};

use menu::MenuButton;
use status::StatusLine;
use tiling::TilingIndicator;
use window_strip::WindowStrip;

/// Single-column gap between the menu button and the center region.
const MENU_GAP: u16 = 1;
/// Horizontal gap (columns) between the center region's right edge (and its
/// `▶` chevron) and the right-aligned tiling indicator.
const TILING_GAP: u16 = 1;

#[derive(Debug)]
pub struct WmTopPanelComponent {
    visible: bool,
    height: u16,
    area: LayoutRect,
    app_name: String,
    // WmComponent render state (pushed via process_action before render)
    active: bool,
    focus_current: Option<WindowKey>,
    display_order: Vec<WindowKey>,
    status_line: Option<String>,
    menu_open: bool,
    window_labels: BTreeMap<WindowKey, String>,
    hitbox_id: HitboxId,

    // Applets (each owns a bounded region of the row).
    menu: MenuButton,
    strip: WindowStrip,
    status: StatusLine,
    tiling: TilingIndicator,
}

impl WmTopPanelComponent {
    pub fn new(app_name: &str) -> Self {
        Self {
            visible: true,
            height: 1,
            area: LayoutRect::default(),
            app_name: app_name.to_string(),
            active: false,
            focus_current: None,
            display_order: Vec::new(),
            status_line: None,
            menu_open: false,
            window_labels: BTreeMap::new(),
            hitbox_id: HitboxId::new(),
            menu: MenuButton::new(app_name),
            strip: WindowStrip::new(),
            status: StatusLine::new(),
            tiling: TilingIndicator::new(),
        }
    }

    pub fn begin_frame(&mut self) {
        self.menu.begin_frame();
        self.strip.begin_frame();
        self.tiling.begin_frame();
    }

    pub fn visible(&self) -> bool {
        self.visible
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn area(&self) -> LayoutRect {
        self.area
    }

    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    pub fn set_height(&mut self, height: u16) {
        self.height = height.max(1);
    }

    pub fn menu_icon_rect(&self) -> Option<LayoutRect> {
        self.menu.rect()
    }

    pub fn menu_icon_contains_point(&self, column: u16, row: u16) -> bool {
        self.menu.contains(column, row)
    }

    pub fn split_area(&mut self, active: bool, area: LayoutRect) -> (LayoutRect, LayoutRect) {
        let top_h = if active {
            self.height.min(area.height)
        } else {
            0
        };
        let panel = LayoutRect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: top_h,
        };
        let managed_height = area.height.saturating_sub(top_h);
        let managed = LayoutRect {
            x: area.x,
            y: area.y.saturating_add(i32::from(top_h)),
            width: area.width,
            height: managed_height,
        };
        self.area = panel;
        (panel, managed)
    }

    /// Hit-test a window entry in the strip (delegates to the strip applet).
    pub fn hit_test_window(&self, column: u16, row: u16) -> Option<WindowKey> {
        self.strip.hit_test_window(column, row)
    }

    /// Layout pass + applet rendering into `self.area`.
    ///
    /// Reserves the menu (left) and tiling label (right), then hands the middle
    /// region to EITHER the status line OR the window strip (mutually
    /// exclusive), so applets never overlap.
    fn render_contents(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        theme: &term_wm_core::theme::Theme,
    ) {
        let area = self.area;
        if area.width == 0 || area.height == 0 {
            return;
        }
        let ratatui_backend = term_wm_ui_components::helpers::downcast_ratatui(backend);
        let ratatui_area = layout_rect_to_clipped_rect(area);
        let bounds = ratatui_area.intersection(ratatui_backend.buffer.area);
        if bounds.width == 0 || bounds.height == 0 {
            return;
        }
        for yy in bounds.y..bounds.y.saturating_add(bounds.height) {
            for xx in bounds.x..bounds.x.saturating_add(bounds.width) {
                if let Some(cell) = ratatui_backend.buffer.cell_mut((xx, yy)) {
                    let mut st = cell.style();
                    st.bg = Some(color_to_ratatui(theme.bottom_panel_bg));
                    st.fg = Some(color_to_ratatui(theme.bottom_panel_fg));
                    cell.set_style(st);
                    cell.set_symbol(" ");
                }
            }
        }
        let y = area.y;
        let max_x = area.x.saturating_add(i32::from(area.width));

        // Menu (left edge).
        let menu_width = self.menu.label_width();
        if area.x.saturating_add(i32::from(menu_width)) <= max_x {
            let menu_slot = LayoutRect {
                x: area.x,
                y,
                width: menu_width,
                height: 1,
            };
            self.menu.render(backend, menu_slot, self.menu_open, theme);
        }

        // Center region: [menu + gap, max_x - tiling_width - TILING_GAP).
        let tiling_width = self.tiling.label_width();
        let strip_start = area
            .x
            .saturating_add(i32::from(menu_width))
            .saturating_add(i32::from(MENU_GAP));
        let strip_end = if tiling_width > 0 {
            max_x.saturating_sub(i32::from(tiling_width + TILING_GAP))
        } else {
            max_x
        };
        let strip_width = strip_end.saturating_sub(strip_start).max(0);
        let strip_rect = LayoutRect {
            x: strip_start,
            y,
            width: strip_width as u16,
            height: 1,
        };

        if let Some(status) = self.status_line.clone() {
            self.status.render(backend, strip_rect, &status, theme);
        } else if let Some(focus) = self.focus_current {
            let display_order = self.display_order.clone();
            self.strip.render(
                backend,
                strip_rect,
                &display_order,
                &self.window_labels,
                focus,
                theme,
            );
        }

        // Tiling indicator (right edge).
        if tiling_width > 0 {
            self.tiling.render(backend, area, theme);
        }
    }
}

impl Component<TermWmAction> for WmTopPanelComponent {
    fn hitbox_id(&self) -> Option<HitboxId> {
        Some(self.hitbox_id)
    }

    fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        ctx: &ComponentContext,
        _registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        let theme = ctx.config().theme;
        if !self.active {
            // Still render the tiling indicator even when inactive so the
            // label is visible and its rect is populated for clicks.
            self.strip.clear_drag_state();
            self.tiling.render(backend, area, &theme);
            return;
        }
        let app_name = ctx.app_name().to_string();
        if app_name != self.app_name {
            self.app_name = app_name.clone();
            self.menu.set_app_name(&app_name);
        }
        self.area = area;
        self.render_contents(backend, &theme);
    }

    fn handle_events(
        &mut self,
        event: &Event,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        let Event::Mouse(mouse) = event else {
            return EventResult::Ignored;
        };
        match mouse.kind {
            MouseEventKind::Press(_) => {
                if self.menu.contains(mouse.column, mouse.row) {
                    return EventResult::Action(TermWmAction::OpenCommandPalette);
                }
                let strip_res = self.strip.handle_press(mouse.column, mouse.row);
                if !strip_res.is_ignored() {
                    return strip_res;
                }
                if self.tiling.contains(mouse.column, mouse.row) {
                    if let Some(action) = self.tiling.action() {
                        return EventResult::Action(action);
                    }
                    return EventResult::Consumed;
                }
                EventResult::Ignored
            }
            // Drag/Release/Scroll are delivered under mouse capture (or to the
            // panel's layer) and must never fall through to the terminal below.
            MouseEventKind::Drag(_) => self.strip.handle_drag(mouse.column),
            MouseEventKind::Release(_) => self.strip.handle_release(),
            MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight => {
                self.strip.handle_scroll(mouse.kind)
            }
            _ => EventResult::Consumed,
        }
    }
}

impl WmComponent for WmTopPanelComponent {
    fn consume_area(&mut self, available: LayoutRect) -> (LayoutRect, LayoutRect) {
        self.split_area(self.active, available)
    }

    fn process_action(&mut self, action: &ComponentAction) {
        match action {
            ComponentAction::ToggleVisibility => {
                self.set_visible(!self.visible);
            }
            ComponentAction::SetHintVisibility(hv) => {
                use term_wm_core::wm_config::HintVisibility;
                match hv {
                    HintVisibility::Always => self.set_visible(true),
                    HintVisibility::Never => self.set_visible(false),
                    HintVisibility::OnDemand => {}
                }
            }
            ComponentAction::SetPanelActive(active) => {
                self.active = *active;
            }
            ComponentAction::SetTopPanelState(state) => {
                self.focus_current = state.focus_current;
                self.display_order = state.display_order.clone();
                self.status_line = state.status_line.clone();
                self.menu_open = state.menu_open;
                self.tiling.set_indicator(state.tiling_indicator.clone());
            }
            ComponentAction::SetWindowLabels(labels) => {
                self.window_labels = labels.clone();
            }
            _ => {}
        }
    }

    fn query(&self, query: &ComponentQuery) -> ComponentResponse {
        match query {
            ComponentQuery::MenuIconRect => ComponentResponse::Rect(self.menu.rect()),
            _ => ComponentResponse::None,
        }
    }

    fn hit_test(&self, x: u16, y: u16) -> bool {
        !self.area.is_empty() && rect_contains(self.area, x, y)
    }

    fn begin_frame(&mut self) {
        self.begin_frame();
    }

    fn visible(&self) -> bool {
        self.visible
    }

    fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }
}

impl Default for WmTopPanelComponent {
    fn default() -> Self {
        Self::new("unknown")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::window_strip::{CHEVRON_GAP, PanelWindowHit, SCROLL_STEP};
    use ratatui::buffer::Buffer;
    use term_wm_core::components::{
        ComponentAction, ComponentQuery, ComponentResponse, WmComponent,
    };
    use term_wm_core::events::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use term_wm_core::theme::NOIR;
    use term_wm_core::wm_config::HintVisibility;

    fn make_backend(w: u16, h: u16) -> term_wm_console::RatatuiBackend {
        let buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, w, h));
        term_wm_console::RatatuiBackend::new_simple(buf, ratatui::layout::Rect::new(0, 0, w, h))
    }

    #[test]
    fn top_panel_basic_methods_and_split_area() {
        let mut p = WmTopPanelComponent::new("test-app");
        assert!(p.visible());
        p.set_visible(false);
        assert!(!p.visible());
        p.set_height(0);
        assert!(p.height() >= 1);
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 10,
            height: 5,
        };
        let (panel_rect, managed) = p.split_area(true, area);
        assert_eq!(panel_rect.width, 10);
        assert_eq!(managed.width, 10);

        assert!(p.hit_test_window(0, 0).is_none());
    }

    #[test]
    fn top_panel_split_area_inactive() {
        let mut p = WmTopPanelComponent::new("test");
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let (panel, managed) = p.split_area(false, area);
        assert_eq!(panel.height, 0);
        assert_eq!(managed, area);
    }

    #[test]
    fn default_is_same_as_new() {
        let p = WmTopPanelComponent::default();
        assert!(p.visible());
        assert_eq!(p.height(), 1);
    }

    #[test]
    fn hitbox_id_returns_some() {
        let p = WmTopPanelComponent::new("test");
        assert!(p.hitbox_id().is_some());
    }

    #[test]
    fn set_height_enforces_minimum() {
        let mut p = WmTopPanelComponent::new("test");
        p.set_height(0);
        assert!(p.height() >= 1);
        p.set_height(5);
        assert_eq!(p.height(), 5);
    }

    #[test]
    fn area_returns_stored_area() {
        let mut p = WmTopPanelComponent::new("test");
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        let _ = p.split_area(true, area);
        assert_eq!(p.area(), area);
    }

    #[test]
    fn menu_icon_contains_point_returns_false_when_no_rect() {
        let p = WmTopPanelComponent::new("test");
        assert!(!p.menu_icon_contains_point(0, 0));
    }

    #[test]
    fn menu_icon_rect_none_initially() {
        let p = WmTopPanelComponent::new("test");
        assert!(p.menu_icon_rect().is_none());
    }

    #[test]
    fn hit_test_window_after_render_with_display_order() {
        let mut p = WmTopPanelComponent::new("test");
        p.active = true;
        let key = WindowKey::default();
        p.focus_current = Some(key);
        p.display_order = vec![key];
        p.window_labels.insert(key, "W".to_string());

        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        let _ = p.split_area(true, area);
        let mut backend = make_backend(80, 24);
        p.render_contents(&mut backend, &NOIR);
        assert!(!p.strip.window_hits.is_empty());
        let hit_rect = p.strip.window_hits[0].rect;
        let hit_key = p.hit_test_window(hit_rect.x as u16 + 1, hit_rect.y as u16);
        assert!(hit_key.is_some());
    }

    #[test]
    fn hit_test_area_returns_true_inside() {
        let mut p = WmTopPanelComponent::new("test");
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        let _ = p.split_area(true, area);
        assert!(p.hit_test(5, 0));
    }

    #[test]
    fn hit_test_area_returns_false_outside() {
        let mut p = WmTopPanelComponent::new("test");
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        let _ = p.split_area(true, area);
        assert!(!p.hit_test(5, 5));
    }

    #[test]
    fn render_when_not_active_does_nothing() {
        let mut p = WmTopPanelComponent::new("test");
        p.active = false;
        let mut backend = make_backend(80, 24);
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        let ctx = ComponentContext::new(true);
        let mut reg = term_wm_core::hitbox_registry::HitboxRegistry::new();
        p.render(&mut backend, area, &ctx, &mut reg);
        // No panic, no-op
    }

    #[test]
    fn render_with_status_line() {
        let mut p = WmTopPanelComponent::new("test");
        p.active = true;
        let key = WindowKey::default();
        p.focus_current = Some(key);
        p.display_order = vec![key];
        p.status_line = Some("Status: OK".to_string());

        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        let _ = p.split_area(true, area);
        let mut backend = make_backend(80, 24);
        p.render_contents(&mut backend, &NOIR);
        // Should render without panic
    }

    #[test]
    fn render_menu_open_style() {
        let mut p = WmTopPanelComponent::new("test");
        p.active = true;
        let key = WindowKey::default();
        p.focus_current = Some(key);
        p.display_order = vec![key];
        p.menu_open = true;

        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        let _ = p.split_area(true, area);
        let mut backend = make_backend(80, 24);
        p.render_contents(&mut backend, &NOIR);
        // Menu rect should be set after render
        assert!(p.menu_icon_rect().is_some());
    }

    #[test]
    fn render_narrow_buffer_truncates_labels() {
        let mut p = WmTopPanelComponent::new("test");
        p.active = true;
        let key = WindowKey::default();
        p.focus_current = Some(key);
        p.display_order = vec![key];
        p.window_labels.insert(
            key,
            "A very long window label that exceeds buffer width".to_string(),
        );

        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 20,
            height: 1,
        };
        let _ = p.split_area(true, area);
        let mut backend = make_backend(20, 1);
        p.render_contents(&mut backend, &NOIR);
    }

    #[test]
    fn process_action_toggle_visibility() {
        let mut p = WmTopPanelComponent::new("test");
        assert!(p.visible());
        p.process_action(&ComponentAction::ToggleVisibility);
        assert!(!p.visible());
        p.process_action(&ComponentAction::ToggleVisibility);
        assert!(p.visible());
    }

    #[test]
    fn process_action_set_hint_visibility_always() {
        let mut p = WmTopPanelComponent::new("test");
        p.set_visible(false);
        p.process_action(&ComponentAction::SetHintVisibility(HintVisibility::Always));
        assert!(p.visible());
    }

    #[test]
    fn process_action_set_hint_visibility_never() {
        let mut p = WmTopPanelComponent::new("test");
        p.process_action(&ComponentAction::SetHintVisibility(HintVisibility::Never));
        assert!(!p.visible());
    }

    #[test]
    fn process_action_set_hint_visibility_on_demand() {
        let mut p = WmTopPanelComponent::new("test");
        p.set_visible(false);
        p.process_action(&ComponentAction::SetHintVisibility(
            HintVisibility::OnDemand,
        ));
        assert!(!p.visible());
    }

    #[test]
    fn process_action_set_panel_active() {
        let mut p = WmTopPanelComponent::new("test");
        p.process_action(&ComponentAction::SetPanelActive(true));
        assert!(p.active);
        p.process_action(&ComponentAction::SetPanelActive(false));
        assert!(!p.active);
    }

    #[test]
    fn process_action_set_window_labels() {
        use std::collections::BTreeMap;
        let mut p = WmTopPanelComponent::new("test");
        let mut labels = BTreeMap::new();
        let key = WindowKey::default();
        labels.insert(key, "My Window".to_string());
        p.process_action(&ComponentAction::SetWindowLabels(labels));
        assert_eq!(
            p.window_labels.get(&key).map(|s| s.as_str()),
            Some("My Window")
        );
    }

    #[test]
    fn process_action_set_top_panel_state() {
        use term_wm_core::components::TopPanelState;
        let mut p = WmTopPanelComponent::new("test");
        let key = WindowKey::default();
        let state = TopPanelState {
            focus_current: Some(key),
            display_order: vec![key],
            status_line: Some("ready".to_string()),
            menu_open: true,
            tiling_indicator: None,
        };
        p.process_action(&ComponentAction::SetTopPanelState(Box::new(state)));
        assert_eq!(p.focus_current, Some(key));
        assert_eq!(p.display_order, vec![key]);
        assert_eq!(p.status_line.as_deref(), Some("ready"));
        assert!(p.menu_open);
    }

    #[test]
    fn query_non_menu_returns_none() {
        let p = WmTopPanelComponent::new("test");
        assert!(matches!(
            p.query(&ComponentQuery::SelectedAction),
            ComponentResponse::None
        ));
        assert!(matches!(
            p.query(&ComponentQuery::KeybindingHints),
            ComponentResponse::None
        ));
    }

    #[test]
    fn query_menu_icon_rect_returns_none_initially() {
        let p = WmTopPanelComponent::new("test");
        let resp = p.query(&ComponentQuery::MenuIconRect);
        assert!(matches!(resp, ComponentResponse::Rect(None)));
    }

    #[test]
    fn begin_frame_clears_state() {
        let mut p = WmTopPanelComponent::new("test");
        p.menu.rect = Some(LayoutRect {
            x: 0,
            y: 0,
            width: 5,
            height: 1,
        });
        p.strip.window_hits.push(PanelWindowHit {
            id: WindowKey::default(),
            rect: LayoutRect {
                x: 0,
                y: 0,
                width: 5,
                height: 1,
            },
        });
        p.begin_frame();
        assert!(p.menu.rect.is_none());
        assert!(p.strip.window_hits.is_empty());
    }

    #[test]
    fn wmbegin_frame_trait_delegates() {
        let mut p = WmTopPanelComponent::new("test");
        p.menu.rect = Some(LayoutRect {
            x: 0,
            y: 0,
            width: 5,
            height: 1,
        });
        WmComponent::begin_frame(&mut p);
        assert!(p.menu.rect.is_none());
    }

    #[test]
    fn wmvisible_trait_delegates() {
        let p = WmTopPanelComponent::new("test");
        assert!(WmComponent::visible(&p));
    }

    #[test]
    fn wmset_visible_trait_delegates() {
        let mut p = WmTopPanelComponent::new("test");
        WmComponent::set_visible(&mut p, false);
        assert!(!WmComponent::visible(&p));
    }

    #[test]
    fn handle_events_non_mouse_returns_ignored() {
        let mut p = WmTopPanelComponent::new("test");
        let ctx = ComponentContext::new(true);
        let event = term_wm_core::events::Event::Key(term_wm_core::events::KeyEvent {
            code: term_wm_core::events::KeyCode::Char('a'),
            modifiers: term_wm_core::events::KeyModifiers::NONE,
            kind: term_wm_core::events::KeyKind::Press,
        });
        let result = p.handle_events(&event, &ctx);
        assert!(result.is_ignored());
    }

    #[test]
    fn handle_events_mouse_events_never_ignored_after_capture() {
        let mut p = WmTopPanelComponent::new("test");
        let ctx = ComponentContext::new(true);
        // Captured mouse events must be Consumed, never Ignored, so drag/scroll
        // coordinates never leak into the terminal/PTY below the panel.
        for kind in [
            MouseEventKind::Moved,
            MouseEventKind::Drag(MouseButton::Left),
            MouseEventKind::Release(MouseButton::Left),
            MouseEventKind::ScrollLeft,
            MouseEventKind::ScrollRight,
        ] {
            let event = Event::Mouse(MouseEvent {
                kind,
                column: 0,
                row: 0,
                modifiers: KeyModifiers::NONE,
            });
            let result = p.handle_events(&event, &ctx);
            assert!(
                matches!(result, EventResult::Consumed),
                "mouse kind {kind:?} must be Consumed, not Ignored"
            );
        }
    }

    #[test]
    fn press_on_empty_panel_returns_ignored() {
        let mut p = WmTopPanelComponent::new("test");
        let ctx = ComponentContext::new(true);
        let press = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        let result = p.handle_events(&press, &ctx);
        assert!(result.is_ignored());
    }

    #[test]
    fn render_with_zero_area_does_nothing() {
        let mut p = WmTopPanelComponent::new("test");
        p.active = true;
        p.area = LayoutRect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        };
        let mut backend = make_backend(80, 24);
        p.render_contents(&mut backend, &NOIR);
    }

    #[test]
    fn render_with_empty_display_order_and_no_status() {
        let mut p = WmTopPanelComponent::new("test");
        p.active = true;
        let key = WindowKey::default();
        p.focus_current = Some(key);
        p.display_order = vec![];

        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        let _ = p.split_area(true, area);
        let mut backend = make_backend(80, 24);
        p.render_contents(&mut backend, &NOIR);
    }

    #[test]
    fn consume_area_delegates_to_split_area() {
        let mut p = WmTopPanelComponent::new("test");
        p.active = true;
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let (panel, managed) = p.consume_area(area);
        assert_eq!(panel.height, 1);
        assert_eq!(managed.height, 23);
    }

    fn push_windows(p: &mut WmTopPanelComponent, keys: &[WindowKey], area: LayoutRect) {
        let labels: std::collections::BTreeMap<_, _> = keys
            .iter()
            .enumerate()
            .map(|(i, k)| (*k, format!("Window {}", i)))
            .collect();
        p.area = area;
        p.active = true;
        p.process_action(&ComponentAction::SetWindowLabels(labels));
        p.focus_current = keys.first().copied();
        p.display_order = keys.to_vec();
    }

    fn make_keys(n: usize) -> Vec<WindowKey> {
        use std::collections::HashMap;
        use std::sync::Arc;
        use term_wm_core::app_context::AppContext;
        use term_wm_core::components::NoopComponent;
        use term_wm_core::window::{LayerManager, WindowManager};
        use term_wm_core::wm_config::WmConfig;

        let mut wm = WindowManager::<NoopComponent>::with_config(
            WmConfig::default(),
            Arc::new(AppContext::new("test", "0.0.0")),
            None,
            LayerManager::new(),
            HashMap::new(),
        );
        (0..n).map(|_| wm.create_window(NoopComponent)).collect()
    }

    fn render_panel(p: &mut WmTopPanelComponent) {
        let mut backend = make_backend(p.area.width, p.area.height);
        p.render_contents(&mut backend, &NOIR);
    }

    #[test]
    fn overflow_clamps_scroll_and_shows_indicators() {
        let keys = make_keys(8);
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 30,
            height: 1,
        };
        let mut p = WmTopPanelComponent::new("test-app");
        push_windows(&mut p, &keys, area);

        // Content overflows the 30-col viewport (assert the premise), and an
        // extreme scroll clamps to max_scroll — no u16 underflow/panic.
        p.focus_current = keys.last().copied();
        p.strip.h_scroll = u16::MAX;
        render_panel(&mut p);
        assert!(p.strip.max_scroll > 0, "content must overflow the viewport");
        assert_eq!(
            p.strip.h_scroll,
            p.strip.max_scroll,
            "h_scroll must clamp to max_scroll without underflow/panic"
        );
        assert!(
            p.strip.left_indicator_rect.is_some(),
            "left ◀ indicator expected when scrolled right"
        );
        assert!(
            p.strip.right_indicator_rect.is_none(),
            "right ▶ indicator absent at the far-right scroll"
        );

        // Auto-scroll pulls the (now leftmost-focused) window back into view.
        p.focus_current = keys.first().copied();
        p.strip.h_scroll = p.strip.max_scroll;
        render_panel(&mut p);
        assert_eq!(
            p.strip.h_scroll, 0,
            "auto-scroll brings the focused window into view"
        );
        assert!(
            p.strip.left_indicator_rect.is_none(),
            "left ◀ absent at scroll origin"
        );
        assert!(
            p.strip.right_indicator_rect.is_some(),
            "right ▶ expected when content remains off-screen"
        );
    }

    #[test]
    fn scroll_events_adjust_h_scroll() {
        let keys = make_keys(8);
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 30,
            height: 1,
        };
        let mut p = WmTopPanelComponent::new("test-app");
        push_windows(&mut p, &keys, area);
        render_panel(&mut p);
        let before = p.strip.h_scroll;

        let ctx = ComponentContext::new(false);
        let scroll_right = Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollRight,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        let res = p.handle_events(&scroll_right, &ctx);
        assert!(matches!(res, EventResult::Consumed), "ScrollRight consumed");
        assert!(
            p.strip.h_scroll > before,
            "ScrollRight must increase h_scroll"
        );

        let scroll_left = Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollLeft,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        let res = p.handle_events(&scroll_left, &ctx);
        assert!(matches!(res, EventResult::Consumed), "ScrollLeft consumed");
        assert!(
            p.strip.h_scroll <= before + SCROLL_STEP,
            "ScrollLeft decreases h_scroll"
        );
    }

    #[test]
    fn drag_ghost_then_release_reorders_to_front() {
        let keys = make_keys(3);
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        let mut p = WmTopPanelComponent::new("test-app");
        push_windows(&mut p, &keys, area);
        render_panel(&mut p);

        // Press the LAST entry (keys[2]).
        let (_key, lx, _width) = p.strip.entry_geometry[2];
        let press_col = (p.strip.entries_start_x + lx) as u16;
        let ctx = ComponentContext::new(false);
        let press = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: press_col,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        let res = p.handle_events(&press, &ctx);
        assert!(
            matches!(res, EventResult::Action(TermWmAction::FocusWindow(k)) if k == keys[2]),
            "pressing an entry focuses it"
        );
        assert_eq!(p.strip.drag_source, Some(keys[2]));

        // Drag to the far left edge — the Drag itself is Consumed (the ghost is
        // the feedback); the reorder is committed on Release.
        let drag_col = p.strip.entries_start_x as u16;
        let drag = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: drag_col,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        let res = p.handle_events(&drag, &ctx);
        assert!(matches!(res, EventResult::Consumed), "drag consumed");
        assert_eq!(p.strip.drop_index, Some(0));

        // Release commits the reorder to index 0.
        let release = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Release(MouseButton::Left),
            column: drag_col,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        let res = p.handle_events(&release, &ctx);
        assert!(
            matches!(res, EventResult::Action(TermWmAction::ReorderWindow { key, index }) if key == keys[2] && index == 0),
            "release must dispatch ReorderWindow with index 0"
        );
        assert!(p.strip.drag_source.is_none(), "drag state cleared on release");
    }

    #[test]
    fn plain_click_does_not_emit_reorder() {
        let keys = make_keys(3);
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        let mut p = WmTopPanelComponent::new("test-app");
        push_windows(&mut p, &keys, area);
        render_panel(&mut p);

        // Press + release without a Drag event = a plain click.
        let press_col = (p.strip.entries_start_x + p.strip.entry_geometry[1].1) as u16;
        let ctx = ComponentContext::new(false);
        let press = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: press_col,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        p.handle_events(&press, &ctx);
        let release = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Release(MouseButton::Left),
            column: press_col,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        let res = p.handle_events(&release, &ctx);
        assert!(
            matches!(res, EventResult::Consumed),
            "a plain click must not emit a ReorderWindow action"
        );
    }

    #[test]
    fn drag_off_right_edge_clamps_to_end() {
        let keys = make_keys(3);
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 30,
            height: 1,
        };
        let mut p = WmTopPanelComponent::new("test-app");
        push_windows(&mut p, &keys, area);
        render_panel(&mut p);
        p.strip.drag_source = Some(keys[0]);

        // Drag far beyond the right edge -> clamps to the end (index == len).
        let far_right = (p.strip.entries_start_x + p.strip.scroll_viewport_width + 50) as u16;
        let ctx = ComponentContext::new(false);
        let drag = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: far_right,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        p.handle_events(&drag, &ctx);
        // The reorder is committed on Release, with the index clamped to the
        // end of the reduced list.
        let release = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Release(MouseButton::Left),
            column: far_right,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        let res = p.handle_events(&release, &ctx);
        assert!(
            matches!(res, EventResult::Action(TermWmAction::ReorderWindow { key, index }) if key == keys[0] && index == keys.len() - 1),
            "dragging off the right edge clamps the reorder index to the end of the reduced list"
        );
    }

    #[test]
    fn chevrons_do_not_overlap_tiling_indicator() {
        let keys = make_keys(8);
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 60,
            height: 1,
        };
        let mut p = WmTopPanelComponent::new("test-app");
        push_windows(&mut p, &keys, area);
        // Enable the right-aligned tiling indicator AND overflow, with a middle
        // focus so auto-scroll lands mid-strip (both chevrons visible).
        p.tiling.set_indicator(Some(("⊞ Float", TermWmAction::ToggleTiling)));
        p.focus_current = Some(keys[3]);
        p.strip.h_scroll = u16::MAX;
        render_panel(&mut p);

        let tiling = p.tiling.rect.expect("tiling label present");
        // The strip's allocated rect ends TILING_GAP before the tiling slot.
        let strip_end = p.strip.rect.x + i32::from(p.strip.rect.width);
        assert!(
            strip_end <= tiling.x - i32::from(TILING_GAP),
            "strip rect must end at least TILING_GAP before the tiling slot"
        );
        // The right ▶ chevron (when shown) is inside the strip rect, left of
        // the tiling label.
        if let Some(chevron) = p.strip.right_indicator_rect {
            assert!(
                chevron.x + i32::from(chevron.width) <= tiling.x,
                "right ▶ (x={}) must end before the tiling label (x={})",
                chevron.x,
                tiling.x
            );
        }
        assert!(
            p.strip.left_indicator_rect.is_some(),
            "left ◀ expected at mid-strip scroll"
        );
    }

    #[test]
    fn chevrons_have_gap_from_entries() {
        let keys = make_keys(8);
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 30,
            height: 1,
        };
        let mut p = WmTopPanelComponent::new("test-app");
        push_windows(&mut p, &keys, area);
        p.focus_current = Some(keys[3]);
        render_panel(&mut p);

        let left_chevron_x = p.strip.left_indicator_rect.expect("left ◀ shown").x;
        let right_chevron_x = p.strip.right_indicator_rect.expect("right ▶ shown").x;
        // The first entry starts one column (chevron) + CHEVRON_GAP right of the
        // left chevron.
        assert_eq!(
            p.strip.entries_start_x,
            left_chevron_x + 1 + i32::from(CHEVRON_GAP),
            "entry viewport must start a chevron column + CHEVRON_GAP past the left chevron"
        );
        // No entry's visible right edge ever reaches the right chevron.
        for hit in &p.strip.window_hits {
            assert!(
                hit.rect.x + i32::from(hit.rect.width) <= right_chevron_x,
                "entry must not bury the right ▶ chevron"
            );
        }
    }

    #[test]
    fn drag_ghost_renders_at_cursor_and_not_duplicated() {
        let keys = make_keys(3);
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        let mut p = WmTopPanelComponent::new("test-app");
        push_windows(&mut p, &keys, area);
        render_panel(&mut p);

        // Press the LAST entry at its left edge (grab offset 0).
        let (_key, lx, _width) = p.strip.entry_geometry[2];
        let press_col = (p.strip.entries_start_x + lx) as u16;
        let ctx = ComponentContext::new(false);
        let press = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: press_col,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        p.handle_events(&press, &ctx);

        // Drag to the left edge of the viewport.
        let drag_col = p.strip.entries_start_x as u16;
        let drag = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: drag_col,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        p.handle_events(&drag, &ctx);

        // Render and inspect: the ghost "Window 2" appears EXACTLY ONCE (the
        // dragged key is skipped in the static loop), at the cursor column
        // (grab offset 0) — i.e. it glides like a thumb.
        let mut backend = make_backend(80, 1);
        p.render_contents(&mut backend, &NOIR);
        let cells: Vec<char> = (0..80u16)
            .map(|xx| {
                backend
                    .buffer
                    .cell((xx, 0))
                    .map(|c| c.symbol().chars().next().unwrap_or(' '))
                    .unwrap_or(' ')
            })
            .collect();
        let needle: Vec<char> = "Window 2".chars().collect();
        let starts: Vec<usize> = (0..=cells.len().saturating_sub(needle.len()))
            .filter(|&i| cells[i..i + needle.len()] == needle)
            .collect();
        assert_eq!(
            starts.len(),
            1,
            "dragged title must render exactly once (ghost, not duplicated), starts={starts:?}"
        );
        // The ghost chunk is `" Window 2 "`, so "Window 2" starts one column
        // after the leading pad space, which is entries_start_x (grab offset 0).
        assert_eq!(
            starts[0] as i32,
            p.strip.entries_start_x + 1,
            "ghost must track the cursor at the grabbed column (grab offset 0)"
        );
    }

    #[test]
    fn manual_scroll_persists_until_focus_changes() {
        let keys = make_keys(8);
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 30,
            height: 1,
        };
        let mut p = WmTopPanelComponent::new("test-app");
        push_windows(&mut p, &keys, area);
        p.focus_current = Some(keys[3]);
        render_panel(&mut p); // first render auto-scrolls to keys[3]
        let after_auto = p.strip.h_scroll;
        assert!(after_auto > SCROLL_STEP, "auto-scroll should land mid-strip");

        // A manual scroll (e.g. chevron click) with unchanged focus must persist —
        // NOT be snapped back by per-frame auto-scroll.
        let manual = after_auto.saturating_sub(SCROLL_STEP);
        p.strip.h_scroll = manual;
        render_panel(&mut p);
        assert_eq!(
            p.strip.h_scroll,
            manual,
            "manual scroll must persist while the focused window is unchanged"
        );

        // Changing focus re-engages auto-scroll to bring the new window into view.
        p.focus_current = Some(keys[6]);
        p.strip.h_scroll = 0;
        render_panel(&mut p);
        assert!(
            p.strip.h_scroll > 0,
            "auto-scroll re-engages when the focused window changes"
        );
    }

    #[test]
    fn reorder_moves_focused_off_screen_then_auto_scrolls_back() {
        let keys = make_keys(8);
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 30,
            height: 1,
        };
        let mut p = WmTopPanelComponent::new("test-app");
        push_windows(&mut p, &keys, area);
        p.focus_current = Some(keys[0]);
        render_panel(&mut p); // auto-scroll: keys[0] at the left → h_scroll 0
        assert_eq!(p.strip.h_scroll, 0);

        // Manual scroll far right persists (focus + logical bounds unchanged).
        p.strip.h_scroll = p.strip.max_scroll;
        render_panel(&mut p);
        assert_eq!(
            p.strip.h_scroll,
            p.strip.max_scroll,
            "manual scroll persists with unchanged focus"
        );

        // A structural mutation (reorder) moves the FOCUSED window to the end:
        // its logical bounds change, so auto-scroll must re-follow it into view
        // even though the focused key is unchanged.
        let mut new_order = keys[1..].to_vec();
        new_order.push(keys[0]);
        p.display_order = new_order;
        p.strip.h_scroll = 0;
        render_panel(&mut p);
        assert_eq!(
            p.strip.h_scroll,
            p.strip.max_scroll,
            "auto-scroll re-follows the focused window after a structural reorder"
        );
    }
}
