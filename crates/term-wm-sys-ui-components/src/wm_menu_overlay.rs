use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use ratatui::style::{Modifier, Style};
use ratatui::widgets::Widget as _;
use ratatui::widgets::{Block, Borders};
use term_wm_core::events::{Event, KeyKind, MouseEventKind};
use term_wm_layout_engine::LayoutRect;

use term_wm_core::{
    actions::{EventResult, TermWmAction},
    components::{
        Component, ComponentAction, ComponentContext, ComponentQuery, ComponentResponse, MenuItem,
        Overlay, WmComponent,
    },
    layout::rect_contains,
    window::WindowKey,
};
use term_wm_ui_components::helpers::{color_to_ratatui, layout_rect_to_rect};

use term_wm_ui_components::DialogOverlayComponent;
use term_wm_ui_components::menu::MenuComponent;

pub struct WmMenuOverlay {
    menu: MenuComponent,
    outlined: Cell<bool>,
    outlined_at: RefCell<Option<Instant>>,
    outline_timeout: Duration,
    menu_bounds_cache: Cell<Option<LayoutRect>>,
    item_hits: std::cell::RefCell<Vec<(usize, LayoutRect)>>,
    anchor: Option<(u16, u16)>,
    managed_area: LayoutRect,
    last_action: Option<TermWmAction>,
}

impl std::fmt::Debug for WmMenuOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WmMenuOverlay")
            .field("outlined", &self.outlined.get())
            .field("anchor", &self.anchor)
            .field("managed_area", &self.managed_area)
            .finish_non_exhaustive()
    }
}

impl Default for WmMenuOverlay {
    fn default() -> Self {
        Self::new()
    }
}

impl WmMenuOverlay {
    pub fn new() -> Self {
        Self {
            menu: MenuComponent::new(),
            outlined: Cell::new(false),
            outlined_at: RefCell::new(None),
            outline_timeout: Duration::ZERO,
            menu_bounds_cache: Cell::new(None),
            item_hits: std::cell::RefCell::new(Vec::new()),
            anchor: None,
            managed_area: LayoutRect::default(),
            last_action: None,
        }
    }

    fn auto_restore(&self) {
        if !self.outlined.get() {
            return;
        }
        let expired = self
            .outlined_at
            .borrow()
            .is_some_and(|t| t.elapsed() > self.outline_timeout);
        if expired {
            self.outlined.set(false);
            self.outlined_at.borrow_mut().take();
        }
    }

    pub fn outline(&self) {
        self.outlined.set(true);
        self.outlined_at.replace(Some(Instant::now()));
    }

    pub fn restore(&self) {
        self.outlined.set(false);
        self.outlined_at.take();
    }

    pub fn set_items(&mut self, items: Vec<MenuItem<TermWmAction>>) {
        self.menu.set_items(items);
    }

    pub fn set_anchor(&mut self, pos: Option<(u16, u16)>) {
        self.anchor = pos;
    }

    pub fn set_managed_area(&mut self, area: LayoutRect) {
        self.managed_area = area;
    }

    pub fn selected_action(&self) -> Option<&TermWmAction> {
        self.last_action.as_ref()
    }

    pub fn set_timeout(&mut self, timeout: Duration) {
        self.outline_timeout = timeout;
    }

