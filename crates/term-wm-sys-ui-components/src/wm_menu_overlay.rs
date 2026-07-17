use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use ratatui::style::{Modifier, Style};
use ratatui::widgets::Widget as _;
use ratatui::widgets::{Block, Borders};
use term_wm_core::events::Event;
use term_wm_layout_engine::LayoutRect;

use term_wm_core::{
    actions::{EventResult, TermWmAction},
    components::{
        Component, ComponentAction, ComponentContext, ComponentQuery, ComponentResponse,
        MenuItem, Overlay, WmComponent,
    },
    layout::rect_contains,
    window::WindowKey,
};
use term_wm_ui_components::helpers::{color_to_ratatui, layout_rect_to_rect};

use term_wm_ui_components::menu::MenuComponent;
use term_wm_ui_components::{Placement, PlacementContainerComponent};

pub struct WmCommandPaletteOverlay {
    menu: PlacementContainerComponent<MenuComponent>,
    outlined: Cell<bool>,
    outlined_at: RefCell<Option<Instant>>,
    outline_timeout: Duration,
    menu_bounds_cache: Cell<Option<LayoutRect>>,
    anchor: Option<(u16, u16)>,
    managed_area: LayoutRect,
    last_action: Option<TermWmAction>,
}

impl std::fmt::Debug for WmCommandPaletteOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WmCommandPaletteOverlay")
            .field("outlined", &self.outlined.get())
            .field("anchor", &self.anchor)
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
        Self {
            menu: PlacementContainerComponent::new(
                MenuComponent::new(),
                Placement::Centered { width: 20, height: 3 },
            ),
            outlined: Cell::new(false),
            outlined_at: RefCell::new(None),
            outline_timeout: Duration::ZERO,
            menu_bounds_cache: Cell::new(None),
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
        self.menu.inner_mut().set_items(items);
    }

    pub fn set_anchor(&mut self, pos: Option<(u16, u16)>) {
        self.anchor = pos;
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
        let item_count = self.menu.inner().items().len();
        let label_width = self
            .menu
            .inner()
            .items()
            .iter()
            .map(|item| item.label.chars().count() as u16)
            .max()
            .unwrap_or(1);
        let icon_width = self
            .menu
            .inner()
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
        _area: LayoutRect,
        ctx: &ComponentContext,
        registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        self.auto_restore();
        if self.outlined.get() {
            self.render_outline(backend);
            return;
        }

        let item_count = self.menu.inner().items().len();
        if item_count == 0 {
            return;
        }

        let (content_width, content_height) = self.compute_content_dimensions();
        let managed = self.managed_area;

        if let Some(anchor) = self.anchor {
            if anchor.0 < managed.x.max(0) as u16
                || anchor.0 >= (managed.x.max(0) as u16).saturating_add(managed.width)
            {
                return;
            }
            let max_w = managed
                .width
                .saturating_sub(anchor.0.saturating_sub(managed.x.max(0) as u16))
                .max(1);
            let max_h = managed
                .height
                .saturating_sub(anchor.1.saturating_sub(managed.y.max(0) as u16))
                .max(1);
            let w = content_width.min(max_w);
            let h = content_height.min(max_h);
            self.menu.set_placement(Placement::Anchored {
                x: anchor.0,
                y: anchor.1,
                managed_area: managed,
                content_width: w,
                content_height: h,
            });
        } else {
            self.menu.set_placement(Placement::Centered {
                width: content_width,
                height: content_height,
            });
        }

        self.menu.render(backend, _area, ctx, registry);

        if let Some(content_rect) = self.menu.content_rect() {
            self.menu_bounds_cache.set(Some(content_rect));
        }
    }

    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        self.auto_restore();
        self.last_action = None;

        let result = self.menu.handle_events(event, ctx);
        match result {
            EventResult::Action(action) => {
                let mut actions = VecDeque::new();
                self.menu.update(action.clone(), ctx, &mut actions);
                if action == TermWmAction::MenuSelect {
                    self.last_action = self.menu.inner().selected_action().cloned();
                    self.restore();
                }
                EventResult::Consumed
            }
            EventResult::Consumed => {
                self.last_action = self.menu.inner().selected_action().cloned();
                self.restore();
                EventResult::Consumed
            }
            EventResult::Ignored => EventResult::Ignored,
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

    fn process(overlay: &mut WmCommandPaletteOverlay, event: &Event, ctx: &ComponentContext) {
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
        let mut overlay = WmCommandPaletteOverlay::new();
        let ctx = ComponentContext::new(true);
        overlay.set_items(make_items());

        process(&mut overlay, &key_event(KeyCode::Down), &ctx);
        assert_eq!(
            overlay.menu.inner().selected(),
            1,
            "Down should select item 1"
        );

        process(&mut overlay, &key_event(KeyCode::Up), &ctx);
        assert_eq!(
            overlay.menu.inner().selected(),
            0,
            "Up should select item 0"
        );

        process(&mut overlay, &key_event(KeyCode::Up), &ctx);
        assert_eq!(
            overlay.menu.inner().selected(),
            2,
            "Up at top should wrap to last"
        );
    }

    #[test]
    fn menu_mouse_click_selects_item_and_stores_action() {
        let mut overlay = WmCommandPaletteOverlay::new();
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
        assert_eq!(
            overlay.menu.inner().selected(),
            0,
            "click should select item 0"
        );
        assert_eq!(
            overlay.selected_action(),
            Some(TermWmAction::CloseWindow),
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
        let mut overlay = WmCommandPaletteOverlay::new();
        let ctx = ComponentContext::new(true);
        overlay.set_items(make_items());

        assert!(
            overlay
                .handle_events(&key_event(KeyCode::Down), &ctx)
                .is_consumed(),
            "Down should be consumed"
        );
        assert_eq!(overlay.menu.inner().selected(), 1);

        assert!(
            overlay
                .handle_events(&key_event(KeyCode::Down), &ctx)
                .is_consumed(),
            "Down should be consumed"
        );
        assert_eq!(overlay.menu.inner().selected(), 2);

        assert!(
            overlay
                .handle_events(&key_event(KeyCode::Down), &ctx)
                .is_consumed(),
            "Down should be consumed"
        );
        assert_eq!(overlay.menu.inner().selected(), 0);

        assert!(
            overlay
                .handle_events(&key_event(KeyCode::Up), &ctx)
                .is_consumed(),
            "Up should be consumed"
        );
        assert_eq!(overlay.menu.inner().selected(), 2);
    }

    #[test]
    fn overlay_mouse_click_on_item() {
        let mut overlay = WmCommandPaletteOverlay::new();
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
            Some(TermWmAction::CloseWindow),
            "click should set last_action"
        );
    }

    #[test]
    fn overlay_mouse_click_outside_returns_no_action() {
        let mut overlay = WmCommandPaletteOverlay::new();
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
        let mut overlay = WmCommandPaletteOverlay::new();
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
        let mut overlay = WmCommandPaletteOverlay::new();
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
        let mut overlay = WmCommandPaletteOverlay::new();
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
