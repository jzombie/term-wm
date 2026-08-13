use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::component_context::ComponentContext;
use term_wm_core::components::{Component, SelectionStatus};
use term_wm_core::events::{Event, KeyCode, KeyEvent};
use term_wm_core::impl_view_component;
use term_wm_core::window::WindowKey;
use term_wm_layout_engine::LayoutRect;
use term_wm_ui_components::helpers::{downcast_ratatui, layout_rect_to_clipped_rect};
use term_wm_ui_components::{CanvasScrollView, CanvasSizingPolicy, ScrollViewComponent};
use term_wm_view::view;

/// A scrolling 2-column grid of system-panel utilities, declared with `view!`.
///
/// The grid content is a nameable [`PanelContentView`] wrapped in the existing
/// scrolling containers — the "scrollable view content" pattern for `view!`.
pub struct WmSystemPanelComponent {
    scroll_view: ScrollViewComponent<CanvasScrollView<PanelContentView>>,
}

/// The panel's scroll content: a per-frame `view!` tree. Nameable so it can be
/// the concrete content type of the scroll view (a `view!` tree itself is an
/// anonymous ephemeral type).
///
/// `view(&self)` (not `&mut self`): the key monitor's state lives in a shared
/// `Rc<RefCell<_>>`, so the view clones it instead of borrowing `self`
/// mutably. That makes the whole tree all-owned, which lets
/// `desired_height(&self)` call `self.view().desired_height(width)` directly —
/// height is computed dynamically by the containers (boxes add border+padding,
/// grids reflow, stacks sum gaps), with no hand-maintained constants.
struct PanelContentView {
    key_monitor: KeyMonitorComponent,
}

impl PanelContentView {
    fn new() -> Self {
        Self {
            key_monitor: KeyMonitorComponent::new(Rc::new(RefCell::new(None))),
        }
    }

    fn set_key_monitor(&mut self, state: Rc<RefCell<Option<KeyEvent>>>) {
        self.key_monitor = KeyMonitorComponent::new(state);
    }

    fn view(&self) -> impl Component<TermWmAction> + '_ {
        view! {
            <VerticalStack gap=1>
                { self.key_monitor.clone() }

                <Box title="Notifications" padding=1>
                    <Grid cols="16 1fr" rows="3">
                        <Label text="System Toast:" />
                        <Button label=" Send Notification " action={TermWmAction::SendNotification("Hello from System Panel!".to_string())} />
                    </Grid>
                </Box>

                <Box title="Debug Utilities" padding=1>
                    <Grid cols="16 1fr" rows="3">
                        <Label text="Diagnostics:" />
                        <Button label=" Trigger Panic " action={TermWmAction::Callback(|| panic!("Manual panic from system panel"))} />
                    </Grid>
                </Box>
            </VerticalStack>
        }
    }
}

// `view(&self)` builds an all-owned tree (the key monitor is cloned via its
// shared Rc), so `impl_view_component!` forwards the whole lifecycle — and
// `desired_height` stays fully dynamic (no constants, no drift if the markup
// changes).
impl_view_component!(PanelContentView);

impl WmSystemPanelComponent {
    pub fn new() -> Self {
        Self {
            scroll_view: ScrollViewComponent::new(CanvasScrollView::new(
                PanelContentView::new(),
                CanvasSizingPolicy::FitViewportWidth,
            )),
        }
    }