    fn render_dropdown(
        &self,
        backend: &mut dyn term_wm_render::RenderBackend,
        ctx: &ComponentContext,
    ) {
        let item_count = self.menu.items().len();
        if item_count == 0 {
            return;
        }
        let Some(anchor) = self.anchor else {
            return;
        };
        let bounds_layout = self.managed_area;
        let bounds = layout_rect_to_rect(bounds_layout);
        let start_x = anchor.0;
        let start_y = anchor.1;
        if start_x < bounds.x || start_x >= bounds.x.saturating_add(bounds.width) {
            return;
        }
        let max_width = bounds
            .width
            .saturating_sub(start_x.saturating_sub(bounds.x))
            .max(1);
        let label_width = self
            .menu
            .items()
            .iter()
            .map(|item| item.label.chars().count() as u16)
            .max()
            .unwrap_or(1);
        let icon_width = self
            .menu
            .items()
            .iter()
            .map(|item| item.icon.map(|v| v.chars().count() as u16).unwrap_or(0))
            .max()
            .unwrap_or(0);
        let width = (label_width + icon_width + 6).min(max_width);
        let max_height = bounds
            .height
            .saturating_sub(start_y.saturating_sub(bounds.y))
            .max(1);
        let height = (item_count as u16).saturating_add(2).min(max_height);

        let drop_rect_layout = LayoutRect {
            x: i32::from(start_x),
            y: i32::from(start_y),
            width,
            height,
        };

        self.menu_bounds_cache.set(Some(drop_rect_layout));

        self.render_backdrop(backend, self.managed_area, drop_rect_layout);

        let drop_rect = layout_rect_to_rect(drop_rect_layout);
        {
            let ratatui_backend = term_wm_ui_components::helpers::downcast_ratatui(backend);
            let buffer = &ratatui_backend.buffer;
            let clip = drop_rect.intersection(buffer.area);
            if clip.width == 0 || clip.height == 0 {
                return;
            }

            let hovered_idx = ctx.hover_pos().and_then(|(mx, my)| {
                (my >= drop_rect.y.saturating_add(1)
                    && my
                        < drop_rect
                            .y
                            .saturating_add((item_count as u16).saturating_add(1))
                    && mx >= drop_rect.x
                    && mx < drop_rect.x.saturating_add(drop_rect.width))
                .then(|| (my.saturating_sub(drop_rect.y).saturating_sub(1)) as usize)
                .filter(|&idx| idx < item_count)
            });
            self.menu.render_items(
                &mut ratatui_backend.buffer,
                drop_rect,
                hovered_idx,
                &ctx.config().theme,
            );
        }

        {
            let ratatui_backend = term_wm_ui_components::helpers::downcast_ratatui(backend);
            let buffer = &ratatui_backend.buffer;
            let clip = drop_rect.intersection(buffer.area);
            let mut hits = self.item_hits.borrow_mut();
            hits.clear();
            for idx in 0..item_count.min((drop_rect.height.saturating_sub(1)) as usize) {
                let y = drop_rect.y.saturating_add(idx as u16 + 1);
                if y < clip.y || y >= clip.y.saturating_add(clip.height) {
                    break;
                }
                hits.push((
                    idx,
                    LayoutRect {
                        x: i32::from(drop_rect.x),
                        y: i32::from(y),
                        width: drop_rect.width,
                        height: 1,
                    },
                ));
            }
        }
    }

    fn render_outline(&self, backend: &mut dyn term_wm_render::RenderBackend) {
        let Some(menu_bounds) = self.menu_bounds_cache.get() else {
            return;
        };
        let ratatui_backend = term_wm_ui_components::helpers::downcast_ratatui(backend);
        let buffer = &mut ratatui_backend.buffer;
        let ratatui_menu_bounds = layout_rect_to_rect(menu_bounds);
        let clip = ratatui_menu_bounds.intersection(buffer.area);
        if clip.width > 0 && clip.height > 0 {
            let dim_style = Style::default().add_modifier(Modifier::DIM);
            for y in clip.y..clip.y.saturating_add(clip.height) {
                for x in clip.x..clip.x.saturating_add(clip.width) {
                    if let Some(cell) = buffer.cell_mut((x, y)) {
                        cell.set_style(dim_style);
                    }
                }
            }
        }
        let block = Block::default().borders(Borders::ALL).border_style(
            Style::default()
                .fg(color_to_ratatui(term_wm_core::theme::NOIR.menu_fg))
                .add_modifier(Modifier::DIM),
        );
        block.render(ratatui_menu_bounds, buffer);
    }

    fn render_backdrop(
        &self,
        backend: &mut dyn term_wm_render::RenderBackend,
        bounds: LayoutRect,
        exclude: LayoutRect,
    ) {
        let dialog = DialogOverlayComponent::default();
        dialog.render_backdrop(backend, bounds, Some(exclude));
    }

    fn hit_test_item(&self, column: u16, row: u16) -> Option<usize> {
        self.item_hits
            .borrow()
            .iter()
            .find(|(_, rect)| rect_contains(*rect, column, row))
            .map(|(idx, _)| *idx)
    }
}

