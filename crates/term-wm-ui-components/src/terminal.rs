use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::sync::Arc;

use portable_pty::{CommandBuilder, PtySize};
use ratatui::style::{Color as TColor, Modifier, Style};
use term_wm_core::events::{Event, KeyCode, KeyKind, MouseButton, MouseEvent, MouseEventKind};
use vt100::MouseProtocolEncoding;

use crate::helpers::{
    color_to_ratatui, decorate_link_style, layout_rect_to_clipped_rect, localize_coordinate,
    localize_coordinate_clamped,
};
use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::components::{Component, ComponentContext, SelectionStatus};
use term_wm_core::hitbox_registry::HitboxId;
use term_wm_core::utils::linkifier::{LinkHandler, LinkOverlay, Linkifier, OverlaySignature};
use term_wm_core::utils::selectable_text::{
    LogicalPosition, SelectionController, SelectionHost, SelectionRange, SelectionViewport,
    handle_selection_mouse, maintain_selection_drag,
};
use term_wm_core::window::WindowKey;
use term_wm_layout_engine::LayoutRect;
use term_wm_pty_engine::input_encoding::{key_to_bytes, mouse_event_allowed, mouse_event_to_bytes};
use term_wm_pty_engine::{Pane, PtyStatus};

// This controls the scrollback buffer size in the vt100 parser.
// It determines how many lines you can scroll up to see.
const DEFAULT_SCROLLBACK_LEN: usize = 2000;

// TODO: Refactor?
/// Convert core-owned MouseEventKind to pty-engine MouseEventKind
fn convert_mouse_event_kind(
    kind: MouseEventKind,
) -> term_wm_pty_engine::input_encoding::MouseEventKind {
    match kind {
        MouseEventKind::Press(btn) => {
            term_wm_pty_engine::input_encoding::MouseEventKind::Press(convert_mouse_button(btn))
        }
        MouseEventKind::Release(btn) => {
            term_wm_pty_engine::input_encoding::MouseEventKind::Release(convert_mouse_button(btn))
        }
        MouseEventKind::Drag(btn) => {
            term_wm_pty_engine::input_encoding::MouseEventKind::Drag(convert_mouse_button(btn))
        }
        MouseEventKind::Moved => term_wm_pty_engine::input_encoding::MouseEventKind::Moved,
        MouseEventKind::ScrollUp => term_wm_pty_engine::input_encoding::MouseEventKind::ScrollUp,
        MouseEventKind::ScrollDown => {
            term_wm_pty_engine::input_encoding::MouseEventKind::ScrollDown
        }
        MouseEventKind::ScrollLeft => {
            term_wm_pty_engine::input_encoding::MouseEventKind::ScrollLeft
        }
        MouseEventKind::ScrollRight => {
            term_wm_pty_engine::input_encoding::MouseEventKind::ScrollRight
        }
    }
}

// TODO: Refactor?
fn convert_mouse_button(btn: MouseButton) -> term_wm_pty_engine::input_encoding::MouseButton {
    match btn {
        MouseButton::Left => term_wm_pty_engine::input_encoding::MouseButton::Left,
        MouseButton::Right => term_wm_pty_engine::input_encoding::MouseButton::Right,
        MouseButton::Middle => term_wm_pty_engine::input_encoding::MouseButton::Middle,
    }
}

// TODO: Refactor?
fn convert_pty_mouse_event(mouse: &MouseEvent) -> term_wm_pty_engine::input_encoding::MouseEvent {
    term_wm_pty_engine::input_encoding::MouseEvent {
        kind: convert_mouse_event_kind(mouse.kind),
        modifiers: term_wm_pty_engine::input_encoding::KeyModifiers {
            shift: mouse.modifiers.shift,
            control: mouse.modifiers.control,
            alt: mouse.modifiers.alt,
        },
        column: mouse.column,
        row: mouse.row,
    }
}

pub struct TerminalComponent {
    hitbox_id: HitboxId,
    pane: RefCell<Box<dyn Pane>>,
    last_size: Cell<(u16, u16)>,
    linkifier: Linkifier,
    link_overlay: RefCell<LinkOverlay>,
    link_handler: Option<LinkHandler>,
    command_description: String,
    selection: RefCell<SelectionController>,
    selection_enabled: bool,
    last_scrollback: Cell<usize>,
    last_max_scrollback: Cell<usize>,
    window_key: Option<term_wm_core::window::WindowKey>,
}

impl Component<TermWmAction> for TerminalComponent {
    fn on_mount(
        &mut self,
        key: term_wm_core::window::WindowKey,
        _app: &term_wm_core::app_context::AppContext,
    ) {
        self.window_key = Some(key);
    }

    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        match event {
            Event::Key(key) => {
                if key.kind == KeyKind::Release {
                    return EventResult::Ignored;
                }
                if matches!(key.code, KeyCode::PageUp | KeyCode::PageDown)
                    && key.modifiers.shift
                    && !self.pane.borrow_mut().alternate_screen()
                {
                    let delta = if key.code == KeyCode::PageUp {
                        10isize
                    } else {
                        -10isize
                    };
                    return EventResult::Action(TermWmAction::Scroll(delta));
                }
                // TODO: Refactor?
                // Convert core-owned KeyEvent to pty-engine KeyEvent for key_to_bytes
                let pty_key = term_wm_pty_engine::input_encoding::KeyEvent {
                    code: match key.code {
                        KeyCode::Char(c) => term_wm_pty_engine::input_encoding::KeyCode::Char(c),
                        KeyCode::Enter => term_wm_pty_engine::input_encoding::KeyCode::Enter,
                        KeyCode::Tab => term_wm_pty_engine::input_encoding::KeyCode::Tab,
                        KeyCode::Backspace => {
                            term_wm_pty_engine::input_encoding::KeyCode::Backspace
                        }
                        KeyCode::Esc => term_wm_pty_engine::input_encoding::KeyCode::Esc,
                        KeyCode::Left => term_wm_pty_engine::input_encoding::KeyCode::Left,
                        KeyCode::Right => term_wm_pty_engine::input_encoding::KeyCode::Right,
                        KeyCode::Up => term_wm_pty_engine::input_encoding::KeyCode::Up,
                        KeyCode::Down => term_wm_pty_engine::input_encoding::KeyCode::Down,
                        KeyCode::Home => term_wm_pty_engine::input_encoding::KeyCode::Home,
                        KeyCode::End => term_wm_pty_engine::input_encoding::KeyCode::End,
                        KeyCode::PageUp => term_wm_pty_engine::input_encoding::KeyCode::PageUp,
                        KeyCode::PageDown => term_wm_pty_engine::input_encoding::KeyCode::PageDown,
                        KeyCode::Delete => term_wm_pty_engine::input_encoding::KeyCode::Delete,
                        KeyCode::Insert => term_wm_pty_engine::input_encoding::KeyCode::Insert,
                        KeyCode::F(n) => term_wm_pty_engine::input_encoding::KeyCode::F(n),
                        _ => return EventResult::Ignored, // Media keys not supported
                    },
                    modifiers: term_wm_pty_engine::input_encoding::KeyModifiers {
                        shift: key.modifiers.shift,
                        control: key.modifiers.control,
                        alt: key.modifiers.alt,
                    },
                };
                let bytes = key_to_bytes(&pty_key);
                if bytes.is_empty() {
                    return EventResult::Ignored;
                }
                EventResult::Action(TermWmAction::KeyToBytes(bytes))
            }
            Event::Mouse(mouse) => {
                if !ctx.direct_mode() {
                    let selection_ready = self.selection_enabled;
                    let area = ctx.screen_area().unwrap_or_default();
                    if handle_selection_mouse(self, selection_ready, mouse, area) {
                        return EventResult::Consumed;
                    }
                    if self.try_handle_link_click(area, mouse) {
                        return EventResult::Consumed;
                    }
                }
                let area = ctx.screen_area().unwrap_or_default();
                let mut pane = self.pane.borrow_mut();
                let parser_arc = pane.shared_parser();
                let parser = parser_arc.lock().unwrap();
                let screen = parser.screen();
                let encoding = screen.mouse_protocol_encoding();

                match encoding {
                    MouseProtocolEncoding::Default | MouseProtocolEncoding::Sgr => {}
                    _ => return EventResult::Ignored,
                }

                let mode = screen.mouse_protocol_mode();
                let pty_kind = convert_mouse_event_kind(mouse.kind);
                if !mouse_event_allowed(mode, pty_kind) {
                    return EventResult::Ignored;
                }
                let Some((local_col, local_row)) =
                    localize_coordinate(area, mouse.column, mouse.row)
                else {
                    return EventResult::Ignored;
                };
                let local = MouseEvent {
                    column: local_col,
                    row: local_row,
                    kind: mouse.kind,
                    modifiers: mouse.modifiers,
                };
                let pty_mouse = convert_pty_mouse_event(&local);
                let bytes = mouse_event_to_bytes(&pty_mouse, encoding);
                if bytes.is_empty() {
                    return EventResult::Ignored;
                }
                EventResult::Action(TermWmAction::MouseToBytes(bytes))
            }
            _ => EventResult::Ignored,
        }
    }

    fn update(
        &mut self,
        action: TermWmAction,
        _ctx: &ComponentContext,
        _actions: &mut VecDeque<(WindowKey, TermWmAction)>,
    ) {
        match action {
            TermWmAction::KeyToBytes(bytes) => {
                self.selection.borrow_mut().clear();
                if self.pane.borrow_mut().scrollback() > 0 {
                    self.pane.borrow_mut().set_scrollback(0);
                }
                if let Err(_err) = self.pane.borrow_mut().write_bytes(&bytes) {
                    #[cfg(windows)]
                    eprintln!("terminal input write failed: {_err}");
                }
            }
            TermWmAction::Scroll(delta) => {
                self.scroll_scrollback(delta);
            }
            TermWmAction::MouseToBytes(bytes) => {
                if let Err(_err) = self.pane.borrow_mut().write_bytes(&bytes) {
                    #[cfg(windows)]
                    eprintln!("terminal mouse write failed: {_err}");
                }
            }
            _ => {}
        }
    }

    fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        ctx: &ComponentContext,
        registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        if !ctx.focused() {
            self.selection.borrow_mut().clear();
        }
        if area.height == 0 || area.width == 0 {
            return;
        }
        let size = (area.width, area.height);
        if size != self.last_size.get() {
            let mut pane = self.pane.borrow_mut();
            let _ = pane.resize(PtySize {
                rows: area.height,
                cols: area.width,
                pixel_width: 0,
                pixel_height: 0,
            });
            self.last_size.set(size);
        }
        // The render-local `area` is offscreen-local; `screen_area` is absolute.
        let screen_area_lr = ctx.screen_area().unwrap_or(LayoutRect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: area.height,
        });
        let _exited = self.pane.borrow_mut().has_exited();
        // Register this terminal's clickable area in the hitbox registry.
        // Use screen coordinates so hit_test matches screen-space mouse positions.
        if let Some(key) = ctx.window_key() {
            registry.register(
                self.hitbox_id,
                term_wm_core::hitbox_registry::ComponentOwner::Window(key),
                screen_area_lr,
            );
        }
        self.render_screen(backend, area, ctx);
    }

    fn destroy(&mut self) {
        // Kill the child process on teardown so the OS reaps it.
        let _ = self.pane.get_mut().kill_child();
    }

    fn selection_status(&self) -> SelectionStatus {
        if !self.selection_enabled {
            return SelectionStatus::default();
        }
        let sel = self.selection.borrow();
        SelectionStatus {
            active: sel.has_selection(),
            dragging: sel.is_dragging(),
        }
    }

    fn selection_text(&self) -> Option<String> {
        if !self.selection_enabled {
            return None;
        }
        let range = self.selection.borrow().selection_range()?.normalized();
        if !range.is_non_empty() {
            return None;
        }
        self.selection_text_for_range(range)
    }

    fn clear_selection(&mut self) {
        self.selection.get_mut().clear();
    }

    fn paste(&mut self, text: &str) -> bool {
        if text.is_empty() {
            return false;
        }
        self.pane.borrow_mut().write_bytes(text.as_bytes()).is_ok()
    }

    fn take_pending_title(&mut self) -> Option<String> {
        self.pane.get_mut().take_pending_title()
    }

    fn set_selection_enabled(&mut self, enabled: bool) {
        if self.selection_enabled == enabled {
            return;
        }
        self.selection_enabled = enabled;
        if !enabled {
            self.selection.get_mut().clear();
        }
    }

    fn hitbox_id(&self) -> Option<HitboxId> {
        Some(self.hitbox_id)
    }
}

