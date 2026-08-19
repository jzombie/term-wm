//! Top status panel — composed of small applets (menu, tab bar, status line,
//! tiling indicator) that each own a bounded region of the single row.
//!
//! The parent runs a small layout pass each frame: it reserves the menu on the
//! left and the tiling indicator on the right, then hands the middle region to
//! either the status line or the scrollable, draggable tab bar. Because each
//! applet renders and interacts strictly inside its own allocated rect, the tab
//! bar's `◀`/`▶` overflow indicators are always contained and never overwritten
//! by the menu or the tiling label.

mod menu;
mod status;
mod tiling;

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
use term_wm_ui_components::tab_bar::{TabBarComponent, TabBarEvent, TabItem};

use menu::MenuButton;
use status::StatusLine;
use tiling::TilingIndicator;

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
    bar: TabBarComponent<WindowKey>,
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
            bar: TabBarComponent::new(),
            status: StatusLine::new(),
            tiling: TilingIndicator::new(),
        }
    }

    pub fn begin_frame(&mut self) {
        self.menu.begin_frame();
        self.bar.begin_frame();
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

    /// Hit-test a window entry in the tab bar (delegates to the bar applet).
    pub fn hit_test_window(&self, column: u16, row: u16) -> Option<WindowKey> {
        self.bar.hit_test(column, row)
    }

    /// Layout pass + applet rendering into `self.area`.
    ///
    /// Reserves the menu (left) and tiling label (right), then hands the middle
    /// region to EITHER the status line OR the tab bar (mutually exclusive), so
    /// applets never overlap.
    fn render_contents(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        ctx: &ComponentContext,
        registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        let theme = ctx.config().theme;
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
            self.menu.render(backend, menu_slot, self.menu_open, &theme);
        }

        // Center region: [menu + gap, max_x - tiling_width - TILING_GAP).
        let tiling_width = self.tiling.label_width();
        let bar_start = area
            .x
            .saturating_add(i32::from(menu_width))
            .saturating_add(i32::from(MENU_GAP));
        let bar_end = if tiling_width > 0 {
            max_x.saturating_sub(i32::from(tiling_width + TILING_GAP))
        } else {
            max_x
        };
        let bar_width = bar_end.saturating_sub(bar_start).max(0);
        let bar_rect = LayoutRect {
            x: bar_start,
            y,
            width: bar_width as u16,
            height: 1,
        };

        if let Some(status) = self.status_line.clone() {
            self.status.render(backend, bar_rect, &status, &theme);
        } else if self.focus_current.is_some() {
            let items: Vec<TabItem<WindowKey>> = self
                .display_order
                .iter()
                .map(|key| TabItem {
                    key: *key,
                    label: self
                        .window_labels
                        .get(key)
                        .cloned()
                        .unwrap_or_else(|| format!("{key:?}")),
                    closable: false,
                    style_override: None,
                })
                .collect();
            self.bar.set_items(items);
            self.bar.set_active(self.focus_current);
            self.bar.render(backend, bar_rect, ctx, registry);
        }

        // Tiling indicator (right edge).
        if tiling_width > 0 {
            self.tiling.render(backend, area, &theme);
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
        registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        let theme = ctx.config().theme;
        if !self.active {
            // Still render the tiling indicator even when inactive so the
            // label is visible and its rect is populated for clicks.
            self.bar.clear_drag_state();
            self.tiling.render(backend, area, &theme);
            return;
        }
        let app_name = ctx.app_name().to_string();
        if app_name != self.app_name {
            self.app_name = app_name.clone();
            self.menu.set_app_name(&app_name);
        }
        self.area = area;
        self.render_contents(backend, ctx, registry);
    }

    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        let Event::Mouse(mouse) = event else {
            return EventResult::Ignored;
        };
        match mouse.kind {
            MouseEventKind::Press(_) => {
                if self.menu.contains(mouse.column, mouse.row) {
                    return EventResult::Action(TermWmAction::OpenCommandPalette);
                }
                let bar_res = map_bar(self.bar.handle_events(event, ctx));
                if !bar_res.is_ignored() {
                    return bar_res;
                }
                if self.tiling.contains(mouse.column, mouse.row) {
                    if let Some(action) = self.tiling.action() {
                        return EventResult::Action(action);
                    }
                    return EventResult::Consumed;
                }
                // A press on the panel's background (no applet handled it) must be
                // consumed so it never falls through to a window behind the panel
                // (e.g. a floating window overlapping the top row in monocle),
                // which would close the Command Palette and could hit the window's
                // close button. Only consume when the panel is actually rendered
                // (its area is set); an unrendered panel keeps Ignored.
                if !self.area.is_empty() && rect_contains(self.area, mouse.column, mouse.row) {
                    return EventResult::Consumed;
                }
                EventResult::Ignored
            }
            // Drag/Release/Scroll are delivered under mouse capture (or to the
            // panel's layer) and must never fall through to the terminal below.
            MouseEventKind::Drag(_) | MouseEventKind::Release(_) => {
                map_bar(self.bar.handle_events(event, ctx))
            }
            MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight => {
                map_bar(self.bar.handle_events(event, ctx))
            }
            _ => EventResult::Consumed,
        }
    }
}