impl Component<TermWmAction> for WmMenuOverlay {
    fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        _area: LayoutRect,
        ctx: &ComponentContext,
        _registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        self.auto_restore();
        if self.outlined.get() {
            self.render_outline(backend);
        } else {
            self.render_dropdown(backend, ctx);
        }
    }

    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        self.auto_restore();
        self.last_action = None;

        if let Event::Mouse(mouse) = event
            && matches!(mouse.kind, MouseEventKind::Press(_))
        {
            if let Some(idx) = self.hit_test_item(mouse.column, mouse.row) {
                self.menu.set_selected(idx);
                self.restore();
                self.last_action = self.menu.selected_action().cloned();
                return EventResult::Consumed;
            }
            return EventResult::Ignored;
        }

        let Event::Key(key) = event else {
            return EventResult::Ignored;
        };
        if key.kind != KeyKind::Press {
            return EventResult::Ignored;
        }

        let total = self.menu.items().len();
        if total == 0 {
            return EventResult::Ignored;
        }

        let kb = ctx.keybindings().unwrap_or_default();
        if kb.matches(TermWmAction::MenuUp, key) || kb.matches(TermWmAction::MenuPrev, key) {
            let current = self.menu.selected();
            self.menu
                .set_selected(if current == 0 { total - 1 } else { current - 1 });
            self.restore();
            EventResult::Consumed
        } else if kb.matches(TermWmAction::MenuDown, key) || kb.matches(TermWmAction::MenuNext, key)
        {
            let current = self.menu.selected();
            self.menu.set_selected((current + 1) % total);
            self.restore();
            EventResult::Consumed
        } else if kb.matches(TermWmAction::MenuSelect, key) {
            self.last_action = self.menu.selected_action().cloned();
            EventResult::Consumed
        } else {
            EventResult::Ignored
        }
    }

    fn update(
        &mut self,
        _action: TermWmAction,
        _ctx: &ComponentContext,
        _actions: &mut VecDeque<(WindowKey, TermWmAction)>,
    ) {
    }

    fn destroy(&mut self) {}
}

impl Overlay<TermWmAction> for WmMenuOverlay {
    fn visible(&self) -> bool {
        !self.outlined.get()
    }
}

impl WmComponent for WmMenuOverlay {
    fn consume_area(&mut self, available: LayoutRect) -> (LayoutRect, LayoutRect) {
        // Overlays render on top, claim no area
        (LayoutRect::default(), available)
    }

    fn process_action(&mut self, action: &ComponentAction) {
        match action {
            ComponentAction::Restore => self.restore(),
            ComponentAction::Outline => self.outline(),
            ComponentAction::SetMenuItems(items) => self.set_items(items.clone()),
            ComponentAction::SetMenuAnchor(pos) => self.set_anchor(*pos),
            ComponentAction::SetManagedArea(area) => self.set_managed_area(*area),
            ComponentAction::ToggleVisibility => {
                if self.outlined.get() {
                    self.restore();
                } else {
                    self.outline();
                }
            }
            _ => {}
        }
    }

    fn query(&self, query: &ComponentQuery) -> ComponentResponse {
        match query {
            ComponentQuery::SelectedAction => ComponentResponse::Action(self.last_action.clone()),
            _ => ComponentResponse::None,
        }
    }

    fn hit_test(&self, x: u16, y: u16) -> bool {
        if let Some(bounds) = self.menu_bounds_cache.get() {
            return rect_contains(bounds, x, y);
        }
        false
    }