impl TerminalComponent {
    /// Return a reasonable default PTY size used when spawning a terminal
    /// when the caller doesn't need to pick a custom size.
    pub fn default_pty_size() -> PtySize {
        PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        }
    }

    /// Convenience spawn that uses `default_pty_size()`.
    pub fn spawn_default(command: CommandBuilder) -> term_wm_pty_engine::PtyResult<Self> {
        Self::spawn(command, Self::default_pty_size())
    }

    /// Construct a terminal wrapper around any Pane implementation.
    pub fn from_pane(pane: Box<dyn Pane>) -> Self {
        Self {
            hitbox_id: HitboxId::new(),
            pane: RefCell::new(pane),
            last_size: Cell::new((80, 24)),
            linkifier: Linkifier::new(),
            link_overlay: RefCell::new(LinkOverlay::new()),
            link_handler: None,
            command_description: "pane-override".to_string(),
            selection: RefCell::new(SelectionController::new()),
            selection_enabled: false,
            last_scrollback: Cell::new(0),
            last_max_scrollback: Cell::new(0),
            window_key: None,
        }
    }

    pub fn spawn(command: CommandBuilder, size: PtySize) -> term_wm_pty_engine::PtyResult<Self> {
        let command_description = format!("{:?}", command);
        let pane: Box<dyn Pane> = Box::new(term_wm_pty_engine::Pty::spawn_with_scrollback(
            command,
            size,
            DEFAULT_SCROLLBACK_LEN,
        )?);
        let comp = Self {
            hitbox_id: HitboxId::new(),
            pane: RefCell::new(pane),
            last_size: Cell::new((size.cols, size.rows)),
            linkifier: Linkifier::new(),
            link_overlay: RefCell::new(LinkOverlay::new()),
            link_handler: None,
            command_description,
            selection: RefCell::new(SelectionController::new()),
            selection_enabled: false,
            last_scrollback: Cell::new(0),
            last_max_scrollback: Cell::new(0),
            window_key: None,
        };
        Ok(comp)
    }

    pub fn write_bytes(&mut self, input: &[u8]) -> std::io::Result<()> {
        self.pane.get_mut().write_bytes(input)
    }

    #[allow(clippy::collapsible_if)]
    pub fn has_exited(&mut self) -> bool {
        let pane = self.pane.get_mut();
        let exited = pane.has_exited();
        if exited {
            if let Some(status) = pane.take_exit_status()
                && !status.success()
            {
                tracing::error!(
                    "Terminal exited with error: {:?} (Command: {})",
                    status,
                    self.command_description
                );
            }
        }
        exited
    }

    pub fn exit_status(&self) -> Option<portable_pty::ExitStatus> {
        self.pane.borrow().exit_status()
    }

    pub fn take_exit_status(&mut self) -> Option<portable_pty::ExitStatus> {
        self.pane.get_mut().take_exit_status()
    }

    pub fn bytes_received(&self) -> usize {
        self.pane.borrow().bytes_received()
    }

    pub fn last_bytes_text(&self) -> String {
        self.pane.borrow().last_bytes_text()
    }

    pub fn set_link_handler(&mut self, handler: Option<LinkHandler>) {
        self.link_handler = handler;
    }

    pub fn set_link_handler_fn<F>(&mut self, handler: F)
    where
        F: Fn(&str) -> bool + Send + Sync + 'static,
    {
        self.link_handler = Some(Arc::new(handler));
    }

    /// Direct access to internal state for testing scroll sync logic.
    #[cfg(test)]
    pub fn set_last_scrollback(&mut self, val: usize) {
        self.last_scrollback.set(val);
    }

    #[cfg(test)]
    pub fn set_last_max_scrollback(&mut self, val: usize) {
        self.last_max_scrollback.set(val);
    }

    #[cfg(test)]
    pub fn pane_mut(&mut self) -> &mut Box<dyn Pane> {
        self.pane.get_mut()
    }

    pub fn take_pending_title(&mut self) -> Option<String> {
        self.pane.get_mut().take_pending_title()
    }

    fn render_screen(
        &self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        ctx: &ComponentContext,
    ) {
        let screen_area = ctx.screen_area().unwrap_or(area);

        // 1. PULL EXTERNAL SCROLL STATE FIRST — apply scroll wheel changes
        //    to internal scrollback before drag maintenance uses them.
        let sb_before_drag = {
            let clipped = layout_rect_to_clipped_rect(area);
            let mut pane = self.pane.borrow_mut();
            if !pane.alternate_screen()
                && let Some(handle) = ctx.scroll_handle()
            {
                let used = pane.max_scrollback();
                handle.set_content_size(clipped.width as usize, used + clipped.height as usize);

                let current_sb = pane.scrollback();
                let view_offset = ctx.viewport().offset_y;
                if current_sb == 0 {
                    if view_offset < self.last_max_scrollback.get().saturating_sub(1) {
                        let target_sb = used.saturating_sub(view_offset);
                        pane.set_scrollback(target_sb);
                    } else {
                        handle.scroll_vertical_to(usize::MAX);
                    }
                } else if current_sb != self.last_scrollback.get() {
                    let new_offset = used.saturating_sub(current_sb);
                    handle.scroll_vertical_to(new_offset);
                } else {
                    let target_sb = used.saturating_sub(view_offset);
                    if target_sb != current_sb {
                        pane.set_scrollback(target_sb);
                    }
                }
                self.last_max_scrollback.set(used);
            }
            pane.scrollback()
        };

        // 2. MAINTAIN DRAG SELECTION — using accurate, freshly-pulled scroll state
        {
            let mut sel_guard = self.selection.borrow_mut();
            let mut dh = RenderDragHost {
                selection: &mut sel_guard,
                pane: &self.pane,
                viewport_width: area.width,
                viewport_height: area.height,
            };
            maintain_selection_drag(&mut dh, screen_area);
        }

        // 3. PUSH INTERNAL AUTO-SCROLLS back to parent ScrollView
        //    (only if scrollback changed during drag maintenance above)
        let mut pane = self.pane.borrow_mut();
        let new_sb = pane.scrollback();
        if new_sb != sb_before_drag
            && !pane.alternate_screen()
            && let Some(handle) = ctx.scroll_handle()
        {
            let used = pane.max_scrollback();
            handle.scroll_vertical_to(used.saturating_sub(new_sb));
        }
        self.last_scrollback.set(new_sb);

        // 4. Shadow to clipped Rect for ratatui rendering
        let area = layout_rect_to_clipped_rect(area);

        let scrollback_value = new_sb;
        let show_cursor = scrollback_value == 0;
        let used = pane.max_scrollback();
        let selection_row_base = used.saturating_sub(scrollback_value);
        let selection_range = if self.selection_enabled {
            self.selection
                .borrow()
                .selection_range()
                .filter(|r| r.is_non_empty())
                .map(|r| r.normalized())
        } else {
            None
        };
        let backend = crate::helpers::downcast_ratatui(backend);
        let buffer = &mut backend.buffer;

        let visible = area.intersection(buffer.area);
        if visible.width == 0 || visible.height == 0 {
            self.link_overlay.borrow_mut().clear();
            return;
        }

        let start_col = visible.x.saturating_sub(area.x);
        let start_row = visible.y.saturating_sub(area.y);

        // Call sync_screen() to handle DSR, foreground polling.
        pane.sync_screen();

        // Lock the shared parser once for both link overlay and cell rendering.
        let parser_arc = pane.shared_parser();
        let parser = parser_arc.lock().unwrap();
        let screen = parser.screen();

        let bytes_seen = pane.bytes_received();
        let signature = OverlaySignature::new(
            bytes_seen,
            scrollback_value,
            area.width,
            area.height,
            start_row,
            start_col,
        );
        if !self.link_overlay.borrow().is_signature_current(&signature) {
            let viewport_height = area.height as usize;
            let viewport_width = area.width as usize;
            let mut row_data: Vec<(usize, usize, String, Vec<usize>)> =
                Vec::with_capacity(visible.height as usize);
            for row in start_row..start_row + visible.height {
                let viewport_row = row.saturating_sub(start_row) as usize;
                if viewport_row >= viewport_height {
                    continue;
                }
                let mut line = String::with_capacity(visible.width as usize);
                let mut offsets = Vec::with_capacity(visible.width as usize + 1);
                offsets.push(0);
                for col in start_col..start_col + visible.width {
                    let ch = screen
                        .cell(row, col)
                        .and_then(|cell| cell.contents().chars().next())
                        .unwrap_or(' ');
                    line.push(ch);
                    offsets.push(line.len());
                }
                row_data.push((viewport_row, start_col as usize, line, offsets));
            }
            self.link_overlay.borrow_mut().update_view(
                signature,
                viewport_height,
                viewport_width,
                &row_data,
                &self.linkifier,
            );
        }

        // Hoist loop-invariant color defaults
        let default_fg = screen.fgcolor();
        let default_bg = screen.bgcolor();

        let focused = ctx.focused();
        for row in start_row..start_row + visible.height {
            for col in start_col..start_col + visible.width {
                let cell_x = area.x.saturating_add(col);
                let cell_y = area.y.saturating_add(row);
                let viewport_row = row.saturating_sub(start_row) as usize;
                let viewport_col = col.saturating_sub(start_col) as usize;

                if let Some(cell) = screen.cell(row, col) {
                    let mut symbol = cell.contents().chars().next().unwrap_or(' ');
                    let (fg, bg) = resolve_colors_with_defaults(cell, default_fg, default_bg);
                    let mut style = Style::default();
                    if let Some(fg) = fg {
                        style = style.fg(fg);
                    }
                    if let Some(bg) = bg {
                        style = style.bg(bg);
                    }
                    if cell.bold() {
                        style = style.add_modifier(Modifier::BOLD);
                    }
                    if cell.dim() {
                        style = style.add_modifier(Modifier::DIM);
                    }
                    if cell.italic() {
                        style = style.add_modifier(Modifier::ITALIC);
                    }
                    if cell.underline() {
                        style = style.add_modifier(Modifier::UNDERLINED);
                    }
                    if cell.inverse() {
                        style = style.add_modifier(Modifier::REVERSED);
                    }
                    if cell.is_wide_continuation() {
                        symbol = ' ';
                    }

                    let theme = ctx.config().theme;
                    if self
                        .link_overlay
                        .borrow()
                        .is_link_cell(viewport_row, viewport_col)
                    {
                        style = decorate_link_style(style, &theme);
                    }

                    if let Some(range) = selection_range {
                        let abs_row = selection_row_base.saturating_add(row as usize);
                        let abs_col = col as usize;
                        if range.contains(LogicalPosition::new(abs_row, abs_col)) {
                            style = style
                                .bg(color_to_ratatui(theme.selection_bg))
                                .fg(color_to_ratatui(theme.selection_fg));
                        }
                    }

                    if let Some(buf_cell) = buffer.cell_mut((cell_x, cell_y)) {
                        let mut buf = [0u8; 4];
                        if bg.is_none() {
                            buf_cell.reset();
                        }
                        let sym = symbol.encode_utf8(&mut buf);
                        buf_cell.set_symbol(sym).set_style(style);
                    }
                } else if let Some(buf_cell) = buffer.cell_mut((cell_x, cell_y)) {
                    buf_cell.reset();
                    buf_cell.set_symbol(" ");
                }
            }
        }

        // Clear dirty and notify reader thread via Condvar.
        // This is the primary mechanism for I/O burst budget backpressure.
        pane.clear_dirty_and_notify();

        if focused && !screen.hide_cursor() && show_cursor {
            let (row, col) = screen.cursor_position();
            if row < area.height
                && col < area.width
                && let Some(cell) = buffer.cell_mut((area.x + col, area.y + row))
            {
                cell.set_style(cell.style().add_modifier(Modifier::REVERSED));
            }
        }
    }
}