    /// Attach a key-monitor applet that displays the most recently pressed key.
    /// The shared `state` is populated externally (e.g. by the event loop).
    pub fn with_key_monitor(self, state: Rc<RefCell<Option<KeyEvent>>>) -> Self {
        self.scroll_view
            .content
            .borrow_mut()
            .inner
            .set_key_monitor(state);
        self
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
        self.scroll_view.render(backend, area, ctx, registry);
    }

    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
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

// TODO: This should be decoded elsewhere
/// Format a `KeyEvent` into a human-readable display string.
///
/// Examples: `Ctrl+A`, `Alt+Tab`, `Shift+Enter`, `Ctrl+Shift+F1`, `Esc`.
fn format_key_event(key: &KeyEvent) -> String {
    let mut parts: Vec<String> = Vec::new();

    if key.modifiers.control {
        parts.push("Ctrl".to_string());
    }
    if key.modifiers.alt {
        parts.push("Alt".to_string());
    }

    match key.code {
        KeyCode::Char(c) => {
            // For Char keys, the case already reflects Shift; don't add redundant prefix.
            // Non-alphabetic chars with shift (e.g. '!') are already the shifted form.
            parts.push(c.to_string());
        }
        _ => {
            if key.modifiers.shift {
                parts.push("Shift".to_string());
            }
            let name = match key.code {
                KeyCode::Enter => "Enter",
                KeyCode::Tab => "Tab",
                KeyCode::Esc => "Esc",
                KeyCode::Backspace => "Backspace",
                KeyCode::Left => "Left",
                KeyCode::Right => "Right",
                KeyCode::Up => "Up",
                KeyCode::Down => "Down",
                KeyCode::Home => "Home",
                KeyCode::End => "End",
                KeyCode::PageUp => "PageUp",
                KeyCode::PageDown => "PageDown",
                KeyCode::Delete => "Delete",
                KeyCode::Insert => "Insert",
                KeyCode::F(n) => {
                    parts.push(format!("F{}", n));
                    return parts.join("+");
                }
                _ => {
                    parts.push(format!("{:?}", key.code));
                    return parts.join("+");
                }
            };
            parts.push(name.to_string());
        }
    }

    parts.join("+")
}

/// A single-line label that displays the last key pressed, sourced from
/// externally-populated shared state.
#[derive(Clone)]
struct KeyMonitorComponent {
    state: Rc<RefCell<Option<KeyEvent>>>,
}

impl KeyMonitorComponent {
    fn new(state: Rc<RefCell<Option<KeyEvent>>>) -> Self {
        Self { state }
    }
}

impl Component<TermWmAction> for KeyMonitorComponent {
    fn desired_height(&self, _width: u16) -> u16 {
        1
    }

    fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        _ctx: &ComponentContext,
        _registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let display = self
            .state
            .borrow()
            .as_ref()
            .map(format_key_event)
            .unwrap_or_else(|| "—".to_string());
        let rect = layout_rect_to_clipped_rect(area);
        let backend = downcast_ratatui(backend);
        let line = Line::from(vec![
            Span::raw("Last key pressed: "),
            Span::styled(display, Style::default().fg(Color::Cyan)),
        ]);
        let para = Paragraph::new(line);
        para.render(rect, &mut backend.buffer);
    }

    fn handle_events(
        &mut self,
        _event: &Event,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        EventResult::Ignored
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use term_wm_core::events::{KeyKind, KeyModifiers};

    #[test]
    fn system_panel_new_constructs() {
        let panel = WmSystemPanelComponent::new();
        let _ = &panel;
    }

    #[test]
    fn system_panel_default_is_same_as_new() {
        let panel = WmSystemPanelComponent::default();
        let _ = &panel;
    }

    #[test]
    fn system_panel_render_does_not_panic() {
        let mut panel = WmSystemPanelComponent::new();
        let buffer = Buffer::empty(Rect::new(0, 0, 60, 20));
        let mut backend =
            term_wm_console::RatatuiBackend::new_simple(buffer, Rect::new(0, 0, 60, 20));
        let ctx = ComponentContext::new(true).with_screen_area(LayoutRect {
            x: 0,
            y: 0,
            width: 60,
            height: 20,
        });
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        panel.render(
            &mut backend,
            LayoutRect {
                x: 0,
                y: 0,
                width: 60,
                height: 20,
            },
            &ctx,
            &mut registry,
        );
    }

    #[test]
    fn system_panel_render_shows_grid_cells() {
        let mut panel = WmSystemPanelComponent::new();
        let buffer = Buffer::empty(Rect::new(0, 0, 60, 20));
        let mut backend =
            term_wm_console::RatatuiBackend::new_simple(buffer, Rect::new(0, 0, 60, 20));
        let ctx = ComponentContext::new(true).with_screen_area(LayoutRect {
            x: 0,
            y: 0,
            width: 60,
            height: 20,
        });
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        panel.render(
            &mut backend,
            LayoutRect {
                x: 0,
                y: 0,
                width: 60,
                height: 20,
            },
            &ctx,
            &mut registry,
        );
        let content: String = backend
            .buffer
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            content.contains("System Toast:"),
            "grid label row should render: {content:?}"
        );
        assert!(
            content.contains("Trigger Panic"),
            "grid button row should render: {content:?}"
        );
    }

