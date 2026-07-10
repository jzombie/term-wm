use std::cell::Cell;
use std::collections::VecDeque;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::components::{Component, ComponentContext, SelectionStatus};
use term_wm_core::events::MouseButton;
use term_wm_core::window::WindowKey;
use term_wm_layout_engine::LayoutRect;
use term_wm_ui_components::{ScrollViewComponent, TextRendererComponent};

/// A system panel with utility buttons inside a scrollable view.
///
/// The "Send Notification" button is rendered as styled text lines
/// within the scroll view. Hit-testing accounts for scroll offset.
#[derive(Debug)]
pub struct WmSystemPanelComponent {
    scroll_view: ScrollViewComponent<TextRendererComponent>,
    /// Button position in VIRTUAL coordinates (relative to content top-left, before scroll).
    button_rect: Cell<Option<LayoutRect>>,
}

const BUTTON_WIDTH: u16 = 24;
const BUTTON_LABEL: &str = " Send Notification ";
const BUTTON_TOP_BORDER: &str = "╭─────────────────────╮";
const BUTTON_BOTTOM_BORDER: &str = "╰─────────────────────╯";

impl WmSystemPanelComponent {
    pub fn new() -> Self {
        let mut renderer = TextRendererComponent::new();
        renderer.set_wrap(false);
        let scroll_view = ScrollViewComponent::new(renderer);
        Self {
            scroll_view,
            button_rect: Cell::new(None),
        }
    }

    /// Build the text content including the button lines.
    fn build_content() -> Text<'static> {
        let mut lines: Vec<Line<'static>> = Vec::new();

        // Info text
        lines.push(Line::from(Span::styled(
            "Notification test panel",
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Click the button below to send",
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(Span::styled(
            "a test toast notification.",
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(""));

        // Button — 3 styled lines
        lines.push(Line::from(Span::styled(
            BUTTON_TOP_BORDER.to_string(),
            Style::default().fg(Color::Cyan),
        )));
        lines.push(Line::from(Span::styled(
            format!("│{BUTTON_LABEL}│"),
            Style::default()
                .fg(Color::White)
                .bg(Color::Rgb(30, 30, 50))
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            BUTTON_BOTTOM_BORDER.to_string(),
            Style::default().fg(Color::Cyan),
        )));
        lines.push(Line::from(""));

        // Hint
        lines.push(Line::from(Span::styled(
            "Scroll to see more content below.",
            Style::default().fg(Color::DarkGray),
        )));

        // Extra lines to make the content scrollable
        for i in 0..20 {
            lines.push(Line::from(Span::styled(
                format!("  Item {}", i + 1),
                Style::default().fg(Color::DarkGray),
            )));
        }

        Text::from(lines)
    }
}

impl Default for WmSystemPanelComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component<TermWmAction> for WmSystemPanelComponent {
    fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        ctx: &ComponentContext,
        registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        // Set text content on the inner renderer
        let text = Self::build_content();
        {
            let mut content = self.scroll_view.content.borrow_mut();
            content.set_text(text);
            content.set_wrap(false);
        }

        // The button is at virtual row 5 (after 5 info/empty lines)
        // in the full content, spanning 3 lines (top border, label, bottom border).
        self.button_rect.set(Some(LayoutRect {
            x: 0,
            y: 5,
            width: BUTTON_WIDTH,
            height: 3,
        }));

        // Delegate to scroll view — it handles rendering the scrollable content
        self.scroll_view.render(backend, area, ctx, registry);
    }

    fn on_mouse_press(
        &mut self,
        local_x: u16,
        local_y: u16,
        button: MouseButton,
        _modifiers: term_wm_core::events::KeyModifiers,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        if button != MouseButton::Left {
            return EventResult::Ignored;
        }

        let Some(btn) = self.button_rect.get() else {
            return EventResult::Ignored;
        };

        // Fetch scroll offset from the scroll view
        let scroll_offset = self.scroll_view.scroll_handle().info().offset_y as u16;

        // Translate local viewport Y to virtual content Y
        let virtual_y = local_y.saturating_add(scroll_offset);

        // Strict u16 hit-test against the button's virtual position
        let btn_y = btn.y as u16;
        if local_x < btn.width
            && virtual_y >= btn_y
            && virtual_y < btn_y.saturating_add(btn.height)
        {
            return EventResult::Action(TermWmAction::SendNotification(
                "Hello from System Panel!".to_string(),
            ));
        }

        EventResult::Ignored
    }

    fn handle_events(
        &mut self,
        event: &term_wm_core::events::Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        // Check for button click first (before scroll view intercepts)
        if let term_wm_core::events::Event::Mouse(mouse) = event
            && let term_wm_core::events::MouseEventKind::Press(MouseButton::Left) = mouse.kind
            && let Some(screen_area) = ctx.screen_area()
        {
            let sa_x = screen_area.x as u16;
            let sa_y = screen_area.y as u16;
            tracing::info!(
                "system_panel: mouse press at ({}, {}), screen_area=({}, {}, {}, {})",
                mouse.column, mouse.row, sa_x, sa_y, screen_area.width, screen_area.height
            );
            if mouse.column >= sa_x
                && mouse.column < sa_x.saturating_add(screen_area.width)
                && mouse.row >= sa_y
                && mouse.row < sa_y.saturating_add(screen_area.height)
            {
                let local_x = mouse.column.saturating_sub(sa_x);
                let local_y = mouse.row.saturating_sub(sa_y);
                tracing::info!(
                    "system_panel: local ({}, {}), button_rect={:?}, scroll_offset={}",
                    local_x,
                    local_y,
                    self.button_rect.get(),
                    self.scroll_view.scroll_handle().info().offset_y
                );
                let result = self.on_mouse_press(
                    local_x,
                    local_y,
                    MouseButton::Left,
                    mouse.modifiers,
                    ctx,
                );
                tracing::info!("system_panel: on_mouse_press result={:?}", result);
                if !result.is_ignored() {
                    return result;
                }
            }
        }
        self.scroll_view.handle_events(event, ctx)
    }

    fn update(
        &mut self,
        action: TermWmAction,
        ctx: &ComponentContext,
        actions: &mut VecDeque<(WindowKey, TermWmAction)>,
    ) {
        self.scroll_view.update(action, ctx, actions);
    }

    fn destroy(&mut self) {}

    fn selection_status(&self) -> SelectionStatus {
        self.scroll_view.selection_status()
    }

    fn selection_text(&self) -> Option<String> {
        self.scroll_view.selection_text()
    }
}