/// Map a [`TabBarEvent`] from the tab bar applet to a [`TermWmAction`].
fn map_bar(res: EventResult<TabBarEvent<WindowKey>>) -> EventResult<TermWmAction> {
    match res {
        EventResult::Action(TabBarEvent::Select(k)) => {
            EventResult::Action(TermWmAction::FocusWindow(k))
        }
        EventResult::Action(TabBarEvent::Close(k)) => {
            EventResult::Action(TermWmAction::CloseWindow(k))
        }
        EventResult::Action(TabBarEvent::Reorder { key, target_index }) => {
            EventResult::Action(TermWmAction::ReorderWindow {
                key,
                index: target_index,
            })
        }
        EventResult::Consumed => EventResult::Consumed,
        EventResult::Ignored => EventResult::Ignored,
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

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use term_wm_core::components::{
        ComponentAction, ComponentQuery, ComponentResponse, WmComponent,
    };
    use term_wm_core::events::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use term_wm_core::wm_config::HintVisibility;
    use term_wm_ui_components::helpers::menu_icon;

    fn make_backend(w: u16, h: u16) -> term_wm_console::RatatuiBackend {
        let buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, w, h));
        term_wm_console::RatatuiBackend::new_simple(buf, ratatui::layout::Rect::new(0, 0, w, h))
    }

    fn ctx() -> ComponentContext {
        ComponentContext::new(false)
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> Event {
        Event::Mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        })
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

    fn render_panel(p: &mut WmTopPanelComponent) {
        let mut backend = make_backend(p.area.width, p.area.height);
        let mut reg = term_wm_core::hitbox_registry::HitboxRegistry::new();
        p.render_contents(&mut backend, &ctx(), &mut reg);
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
        let key = WindowKey::default();
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        push_windows(&mut p, &[key], area);
        render_panel(&mut p);
        // First tab starts at menu width + gap (no overflow).
        let bar_start = (menu_icon("test").chars().count() as u16) + MENU_GAP;
        assert_eq!(p.hit_test_window(bar_start + 1, 0), Some(key));
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
        p.render(
            &mut backend,
            area,
            &ctx(),
            &mut term_wm_core::hitbox_registry::HitboxRegistry::new(),
        );
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
        render_panel(&mut p);
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
        render_panel(&mut p);
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
        render_panel(&mut p);
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
    fn begin_frame_clears_menu_rect() {
        let mut p = WmTopPanelComponent::new("test");
        p.menu.rect = Some(LayoutRect {
            x: 0,
            y: 0,
            width: 5,
            height: 1,
        });
        p.begin_frame();
        assert!(p.menu.rect.is_none());
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
        let event = term_wm_core::events::Event::Key(term_wm_core::events::KeyEvent {
            code: term_wm_core::events::KeyCode::Char('a'),
            modifiers: term_wm_core::events::KeyModifiers::NONE,
            kind: term_wm_core::events::KeyKind::Press,
        });
        let result = p.handle_events(&event, &ctx());
        assert!(result.is_ignored());
    }

    #[test]
    fn handle_events_mouse_events_never_ignored_after_capture() {
        let mut p = WmTopPanelComponent::new("test");
        // Captured mouse events must be Consumed, never Ignored, so drag/scroll
        // coordinates never leak into the terminal/PTY below the panel.
        for kind in [
            MouseEventKind::Moved,
            MouseEventKind::Drag(MouseButton::Left),
            MouseEventKind::Release(MouseButton::Left),
            MouseEventKind::ScrollLeft,
            MouseEventKind::ScrollRight,
        ] {
            let result = p.handle_events(&mouse(kind, 0, 0), &ctx());
            assert!(
                matches!(result, EventResult::Consumed),
                "mouse kind {kind:?} must be Consumed, not Ignored"
            );
        }
    }

    #[test]
    fn press_on_empty_panel_returns_ignored() {
        let mut p = WmTopPanelComponent::new("test");
        let result = p.handle_events(
            &mouse(MouseEventKind::Press(MouseButton::Left), 0, 0),
            &ctx(),
        );
        assert!(result.is_ignored());
    }

    #[test]
    fn press_on_panel_background_is_consumed() {
        let keys = make_keys(1);
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        let mut p = WmTopPanelComponent::new("test-app");
        push_windows(&mut p, &keys, area);
        render_panel(&mut p);

        // A press on the rendered panel's background (past the single tab, not on
        // menu/tab/chevron/tiling) must be CONSUMED so it never falls through to
        // a window behind the panel (which would close the Command Palette and
        // could click the window's close button).
        let res = p.handle_events(
            &mouse(MouseEventKind::Press(MouseButton::Left), 60, 0),
            &ctx(),
        );
        assert!(
            matches!(res, EventResult::Consumed),
            "panel-background press must be consumed, not Ignored"
        );
    }

    #[test]
    fn press_window_tab_focuses_window() {
        let keys = make_keys(2);
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        let mut p = WmTopPanelComponent::new("test-app");
        push_windows(&mut p, &keys, area);
        render_panel(&mut p);

        // First tab starts at menu width + MENU_GAP (no overflow).
        let bar_start = (menu_icon("test-app").chars().count() as u16) + MENU_GAP;
        let res = p.handle_events(
            &mouse(MouseEventKind::Press(MouseButton::Left), bar_start + 1, 0),
            &ctx(),
        );
        assert!(
            matches!(res, EventResult::Action(TermWmAction::FocusWindow(k)) if k == keys[0]),
            "pressing a tab must map to FocusWindow"
        );
    }

    #[test]
    fn drag_release_reorders_window() {
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

        let bar_start = (menu_icon("test-app").chars().count() as u16) + MENU_GAP;
        // Tab i spans [bar_start + 10*i, bar_start + 10*(i+1)) (label 8 + 2 pad).
        let tab2 = bar_start + 20;
        let res = p.handle_events(
            &mouse(MouseEventKind::Press(MouseButton::Left), tab2 + 1, 0),
            &ctx(),
        );
        assert!(matches!(res, EventResult::Action(TermWmAction::FocusWindow(k)) if k == keys[2]));
        p.handle_events(
            &mouse(MouseEventKind::Drag(MouseButton::Left), bar_start, 0),
            &ctx(),
        );
        let res = p.handle_events(
            &mouse(MouseEventKind::Release(MouseButton::Left), bar_start, 0),
            &ctx(),
        );
        assert!(
            matches!(res, EventResult::Action(TermWmAction::ReorderWindow { key, index }) if key == keys[2] && index == 0),
            "drag+release must map to ReorderWindow"
        );
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
        render_panel(&mut p);
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
        render_panel(&mut p);
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
}