    #[test]
    fn panel_content_view_desired_height_is_dynamic() {
        // CanvasScrollView sizes its scrollable canvas from this value; a 0
        // default would disable scrolling. `view(&self)` builds the tree
        // all-owned (key monitor cloned via its shared Rc), so the height is
        // computed by the containers themselves — no constants. Assert it
        // matches the actual layout: wide = key monitor (1) + 2 gaps + two
        // 7-row cards = 17; narrow (grids reflow) = 19.
        let content = PanelContentView::new();
        assert_eq!(content.desired_height(40), 17);
        assert_eq!(content.desired_height(12), 19);
    }

    #[test]
    fn system_panel_narrow_render_stacks_full_width() {
        // Reflow regression: at a narrow width the grid must stack labels above
        // buttons (full-width rows) instead of cramming a shrunken button next
        // to a truncated label.
        let mut panel = WmSystemPanelComponent::new();
        let buffer = Buffer::empty(Rect::new(0, 0, 20, 20));
        let mut backend =
            term_wm_console::RatatuiBackend::new_simple(buffer, Rect::new(0, 0, 20, 20));
        let ctx = ComponentContext::new(true).with_screen_area(LayoutRect {
            x: 0,
            y: 0,
            width: 20,
            height: 20,
        });
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        panel.render(
            &mut backend,
            LayoutRect {
                x: 0,
                y: 0,
                width: 20,
                height: 20,
            },
            &ctx,
            &mut registry,
        );
        let content: String = backend
            .buffer
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        // The section label must be a full-width row on its own line inside its
        // card, not truncated to a few characters beside a button border; the
        // card titles render in the box borders.
        assert!(
            content.contains("System Toast:"),
            "stacked label should render untruncated: {content:?}"
        );
        assert!(
            content.contains("Trigger Panic"),
            "stacked button should render: {content:?}"
        );
        assert!(
            content.contains("Notifications") && content.contains("Debug Utilities"),
            "box card titles should render: {content:?}"
        );
    }

    #[test]
    fn system_panel_handle_events_ignores_key() {
        let mut panel = WmSystemPanelComponent::new();
        let ctx = ComponentContext::new(true).with_screen_area(LayoutRect {
            x: 0,
            y: 0,
            width: 60,
            height: 20,
        });
        let event = Event::Key(KeyEvent::new(
            KeyCode::Char('x'),
            KeyModifiers::NONE,
            KeyKind::Press,
        ));
        assert!(panel.handle_events(&event, &ctx).is_ignored());
    }

    #[test]
    fn system_panel_update_is_noop() {
        let mut panel = WmSystemPanelComponent::new();
        let ctx = ComponentContext::new(true);
        let mut actions = VecDeque::new();
        panel.update(TermWmAction::Quit, &ctx, &mut actions);
    }

    #[test]
    fn system_panel_selection_status() {
        let panel = WmSystemPanelComponent::new();
        let _ = panel.selection_status();
    }

    #[test]
    fn system_panel_selection_text() {
        let panel = WmSystemPanelComponent::new();
        let _ = panel.selection_text();
    }

    #[test]
    fn system_panel_destroy_is_noop() {
        let mut panel = WmSystemPanelComponent::new();
        panel.destroy();
    }

    #[test]
    fn with_key_monitor_builds() {
        let state = Rc::new(RefCell::new(None));
        let panel = WmSystemPanelComponent::new().with_key_monitor(state);
        let _ = &panel;
    }

    #[test]
    fn key_monitor_desired_height() {
        let state = Rc::new(RefCell::new(None));
        let monitor = KeyMonitorComponent::new(state);
        assert_eq!(monitor.desired_height(40), 1);
    }

    #[test]
    fn key_monitor_handle_events_ignored() {
        let state = Rc::new(RefCell::new(None));
        let mut monitor = KeyMonitorComponent::new(state);
        let ctx = ComponentContext::new(true);
        let event = Event::Key(KeyEvent::new(
            KeyCode::Char('x'),
            KeyModifiers::NONE,
            KeyKind::Press,
        ));
        assert!(monitor.handle_events(&event, &ctx).is_ignored());
    }

