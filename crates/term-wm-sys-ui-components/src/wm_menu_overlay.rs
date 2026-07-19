use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use ratatui::style::{Modifier, Style};
use ratatui::widgets::Widget as _;
use ratatui::widgets::{Block, Borders, Clear};
use term_wm_core::events::Event;
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
use term_wm_ui_components::helpers::{color_to_ratatui, downcast_ratatui, layout_rect_to_rect};

use term_wm_ui_components::menu::MenuComponent;
use term_wm_ui_components::DialogOverlayComponent;

pub struct WmCommandPaletteOverlay {
    dialog: DialogOverlayComponent,
    menu: MenuComponent,
    outlined: Cell<bool>,
    outlined_at: RefCell<Option<Instant>>,
    outline_timeout: Duration,
    menu_bounds_cache: Cell<Option<LayoutRect>>,
    managed_area: LayoutRect,
    last_action: Option<TermWmAction>,
}

impl std::fmt::Debug for WmCommandPaletteOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WmCommandPaletteOverlay")
            .field("outlined", &self.outlined.get())
            .field("managed_area", &self.managed_area)
            .finish_non_exhaustive()
    }
}

impl Default for WmCommandPaletteOverlay {
    fn default() -> Self {
        Self::new()
    }
}

const MIN_MODAL_WIDTH: u16 = 20;

impl WmCommandPaletteOverlay {
    pub fn new() -> Self {
        let mut dialog = DialogOverlayComponent::new();
        dialog.set_dim_backdrop(true);
        dialog.set_auto_close_on_outside_click(true);
        Self {
            dialog,
            menu: MenuComponent::new(),
            outlined: Cell::new(false),
            outlined_at: RefCell::new(None),
            outline_timeout: Duration::ZERO,
            menu_bounds_cache: Cell::new(None),
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

    pub fn set_managed_area(&mut self, area: LayoutRect) {
        self.managed_area = area;
    }

    pub fn selected_action(&self) -> Option<TermWmAction> {
        self.last_action.clone()
    }

    pub fn set_timeout(&mut self, timeout: Duration) {
        self.outline_timeout = timeout;
    }

    fn compute_content_dimensions(&self) -> (u16, u16) {
        let item_count = self.menu.items().len();
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
        let content_width = (label_width + icon_width + 6).max(MIN_MODAL_WIDTH);
        let content_height = (item_count as u16).saturating_add(2);
        (content_width, content_height)
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
}

impl Component<TermWmAction> for WmCommandPaletteOverlay {
    fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        ctx: &ComponentContext,
        registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        self.auto_restore();
        if self.outlined.get() {
            self.render_outline(backend);
            return;
        }

        let item_count = self.menu.items().len();
        if item_count == 0 {
            return;
        }

        let (content_width, content_height) = self.compute_content_dimensions();
        self.dialog.set_size(content_width, content_height);

        let rect = self.dialog.rect_for(layout_rect_to_rect(area));
        let content_rect = LayoutRect {
            x: i32::from(rect.x),
            y: i32::from(rect.y),
            width: rect.width,
            height: rect.height,
        };

        if content_rect.width == 0 || content_rect.height == 0 {
            return;
        }

        self.dialog.render_backdrop(backend, area, Some(content_rect));
        {
            let ratatui = downcast_ratatui(backend);
            Clear.render(layout_rect_to_rect(content_rect), &mut ratatui.buffer);
        }

        self.menu_bounds_cache.set(Some(content_rect));
        self.menu.render(backend, content_rect, ctx, registry);
    }

    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        self.auto_restore();
        self.last_action = None;

        if let Event::Mouse(_) = event {
            let screen_area = ctx.screen_area().unwrap_or_default();
            let ratatui_area = layout_rect_to_rect(screen_area);

            if self.dialog.handle_click_outside(event, ratatui_area) {
                self.outline();
                return EventResult::Consumed;
            }

            let rect = self.dialog.rect_for(ratatui_area);
            let content_rect = LayoutRect {
                x: rect.x as i32,
                y: rect.y as i32,
                width: rect.width,
                height: rect.height,
            };
            let adjusted_ctx = ctx.with_screen_area(content_rect);
            let result = self.menu.handle_events(event, &adjusted_ctx);

            match result {
                EventResult::Action(action) => {
                    let mut actions = VecDeque::new();
                    self.menu.update(action.clone(), ctx, &mut actions);
                    if action == TermWmAction::MenuSelect {
                        self.last_action = self.menu.selected_action().cloned();
                        self.restore();
                    }
                    EventResult::Consumed
                }
                EventResult::Consumed => {
                    self.last_action = self.menu.selected_action().cloned();
                    self.restore();
                    EventResult::Consumed
                }
                EventResult::Ignored => EventResult::Ignored,
            }
        } else {
            let result = self.menu.handle_events(event, ctx);

            match result {
                EventResult::Action(action) => {
                    let mut actions = VecDeque::new();
                    self.menu.update(action.clone(), ctx, &mut actions);
                    if action == TermWmAction::MenuSelect {
                        self.last_action = self.menu.selected_action().cloned();
                        self.restore();
                    }
                    EventResult::Consumed
                }
                EventResult::Consumed => {
                    self.last_action = self.menu.selected_action().cloned();
                    self.restore();
                    EventResult::Consumed
                }
                EventResult::Ignored => EventResult::Ignored,
            }
        }
    }

    fn update(
        &mut self,
        action: TermWmAction,
        ctx: &ComponentContext,
        actions: &mut VecDeque<(WindowKey, TermWmAction)>,
    ) {
        self.menu.update(action, ctx, actions);
    }

    fn destroy(&mut self) {}
}

impl Overlay<TermWmAction> for WmCommandPaletteOverlay {
    fn visible(&self) -> bool {
        !self.outlined.get()
    }
}

impl WmComponent for WmCommandPaletteOverlay {
    fn consume_area(&mut self, available: LayoutRect) -> (LayoutRect, LayoutRect) {
        (LayoutRect::default(), available)
    }