/// RAII guard that saves and restores the vt100 scrollback position.
/// Guarantees restoration even on panic or early return.
struct ScrollbackGuard<'a> {
    screen: &'a mut vt100::Screen,
    original_offset: usize,
}

impl<'a> ScrollbackGuard<'a> {
    fn new(screen: &'a mut vt100::Screen) -> Self {
        let original_offset = screen.scrollback();
        Self {
            screen,
            original_offset,
        }
    }
}

impl<'a> Drop for ScrollbackGuard<'a> {
    fn drop(&mut self) {
        self.screen.set_scrollback(self.original_offset);
    }
}

impl SelectionViewport for TerminalComponent {
    fn selection_viewport(&self, area: LayoutRect) -> LayoutRect {
        area
    }

    fn logical_position_from_point(
        &mut self,
        area: LayoutRect,
        column: u16,
        row: u16,
    ) -> Option<LogicalPosition> {
        TerminalComponent::logical_position_from_point(self, area, column, row)
    }

    fn scroll_selection_vertical(&mut self, delta: isize) {
        if delta == 0 {
            return;
        }
        self.scroll_scrollback(-delta);
    }

    fn scroll_selection_horizontal(&mut self, _delta: isize) {}
}

impl SelectionHost for TerminalComponent {
    fn selection_controller(&mut self) -> &mut SelectionController {
        self.selection.get_mut()
    }
}

#[cfg(unix)]
pub fn default_shell() -> String {
    std::env::var("SHELL")
        .unwrap_or_else(|_| term_wm_core::constants::DEFAULT_SHELL_FALLBACK.to_string())
}

#[cfg(windows)]
pub fn default_shell() -> String {
    std::env::var("COMSPEC")
        .unwrap_or_else(|_| term_wm_core::constants::DEFAULT_SHELL_FALLBACK.to_string())
}

pub fn default_shell_command() -> CommandBuilder {
    let mut cmd = CommandBuilder::new(default_shell());
    if let Ok(cwd) = std::env::current_dir() {
        cmd.cwd(cwd);
    }

    cmd
}

