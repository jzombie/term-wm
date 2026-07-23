use ratatui::style::Style;
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
    power_profile::PowerProfile,
    utils::truncate_to_width,
};
use term_wm_ui_components::helpers::{
    color_to_ratatui, layout_rect_to_clipped_rect, safe_set_string,
};

#[derive(Debug)]
pub struct WmBottomPanelComponent {
    area: LayoutRect,
    app_name: String,
    app_version: String,
    hostname: Option<String>,
    keybinding_hints: Vec<(TermWmAction, Vec<String>)>,
    hint_rects: Vec<(LayoutRect, TermWmAction)>,
    power_profile: PowerProfile,
    hitbox_id: HitboxId,
}

impl WmBottomPanelComponent {
    pub fn new(app_name: &str, app_version: &str, hostname: Option<&str>) -> Self {
        Self {
            area: LayoutRect::default(),
            app_name: app_name.to_string(),
            app_version: app_version.to_string(),
            hostname: hostname.map(|h| h.to_string()),
            keybinding_hints: Vec::new(),
            hint_rects: Vec::new(),
            power_profile: PowerProfile::PowerSaver,
            hitbox_id: HitboxId::new(),
        }
    }

    pub fn begin_frame(&mut self) {
        self.hint_rects.clear();
    }

    pub fn area(&self) -> LayoutRect {
        self.area
    }

    pub fn set_hostname(&mut self, hostname: &str) {
        self.hostname = Some(hostname.to_string());
    }

    pub fn set_keybinding_hints(&mut self, hints: Vec<(TermWmAction, Vec<String>)>) {
        self.keybinding_hints = hints;
    }

    pub fn keybinding_hints(&self) -> &[(TermWmAction, Vec<String>)] {
        &self.keybinding_hints
    }

    pub fn set_power_profile(&mut self, profile: PowerProfile) {
        self.power_profile = profile;
    }