    fn process_action(&mut self, action: &ComponentAction) {
        match action {
            ComponentAction::Restore => {
                self.dialog.set_visible(true);
                self.restore();
            }
            ComponentAction::Outline => self.outline(),
            ComponentAction::SetMenuItems(items) => self.set_items(items.clone()),
            ComponentAction::SetManagedArea(area) => self.set_managed_area(*area),
            ComponentAction::ToggleVisibility => {
                if self.outlined.get() {
                    self.restore();
                    self.dialog.set_visible(true);
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
        KeyCode, KeyEvent, KeyKind, KeyModifiers,
    };
    fn key_event(code: KeyCode) -> Event {
        Event::Key(KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyKind::Press,
        })
    }

    fn process(overlay: &mut WmCommandPaletteOverlay, event: &Event, ctx: &ComponentContext) {
        if let EventResult::Action(action) = overlay.handle_events(event, ctx) {
            overlay.update(action, ctx, &mut VecDeque::new());
        }
    }

    fn make_items() -> Vec<MenuItem<TermWmAction>> {
        vec![
            MenuItem {
                icon: Some("A"),
                label: "Alpha".into(),
                action: TermWmAction::CloseWindow,
            },
            MenuItem {
                icon: Some("B"),
                label: "Beta".into(),
                action: TermWmAction::NewWindow,
            },
            MenuItem {
                icon: Some("C"),
                label: "Gamma".into(),
                action: TermWmAction::Help,
            },
        ]
    }

    #[test]
    fn menu_up_down_selections() {
        let mut overlay = WmCommandPaletteOverlay::new();
        let ctx = ComponentContext::new(true);
        overlay.set_items(make_items());

        process(&mut overlay, &key_event(KeyCode::Down), &ctx);
        assert_eq!(
            overlay.menu.selected(),
            1,
            "Down should select item 1"
        );

        process(&mut overlay, &key_event(KeyCode::Up), &ctx);
        assert_eq!(
            overlay.menu.selected(),
            0,
            "Up should select item 0"
        );

        process(&mut overlay, &key_event(KeyCode::Up), &ctx);
        assert_eq!(
            overlay.menu.selected(),
            2,
            "Up at top should wrap to last"
        );
    }

    #[test]
    fn overlay_keyboard_navigation() {
        let mut overlay = WmCommandPaletteOverlay::new();
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
    fn overlay_renders_dropdown_when_not_outlined() {
        let mut overlay = WmCommandPaletteOverlay::new();
        overlay.set_items(make_items());

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
        // With 3 items and area 80x25, centered rect width >= 24, height = 5,
        // positioned at (28, 10) — item text should appear.
        let cell = buf.cell((30, 11)).expect("first item text cell");
        assert!(cell.symbol().contains("A"), "dropdown should render items");
    }

    #[test]
    fn overlay_renders_nothing_when_no_items() {
        let mut overlay = WmCommandPaletteOverlay::new();

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
        let cell = buf.cell((0, 0)).expect("cell at origin");
        assert_eq!(cell.symbol(), " ", "should be empty when no items");
    }

    #[test]
    fn overlay_outline_then_restore() {
        let mut overlay = WmCommandPaletteOverlay::new();
        overlay.set_items(make_items());

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
        let normal = buf.cell((30, 11)).map(|c| c.symbol().to_string());

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
        let outlined = buf2.cell((30, 11)).map(|c| c.symbol().to_string());
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
        let restored = buf3.cell((30, 11)).map(|c| c.symbol().to_string());
        assert_eq!(normal, restored, "restore should revert to dropdown");
    }

    #[test]
    fn debug_format() {
        let overlay = WmCommandPaletteOverlay::new();
        let s = format!("{:?}", overlay);
        assert!(
            s.contains("WmCommandPaletteOverlay"),
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