    #[test]
    fn key_monitor_render_no_panic_with_key() {
        let state = Rc::new(RefCell::new(Some(KeyEvent::new(
            KeyCode::Char('a'),
            KeyModifiers {
                shift: false,
                control: true,
                alt: false,
            },
            KeyKind::Press,
        ))));
        let mut monitor = KeyMonitorComponent::new(state);
        let buffer = Buffer::empty(Rect::new(0, 0, 40, 1));
        let mut backend =
            term_wm_console::RatatuiBackend::new_simple(buffer, Rect::new(0, 0, 40, 1));
        let ctx = ComponentContext::new(true).with_screen_area(LayoutRect {
            x: 0,
            y: 0,
            width: 40,
            height: 1,
        });
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        monitor.render(
            &mut backend,
            LayoutRect {
                x: 0,
                y: 0,
                width: 40,
                height: 1,
            },
            &ctx,
            &mut registry,
        );
    }

    #[test]
    fn key_monitor_render_skips_zero_area() {
        let state = Rc::new(RefCell::new(None));
        let mut monitor = KeyMonitorComponent::new(state);
        let buffer = Buffer::empty(Rect::new(0, 0, 40, 1));
        let mut backend =
            term_wm_console::RatatuiBackend::new_simple(buffer, Rect::new(0, 0, 40, 1));
        let ctx = ComponentContext::new(true);
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        monitor.render(
            &mut backend,
            LayoutRect {
                x: 0,
                y: 0,
                width: 0,
                height: 1,
            },
            &ctx,
            &mut registry,
        );
        monitor.render(
            &mut backend,
            LayoutRect {
                x: 0,
                y: 0,
                width: 1,
                height: 0,
            },
            &ctx,
            &mut registry,
        );
    }

    #[test]
    fn key_monitor_update_is_noop() {
        let state = Rc::new(RefCell::new(None));
        let mut monitor = KeyMonitorComponent::new(state);
        let ctx = ComponentContext::new(true);
        let mut actions = VecDeque::new();
        monitor.update(TermWmAction::Quit, &ctx, &mut actions);
    }

    #[test]
    fn key_monitor_destroy_is_noop() {
        let state = Rc::new(RefCell::new(None));
        let mut monitor = KeyMonitorComponent::new(state);
        monitor.destroy();
    }

    #[test]
    fn format_key_event_ctrl_a() {
        let key = KeyEvent::new(
            KeyCode::Char('a'),
            KeyModifiers {
                shift: false,
                control: true,
                alt: false,
            },
            KeyKind::Press,
        );
        assert_eq!(format_key_event(&key), "Ctrl+a");
    }

    #[test]
    fn format_key_event_alt_tab() {
        let key = KeyEvent::new(
            KeyCode::Tab,
            KeyModifiers {
                shift: false,
                control: false,
                alt: true,
            },
            KeyKind::Press,
        );
        assert_eq!(format_key_event(&key), "Alt+Tab");
    }

    #[test]
    fn format_key_event_shift_enter() {
        let key = KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers {
                shift: true,
                control: false,
                alt: false,
            },
            KeyKind::Press,
        );
        assert_eq!(format_key_event(&key), "Shift+Enter");
    }

    #[test]
    fn format_key_event_plain_char() {
        let key = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE, KeyKind::Press);
        assert_eq!(format_key_event(&key), "x");
    }

    #[test]
    fn format_key_event_esc() {
        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE, KeyKind::Press);
        assert_eq!(format_key_event(&key), "Esc");
    }

    #[test]
    fn format_key_event_f1() {
        let key = KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE, KeyKind::Press);
        assert_eq!(format_key_event(&key), "F1");
    }

    #[test]
    fn format_key_event_ctrl_shift_f5() {
        let key = KeyEvent::new(
            KeyCode::F(5),
            KeyModifiers {
                shift: true,
                control: true,
                alt: false,
            },
            KeyKind::Press,
        );
        assert_eq!(format_key_event(&key), "Ctrl+Shift+F5");
    }
}
