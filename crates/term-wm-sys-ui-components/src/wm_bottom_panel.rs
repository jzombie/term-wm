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
use unicode_width::UnicodeWidthStr;

/// Columns between the keybinding-hint row and the right-aligned info line.
const INFO_HINT_GAP: u16 = 2;

/// Segment separator in the info line (` · `): 3 display columns.
const INFO_SEPARATOR: &str = " \u{00b7} ";

/// Info segments drop lowest-value-first when horizontal space runs out.
/// Indices refer to the display-order slot array
/// `[app+version, platform, environment, hostname]`: the static platform
/// goes first, then the app/version label, then the hostname; the tiny,
/// high-signal environment segment survives longest. Absent slots are
/// skipped during the walk.
const INFO_DROP_ORDER: [usize; 4] = [1, 0, 3, 2];

/// Display columns of one info segment (terminal cells, not bytes).
fn info_slot_cols(s: &str) -> u16 {
    UnicodeWidthStr::width(s) as u16
}

/// Total rendered columns of the surviving slots after `drops` applications
/// of [`INFO_DROP_ORDER`], including separators between neighbors. Returns
/// `0` when every slot is dropped or absent, so callers can suppress a
/// zero-width draw outright.
fn info_slots_width(slots: &[Option<&str>; 4], drops: usize) -> u16 {
    let is_dropped = |slot_idx: usize| INFO_DROP_ORDER.iter().take(drops).any(|&i| i == slot_idx);
    let mut width = 0u16;
    let mut emitted = 0usize;
    for (idx, seg) in slots.iter().enumerate() {
        let Some(text) = seg.filter(|_| !is_dropped(idx)) else {
            continue;
        };
        if emitted > 0 {
            width = width.saturating_add(info_slot_cols(INFO_SEPARATOR));
        }
        width = width.saturating_add(info_slot_cols(text));
        emitted += 1;
    }
    width
}

/// Widest-first search over degradation tiers: returns the smallest number
/// of drops (0..=`INFO_DROP_ORDER.len()`) under which the FULL keybinding
/// hint row still fits alongside the info line, or `None` when nothing
/// fits or only a zero-width tier remains (complete suppression).
///
/// The fit check is additive with saturating arithmetic so narrow or
/// resizing terminals can never trigger an unsigned-subtraction panic:
/// `total_full_hints_w + tier_w + gap + indicator <= bounds_w`.
fn best_info_drops(
    slots: &[Option<&str>; 4],
    total_full_hints_w: u16,
    bounds_w: u16,
    indicator_reserved: u16,
) -> Option<usize> {
    for drops in 0..=INFO_DROP_ORDER.len() {
        let tier_w = info_slots_width(slots, drops);
        if tier_w == 0 {
            return None;
        }
        let required_w = total_full_hints_w
            .saturating_add(tier_w)
            .saturating_add(INFO_HINT_GAP)
            .saturating_add(indicator_reserved);
        if required_w <= bounds_w {
            return Some(drops);
        }
    }
    None
}

#[derive(Debug)]
pub struct WmBottomPanelComponent {
    area: LayoutRect,
    /// Precomputed `{app_name} {app_version}` label so the render path never
    /// formats strings per frame.
    app_label: String,
    hostname: Option<String>,
    environment: Option<String>,
    keybinding_hints: Vec<(TermWmAction, Vec<String>)>,
    hint_rects: Vec<(LayoutRect, TermWmAction)>,
    power_profile: PowerProfile,
    hitbox_id: HitboxId,
}

impl WmBottomPanelComponent {
    pub fn new(app_name: &str, app_version: &str, hostname: Option<&str>) -> Self {
        Self {
            area: LayoutRect::default(),
            app_label: format!("{app_name} {app_version}"),
            hostname: hostname.map(|h| h.to_string()),
            environment: None,
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

    /// Show the active runtime environment in the info segment (e.g. `dev`
    /// / `prod` / `test`). Omitted until set so library embedders are
    /// unaffected.
    pub fn set_environment(&mut self, environment: &str) {
        self.environment = Some(environment.to_string());
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
                    cell.set_symbol(" ");
                }
            }
        }
        let style = Style::default()
            .fg(color_to_ratatui(theme.bottom_panel_fg))
            .bg(color_to_ratatui(theme.bottom_panel_bg));

        // Reserve rightmost cell for the profile indicator
        let indicator_reserved = 1u16;

