use std::collections::BTreeMap;

use ratatui::style::{Modifier, Style};
use term_wm_core::events::{Event, KeyModifiers, MouseButton, MouseEventKind};
use term_wm_layout_engine::LayoutRect;

use term_wm_core::{
    actions::{EventResult, TermWmAction},
    components::{
        Component, ComponentAction, ComponentContext, ComponentQuery, ComponentResponse,
        WmComponent,
    },
    hitbox_registry::HitboxId,
    layout::rect_contains,
    utils::truncate_to_width,
    window::WindowKey,
};
use term_wm_ui_components::helpers::{
    color_to_ratatui, layout_rect_to_clipped_rect, safe_set_string,
};

#[derive(Debug, Clone, Copy)]
struct PanelWindowHit {
    id: WindowKey,
    rect: LayoutRect,
}

#[derive(Debug)]
struct WindowList {
    window_hits: Vec<PanelWindowHit>,
}

impl WindowList {
    fn new() -> Self {
        Self {
            window_hits: Vec::new(),
        }
    }

    fn begin_frame(&mut self) {
        self.window_hits.clear();
    }
}

#[derive(Debug)]
pub struct WmTopPanelComponent {
    visible: bool,
    height: u16,
    area: LayoutRect,
    menu_rect: Option<LayoutRect>,
    list: WindowList,
    app_name: String,
    // WmComponent render state (pushed via process_action before render)
    active: bool,
    focus_current: Option<WindowKey>,
    display_order: Vec<WindowKey>,
    status_line: Option<String>,
    menu_open: bool,
    window_labels: BTreeMap<WindowKey, String>,
    hitbox_id: HitboxId,
}

impl WmTopPanelComponent {
    pub fn new(app_name: &str) -> Self {
        Self {
            visible: true,
            height: 1,
            area: LayoutRect::default(),
            menu_rect: None,
            list: WindowList::new(),
            app_name: app_name.to_string(),
            active: false,
            focus_current: None,
            display_order: Vec::new(),
            status_line: None,
            menu_open: false,
            window_labels: BTreeMap::new(),
            hitbox_id: HitboxId::new(),
        }
    }

    pub fn begin_frame(&mut self) {
        self.list.begin_frame();
        self.menu_rect = None;
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
        self.menu_rect
    }

    pub fn menu_icon_contains_point(&self, column: u16, row: u16) -> bool {
        if let Some(rect) = self.menu_rect {
            return rect_contains(rect, column, row);
        }
        false
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

    #[allow(clippy::too_many_arguments)]
    pub fn render_inner(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        active: bool,
        focus_current: WindowKey,
        display_order: &[WindowKey],
        status_line: Option<&str>,
        menu_open: bool,
        theme: &term_wm_core::theme::Theme,
    ) {
        if !active {
            return;
        }
        let area = self.area;
        if area.width == 0 || area.height == 0 {
            return;
        }
        let ratatui_backend = term_wm_ui_components::helpers::downcast_ratatui(backend);
        let buffer = &mut ratatui_backend.buffer;
        let ratatui_area = layout_rect_to_clipped_rect(area);
        let bounds = ratatui_area.intersection(buffer.area);
        if bounds.width == 0 || bounds.height == 0 {
            return;
        }
        for yy in bounds.y..bounds.y.saturating_add(bounds.height) {
            for xx in bounds.x..bounds.x.saturating_add(bounds.width) {
                if let Some(cell) = buffer.cell_mut((xx, yy)) {
                    let mut st = cell.style();
                    st.bg = Some(color_to_ratatui(theme.bottom_panel_bg));
                    st.fg = Some(color_to_ratatui(theme.bottom_panel_fg));
                    cell.set_style(st);
                    cell.set_symbol(" ");
                }
            }
        }
        let mut x = area.x;
        let y = area.y;
        let max_x = area.x.saturating_add(i32::from(area.width));
        let menu_icon = format!("\u{2261} {}", self.app_name);
        let menu_width = menu_icon.chars().count() as u16;
        if x.saturating_add(i32::from(menu_width)) <= max_x {
            let menu_style = if menu_open {
                Style::default()
                    .bg(color_to_ratatui(theme.menu_bg))
                    .fg(color_to_ratatui(theme.menu_fg))
            } else {
                Style::default()
            };
            safe_set_string(
                buffer,
                bounds,
                x as u16,
                y as u16,
                menu_icon.as_str(),
                menu_style,
            );
            self.menu_rect = Some(LayoutRect {
                x,
                y,
                width: menu_width,
                height: 1,
            });
            x = x.saturating_add(i32::from(menu_width));
        }
        if x < max_x {
            safe_set_string(buffer, bounds, x as u16, y as u16, " ", Style::default());
            x = x.saturating_add(1);
        }
        if let Some(status) = status_line {
            let available = (max_x.saturating_sub(x)).max(1);
            let text = truncate_to_width(status, available as usize);
            safe_set_string(buffer, bounds, x as u16, y as u16, &text, Style::default());
        } else {
            for id in display_order.iter().copied() {
                let focused = id == focus_current;
                let mut label = self
                    .window_labels
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| format!("{id:?}"));
                let max_label = (max_x.saturating_sub(x).saturating_sub(2)) as usize;
                if label.chars().count() > max_label {
                    label = truncate_to_width(&label, max_label);
                }
                let chunk = format!(" {label} ");
                let chunk_width = chunk.chars().count() as u16;
                if x.saturating_add(i32::from(chunk_width)) > max_x {
                    break;
                }
                let item_style = if focused {
                    Style::default()
                        .bg(color_to_ratatui(theme.menu_selected_bg))
                        .fg(color_to_ratatui(theme.menu_selected_fg))
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(color_to_ratatui(theme.panel_inactive_fg))
                };
                safe_set_string(buffer, bounds, x as u16, y as u16, &chunk, item_style);
                self.list.window_hits.push(PanelWindowHit {
                    id,
                    rect: LayoutRect {
                        x,
                        y,
                        width: chunk_width,
                        height: 1,
                    },
                });
                x = x.saturating_add(i32::from(chunk_width));
            }
        }

    }