impl TerminalComponent {
    fn scroll_scrollback(&mut self, delta: isize) {
        let pane = self.pane.get_mut();
        let current = pane.scrollback() as isize;
        let next = (current + delta).clamp(0, pane.scrollback_len() as isize) as usize;
        pane.set_scrollback(next);
    }

    fn logical_position_from_point(
        &mut self,
        area: LayoutRect,
        column: u16,
        row: u16,
    ) -> Option<LogicalPosition> {
        let (local_col, local_row) = localize_coordinate_clamped(area, column, row)?;
        let pane = self.pane.get_mut();
        let scrollback_value = pane.scrollback();
        let used = pane.max_scrollback();
        let row_base = used.saturating_sub(scrollback_value);
        Some(LogicalPosition::new(
            row_base.saturating_add(local_row as usize),
            local_col as usize,
        ))
    }

    fn selection_text_for_range(&self, range: SelectionRange) -> Option<String> {
        let mut pane = self.pane.borrow_mut();

        // 1. SAFE EXTRACTION: Get max_scrollback BEFORE locking the parser
        // This prevents Mutex deadlocks if the pane internally accesses the parser.
        let max_scrollback = pane.max_scrollback();

        let parser_arc = pane.shared_parser();
        let mut parser = parser_arc.lock().unwrap();
        let screen = parser.screen_mut();

        let (viewport_rows, cols) = screen.size();
        if viewport_rows == 0 || cols == 0 {
            return None;
        }

        let guard = ScrollbackGuard::new(screen);

        // 2. BOUNDED PROBE: Use the known max_scrollback instead of usize::MAX.
        // This prevents O(N) CPU hangs and integer overflow panics.
        guard.screen.set_scrollback(max_scrollback);
        let vt100_max_scrollback = guard.screen.scrollback();
        let vt100_total_lines = vt100_max_scrollback + viewport_rows as usize;

        // Map Pane coordinates to vt100 coordinates
        let offset_from_pane_to_vt100 = max_scrollback.saturating_sub(vt100_max_scrollback);

        let mut end_row = range.end.row;
        let mut end_col = range.end.column;
        if end_col == 0 && end_row > range.start.row {
            end_row = end_row.saturating_sub(1);
            end_col = cols as usize;
        }

        let vt100_start_row = range.start.row.saturating_sub(offset_from_pane_to_vt100);
        let vt100_end_row = end_row.saturating_sub(offset_from_pane_to_vt100);

        // Clamp to actual vt100 bounds
        let vt100_start_row = vt100_start_row.min(vt100_total_lines.saturating_sub(1));
        let vt100_end_row = vt100_end_row.min(vt100_total_lines.saturating_sub(1));
        let start_col = range.start.column.min(cols as usize);
        let end_col = end_col.min(cols as usize);

        if vt100_end_row < vt100_start_row {
            return None;
        }

        let mut result = String::new();

        // Paginate using the translated vt100 coordinates
        for absolute_row in vt100_start_row..=vt100_end_row {
            let viewport_start = absolute_row.min(vt100_max_scrollback);
            let offset = vt100_max_scrollback - viewport_start;
            guard.screen.set_scrollback(offset);

            let viewport_row = (absolute_row - viewport_start) as u16;

            let col_start = if absolute_row == vt100_start_row {
                start_col as u16
            } else {
                0
            };

            let col_end = if absolute_row == vt100_end_row {
                end_col as u16
            } else {
                cols
            };

            let line =
                guard
                    .screen
                    .contents_between(viewport_row, col_start, viewport_row, col_end);
            result.push_str(&line);

            if absolute_row < vt100_end_row {
                result.push('\n');
            }
        }

        Some(result)
    }

    pub fn terminate(&mut self) {
        let _ = self.pane.get_mut().kill_child();
    }

    pub fn take_parts(
        &mut self,
    ) -> Option<(
        Box<dyn portable_pty::Child + Send + Sync>,
        std::thread::JoinHandle<()>,
    )> {
        self.pane.get_mut().take_parts()
    }

    pub fn set_status_callback(&mut self, cb: Option<Box<dyn Fn(PtyStatus) + Send + Sync>>) {
        self.pane.get_mut().set_status_callback(cb);
    }

    fn link_at_position(&self, area: LayoutRect, mouse: &MouseEvent) -> Option<String> {
        let (local_x, local_y) = localize_coordinate(area, mouse.column, mouse.row)?;
        self.link_overlay
            .borrow()
            .link_at(local_y as usize, local_x as usize)
    }

    fn try_handle_link_click(&mut self, area: LayoutRect, mouse: &MouseEvent) -> bool {
        if !matches!(mouse.kind, MouseEventKind::Press(MouseButton::Left)) {
            return false;
        }

        if let Some(url) = self.link_at_position(area, mouse)
            && self.invoke_link_handler(&url)
        {
            return true;
        }

        false
    }

    fn invoke_link_handler(&self, url: &str) -> bool {
        if let Some(handler) = &self.link_handler {
            handler(url)
        } else {
            webbrowser::open(url).is_ok()
        }
    }
}

/// Helper that bridges interior-mutability fields to the `SelectionViewport` /
/// `SelectionHost` traits so `maintain_selection_drag` can be called from
/// `render(&self)`.
struct RenderDragHost<'a> {
    selection: &'a mut SelectionController,
    pane: &'a RefCell<Box<dyn Pane>>,
    viewport_width: u16,
    viewport_height: u16,
}

impl SelectionViewport for RenderDragHost<'_> {
    fn selection_viewport(&self, area: LayoutRect) -> LayoutRect {
        area
    }

    fn selection_viewport_offsets(&self) -> (usize, usize) {
        let mut pane = self.pane.borrow_mut();
        let scrollback = pane.scrollback();
        let used = pane.max_scrollback();
        (0, used.saturating_sub(scrollback))
    }

    fn selection_content_size(&self) -> (usize, usize) {
        let mut pane = self.pane.borrow_mut();
        let used = pane.max_scrollback();
        (
            self.viewport_width as usize,
            used + self.viewport_height as usize,
        )
    }

    fn logical_position_from_point(
        &mut self,
        area: LayoutRect,
        column: u16,
        row: u16,
    ) -> Option<LogicalPosition> {
        let (local_col, local_row) = localize_coordinate_clamped(area, column, row)?;
        let mut pane = self.pane.borrow_mut();
        let scrollback_value = pane.scrollback();
        let used = pane.max_scrollback();
        let row_base = used.saturating_sub(scrollback_value);
        Some(LogicalPosition::new(
            row_base.saturating_add(local_row as usize),
            local_col as usize,
        ))
    }

    fn scroll_selection_vertical(&mut self, delta: isize) {
        if delta == 0 {
            return;
        }
        let mut pane = self.pane.borrow_mut();
        let current = pane.scrollback() as isize;
        let next = (current - delta).clamp(0, pane.scrollback_len() as isize) as usize;
        pane.set_scrollback(next);
    }

    fn scroll_selection_horizontal(&mut self, _delta: isize) {}
}

impl SelectionHost for RenderDragHost<'_> {
    fn selection_controller(&mut self) -> &mut SelectionController {
        self.selection
    }
}

#[allow(dead_code)]
fn resolve_colors(cell: &vt100::Cell, screen: &vt100::Screen) -> (Option<TColor>, Option<TColor>) {
    let mut fg = resolve_color(cell.fgcolor(), screen.fgcolor());
    let bg = resolve_color(cell.bgcolor(), screen.bgcolor());
    if cell.bold() {
        fg = brighten_indexed(fg);
    }
    (fg, bg)
}

/// Like `resolve_colors` but takes pre-computed default fg/bg colors
/// to avoid redundant `screen.fgcolor()`/`bgcolor()` calls per cell.
fn resolve_colors_with_defaults(
    cell: &vt100::Cell,
    default_fg: vt100::Color,
    default_bg: vt100::Color,
) -> (Option<TColor>, Option<TColor>) {
    let mut fg = resolve_color(cell.fgcolor(), default_fg);
    let bg = resolve_color(cell.bgcolor(), default_bg);
    if cell.bold() {
        fg = brighten_indexed(fg);
    }
    (fg, bg)
}

fn vt_color_to_ratatui(color: vt100::Color) -> Option<TColor> {
    #[allow(unused_imports)]
    use term_wm_core::term_color::map_rgb_to_color;
    match color {
        vt100::Color::Default => None,
        vt100::Color::Idx(idx) => Some(TColor::Indexed(idx)),
        vt100::Color::Rgb(r, g, b) => Some(crate::helpers::map_rgb_to_ratatui(r, g, b)),
    }
}

fn resolve_color(color: vt100::Color, screen_default: vt100::Color) -> Option<TColor> {
    match color {
        vt100::Color::Default => match screen_default {
            // Default to Reset (No Color) which ratatui treats as "Inherit" or "Transparent" usually.
            // But since this is a Terminal component, we treat Default as Black/Opaque if undefined,
            // otherwise we risk bleeding through windows underneath.
            vt100::Color::Default => None,
            other => vt_color_to_ratatui(other),
        },
        other => vt_color_to_ratatui(other),
    }
}