        // Info segment slots in display order. All borrows; nothing is
        // formatted or cloned per frame (app_label is precomputed at
        // construction).
        let info_slots: [Option<&str>; 4] = [
            Some(self.app_label.as_str()),
            Some(std::env::consts::OS),
            self.environment.as_deref(),
            Some(self.hostname.as_deref().unwrap_or("unknown-host")),
        ];

        let total_full_hints_w: u16 = self
            .keybinding_hints
            .iter()
            .map(|(action, combos)| {
                let combo_str = combos.join("/");
                let combo_w = UnicodeWidthStr::width(combo_str.as_str()) as u16;
                let action_str = format!(" {}", action);
                let action_w = UnicodeWidthStr::width(action_str.as_str()) as u16;
                combo_w + action_w + 1
            })
            .sum();

        // Widest-first degradation: keep the fullest info tier under which
        // the full hint row still fits; drop segments lowest-value-first
        // until it does. `None` = suppress the info line entirely so hints
        // reclaim everything except the indicator cell.
        let chosen_drops = if show_info {
            best_info_drops(
                &info_slots,
                total_full_hints_w,
                bounds.width,
                indicator_reserved,
            )
        } else {
            None
        };
        let actual_info_width = chosen_drops
            .map(|drops| info_slots_width(&info_slots, drops))
            .unwrap_or(0);

        let right_margin = if actual_info_width > 0 {
            actual_info_width
                .saturating_add(INFO_HINT_GAP)
                .saturating_add(indicator_reserved)
        } else {
            indicator_reserved
        };
        let max_hint_x = bounds
            .x
            .saturating_add(bounds.width)
            .saturating_sub(right_margin);

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
                let combo_cols = UnicodeWidthStr::width(combo_str.as_str()) as u16;

                if cursor_x.saturating_add(combo_cols) > max_hint_x {
                    break;
                }

                let entry_start = cursor_x;

                safe_set_string(
                    buffer,
                    bounds,
                    cursor_x,
                    area.y as u16,
                    &combo_str,
                    combo_style,
                );
                cursor_x = cursor_x.saturating_add(combo_cols);

                let remaining = max_hint_x.saturating_sub(cursor_x);
                if remaining > 0 {
                    let desc = format!(" {action}");
                    let desc_cols = UnicodeWidthStr::width(desc.as_str()) as u16;
                    let display_desc = if desc_cols <= remaining {
                        desc
                    } else {
                        truncate_to_width(&desc, remaining as usize)
                    };
                    safe_set_string(
                        buffer,
                        bounds,
                        cursor_x,
                        area.y as u16,
                        &display_desc,
                        style,
                    );
                    cursor_x = cursor_x
                        .saturating_add(UnicodeWidthStr::width(display_desc.as_str()) as u16);
                }