    fn visible(&self) -> bool {
        !self.outlined.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_wm_core::components::MenuItem;
    use term_wm_core::events::{
        KeyCode, KeyEvent, KeyKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    fn key_event(code: KeyCode) -> Event {
        Event::Key(KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyKind::Press,
        })
    }

    fn mouse_click(col: u16, row: u16) -> Event {
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        })
    }

    fn process(overlay: &mut WmMenuOverlay, event: &Event, ctx: &ComponentContext) {
        if let EventResult::Action(action) = overlay.handle_events(event, ctx) {
            overlay.update(action, ctx, &mut VecDeque::new());
        }
    }

    fn make_items() -> Vec<MenuItem<TermWmAction>> {
        vec![
            MenuItem {
                icon: Some("A"),
                label: "Alpha",
                action: TermWmAction::CloseWindow,
            },
            MenuItem {
                icon: Some("B"),
                label: "Beta",
                action: TermWmAction::NewWindow,
            },
            MenuItem {
                icon: Some("C"),
                label: "Gamma",
                action: TermWmAction::Help,
            },
        ]
    }

    #[test]
    fn menu_up_down_selections() {
        let mut overlay = WmMenuOverlay::new();
        let ctx = ComponentContext::new(true);
        overlay.set_items(make_items());

        // Down selects the second item
        process(&mut overlay, &key_event(KeyCode::Down), &ctx);
        assert_eq!(overlay.menu.selected(), 1, "Down should select item 1");

        // Up goes back to first
        process(&mut overlay, &key_event(KeyCode::Up), &ctx);
        assert_eq!(overlay.menu.selected(), 0, "Up should select item 0");

        // Up again wraps to last
        process(&mut overlay, &key_event(KeyCode::Up), &ctx);
        assert_eq!(overlay.menu.selected(), 2, "Up at top should wrap to last");
    }

    #[test]
    fn menu_mouse_click_selects_item_and_stores_action() {
        let mut overlay = WmMenuOverlay::new();
        let ctx = ComponentContext::new(true);
        overlay.set_items(make_items());
        overlay.set_anchor(Some((0, 0)));
        overlay.set_managed_area(LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });

        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let ratatui_area = layout_rect_to_rect(area);
        let buf = ratatui::buffer::Buffer::empty(ratatui_area);
        let mut backend = term_wm_console::RatatuiBackend::new(buf, ratatui_area);
        overlay.render(
            &mut backend,
            area,
            &ctx,
            &mut term_wm_core::hitbox_registry::HitboxRegistry::new(),
        );

        // Click on first item at row 1 (header is row 0)
        process(&mut overlay, &mouse_click(1, 1), &ctx);
        assert_eq!(overlay.menu.selected(), 0, "click should select item 0");
        assert_eq!(
            overlay.selected_action(),
            Some(&TermWmAction::CloseWindow),
            "click should set last_action"
        );

        // Click outside all items
        process(&mut overlay, &mouse_click(50, 50), &ctx);
        assert!(
            overlay.selected_action().is_none(),
            "outside click should not set action"
        );
    }

    #[test]
    fn overlay_keyboard_navigation() {
        let mut overlay = WmMenuOverlay::new();
        let ctx = ComponentContext::new(true);
        overlay.set_items(make_items());

        assert!(
            overlay
                .handle_events(&key_event(KeyCode::Down), &ctx)
                .is_consumed(),
            "Down should be consumed"
        );
        assert_eq!(overlay.menu.selected(), 1);

        assert!(
            overlay
                .handle_events(&key_event(KeyCode::Down), &ctx)
                .is_consumed(),
            "Down should be consumed"
        );
        assert_eq!(overlay.menu.selected(), 2);

        assert!(
            overlay
                .handle_events(&key_event(KeyCode::Down), &ctx)
                .is_consumed(),
            "Down should be consumed"
        );
        assert_eq!(overlay.menu.selected(), 0);

        assert!(
            overlay
                .handle_events(&key_event(KeyCode::Up), &ctx)
                .is_consumed(),
            "Up should be consumed"
        );
        assert_eq!(overlay.menu.selected(), 2);
    }

    #[test]
    fn overlay_mouse_click_on_item() {
        let mut overlay = WmMenuOverlay::new();
        let ctx = ComponentContext::new(true);
        overlay.set_items(make_items());
        overlay.set_anchor(Some((0, 1)));
        overlay.set_managed_area(LayoutRect {
            x: 0,
            y: 1,
            width: 80,
            height: 24,
        });

        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 25,
        };
        let ratatui_area = layout_rect_to_rect(area);
        let buf = ratatui::buffer::Buffer::empty(ratatui_area);
        let mut backend = term_wm_console::RatatuiBackend::new(buf, ratatui_area);
        overlay.render(
            &mut backend,
            area,
            &ctx,
            &mut term_wm_core::hitbox_registry::HitboxRegistry::new(),
        );

        assert!(
            overlay
                .handle_events(&mouse_click(1, 2), &ctx)
                .is_consumed(),
            "click on item should be consumed"
        );
        assert_eq!(
            overlay.selected_action(),
            Some(&TermWmAction::CloseWindow),
            "click should set last_action"
        );
    }

    #[test]
    fn overlay_mouse_click_outside_returns_no_action() {
        let mut overlay = WmMenuOverlay::new();
        let ctx = ComponentContext::new(true);
        overlay.set_items(make_items());
        overlay.set_anchor(Some((0, 1)));
        overlay.set_managed_area(LayoutRect {
            x: 0,
            y: 1,
            width: 80,
            height: 24,
        });

        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 25,
        };
        let ratatui_area = layout_rect_to_rect(area);
        let buf = ratatui::buffer::Buffer::empty(ratatui_area);
        let mut backend = term_wm_console::RatatuiBackend::new(buf, ratatui_area);
        overlay.render(
            &mut backend,
            area,
            &ctx,
            &mut term_wm_core::hitbox_registry::HitboxRegistry::new(),
        );

        overlay.handle_events(&mouse_click(50, 50), &ctx);
        assert!(
            overlay.selected_action().is_none(),
            "click outside should not set action"
        );
    }

    #[test]
    fn overlay_renders_dropdown_when_not_outlined() {
        let mut overlay = WmMenuOverlay::new();
        overlay.set_items(make_items());
        overlay.set_anchor(Some((0, 1)));
        overlay.set_managed_area(LayoutRect {
            x: 0,
            y: 1,
            width: 80,
            height: 24,
        });

        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 25,
        };
        let ratatui_area = layout_rect_to_rect(area);
        let mut buf = ratatui::buffer::Buffer::empty(ratatui_area);
        {
            let mut backend = term_wm_console::RatatuiBackend::new(buf, ratatui_area);
            let ctx = ComponentContext::new(true);
            overlay.render(
                &mut backend,
                area,
                &ctx,
                &mut term_wm_core::hitbox_registry::HitboxRegistry::new(),
            );
            buf = backend.buffer;
        }
        let cell = buf.cell((5, 2)).expect("first item text cell");
        assert!(cell.symbol().contains("A"), "dropdown should render items");
    }

    #[test]
    fn overlay_renders_nothing_when_no_items() {
        let mut overlay = WmMenuOverlay::new();
        overlay.set_anchor(Some((0, 1)));

        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 25,
        };
        let ratatui_area = layout_rect_to_rect(area);
        let mut buf = ratatui::buffer::Buffer::empty(ratatui_area);
        {
            let mut backend = term_wm_console::RatatuiBackend::new(buf, ratatui_area);
            let ctx = ComponentContext::new(true);
            overlay.render(
                &mut backend,
                area,
                &ctx,
                &mut term_wm_core::hitbox_registry::HitboxRegistry::new(),
            );
            buf = backend.buffer;
        }
        let cell = buf.cell((0, 1)).expect("cell below anchor");
        assert_eq!(cell.symbol(), " ", "should be empty when no items");
    }

    #[test]
    fn overlay_outline_then_restore() {
        let mut overlay = WmMenuOverlay::new();
        overlay.set_items(make_items());
        overlay.set_anchor(Some((0, 1)));
        overlay.set_managed_area(LayoutRect {
            x: 0,
            y: 1,
            width: 80,
            height: 24,
        });

        overlay.set_timeout(Duration::from_secs(60));

        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 25,
        };
        let ctx = ComponentContext::new(true);

        let ratatui_area = layout_rect_to_rect(area);
        let mut buf = ratatui::buffer::Buffer::empty(ratatui_area);
        {
            let mut backend = term_wm_console::RatatuiBackend::new(buf, ratatui_area);
            overlay.render(
                &mut backend,
                area,
                &ctx,
                &mut term_wm_core::hitbox_registry::HitboxRegistry::new(),
            );
            buf = backend.buffer;
        }
        let normal = buf.cell((1, 2)).map(|c| c.symbol().to_string());

        overlay.outline();
        let mut buf2 = ratatui::buffer::Buffer::empty(ratatui_area);
        {
            let mut backend2 = term_wm_console::RatatuiBackend::new(buf2, ratatui_area);
            overlay.render(
                &mut backend2,
                area,
                &ctx,
                &mut term_wm_core::hitbox_registry::HitboxRegistry::new(),
            );
            buf2 = backend2.buffer;
        }
        let outlined = buf2.cell((1, 2)).map(|c| c.symbol().to_string());
        assert_ne!(normal, outlined, "outline mode should change rendering");

        overlay.restore();
        let mut buf3 = ratatui::buffer::Buffer::empty(ratatui_area);
        {
            let mut backend3 = term_wm_console::RatatuiBackend::new(buf3, ratatui_area);
            overlay.render(
                &mut backend3,
                area,
                &ctx,
                &mut term_wm_core::hitbox_registry::HitboxRegistry::new(),
            );
            buf3 = backend3.buffer;
        }
        let restored = buf3.cell((1, 2)).map(|c| c.symbol().to_string());
        assert_eq!(normal, restored, "restore should revert to dropdown");
    }

    #[test]
    fn debug_format() {
        let overlay = WmMenuOverlay::new();
        let s = format!("{:?}", overlay);
        assert!(
            s.contains("WmMenuOverlay"),
            "Debug should include struct name: {s}"
        );
        assert!(
            s.contains("outlined"),
            "Debug should include outlined field: {s}"
        );
        assert!(
            !s.contains("last_action"),
            "Debug should NOT include last_action field"
        );
    }
}