fn brighten_indexed(color: Option<TColor>) -> Option<TColor> {
    match color {
        Some(TColor::Indexed(idx)) if idx < 8 => Some(TColor::Indexed(idx + 8)),
        _ => color,
    }
}

/// Simulated terminal pane for testing scroll synchronization logic without a
/// real PTY process.
#[cfg(test)]
struct TestPane {
    parser: std::sync::Arc<std::sync::Mutex<vt100::Parser>>,
    current_scrollback: usize,
    max_sb: usize,
    alt_screen: bool,
    pending_title: Option<String>,
    kill_count: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(test)]
impl TestPane {
    fn new(max_sb: usize) -> Self {
        Self {
            parser: std::sync::Arc::new(std::sync::Mutex::new(vt100::Parser::new(24, 80, max_sb))),
            current_scrollback: 0,
            max_sb,
            alt_screen: false,
            pending_title: None,
            kill_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    fn with_kill_tracker(max_sb: usize) -> (Self, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        let kill_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let pane = Self {
            parser: std::sync::Arc::new(std::sync::Mutex::new(vt100::Parser::new(24, 80, max_sb))),
            current_scrollback: 0,
            max_sb,
            alt_screen: false,
            pending_title: None,
            kill_count: std::sync::Arc::clone(&kill_count),
        };
        (pane, kill_count)
    }

    fn set_scrollback_value(&mut self, val: usize) {
        self.current_scrollback = val.min(self.max_sb);
    }

    fn write_to_parser(&mut self, bytes: &[u8]) {
        let mut parser = self.parser.lock().unwrap();
        parser.process(bytes);
    }

    fn set_parser_size(&mut self, rows: u16, cols: u16) {
        let mut parser = self.parser.lock().unwrap();
        parser.screen_mut().set_size(rows, cols);
    }

    #[allow(dead_code)]
    fn set_pending_title(&mut self, title: &str) {
        self.pending_title = Some(title.to_string());
    }
}

#[cfg(test)]
impl Pane for TestPane {
    fn resize(&mut self, _size: PtySize) -> term_wm_pty_engine::PtyResult<()> {
        Ok(())
    }

    fn has_exited(&mut self) -> bool {
        false
    }

    fn alternate_screen(&mut self) -> bool {
        self.alt_screen
    }

    fn scrollback(&mut self) -> usize {
        self.current_scrollback
    }

    fn set_scrollback(&mut self, rows: usize) {
        self.current_scrollback = rows.min(self.max_sb);
    }

    fn write_bytes(&mut self, _input: &[u8]) -> std::io::Result<()> {
        Ok(())
    }

    fn shared_parser(&mut self) -> std::sync::Arc<std::sync::Mutex<vt100::Parser>> {
        self.parser.clone()
    }

    fn max_scrollback(&mut self) -> usize {
        self.max_sb
    }

    fn scrollback_len(&self) -> usize {
        0
    }

    fn take_exit_status(&mut self) -> Option<portable_pty::ExitStatus> {
        None
    }

    fn exit_status(&self) -> Option<portable_pty::ExitStatus> {
        None
    }

    fn bytes_received(&self) -> usize {
        0
    }

    fn last_bytes_text(&self) -> String {
        String::new()
    }

    fn kill_child(&mut self) -> term_wm_pty_engine::PtyResult<()> {
        self.kill_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    fn take_pending_title(&mut self) -> Option<String> {
        self.pending_title.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use std::cell::RefCell;
    use std::rc::Rc;
    use term_wm_core::component_context::ScrollHandle;
    use term_wm_layout_engine::LayoutRect;

    fn make_ctx(view_offset: usize, handle: ScrollHandle) -> ComponentContext {
        ComponentContext::default().with_viewport(
            term_wm_core::component_context::ScrollViewport {
                offset_x: 0,
                offset_y: view_offset,
                width: 80,
                height: 24,
            },
            Some(handle),
        )
    }

    fn make_handle() -> (
        ScrollHandle,
        Rc<RefCell<term_wm_core::component_context::ScrollBounds>>,
    ) {
        let shared = Rc::new(RefCell::new(
            term_wm_core::component_context::ScrollBounds {
                offset_x: 0,
                offset_y: 0,
                width: 80,
                height: 24,
                content_width: 80,
                content_height: 24,
                pending_offset_x: None,
                pending_offset_y: None,
                sticky_bottom: false,
            },
        ));
        (
            ScrollHandle {
                scroll: shared.clone(),
            },
            shared,
        )
    }

    fn run_sync(
        term: &mut TerminalComponent,
        view_offset: usize,
    ) -> (
        Rc<RefCell<term_wm_core::component_context::ScrollBounds>>,
        usize,
    ) {
        let (handle, shared) = make_handle();
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let buffer = Buffer::empty(area);
        let mut backend = term_wm_console::RatatuiBackend::new(buffer, area);
        let ctx = make_ctx(view_offset, handle);
        term.render(
            &mut backend,
            LayoutRect {
                x: 0,
                y: 0,
                width: 80,
                height: 24,
            },
            &ctx,
            &mut term_wm_core::hitbox_registry::HitboxRegistry::new(),
        );
        let sb = term.pane_mut().scrollback();
        (shared, sb)
    }

    fn run_sync_with_handle(
        term: &mut TerminalComponent,
        shared: &Rc<RefCell<term_wm_core::component_context::ScrollBounds>>,
    ) -> usize {
        let handle = ScrollHandle {
            scroll: shared.clone(),
        };
        let view_offset = shared.borrow().offset_y;
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let buffer = Buffer::empty(area);
        let mut backend = term_wm_console::RatatuiBackend::new(buffer, area);
        let ctx = make_ctx(view_offset, handle);
        term.render(
            &mut backend,
            LayoutRect {
                x: 0,
                y: 0,
                width: 80,
                height: 24,
            },
            &ctx,
            &mut term_wm_core::hitbox_registry::HitboxRegistry::new(),
        );
        term.pane_mut().scrollback()
    }

    // --- Destroy / kill tests ---

    #[test]
    fn destroy_calls_kill_child() {
        let (pane, kill_count) = TestPane::with_kill_tracker(200);
        let mut term = TerminalComponent::from_pane(Box::new(pane));
        term.destroy();
        assert!(
            kill_count.load(std::sync::atomic::Ordering::SeqCst) >= 1,
            "destroy() must call pane.kill_child()"
        );
    }

    // --- Scroll sync tests ---

    #[test]
    fn scroll_sync_current_sb_zero_view_offset_below_last_max_syncs_scrollback() {
        // current_sb=0, view_offset < last_max_scrollback - 1
        // Expect: set_scrollback is called (scrollback becomes non-zero)
        let mut term = TerminalComponent::from_pane(Box::new(TestPane::new(200)));
        term.set_last_scrollback(0);
        // last_max_scrollback was 100 from previous render, viewport is at offset 50
        term.set_last_max_scrollback(100);
        let (shared, sb) = run_sync(&mut term, 50);
        assert!(sb > 0, "should have scrolled back from viewport offset");
        let max_sb = term.pane_mut().max_scrollback();
        assert_eq!(
            sb,
            max_sb.saturating_sub(50),
            "scrollback should equal used - view_offset"
        );
        // handle should NOT have scroll_vertical_to (set_content_size doesn't set pending)
        assert_eq!(
            shared.borrow().offset_y,
            0,
            "viewport offset should be 0 (set by content_size init)"
        );
        assert!(
            shared.borrow().pending_offset_y.is_none(),
            "scroll_vertical_to should not have been called"
        );
    }

    #[test]
    fn scroll_sync_current_sb_zero_view_offset_at_last_max_follows_tail() {
        // current_sb=0, view_offset >= last_max_scrollback - 1
        // Expect: scroll_vertical_to(usize::MAX) is called
        let mut term = TerminalComponent::from_pane(Box::new(TestPane::new(200)));
        term.set_last_scrollback(0);
        term.set_last_max_scrollback(100);
        let (shared, sb) = run_sync(&mut term, 99);
        assert_eq!(sb, 0, "should remain at bottom");
        assert_eq!(
            shared.borrow().pending_offset_y,
            Some(200),
            "viewport should be scrolled to max (200)"
        );
    }

    #[test]
    fn scroll_sync_current_sb_zero_view_offset_greater_than_last_max_follows_tail() {
        // view_offset > last_max_scrollback - 1 also triggers follow-tail
        let mut term = TerminalComponent::from_pane(Box::new(TestPane::new(200)));
        term.set_last_scrollback(0);
        term.set_last_max_scrollback(100);
        let (shared, sb) = run_sync(&mut term, 150);
        assert_eq!(sb, 0, "should remain at bottom");
        assert_eq!(
            shared.borrow().pending_offset_y,
            Some(200),
            "viewport should be scrolled to max"
        );
    }

    #[test]
    fn scroll_sync_current_sb_changed_from_last_pushes_to_viewport() {
        // current_sb != last_scrollback — push internal scrollback to viewport
        let mut term = TerminalComponent::from_pane(Box::new(TestPane::new(200)));
        term.pane_mut().set_scrollback(50);
        term.set_last_scrollback(0);
        term.set_last_max_scrollback(200);
        let (shared, sb) = run_sync(&mut term, 0);
        assert_eq!(sb, 50, "scrollback unchanged by sync");
        assert_eq!(
            shared.borrow().pending_offset_y,
            Some(150),
            "viewport should show offset = used - scrollback = 200-50"
        );
    }

    #[test]
    fn scroll_sync_current_sb_matches_last_syncs_from_viewport() {
        // current_sb == last_scrollback > 0 — sync from viewport
        let mut term = TerminalComponent::from_pane(Box::new(TestPane::new(200)));
        term.pane_mut().set_scrollback(50);
        term.set_last_scrollback(50);
        term.set_last_max_scrollback(200);
        // viewport at offset 100 means target_sb = 200-100 = 100
        let (shared, sb) = run_sync(&mut term, 100);
        assert_eq!(sb, 100, "should sync scrollback from viewport offset");
        assert_eq!(
            shared.borrow().pending_offset_y,
            None,
            "viewport should NOT be scrolled"
        );
    }

    #[test]
    fn scroll_sync_current_sb_matches_last_viewport_same_does_nothing() {
        // current_sb == last_scrollback AND target_sb == current_sb — no-op
        let mut term = TerminalComponent::from_pane(Box::new(TestPane::new(200)));
        term.pane_mut().set_scrollback(100);
        term.set_last_scrollback(100);
        term.set_last_max_scrollback(200);
        // viewport offset 100 → target_sb = 200-100 = 100 → same as current → no-op
        let (shared, sb) = run_sync(&mut term, 100);
        assert_eq!(sb, 100, "scrollback unchanged");
        assert!(
            shared.borrow().pending_offset_y.is_none(),
            "viewport unchanged"
        );
    }

    #[test]
    fn scroll_sync_alternate_screen_skips_sync() {
        // When alternate_screen is true, the entire sync block is skipped
        let mut term = TerminalComponent::from_pane(Box::new(TestPane::new(200)));
        term.pane_mut().set_scrollback(50);
        term.set_last_scrollback(50);
        term.set_last_max_scrollback(200);
        // Hack: need a way to set alt_screen — TestPane controls it
        // TestPane is private to this module, so we need to access it via pane_mut
        // But pane_mut returns &mut Box<dyn Pane>, not &mut TestPane...
        // We need to downcast or add a method.
        // Instead, let's use a different approach: make a TestPane, set alt_screen,
        // then call sync logic directly
        let mut pane = TestPane::new(200);
        pane.alt_screen = true;
        pane.set_scrollback(50);
        let mut term = TerminalComponent::from_pane(Box::new(pane));
        term.set_last_scrollback(50);
        term.set_last_max_scrollback(200);
        let (handle, shared) = make_handle();
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let buffer = Buffer::empty(area);
        let mut backend = term_wm_console::RatatuiBackend::new(buffer, area);
        let ctx = make_ctx(100, handle);
        term.render(
            &mut backend,
            LayoutRect {
                x: 0,
                y: 0,
                width: 80,
                height: 24,
            },
            &ctx,
            &mut term_wm_core::hitbox_registry::HitboxRegistry::new(),
        );
        let sb = term.pane_mut().scrollback();
        assert_eq!(sb, 50, "scrollback should be unchanged during alt screen");
        assert!(
            shared.borrow().pending_offset_y.is_none(),
            "viewport should NOT be touched during alt screen"
        );
        assert_eq!(
            shared.borrow().content_height,
            24,
            "content size should NOT have been set during alt screen"
        );
    }

    #[test]
    fn scroll_sync_current_sb_zero_view_offset_zero_content_grows() {
        // Empty pane, 0 max_scrollback, content just started filling
        let mut term = TerminalComponent::from_pane(Box::new(TestPane::new(0)));
        term.set_last_scrollback(0);
        term.set_last_max_scrollback(0);
        let (shared, sb) = run_sync(&mut term, 0);
        assert_eq!(sb, 0, "should stay at bottom");
        assert_eq!(
            shared.borrow().pending_offset_y,
            Some(0),
            "viewport should be at 0 (max of 0)"
        );
    }

    #[test]
    fn scroll_sync_user_scrolls_up_then_content_added() {
        // User scrolled up to offset 50, then content grew from 100 to 200
        // current_sb=0, view_offset=50, last_max_scrollback=100
        // Expect: sync from viewport since view_offset < last_max
        let pane = TestPane::new(200);
        let mut term = TerminalComponent::from_pane(Box::new(pane));
        term.set_last_scrollback(0);
        term.set_last_max_scrollback(100);
        let (shared, sb) = run_sync(&mut term, 50);
        assert_eq!(sb, 150, "scrollback = used(200) - view_offset(50)");
        assert!(
            shared.borrow().pending_offset_y == Some(0)
                || shared.borrow().pending_offset_y.is_none(),
            "viewport offset should be set by set_content_size, not scroll_vertical_to"
        );
    }

    #[test]
    fn scroll_sync_two_renders_user_scrolls_then_stays_synced() {
        // First render: at bottom, content grows from 0 to 100
        let pane = TestPane::new(100);
        let mut term = TerminalComponent::from_pane(Box::new(pane));
        let (handle, shared) = make_handle();
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let buffer = Buffer::empty(area);
        let mut backend = term_wm_console::RatatuiBackend::new(buffer, area);
        let ctx = make_ctx(0, handle);
        term.render(
            &mut backend,
            LayoutRect {
                x: 0,
                y: 0,
                width: 80,
                height: 24,
            },
            &ctx,
            &mut term_wm_core::hitbox_registry::HitboxRegistry::new(),
        );
        // After first render, last_max=100, last_sb=0, viewport at max (offset=100)
        let sb2 = run_sync_with_handle(&mut term, &shared);
        assert_eq!(sb2, 0, "at bottom");
        assert_eq!(shared.borrow().offset_y, 100, "viewport at max");
    }

    #[test]
    fn scroll_sync_user_manually_drags_scrollbar() {
        // User drags scrollbar, viewport offset changes independently
        let mut term = TerminalComponent::from_pane(Box::new(TestPane::new(100)));
        term.set_last_scrollback(0);
        term.set_last_max_scrollback(100);
        // view_offset=30, but last_max was 100
        let (_, sb) = run_sync(&mut term, 30);
        assert_eq!(sb, 70, "scrollback = 100 - 30 = 70");
    }

    #[test]
    fn scroll_sync_user_at_bottom_content_grows_pane_updated() {
        // User at bottom (current_sb=0, view_offset at max), content grows
        // from 100 to 200 between render 1 and render 2
        let pane = TestPane::new(100);
        let mut term = TerminalComponent::from_pane(Box::new(pane));
        let _ = run_sync(&mut term, 100);
        // Now grow content: we need a new pane, since TestPane has fixed max_sb
        let mut pane2 = TestPane::new(200);
        pane2.set_scrollback_value(0);
        let mut term2 = TerminalComponent::from_pane(Box::new(pane2));
        term2.set_last_scrollback(0);
        term2.set_last_max_scrollback(100);
        let (shared, sb) = run_sync(&mut term2, 100);
        assert_eq!(sb, 0, "at bottom");
        assert_eq!(
            shared.borrow().pending_offset_y,
            Some(200),
            "viewport should follow to new max"
        );
    }

    #[test]
    fn vt_color_and_resolve_color() {
        assert_eq!(vt_color_to_ratatui(vt100::Color::Default), None);
        assert_eq!(
            vt_color_to_ratatui(vt100::Color::Idx(5)),
            Some(TColor::Indexed(5))
        );
        // RGB passthrough depends on COLORTERM truecolor support;
        // on terminals without it, vt100::Color::Rgb maps to a nearest
        // indexed color. Assert the function produces *some* color.
        assert!(vt_color_to_ratatui(vt100::Color::Rgb(1, 2, 3)).is_some());

        // resolve_color: when both default -> None
        assert_eq!(
            resolve_color(vt100::Color::Default, vt100::Color::Default),
            None
        );
        // when screen default is idx, default maps to that
        assert_eq!(
            resolve_color(vt100::Color::Default, vt100::Color::Idx(7)),
            Some(TColor::Indexed(7))
        );
    }

    #[test]
    fn brighten_indexed_moves_0_7_to_8_15() {
        assert_eq!(
            brighten_indexed(Some(TColor::Indexed(0))),
            Some(TColor::Indexed(8))
        );
        assert_eq!(
            brighten_indexed(Some(TColor::Indexed(7))),
            Some(TColor::Indexed(15))
        );
        // values >=8 unchanged
        assert_eq!(
            brighten_indexed(Some(TColor::Indexed(8))),
            Some(TColor::Indexed(8))
        );
        assert_eq!(brighten_indexed(None), None);
    }

    #[test]
    fn scroll_sync_prevents_stuck_at_top_on_fill() {
        let pane = TestPane::new(0);
        let mut term = TerminalComponent::from_pane(Box::new(pane));
        let (shared, _) = run_sync(&mut term, 0);
        assert_eq!(
            shared.borrow().offset_y,
            0,
            "empty pane should leave viewport at 0"
        );
        let pane2 = TestPane::new(100);
        let mut term2 = TerminalComponent::from_pane(Box::new(pane2));
        term2.set_last_scrollback(0);
        term2.set_last_max_scrollback(0);
        let (shared2, sb) = run_sync(&mut term2, 0);
        assert_eq!(sb, 0, "content-filled pane should stay at bottom");
        assert_eq!(
            shared2.borrow().offset_y,
            100,
            "viewport should follow new content to max"
        );
    }

    // --- Selection text extraction tests ---

    /// Helper: creates a TerminalComponent with a TestPane containing known content.
    /// Returns (term, row_base) where row_base = max_scrollback - scrollback
    /// so tests can compute correct logical positions for visible row N as row_base + N.
    fn make_term_with_content(
        width: u16,
        height: u16,
        max_sb: usize,
        text: &str,
    ) -> (TerminalComponent, usize) {
        let mut pane = TestPane::new(max_sb);
        pane.set_parser_size(height, width);
        pane.write_to_parser(text.as_bytes());
        let mut term = TerminalComponent::from_pane(Box::new(pane));
        term.selection_enabled = true;
        let row_base = term.pane.borrow_mut().max_scrollback();
        (term, row_base)
    }

    #[test]
    fn selection_text_single_line_ascii() {
        let (term, rb) = make_term_with_content(80, 24, 2000, "Hello World");
        term.selection
            .borrow_mut()
            .begin_drag(LogicalPosition::new(rb, 0));
        term.selection
            .borrow_mut()
            .update_drag(LogicalPosition::new(rb, 11));
        let text = term.selection_text();
        assert_eq!(text, Some("Hello World".to_string()));
    }

    #[test]
    fn selection_text_multi_line_crlf() {
        let (term, rb) = make_term_with_content(80, 24, 2000, "Line one\r\nLine two\r\nLine three");
        term.selection
            .borrow_mut()
            .begin_drag(LogicalPosition::new(rb, 0));
        term.selection
            .borrow_mut()
            .update_drag(LogicalPosition::new(rb + 2, 5));
        let text = term.selection_text();
        assert_eq!(text, Some("Line one\nLine two\nLine ".to_string()));
    }

    #[test]
    fn selection_text_end_col_zero_adjustment() {
        let (term, rb) =
            make_term_with_content(80, 24, 2000, "First line of content\r\nSecond line here");
        term.selection
            .borrow_mut()
            .begin_drag(LogicalPosition::new(rb, 5));
        term.selection
            .borrow_mut()
            .update_drag(LogicalPosition::new(rb + 1, 0));
        let text = term.selection_text();
        assert_eq!(text, Some(" line of content".to_string()));
    }

    #[test]
    fn selection_text_full_row() {
        let (term, rb) =
            make_term_with_content(80, 24, 2000, "The quick brown fox jumps over the lazy dog");
        term.selection
            .borrow_mut()
            .begin_drag(LogicalPosition::new(rb, 0));
        term.selection
            .borrow_mut()
            .update_drag(LogicalPosition::new(rb, 80));
        let text = term.selection_text();
        assert_eq!(
            text,
            Some("The quick brown fox jumps over the lazy dog".to_string())
        );
    }

    #[test]
    fn selection_text_wide_chars() {
        let (term, rb) = make_term_with_content(80, 24, 2000, "Hello 世界 World");
        term.selection
            .borrow_mut()
            .begin_drag(LogicalPosition::new(rb, 0));
        term.selection
            .borrow_mut()
            .update_drag(LogicalPosition::new(rb, 19));
        let text = term.selection_text();
        assert!(text.is_some(), "should have selection text");
        let t = text.unwrap();
        assert!(t.contains("世界"), "should include CJK chars: got {:?}", t);
    }

    #[test]
    fn selection_text_clipboard_full_26_chars() {
        let (term, rb) = make_term_with_content(80, 24, 2000, "ABCDEFGHIJKLMNOPQRSTUVWXYZ");
        term.selection
            .borrow_mut()
            .begin_drag(LogicalPosition::new(rb, 0));
        term.selection
            .borrow_mut()
            .update_drag(LogicalPosition::new(rb, 26));
        let text = term.selection_text();
        assert_eq!(
            text,
            Some("ABCDEFGHIJKLMNOPQRSTUVWXYZ".to_string()),
            "should copy all 26 chars, got: {:?}",
            text
        );
    }

    /// Selection within scrollback region using a small max_sb that fills quickly.
    /// This exercises the paginated extraction loop where absolute_row falls
    /// entirely in scrollback (not viewport), testing offset_from_pane_to_vt100.
    #[test]
    fn selection_text_in_scrollback_with_small_max_sb() {
        let max_sb = 30;
        let width = 80u16;
        let height = 24u16;
        // Fill 50 lines so scrollback is maxed out at 30
        let text: String = (1..=50)
            .map(|i| format!("line {:02} data", i))
            .collect::<Vec<_>>()
            .join("\r\n");

        let mut pane = TestPane::new(max_sb);
        pane.set_parser_size(height, width);
        pane.write_to_parser(text.as_bytes());
        let mut term = TerminalComponent::from_pane(Box::new(pane));
        term.selection_enabled = true;

        // row_base = max_scrollback = 30
        let rb = max_sb;

        // Select lines 5-8 in the scrollback region (well above viewport)
        // These are the 5th through 8th most recent lines in the buffer.
        // Lines in order from oldest to newest in scrollback (index 0 = oldest):
        // line 01, line 02, ..., line 30 (scrollback)
        // line 31..50 (in viewport, but we select in scrollback)
        // row_base=30 means row 30 = viewport row 0, row 29 = scrollback bottom, row 0 = scrollback top
        term.selection
            .borrow_mut()
            .begin_drag(LogicalPosition::new(rb - 8, 0));
        term.selection
            .borrow_mut()
            .update_drag(LogicalPosition::new(rb - 5, 13));

        let text = term.selection_text();
        assert!(text.is_some(), "should extract text from scrollback region");
        let t = text.unwrap();
        // The 4 lines (indices rb-8 through rb-5) should contain unique line identifiers
        assert!(
            t.contains("line"),
            "scrollback selection must contain 'line', got: {:?}",
            t
        );
        let line_count = t.lines().count();
        assert_eq!(line_count, 4, "should span 4 lines, got {}", line_count);
    }

    /// A zero-length selection (click without drag) must return None.
    #[test]
    fn selection_text_empty_range_returns_none() {
        let (term, rb) = make_term_with_content(80, 24, 2000, "Hello World");
        // Click at a single position without dragging — start == end
        term.selection
            .borrow_mut()
            .begin_drag(LogicalPosition::new(rb, 5));
        term.selection
            .borrow_mut()
            .update_drag(LogicalPosition::new(rb, 5));
        let text = term.selection_text();
        assert!(text.is_none(), "zero-length selection must return None");
    }

    #[test]
    fn mouse_selection_works_through_handle_events() {
        use term_wm_core::components::ComponentContext;
        use term_wm_core::events::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

        let (mut term, _rb) = make_term_with_content(80, 24, 2000, "Hello World");
        let ctx = ComponentContext::new(true).with_screen_area(LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });

        // Verify selection is enabled
        assert!(term.selection_enabled, "selection_enabled must be true");

        // Mouse Down at column 1, row 0 — handle_selection_mouse prepares
        // the drag anchor (sets button_down=true) but returns false.
        // The event falls through to PTY mouse encoding which returns
        // Ignored if the PTY hasn't enabled mouse protocol. That's OK —
        // the anchor is set and the next Drag will activate selection.
        let down = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: 1,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        let _result = term.handle_events(&down, &ctx);
        // Verify that the drag anchor was prepared
        assert!(
            term.selection.borrow().button_down(),
            "button_down should be set after Down"
        );

        // Mouse Drag to column 5, row 0 — activates selection
        let drag = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 5,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        let drag_result = term.handle_events(&drag, &ctx);
        assert!(
            drag_result.is_consumed(),
            "Drag should be consumed by selection: got {:?}",
            drag_result
        );

        // Mouse Up — finalizes selection
        let up = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Release(MouseButton::Left),
            column: 5,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        let up_result = term.handle_events(&up, &ctx);
        assert!(
            !up_result.is_ignored(),
            "Up should not be ignored after drag: got {:?}",
            up_result
        );

        // Verify the selection text (cols 1-5 → "ello" from "Hello World")
        let status = term.selection_status();
        assert!(status.active, "selection should be active after up");
        let text = term.selection_text();
        assert_eq!(
            text,
            Some("ello".to_string()),
            "should select 'ello' (cols 1-5 of row 0), got: {:?}",
            text
        );
    }

    #[test]
    fn mouse_selection_via_dispatch_focused_event() {
        use std::sync::Arc;
        use term_wm_core::app_context::AppContext;
        use term_wm_core::components::Component;
        use term_wm_core::config::AppBuilder;
        use term_wm_core::events::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

        let (term, _rb) = make_term_with_content(80, 24, 2000, "Hello World");
        let mut sv = crate::scroll_view::ScrollViewComponent::new(term);
        sv.set_selection_enabled(true);
        sv.set_keyboard_mode(crate::scroll_view::ScrollKeyMode::PaginationOnly);

        let mut wm = AppBuilder::bare()
            .app_ctx(Arc::new(AppContext::new("test", "0.0.0")))
            .build()
            .expect("test build");
        wm.set_panel_visible(false);

        let key = wm.create_window(sv);
        let raw = wm.component_for_key_mut(key).unwrap();
        // The component inside the Window IS our ScrollViewComponent.
        // Verify set_selection_enabled works through the trait.
        raw.set_selection_enabled(true);

        // Set up managed layout so focus can route events
        let layout =
            term_wm_core::layout::TilingLayout::new(term_wm_core::layout::LayoutNode::leaf(key));
        wm.set_managed_layout(layout);
        wm.register_managed_layout(LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });
        wm.focus_app_window(key);

        // Simulate rendering to set last_area on the terminal
        use term_wm_core::components::ComponentContext;
        use term_wm_layout_engine::LayoutRect;
        let area = wm.region(key);
        let buffer = ratatui::buffer::Buffer::empty(ratatui::prelude::Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });
        let mut backend = term_wm_console::RatatuiBackend::new(
            buffer,
            ratatui::prelude::Rect {
                x: 0,
                y: 0,
                width: 80,
                height: 24,
            },
        );
        let ctx = ComponentContext::new(true);
        if let Some(comp) = wm.component_for_key_mut(key) {
            comp.render(
                &mut backend,
                area,
                &ctx,
                &mut term_wm_core::hitbox_registry::HitboxRegistry::new(),
            );
        }

        // Now send a mouse event — it should reach the terminal and be consumed by selection
        let down = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: (area.x + 1) as u16,
            row: (area.y + 1) as u16,
            modifiers: KeyModifiers::NONE,
        });
        let result_down = wm.dispatch_focused_event(&down);
        assert!(
            result_down.is_some(),
            "down must route to component, got None"
        );

