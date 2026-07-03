use crossterm::event::{Event, MouseEventKind};
use ratatui::{layout::Rect, style::Style};

use term_wm_core::{
    bottom_panel_trait::BottomPanel as BottomPanelTrait,
    components::{Component, ComponentContext},
    keybindings::Action,
    layout::rect_contains,
    power_profile::PowerProfile,
    ui::{UiFrame, safe_set_string, truncate_to_width},
};

pub struct WmBottomPanelComponent {
    area: Rect,
    app_name: String,
    app_version: String,
    hostname: Option<String>,
    keybinding_hints: Vec<(Action, Vec<String>)>,
    hint_rects: Vec<(Rect, Action)>,
    power_profile: PowerProfile,
}

impl WmBottomPanelComponent {
    pub fn new(app_name: &str, app_version: &str, hostname: Option<&str>) -> Self {
        Self {
            area: Rect::default(),
            app_name: app_name.to_string(),
            app_version: app_version.to_string(),
            hostname: hostname.map(|h| h.to_string()),
            keybinding_hints: Vec::new(),
            hint_rects: Vec::new(),
            power_profile: PowerProfile::PowerSaver,
        }
    }

    pub fn begin_frame(&mut self) {
        self.hint_rects.clear();
    }

    pub fn area(&self) -> Rect {
        self.area
    }

    pub fn set_hostname(&mut self, hostname: &str) {
        self.hostname = Some(hostname.to_string());
    }

    pub fn set_keybinding_hints(&mut self, hints: Vec<(Action, Vec<String>)>) {
        self.keybinding_hints = hints;
    }

    pub fn keybinding_hints(&self) -> &[(Action, Vec<String>)] {
        &self.keybinding_hints
    }

    pub fn set_power_profile(&mut self, profile: PowerProfile) {
        self.power_profile = profile;
    }

    pub fn split_bottom_area(&mut self, area: Rect, height: u16) -> (Rect, Rect) {
        let bottom = Rect {
            x: area.x,
            y: area.y.saturating_add(area.height).saturating_sub(height),
            width: area.width,
            height,
        };
        let managed_height = area.height.saturating_sub(height);
        let managed = Rect {
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
        frame: &mut UiFrame<'_>,
        active: bool,
        theme: &term_wm_core::theme::Theme,
    ) {
        if active {
            self.render_bottom_impl(frame, true, theme);
        } else if !self.keybinding_hints.is_empty() {
            self.render_bottom_impl(frame, false, theme);
        }
    }

    fn render_bottom_impl(
        &mut self,
        frame: &mut UiFrame<'_>,
        show_info: bool,
        theme: &term_wm_core::theme::Theme,
    ) {
        let area = self.area;
        if area.width == 0 || area.height == 0 {
            return;
        }
        let buffer = frame.buffer_mut();
        let bounds = area.intersection(buffer.area);
        if bounds.width == 0 || bounds.height == 0 {
            return;
        }
        for yy in bounds.y..bounds.y.saturating_add(bounds.height) {
            for xx in bounds.x..bounds.x.saturating_add(bounds.width) {
                if let Some(cell) = buffer.cell_mut((xx, yy)) {
                    let mut st = cell.style();
                    st.bg = Some(theme.bottom_panel_bg);
                    st.fg = Some(theme.bottom_panel_fg);
                    cell.set_style(st);
                }
            }
        }
        let style = Style::default()
            .fg(theme.bottom_panel_fg)
            .bg(theme.bottom_panel_bg);

        // Reserve rightmost cell for the profile indicator
        let indicator_reserved = 1u16;

        let info_opt = if show_info {
            let platform = std::env::consts::OS;
            let pkg_label = format!("{} {}", self.app_name, self.app_version);
            let hostname = self
                .hostname
                .clone()
                .unwrap_or_else(|| "unknown-host".to_string());
            Some(format!("{pkg_label} · {platform} · {hostname}"))
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
                .fg(theme.menu_selected_fg)
                .bg(theme.menu_selected_bg)
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
                    safe_set_string(buffer, bounds, cursor_x, area.y, &text, style);
                    self.hint_rects.push((
                        Rect {
                            x: cursor_x,
                            y: area.y,
                            width: available_w,
                            height: 1,
                        },
                        *action,
                    ));
                    break;
                }

                self.hint_rects.push((
                    Rect {
                        x: cursor_x,
                        y: area.y,
                        width: entry_width,
                        height: 1,
                    },
                    *action,
                ));

                safe_set_string(buffer, bounds, cursor_x, area.y, &combo_str, combo_style);
                cursor_x = cursor_x.saturating_add(combo_str.chars().count() as u16);
                let desc = format!(" {}", action);
                safe_set_string(buffer, bounds, cursor_x, area.y, &desc, style);
                cursor_x = cursor_x.saturating_add(desc.chars().count() as u16);

                if cursor_x < max_hint_x {
                    safe_set_string(buffer, bounds, cursor_x, area.y, "|", Style::default());
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
            safe_set_string(buffer, bounds, start_x.max(bounds.x), area.y, &text, style);
        }

        // Draw profile indicator in the reserved rightmost cell
        let ind_x = bounds.x.saturating_add(bounds.width).saturating_sub(1);
        if ind_x >= bounds.x
            && let Some(cell) = buffer.cell_mut((ind_x, area.y))
        {
            let mut st = cell.style();
            st.bg = Some(self.power_profile.indicator_color(theme));
            cell.set_style(st);
            cell.set_symbol(" ");
        }
    }

    pub fn hit_test_hint(&self, event: &Event) -> Option<Action> {
        let Event::Mouse(mouse) = event else {
            return None;
        };
        if !matches!(mouse.kind, MouseEventKind::Down(_)) {
            return None;
        }
        for (rect, action) in &self.hint_rects {
            if rect_contains(*rect, mouse.column, mouse.row) {
                return Some(*action);
            }
        }
        None
    }
}

impl BottomPanelTrait for WmBottomPanelComponent {
    fn begin_frame(&mut self) {
        self.begin_frame()
    }