    pub fn hit_test_window(&self, column: u16, row: u16) -> Option<WindowKey> {
        self.list
            .window_hits
            .iter()
            .find(|hit| rect_contains(hit.rect, column, row))
            .map(|hit| hit.id)
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
        if !self.active {
            return;
        }
        let theme = ctx.config().theme;
        let app_name = ctx.app_name().to_string();
        if app_name != self.app_name {
            self.app_name = app_name;
        }
        self.area = area;
        if let Some(focus) = self.focus_current {
            let display_order = self.display_order.clone();
            let status_line = self.status_line.clone();

            self.render_inner(
                backend,
                self.active,
                focus,
                &display_order,
                status_line.as_deref(),
                self.menu_open,
                &theme,
            );
        }
    }

    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        let Event::Mouse(mouse) = event else {
            return EventResult::Ignored;
        };
        if !matches!(mouse.kind, MouseEventKind::Press(_)) {
            return EventResult::Ignored;
        }
        self.on_mouse_press(
            mouse.column,
            mouse.row,
            MouseButton::Left,
            mouse.modifiers,
            ctx,
        )
    }

    fn on_mouse_press(
        &mut self,
        column: u16,
        row: u16,
        _button: MouseButton,
        _modifiers: KeyModifiers,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        if self.menu_icon_contains_point(column, row) {
            return EventResult::Action(TermWmAction::OpenCommandPalette);
        }
        if let Some(key) = self.hit_test_window(column, row) {
            return EventResult::Action(TermWmAction::FocusWindow(key));
        }
        EventResult::Ignored
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
            }
            ComponentAction::SetWindowLabels(labels) => {
                self.window_labels = labels.clone();
            }
            _ => {}
        }
    }

    fn query(&self, query: &ComponentQuery) -> ComponentResponse {
        match query {
            ComponentQuery::MenuIconRect => ComponentResponse::Rect(self.menu_rect),
            _ => ComponentResponse::None,
        }
    }

    fn hit_test(&self, x: u16, y: u16) -> bool {
        if !self.area.is_empty() && rect_contains(self.area, x, y) {
            return true;
        }
        false
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
    use ratatui::buffer::Buffer;
    use term_wm_core::components::{
        ComponentAction, ComponentQuery, ComponentResponse, WmComponent,
    };
    use term_wm_core::theme::NOIR;
    use term_wm_core::wm_config::HintVisibility;

    fn make_backend(w: u16, h: u16) -> term_wm_console::RatatuiBackend {
        let buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, w, h));
        term_wm_console::RatatuiBackend::new(buf, ratatui::layout::Rect::new(0, 0, w, h))
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
        p.render_inner(
            &mut backend,
            true,
            key,
            &[key],
            None,
            false,
            &NOIR,
        );
        assert!(!p.list.window_hits.is_empty());
        let hit_rect = p.list.window_hits[0].rect;
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
        let theme = NOIR;
        let mut backend = make_backend(80, 24);
        p.render_inner(
            &mut backend,
            true,
            key,
            &[],
            Some("Status: OK"),
            false,
            &theme,
        );
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
        let theme = NOIR;
        let mut backend = make_backend(80, 24);
        p.render_inner(
&mut backend,
true,
key,
&[key],
None,
true,
&theme);
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
        let theme = NOIR;
        let mut backend = make_backend(20, 1);
        p.render_inner(
&mut backend,
true,
key,
&[key],
None,
false,
&theme);
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
        p.menu_rect = Some(LayoutRect {
            x: 0,
            y: 0,
            width: 5,
            height: 1,
        });
        p.list.window_hits.push(PanelWindowHit {
            id: WindowKey::default(),
            rect: LayoutRect {
                x: 0,
                y: 0,
                width: 5,
                height: 1,
            },
        });
        p.begin_frame();
        assert!(p.menu_rect.is_none());
        assert!(p.list.window_hits.is_empty());
    }

    #[test]
    fn wmbegin_frame_trait_delegates() {
        let mut p = WmTopPanelComponent::new("test");
        p.menu_rect = Some(LayoutRect {
            x: 0,
            y: 0,
            width: 5,
            height: 1,
        });
        WmComponent::begin_frame(&mut p);
        assert!(p.menu_rect.is_none());
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
    fn handle_events_mouse_not_press_returns_ignored() {
        let mut p = WmTopPanelComponent::new("test");
        let ctx = ComponentContext::new(true);
        let event = term_wm_core::events::Event::Mouse(term_wm_core::events::MouseEvent {
            kind: term_wm_core::events::MouseEventKind::Moved,
            column: 0,
            row: 0,
            modifiers: term_wm_core::events::KeyModifiers::NONE,
        });
        let result = p.handle_events(&event, &ctx);
        assert!(result.is_ignored());
    }

    #[test]
    fn on_mouse_press_no_hit_returns_ignored() {
        let mut p = WmTopPanelComponent::new("test");
        let ctx = ComponentContext::new(true);
        let result = p.on_mouse_press(0, 0, MouseButton::Left, KeyModifiers::NONE, &ctx);
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
        let theme = NOIR;
        let mut backend = make_backend(80, 24);
        let key = WindowKey::default();
        p.render_inner(
&mut backend,
true,
key,
&[],
None,
false,
&theme);
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
        let theme = NOIR;
        let mut backend = make_backend(80, 24);
        p.render_inner(
&mut backend,
true,
key,
&[],
None,
false,
&theme);
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