        let drag = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: (area.x + 5) as u16,
            row: (area.y + 1) as u16,
            modifiers: KeyModifiers::NONE,
        });
        let result_drag = wm.dispatch_focused_event(&drag);
        assert!(
            result_drag.as_ref().is_some_and(|(_, r)| r.is_consumed()),
            "drag must be consumed by selection, got {:?}",
            result_drag.map(|(_, r)| r)
        );
    }

    #[test]
    fn mouse_selection_skipped_in_direct_mode_via_dispatch() {
        use std::sync::Arc;
        use term_wm_core::app_context::AppContext;
        use term_wm_core::components::Component;
        use term_wm_core::config::AppBuilder;
        use term_wm_core::events::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

        let (term, _rb) = make_term_with_content(80, 24, 2000, "Hello World");
        let mut sv = crate::scroll_view::ScrollViewComponent::new(term);
        sv.set_selection_enabled(true);
        sv.set_keyboard_mode(crate::scroll_view::ScrollKeyMode::PaginationOnly);

        let mut wm = AppBuilder::bare()
            .app_ctx(Arc::new(AppContext::new("test", "0.0.0")))
            .build()
            .expect("test build");
        wm.set_panel_visible(false);

        let key = wm.create_window(sv);
        let raw = wm.component_for_key_mut(key).unwrap();
        raw.set_selection_enabled(true);

        let layout =
            term_wm_core::layout::TilingLayout::new(term_wm_core::layout::LayoutNode::leaf(key));
        wm.set_managed_layout(layout);
        wm.register_managed_layout(LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });
        wm.focus_app_window(key);

        // Set direct mode on the window
        wm.set_direct_mode(key, true);
        assert!(wm.direct_mode(key));

        // Render to set last_area
        use term_wm_core::components::ComponentContext;
        use term_wm_layout_engine::LayoutRect;
        let area = wm.region(key);
        let buffer = ratatui::buffer::Buffer::empty(ratatui::prelude::Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });
        let mut backend = term_wm_console::RatatuiBackend::new(
            buffer,
            ratatui::prelude::Rect {
                x: 0,
                y: 0,
                width: 80,
                height: 24,
            },
        );
        let ctx = ComponentContext::new(true);
        if let Some(comp) = wm.component_for_key_mut(key) {
            comp.render(
                &mut backend,
                area,
                &ctx,
                &mut term_wm_core::hitbox_registry::HitboxRegistry::new(),
            );
        }

        // In direct mode: a Down+Drag must NOT be consumed by selection
        let down = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: (area.x + 1) as u16,
            row: (area.y + 1) as u16,
            modifiers: KeyModifiers::NONE,
        });
        let result_down = wm.dispatch_focused_event(&down);
        assert!(
            result_down.is_some(),
            "down must route to component in direct mode, got None"
        );
        let (_, down_evt) = result_down.unwrap();
        // In direct mode, selection is skipped, so Down is not consumed
        // (it falls through to PTY forwarding, which returns Ignored for
        //  press-only mode since the test PTY hasn't enabled mouse tracking)
        assert!(
            !down_evt.is_consumed(),
            "down must NOT be consumed in direct mode: got {:?}",
            down_evt
        );

        // Drag must also not be consumed (selection is skipped in direct mode)
        let drag = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: (area.x + 5) as u16,
            row: (area.y + 1) as u16,
            modifiers: KeyModifiers::NONE,
        });
        let result_drag = wm.dispatch_focused_event(&drag);
        assert!(
            result_drag.is_some(),
            "drag must route to component in direct mode, got None"
        );
        let (_, drag_evt) = result_drag.unwrap();
        assert!(
            !drag_evt.is_consumed(),
            "drag must NOT be consumed in direct mode: got {:?}",
            drag_evt
        );

        // Verify no selection was made
        let sel_status = wm
            .component_for_key(key)
            .map(|c| c.selection_status())
            .unwrap();
        assert!(
            !sel_status.active,
            "selection should not be active after direct mode drag"
        );
    }

    /// In direct mode, mouse Down must skip selection and go to PTY encoding.
    #[test]
    fn direct_mode_mouse_down_skips_selection() {
        use term_wm_core::actions::TermWmAction;
        use term_wm_core::components::{ComponentContext, EventResult};
        use term_wm_core::events::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

        let mut pane = TestPane::new(2000);
        pane.set_parser_size(24, 80);
        pane.write_to_parser(b"Hello World");
        // Enable VT200 mouse mode so mouse_event_allowed() returns true
        pane.write_to_parser(b"\x1b[?1000h");
        let mut term = TerminalComponent::from_pane(Box::new(pane));
        term.set_selection_enabled(true);

        // Direct mode — selection skipped, PTY encoding expected
        let ctx = ComponentContext::new(true)
            .with_direct_mode(true)
            .with_screen_area(LayoutRect {
                x: 0,
                y: 0,
                width: 80,
                height: 24,
            });

        // Down inside the area — selection must NOT consume it
        let down = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: 5,
            row: 2,
            modifiers: KeyModifiers::NONE,
        });
        let result = term.handle_events(&down, &ctx);
        match result {
            EventResult::Action(TermWmAction::MouseToBytes(_)) => {}
            other => panic!(
                "in direct mode, Down must produce MouseToBytes (PTY), got {:?}",
                other
            ),
        }

        // Non-direct mode — selection should consume Drag
        let ctx_normal = ComponentContext::new(true).with_screen_area(LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });

        // Send Down first to set button_down (required for Drag consumption)
        let down_normal = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: 5,
            row: 2,
            modifiers: KeyModifiers::NONE,
        });
        let _ = term.handle_events(&down_normal, &ctx_normal);

        let drag = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 10,
            row: 2,
            modifiers: KeyModifiers::NONE,
        });
        let result_drag = term.handle_events(&drag, &ctx_normal);
        assert!(
            matches!(result_drag, EventResult::Consumed),
            "in normal mode, Drag must be consumed by selection, got {:?}",
            result_drag
        );
    }
}