                if cursor_x.saturating_add(1) <= max_hint_x {
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

                self.hint_rects.push((
                    LayoutRect {
                        x: i32::from(entry_start),
                        y: area.y,
                        width: cursor_x.saturating_sub(entry_start),
                        height: 1,
                    },
                    action.clone(),
                ));
            }
        }

        if let Some(drops) = chosen_drops {
            let total_w = info_slots_width(&info_slots, drops);
            if total_w > 0 {
                // Right-align inside bounds minus the reserved indicator
                // cell; segments and separators are written piecewise (no
                // joined String). The tier fit guarantees the whole line
                // fits, so no per-segment truncation is needed.
                let avail_w = bounds.width.saturating_sub(indicator_reserved);
                let mut cursor = bounds
                    .x
                    .saturating_add(avail_w.saturating_sub(total_w.min(avail_w)));
                let is_dropped =
                    |slot_idx: usize| INFO_DROP_ORDER.iter().take(drops).any(|&i| i == slot_idx);
                let mut emitted = 0usize;
                for (idx, seg) in info_slots.iter().enumerate() {
                    let Some(text) = seg.filter(|_| !is_dropped(idx)) else {
                        continue;
                    };
                    if emitted > 0 && cursor < bounds.x.saturating_add(avail_w) {
                        safe_set_string(
                            buffer,
                            bounds,
                            cursor,
                            area.y as u16,
                            INFO_SEPARATOR,
                            style,
                        );
                        cursor = cursor.saturating_add(info_slot_cols(INFO_SEPARATOR));
                    }
                    safe_set_string(buffer, bounds, cursor, area.y as u16, text, style);
                    cursor = cursor.saturating_add(info_slot_cols(text));
                    emitted += 1;
                }
            }
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

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use term_wm_console::RatatuiBackend;
    use term_wm_core::theme::NOIR;

    fn make_panel(hints: Vec<(TermWmAction, Vec<String>)>) -> WmBottomPanelComponent {
        let mut p = WmBottomPanelComponent::new("term-wm", "0.1.0", Some("test-host"));
        p.set_keybinding_hints(hints);
        p
    }

    fn render_at_width(
        p: &mut WmBottomPanelComponent,
        width: u16,
        show_info: bool,
    ) -> RatatuiBackend {
        let area = LayoutRect {
            x: 0,
            y: 0,
            width,
            height: 1,
        };
        p.area = area;
        let ratatui_area = layout_rect_to_clipped_rect(area);
        let buf = Buffer::empty(ratatui_area);
        let mut backend = term_wm_console::RatatuiBackend::new_simple(buf, ratatui_area);
        p.render_bottom_impl(&mut backend, show_info, &NOIR);
        backend
    }

    fn collect_rendered(backend: &RatatuiBackend) -> String {
        let ratatui_area = layout_rect_to_clipped_rect(LayoutRect {
            x: 0,
            y: 0,
            width: backend.buffer.area.width,
            height: 1,
        });
        let mut s = String::new();
        for xx in ratatui_area.x..ratatui_area.x.saturating_add(ratatui_area.width) {
            if let Some(cell) = backend.buffer.cell((xx, ratatui_area.y)) {
                s.push_str(cell.symbol());
            }
        }
        s
    }

    fn default_hints() -> Vec<(TermWmAction, Vec<String>)> {
        vec![
            (TermWmAction::NewTerminal, vec!["Ctrl+N".into()]),
            (TermWmAction::FocusNext, vec!["Alt+Tab".into()]),
            (TermWmAction::OpenHelp, vec!["F1".into()]),
        ]
    }

    // --- Progressive degradation tests ---

    #[test]
    fn level_0_full_hints_with_info() {
        let mut p = make_panel(default_hints());
        let backend = render_at_width(&mut p, 120, true);
        let rendered = collect_rendered(&backend);

        assert!(rendered.contains("Ctrl+N New Terminal"));
        assert!(rendered.contains("Alt+Tab Focus Next"));
        assert!(rendered.contains("F1 Open Help"));
        assert!(rendered.contains("term-wm 0.1.0"), "info section rendered");
    }

    #[test]
    fn level_1_info_suppressed_to_fit_hints() {
        let mut p = make_panel(default_hints());
        // 68 cols: hints need ~56 cols, info needs ~33 cols -> doesn't fit together
        let backend = render_at_width(&mut p, 68, true);
        let rendered = collect_rendered(&backend);

        assert!(
            rendered.contains("Ctrl+N") || rendered.contains("Alt+Tab"),
            "hints rendered after info suppression"
        );
        assert!(
            !rendered.contains("term-wm 0.1.0"),
            "info section suppressed"
        );
    }

    #[test]
    fn level_2_descriptions_truncated_combos_atomic() {
        let mut p = make_panel(vec![
            (TermWmAction::NewTerminal, vec!["Ctrl+Shift+N".into()]),
            (TermWmAction::FocusNext, vec!["Alt+Tab".into()]),
        ]);
        // 40 cols (hint area = 39): both combos fit, but second description truncates
        let backend = render_at_width(&mut p, 40, false);
        let rendered = collect_rendered(&backend);

        assert!(
            rendered.contains("Ctrl+Shift+N"),
            "first combo fully present, not truncated"
        );
        assert!(rendered.contains("Alt+Tab"), "second combo fully present");
    }

    #[test]
    fn combo_never_truncated_on_boundary() {
        let mut p = make_panel(vec![
            (TermWmAction::NewTerminal, vec!["Ctrl+N".into()]),
            (TermWmAction::FocusNext, vec!["X".into()]),
        ]);
        // 14 cols: exactly fits "Ctrl+N" + "|" + "X", combo must be atomic
        let backend = render_at_width(&mut p, 14, false);
        let rendered = collect_rendered(&backend);

        assert!(
            rendered.contains("Ctrl+N"),
            "first combo fully rendered, not truncated"
        );
    }

    #[test]
    fn no_hints_when_too_narrow() {
        let mut p = make_panel(default_hints());
        let backend = render_at_width(&mut p, 6, false);
        let rendered = collect_rendered(&backend);

        assert!(
            !rendered.contains("Ctrl+N"),
            "no hints rendered when combos don't fit"
        );
    }

    #[test]
    fn combo_highlighting_preserved_after_truncation() {
        let mut p = make_panel(default_hints());
        // 25 cols: combo fits but description is severely truncated or omitted
        let backend = render_at_width(&mut p, 25, false);
        let ratatui_area = layout_rect_to_clipped_rect(LayoutRect {
            x: 0,
            y: 0,
            width: 25,
            height: 1,
        });

        // First non-space cell should have combo_style (green bg)
        let mut found_combo_cell = false;
        for xx in ratatui_area.x..ratatui_area.x.saturating_add(ratatui_area.width) {
            if let Some(cell) = backend.buffer.cell((xx, ratatui_area.y))
                && cell.symbol() != " "
            {
                assert_eq!(
                    cell.style().bg,
                    Some(color_to_ratatui(NOIR.menu_selected_bg)),
                    "first non-space cell should have combo highlighting"
                );
                found_combo_cell = true;
                break;
            }
        }
        assert!(found_combo_cell, "expected at least one rendered hint cell");
    }

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
        let mut backend = term_wm_console::RatatuiBackend::new_simple(buf, ratatui_area);

        p.render_bottom_impl(&mut backend, true, &NOIR);

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
    fn info_slots_width_skips_none_environment() {
        // Full line with env present: "app 1.0 · macos · prod · h"
        let slots: [Option<&str>; 4] = [Some("app 1.0"), Some("macos"), Some("prod"), Some("h")];
        assert_eq!(info_slots_width(&slots, 0), 7 + 3 + 5 + 3 + 4 + 3 + 1);
        // Drop platform (drops=1): "app 1.0 · prod · h" — no phantom
        // separator where the absent segment used to be.
        assert_eq!(info_slots_width(&slots, 1), 7 + 3 + 4 + 3 + 1);

        let no_env: [Option<&str>; 4] = [Some("app 1.0"), Some("macos"), None, Some("h")];
        assert_eq!(info_slots_width(&no_env, 0), 7 + 3 + 5 + 3 + 1);
        assert_eq!(info_slots_width(&no_env, 1), 7 + 3 + 1);
    }

    #[test]
    fn best_info_drops_prefers_full_then_degrades() {
        let slots: [Option<&str>; 4] = [
            Some("app 1.0"),
            Some("macos"),
            Some("prod"),
            Some("my-machine"),
        ];
        // Generous bounds: nothing dropped.
        assert_eq!(best_info_drops(&slots, 0, 80, 1), Some(0));
        // Bounds that only fit after the platform segment is dropped.
        let full = info_slots_width(&slots, 0);
        let tier1 = info_slots_width(&slots, 1);
        let squeezed = full.saturating_add(INFO_HINT_GAP).saturating_add(1) - 1;
        assert!(squeezed >= tier1.saturating_add(INFO_HINT_GAP).saturating_add(1));
        let drops = best_info_drops(&slots, 0, squeezed, 1).expect("tier fits");
        assert!(drops >= 1, "full tier must not fit at {squeezed} cols");
        // Tiny bounds: complete suppression.
        assert_eq!(best_info_drops(&slots, 0, 5, 1), None);
    }

    #[test]
    fn best_info_drops_never_panics_on_degenerate_bounds() {
        let slots: [Option<&str>; 4] = [
            Some("app 1.0"),
            Some("macos"),
            Some("prod"),
            Some("my-machine"),
        ];
        for bounds_w in [0u16, 1u16, u16::MAX] {
            // Non-zero hint totals exercise saturation at arithmetic
            // extremes; must not panic and must resolve deterministically.
            let _ = best_info_drops(&slots, u16::MAX, bounds_w, 1);
            let _ = best_info_drops(&slots, 12, bounds_w, 1);
        }
        // Saturation behavior at the extremes is still correct. At
        // u16::MAX hints + u16::MAX bounds the saturating sum clamps to
        // bounds exactly, so the tier counts as fitting; real hint totals
        // are orders of magnitude smaller, so this only proves determinism.
        assert_eq!(best_info_drops(&slots, u16::MAX, u16::MAX, 1), Some(0));
        assert_eq!(
            best_info_drops(&slots, 12, u16::MAX, 1),
            Some(0),
            "huge bounds always keep the fullest tier"
        );
    }

    #[test]
    fn bottom_panel_degrades_segments_by_priority() {
        // Long app label so the full tier cannot fit a 50-col panel but the
        // platform-less tier can.
        let mut p = WmBottomPanelComponent::new("terminal-wm-app", "10.10.10", Some("my-machine"));
        p.set_environment("prod");
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 50,
            height: 1,
        };
        p.area = area;
        let ratatui_area = layout_rect_to_clipped_rect(area);
        let buf = Buffer::empty(ratatui_area);
        let mut backend = term_wm_console::RatatuiBackend::new_simple(buf, ratatui_area);
        p.render_bottom_impl(&mut backend, true, &NOIR);
        let rendered = collect_row_symbols(&mut backend, ratatui_area);
        assert!(rendered.contains("prod"), "env survives: {rendered}");
        assert!(rendered.contains("my-machine"), "host survives: {rendered}");
        assert!(
            !rendered.contains(std::env::consts::OS),
            "platform must be the first segment dropped: {rendered}"
        );
    }

    #[test]
    fn bottom_panel_drops_all_info_when_pinned() {
        let mut p = WmBottomPanelComponent::new("terminal-wm-app", "10.10.10", Some("my-machine"));
        p.set_environment("prod");
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 6,
            height: 1,
        };
        p.area = area;
        let ratatui_area = layout_rect_to_clipped_rect(area);
        let buf = Buffer::empty(ratatui_area);
        let mut backend = term_wm_console::RatatuiBackend::new_simple(buf, ratatui_area);
        p.render_bottom_impl(&mut backend, true, &NOIR);
        let rendered = collect_row_symbols(&mut backend, ratatui_area);
        assert!(
            !rendered.contains("prod") && !rendered.contains("my-machine"),
            "no info segment may render on a pinned panel: {rendered}"
        );
        // Background fill stays intact on every non-indicator cell; the
        // reserved rightmost cell keeps its power-profile color.
        for xx in ratatui_area.x..ratatui_area.width.saturating_sub(1) {
            let cell = backend.buffer.cell((xx, 0)).expect("cell present");
            assert_eq!(
                cell.style().bg,
                Some(color_to_ratatui(NOIR.bottom_panel_bg))
            );
        }
        let ind_x = ratatui_area.width.saturating_sub(1);
        let cell = backend.buffer.cell((ind_x, 0)).expect("cell present");
        assert_eq!(
            cell.style().bg,
            Some(color_to_ratatui(p.power_profile.indicator_color(&NOIR)))
        );
    }

    #[test]
    fn bottom_panel_omits_environment_until_set() {
        let mut p = WmBottomPanelComponent::new("app", "1.0", Some("host"));
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        p.area = area;
        let ratatui_area = layout_rect_to_clipped_rect(area);
        let buf = Buffer::empty(ratatui_area);
        let mut backend = term_wm_console::RatatuiBackend::new_simple(buf, ratatui_area);
        p.render_bottom_impl(&mut backend, true, &NOIR);
        let rendered = collect_row_symbols(&mut backend, ratatui_area);
        assert!(
            !rendered.contains(" dev "),
            "unset environment must be omitted from the info line: {rendered}"
        );

        p.set_environment("prod");
        let buf = Buffer::empty(ratatui_area);
        let mut backend = term_wm_console::RatatuiBackend::new_simple(buf, ratatui_area);
        p.render_bottom_impl(&mut backend, true, &NOIR);
        let rendered = collect_row_symbols(&mut backend, ratatui_area);
        assert!(
            rendered.contains("\u{00b7} prod \u{00b7}"),
            "bottom bar should include the environment segment: {rendered}"
        );
    }

    /// Collect the rendered symbols of the panel row into one string.
    fn collect_row_symbols(
        backend: &mut term_wm_console::RatatuiBackend,
        area: ratatui::layout::Rect,
    ) -> String {
        let mut rendered = String::new();
        for xx in area.x..area.x.saturating_add(area.width) {
            let cell = backend.buffer.cell((xx, area.y)).expect("cell present");
            rendered.push_str(cell.symbol());
        }
        rendered
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
        let mut backend = term_wm_console::RatatuiBackend::new_simple(buf, ratatui_area);

        p.render_bottom_impl(&mut backend, true, &NOIR);

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
                Some(color_to_ratatui(NOIR.bottom_panel_bg))
            );
            assert_eq!(
                cell.style().fg,
                Some(color_to_ratatui(NOIR.bottom_panel_fg))
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
        let mut backend = term_wm_console::RatatuiBackend::new_simple(buf, ratatui_area);

        p.render_bottom_impl(&mut backend, true, &NOIR);

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

    #[test]
    fn render_overlays_does_not_clobber_command_palette_hints_in_monocle() {
        use std::sync::Arc;

        use term_wm_console::draw_plan_renderer::render_overlays;
        use term_wm_core::app_context::AppContext;
        use term_wm_core::components::NoopComponent;
        use term_wm_core::components::NoopOverlay;
        use term_wm_core::config::AppBuilder;
        use term_wm_core::window::ComponentTag;

        let app_ctx = Arc::new(AppContext::new("test", "0.1.0"));
        let mut wm = AppBuilder::<WmBottomPanelComponent>::new()
            .app_ctx(Arc::clone(&app_ctx))
            .bottom_panel(WmBottomPanelComponent::new(
                "test",
                "0.1.0",
                Some("test-host"),
            ))
            .build::<NoopComponent, NoopOverlay>()
            .expect("test build");

        // Enter cramped monocle (default Auto + narrow viewport) and open the
        // command palette, which sets input_mode = CommandPalette.
        wm.update_monocle_mode(40);
        assert!(wm.is_monocle_cramped(), "test must run in cramped monocle");
        wm.open_command_palette_overlay(NoopOverlay);
        assert!(wm.command_menu_visible());

        let area = term_wm_core::Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 24,
        };
        wm.register_managed_layout(area);

        let ratatui_area = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 24,
        };
        let buf = Buffer::empty(ratatui_area);
        let mut backend = term_wm_console::RatatuiBackend::new_simple(buf, ratatui_area);
        render_overlays(&mut backend, &mut wm);

        // The render pass must NOT have replaced the layout-phase (filtered)
        // hints. With the palette open, Global actions like OpenCommandPalette
        // must be absent and CommandPalette-layer actions present.
        let hints = wm
            .get_semantic_component(ComponentTag::BottomPanel)
            .map(|p| p.keybinding_hints().to_vec())
            .expect("bottom panel present");
        let actions: Vec<_> = hints.iter().map(|(a, _)| a.clone()).collect();
        assert!(
            !actions.contains(&TermWmAction::OpenCommandPalette),
            "render_overlays must not clobber palette-filtered hints, got {actions:?}"
        );
        assert!(
            actions.contains(&TermWmAction::FocusNext),
            "palette-filtered hints must include a CommandPalette-layer action, got {actions:?}"
        );
    }

    #[test]
    fn bottom_panel_overlay_stays_on_absolute_bottom_when_fab_row_reserved() {
        use std::sync::Arc;
        use term_wm_console::draw_plan_renderer::render_overlays;
        use term_wm_core::app_context::AppContext;
        use term_wm_core::components::NoopComponent;
        use term_wm_core::components::NoopOverlay;
        use term_wm_core::config::AppBuilder;

        let app_ctx = Arc::new(AppContext::new("test", "0.1.0"));
        let mut wm = AppBuilder::<WmBottomPanelComponent>::new()
            .app_ctx(Arc::clone(&app_ctx))
            .bottom_panel(WmBottomPanelComponent::new(
                "test",
                "0.1.0",
                Some("test-host"),
            ))
            .build::<NoopComponent, NoopOverlay>()
            .expect("test build");

        // Cramped monocle + command palette open + FAB row reserved (app content
        // detected under the FAB footprint) → managed_area is 23 rows tall.
        wm.update_monocle_mode(40);
        assert!(wm.is_monocle_cramped());
        wm.set_bottom_content_flag(true);
        wm.open_command_palette_overlay(NoopOverlay);
        assert!(wm.command_menu_visible());

        let area = term_wm_core::Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 24,
        };
        wm.register_managed_layout(area);
        assert_eq!(wm.managed_area().height, 23, "FAB row must be reserved");

        let ratatui_area = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 24,
        };
        let buf = Buffer::empty(ratatui_area);
        let mut backend = term_wm_console::RatatuiBackend::new_simple(buf, ratatui_area);
        render_overlays(&mut backend, &mut wm);

        // The bottom-panel hints must stay on the ABSOLUTE bottom row (23), NOT
        // rise up to the reserved row above it (22).
        let has_non_space = |row: u16| {
            (0..40).any(|x| {
                backend
                    .buffer
                    .cell((x, row))
                    .is_some_and(|c| !c.symbol().starts_with(' '))
            })
        };
        assert!(
            has_non_space(23),
            "hints must render on absolute bottom row 23"
        );
        assert!(
            !has_non_space(22),
            "hints must NOT rise up to row 22 when the FAB row is reserved"
        );
    }
}