    fn area(&self) -> Rect {
        self.area()
    }

    fn set_keybinding_hints(&mut self, hints: Vec<(Action, Vec<String>)>) {
        self.set_keybinding_hints(hints);
    }

    fn keybinding_hints(&self) -> &[(Action, Vec<String>)] {
        self.keybinding_hints()
    }

    fn split_bottom_area(&mut self, area: Rect, height: u16) -> (Rect, Rect) {
        self.split_bottom_area(area, height)
    }

    fn render(
        &mut self,
        frame: &mut UiFrame<'_>,
        active: bool,
        theme: &term_wm_core::theme::Theme,
    ) {
        self.render(frame, active, theme);
    }

    fn hit_test_hint(&self, event: &Event) -> Option<Action> {
        self.hit_test_hint(event)
    }

    fn set_power_profile(&mut self, profile: PowerProfile) {
        self.set_power_profile(profile);
    }
}

impl Component for WmBottomPanelComponent {
    fn render(&mut self, _frame: &mut UiFrame<'_>, _area: Rect, _ctx: &ComponentContext) {}

    fn handle_event(&mut self, _event: &Event, _ctx: &ComponentContext) -> bool {
        false
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

        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        p.area = area;
        let mut buf = Buffer::empty(area);
        let mut ui = UiFrame::from_parts(area, &mut buf);

        p.render_bottom_impl(&mut ui, true, &term_wm_core::theme::NOIR);

        let mut rendered = String::new();
        for xx in area.x..area.x.saturating_add(area.width) {
            let cell = buf.cell((xx, area.y)).expect("cell present");
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
        let area = Rect {
            x: 0,
            y: 0,
            width: 30,
            height: 1,
        };
        p.area = area;
        let mut buf = Buffer::empty(area);
        let mut ui = UiFrame::from_parts(area, &mut buf);

        p.render_bottom_impl(&mut ui, true, &term_wm_core::theme::NOIR);

        // All cells except the rightmost indicator cell have panel bg
        let last_x = area.x.saturating_add(area.width).saturating_sub(1);
        for xx in area.x..last_x {
            let cell = buf.cell_mut((xx, area.y)).expect("cell present");
            assert_eq!(
                cell.style().bg,
                Some(term_wm_core::theme::NOIR.bottom_panel_bg)
            );
            assert_eq!(
                cell.style().fg,
                Some(term_wm_core::theme::NOIR.bottom_panel_fg)
            );
        }

        let mut found = false;
        for dx in (0..area.width).rev() {
            let cell = buf.cell((area.x + dx, area.y)).expect("cell present");
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
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        p.area = area;
        let mut buf = Buffer::empty(area);
        let mut ui = UiFrame::from_parts(area, &mut buf);

        p.render_bottom_impl(&mut ui, true, &term_wm_core::theme::NOIR);

        let mut rendered = String::new();
        for xx in area.x..area.x.saturating_add(area.width) {
            let cell = buf.cell((xx, area.y)).expect("cell present");
            rendered.push_str(cell.symbol());
        }
        assert!(rendered.contains("my-app"));
        assert!(rendered.contains("2.0.0"));
        assert!(rendered.contains("my-host"));
    }
}