    pub fn split_bottom_area(&mut self, area: LayoutRect, height: u16) -> (LayoutRect, LayoutRect) {
        let bottom = LayoutRect {
            x: area.x,
            y: area
                .y
                .saturating_add(i32::from(area.height))
                .saturating_sub(i32::from(height)),
            width: area.width,
            height,
        };
        let managed_height = area.height.saturating_sub(height);
        let managed = LayoutRect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: managed_height,
        };
        self.area = bottom;
        (bottom, managed)
    }

    pub fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        active: bool,
        theme: &term_wm_core::theme::Theme,
    ) {
        if active {
            self.render_bottom_impl(backend, true, theme);
        } else if !self.keybinding_hints.is_empty() {
            self.render_bottom_impl(backend, false, theme);
        }
    }

    fn render_bottom_impl(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        show_info: bool,
        theme: &term_wm_core::theme::Theme,
    ) {
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
                }
            }
        }
        let style = Style::default()
            .fg(color_to_ratatui(theme.bottom_panel_fg))
            .bg(color_to_ratatui(theme.bottom_panel_bg));

        // Reserve rightmost cell for the profile indicator
        let indicator_reserved = 1u16;

        let info_opt = if show_info {
            let platform = std::env::consts::OS;
            let pkg_label = format!("{} {}", self.app_name, self.app_version);
            let hostname = self
                .hostname
                .clone()
                .unwrap_or_else(|| "unknown-host".to_string());
            Some(format!(
                "{pkg_label} \u{00b7} {platform} \u{00b7} {hostname}"
            ))
        } else {
            None
        };

        let info_width = info_opt
            .as_ref()
            .map(|s| s.chars().count() as u16)
            .unwrap_or(0);
        let right_margin = info_width + 2 + indicator_reserved;
        let max_hint_x = if info_width > 0 {
            bounds
                .x
                .saturating_add(bounds.width)
                .saturating_sub(right_margin)
        } else {
            bounds.x.saturating_add(bounds.width)
        };

        if !self.keybinding_hints.is_empty() {
            let combo_style = Style::default()
                .fg(color_to_ratatui(theme.menu_selected_fg))
                .bg(color_to_ratatui(theme.menu_selected_bg))
                .add_modifier(ratatui::style::Modifier::BOLD);
            let mut cursor_x = bounds.x;
            self.hint_rects.clear();
            for (action, combos) in &self.keybinding_hints {
                if cursor_x >= max_hint_x {
                    break;
                }
                let combo_str = combos.join("/");
                let entry = format!("{combo_str} {action}");
                let entry_width = entry.chars().count() as u16;

                if cursor_x.saturating_add(entry_width) > max_hint_x && cursor_x > bounds.x {
                    break;
                }

                let available_w = max_hint_x.saturating_sub(cursor_x);
                if cursor_x == bounds.x && entry_width > available_w {
                    let text = truncate_to_width(&entry, available_w as usize);
                    safe_set_string(buffer, bounds, cursor_x, area.y as u16, &text, style);
                    self.hint_rects.push((
                        LayoutRect {
                            x: i32::from(cursor_x),
                            y: area.y,
                            width: available_w,
                            height: 1,
                        },
                        action.clone(),
                    ));
                    break;
                }

                self.hint_rects.push((
                    LayoutRect {
                        x: i32::from(cursor_x),
                        y: area.y,
                        width: entry_width,
                        height: 1,
                    },
                    action.clone(),
                ));

                safe_set_string(
                    buffer,
                    bounds,
                    cursor_x,
                    area.y as u16,
                    &combo_str,
                    combo_style,
                );
                cursor_x = cursor_x.saturating_add(combo_str.chars().count() as u16);
                let desc = format!(" {action}");
                safe_set_string(buffer, bounds, cursor_x, area.y as u16, &desc, style);
                cursor_x = cursor_x.saturating_add(desc.chars().count() as u16);

                if cursor_x < max_hint_x {
                    safe_set_string(
                        buffer,
                        bounds,
                        cursor_x,
                        area.y as u16,
                        "|",
                        Style::default(),
                    );
                    cursor_x = cursor_x.saturating_add(1);
                }
            }
        }

        if let Some(ref info) = info_opt {
            let text = truncate_to_width(
                info,
                (bounds.width.saturating_sub(indicator_reserved)) as usize,
            );
            let text_width = text.chars().count() as u16;
            let available = bounds.width.saturating_sub(indicator_reserved);
            let start_x = if text_width >= available {
                bounds.x
            } else {
                bounds
                    .x
                    .saturating_add(bounds.width)
                    .saturating_sub(indicator_reserved)
                    .saturating_sub(text_width)
            };
            safe_set_string(
                buffer,
                bounds,
                start_x.max(bounds.x),
                area.y as u16,
                &text,
                style,
            );
        }

        // Draw profile indicator in the reserved rightmost cell
        let ind_x = bounds.x.saturating_add(bounds.width).saturating_sub(1);
        if ind_x >= bounds.x
            && let Some(cell) = buffer.cell_mut((ind_x, area.y as u16))
        {
            let mut st = cell.style();
            st.bg = Some(color_to_ratatui(self.power_profile.indicator_color(theme)));
            cell.set_style(st);
            cell.set_symbol(" ");
        }
    }

    pub fn hit_test_hint(&self, column: u16, row: u16) -> Option<TermWmAction> {
        for (rect, action) in &self.hint_rects {
            if rect_contains(*rect, column, row) {
                return Some(action.clone());
            }
        }
        None
    }
}

impl Component<TermWmAction> for WmBottomPanelComponent {
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
        let theme = &ctx.config().theme;
        self.area = area;
        self.render_bottom_impl(backend, true, theme);
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
        if let Some(action) = self.hit_test_hint(column, row) {
            return EventResult::Action(action);
        }
        EventResult::Ignored
    }
}

impl WmComponent for WmBottomPanelComponent {
    fn consume_area(&mut self, available: LayoutRect) -> (LayoutRect, LayoutRect) {
        self.split_bottom_area(available, 1)
    }

    fn process_action(&mut self, action: &ComponentAction) {
        match action {
            ComponentAction::SetKeybindingHints(hints) => {
                self.set_keybinding_hints(hints.clone());
            }
            ComponentAction::SetPowerProfile(profile) => {
                self.set_power_profile(*profile);
            }
            _ => {}
        }
    }

    fn query(&self, query: &ComponentQuery) -> ComponentResponse {
        match query {
            ComponentQuery::KeybindingHints => {
                ComponentResponse::Hints(self.keybinding_hints.to_vec())
            }
            _ => ComponentResponse::None,
        }
    }

    fn hit_test(&self, x: u16, y: u16) -> bool {
        rect_contains(self.area, x, y)
    }

    fn begin_frame(&mut self) {
        self.begin_frame();
    }
}

impl Default for WmBottomPanelComponent {
    fn default() -> Self {
        Self::new("unknown", "0.0.0", None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;

    #[test]
    fn bottom_panel_renders_provided_hostname() {
        let mut p = WmBottomPanelComponent::new("app", "1.0", Some("my-machine"));
        assert_eq!(p.hostname, Some("my-machine".to_string()));

        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        p.area = area;
        let ratatui_area = layout_rect_to_clipped_rect(area);
        let buf = Buffer::empty(ratatui_area);
        let mut backend = term_wm_console::RatatuiBackend::new(buf, ratatui_area);

        p.render_bottom_impl(&mut backend, true, &term_wm_core::theme::NOIR);

        let mut rendered = String::new();
        for xx in ratatui_area.x..ratatui_area.x.saturating_add(ratatui_area.width) {
            let cell = backend
                .buffer
                .cell((xx, ratatui_area.y))
                .expect("cell present");
            rendered.push_str(cell.symbol());
        }
        assert!(
            rendered.contains("my-machine"),
            "bottom bar should include hostname"
        );
    }

    #[test]
    fn bottom_panel_fills_background_and_right_aligns_text() {
        let mut p = WmBottomPanelComponent::new("test", "0.0.1", Some("h"));
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 30,
            height: 1,
        };
        p.area = area;
        let ratatui_area = layout_rect_to_clipped_rect(area);
        let buf = Buffer::empty(ratatui_area);
        let mut backend = term_wm_console::RatatuiBackend::new(buf, ratatui_area);

        p.render_bottom_impl(&mut backend, true, &term_wm_core::theme::NOIR);

        // All cells except the rightmost indicator cell have panel bg
        let last_x = ratatui_area
            .x
            .saturating_add(ratatui_area.width)
            .saturating_sub(1);
        for xx in ratatui_area.x..last_x {
            let cell = backend
                .buffer
                .cell_mut((xx, ratatui_area.y))
                .expect("cell present");
            assert_eq!(
                cell.style().bg,
                Some(color_to_ratatui(term_wm_core::theme::NOIR.bottom_panel_bg))
            );
            assert_eq!(
                cell.style().fg,
                Some(color_to_ratatui(term_wm_core::theme::NOIR.bottom_panel_fg))
            );
        }

        let mut found = false;
        for dx in (0..ratatui_area.width).rev() {
            let cell = backend
                .buffer
                .cell((ratatui_area.x + dx, ratatui_area.y))
                .expect("cell present");
            if !cell.symbol().trim().is_empty() {
                found = true;
                break;
            }
        }
        assert!(found, "expected non-space text in bottom row");
    }

    #[test]
    fn bottom_panel_includes_app_name_and_version() {
        let mut p = WmBottomPanelComponent::new("my-app", "2.0.0", Some("my-host"));
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        p.area = area;
        let ratatui_area = layout_rect_to_clipped_rect(area);
        let buf = Buffer::empty(ratatui_area);
        let mut backend = term_wm_console::RatatuiBackend::new(buf, ratatui_area);

        p.render_bottom_impl(&mut backend, true, &term_wm_core::theme::NOIR);

        let mut rendered = String::new();
        for xx in ratatui_area.x..ratatui_area.x.saturating_add(ratatui_area.width) {
            let cell = backend
                .buffer
                .cell((xx, ratatui_area.y))
                .expect("cell present");
            rendered.push_str(cell.symbol());
        }
        assert!(rendered.contains("my-app"));
        assert!(rendered.contains("2.0.0"));
        assert!(rendered.contains("my-host"));
    }
}
