use std::sync::Arc;

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use portable_pty::{CommandBuilder, PtySize};
use ratatui::{
    layout::Rect,
    style::{Color as TColor, Modifier, Style},
};
use vt100::{MouseProtocolEncoding, MouseProtocolMode};

use crate::components::{Component, ComponentContext, SelectionStatus};
use crate::layout::rect_contains;
use crate::pty::Pty;
use crate::ui::UiFrame;
use crate::utils::linkifier::{
    LinkHandler, LinkOverlay, Linkifier, OverlaySignature, decorate_link_style,
};
use crate::utils::selectable_text::{
    LogicalPosition, SelectionController, SelectionHost, SelectionRange, SelectionViewport,
    handle_selection_mouse, maintain_selection_drag,
};

// This controls the scrollback buffer size in the vt100 parser.
// It determines how many lines you can scroll up to see.
const DEFAULT_SCROLLBACK_LEN: usize = 2000;

pub struct TerminalComponent {
    pane: Pty,
    last_size: (u16, u16),
    last_area: Rect,
    linkifier: Linkifier,
    link_overlay: LinkOverlay,
    link_handler: Option<LinkHandler>,
    command_description: String,
    selection: SelectionController,
    selection_enabled: bool,
    last_scrollback: usize,
}

impl Component for TerminalComponent {
    fn resize(&mut self, area: Rect, _ctx: &ComponentContext) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let size = (area.width, area.height);
        if size != self.last_size {
            let _ = self.pane.resize(PtySize {
                rows: area.height,
                cols: area.width,
                pixel_width: 0,
                pixel_height: 0,
            });
            self.last_size = size;
        }
    }

    fn render(&mut self, frame: &mut UiFrame<'_>, area: Rect, ctx: &ComponentContext) {
        if !ctx.focused() {
            self.selection.clear();
        }
        if area.height == 0 || area.width == 0 {
            self.last_area = Rect::default();
            return;
        }
        self.last_area = area;
        let _exited = self.pane.has_exited();
        self.render_screen(frame, area, ctx);
    }

    fn handle_event(&mut self, event: &Event, _ctx: &ComponentContext) -> bool {
        match event {
            Event::Key(key) => {
                if key.kind == KeyEventKind::Release {
                    return false;
                }
                self.selection.clear();
                if matches!(key.code, KeyCode::PageUp | KeyCode::PageDown)
                    && key.modifiers.contains(KeyModifiers::SHIFT)
                    && !self.pane.alternate_screen()
                {
                    let delta = if key.code == KeyCode::PageUp {
                        10isize
                    } else {
                        -10isize
                    };
                    self.scroll_scrollback(delta);
                    return true;
                }
                let bytes = key_to_bytes(*key);
                if bytes.is_empty() {
                    return false;
                }
                if self.pane.scrollback() > 0 {
                    self.pane.set_scrollback(0);
                }
                if let Err(_err) = self.pane.write_bytes(&bytes) {
                    #[cfg(windows)]
                    eprintln!("terminal input write failed: {_err}");
                }
                true
            }
            Event::Mouse(mouse) => {
                if !self.pane.alternate_screen() {
                    // Logic for scrollbar event was here, but now ScrollView handles it.
                    // If we need to capture events that ScrollView didn't handle (e.g. if we are not wrapped?),
                    // but we assume we are wrapped or don't need it.
                }
                let selection_ready = self.selection_enabled && !self.pane.alternate_screen();
                if handle_selection_mouse(self, selection_ready, mouse) {
                    return true;
                }
                if self.try_handle_link_click(mouse) {
                    return true;
                }
                if !rect_contains(self.last_area, mouse.column, mouse.row) {
                    return false;
                }
                // Forward mouse events only when the nested app enabled mouse reporting
                // (either SGR or the legacy/default X11-style protocol).
                let screen = self.pane.screen();
                let encoding = screen.mouse_protocol_encoding();

                match encoding {
                    MouseProtocolEncoding::Default | MouseProtocolEncoding::Sgr => {}
                    _ => return false,
                }

                let mode = screen.mouse_protocol_mode();
                // Avoid emitting sequences for modes the app didn't request.
                if !mouse_event_allowed(mode, mouse.kind) {
                    return false;
                }
                // Convert global coordinates into the PTY-local viewport.
                let local = MouseEvent {
                    column: mouse.column.saturating_sub(self.last_area.x),
                    row: mouse.row.saturating_sub(self.last_area.y),
                    kind: mouse.kind,
                    modifiers: mouse.modifiers,
                };
                let bytes = mouse_event_to_bytes(local, encoding);
                if bytes.is_empty() {
                    return false;
                }
                if let Err(_err) = self.pane.write_bytes(&bytes) {
                    #[cfg(windows)]
                    eprintln!("terminal mouse write failed: {_err}");
                }
                true
            }
            _ => false,
        }
    }

    fn selection_status(&self) -> SelectionStatus {
        if !self.selection_enabled {
            return SelectionStatus::default();
        }
        SelectionStatus {
            active: self.selection.has_selection(),
            dragging: self.selection.is_dragging(),
        }
    }

    fn selection_text(&mut self) -> Option<String> {
        if !self.selection_enabled {
            return None;
        }
        let range = self.selection.selection_range()?.normalized();
        if !range.is_non_empty() {
            return None;
        }
        self.selection_text_for_range(range)
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
    pub fn spawn_default(command: CommandBuilder) -> crate::pty::PtyResult<Self> {
        Self::spawn(command, Self::default_pty_size())
    }

    pub fn spawn(command: CommandBuilder, size: PtySize) -> crate::pty::PtyResult<Self> {
        let command_description = format!("{:?}", command);
        let pane = Pty::spawn_with_scrollback(command, size, DEFAULT_SCROLLBACK_LEN)?;
        let comp = Self {
            pane,
            last_size: (size.cols, size.rows),
            last_area: Rect::default(),
            linkifier: Linkifier::new(),
            link_overlay: LinkOverlay::new(),
            link_handler: None,
            command_description,
            selection: SelectionController::new(),
            selection_enabled: false,
            last_scrollback: 0,
        };
        Ok(comp)
    }

    pub fn write_bytes(&mut self, input: &[u8]) -> std::io::Result<()> {
        self.pane.write_bytes(input)
    }

    pub fn has_exited(&mut self) -> bool {
        let exited = self.pane.has_exited();
        if exited {
            // If exiting with error, log it to global log which will trigger debug window
            if let Some(status) = self.pane.take_exit_status()
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
        self.pane.exit_status()
    }

    pub fn take_exit_status(&mut self) -> Option<portable_pty::ExitStatus> {
        self.pane.take_exit_status()
    }

    pub fn bytes_received(&self) -> usize {
        self.pane.bytes_received()
    }

    pub fn last_bytes_text(&self) -> String {
        self.pane.last_bytes_text()
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

    pub fn set_selection_enabled(&mut self, enabled: bool) {
        if self.selection_enabled == enabled {
            return;
        }
        self.selection_enabled = enabled;
        if !enabled {
            self.selection.clear();
        }
    }

    fn render_screen(&mut self, frame: &mut UiFrame<'_>, area: Rect, ctx: &ComponentContext) {
        maintain_selection_drag(self);

        // Synchronize scroll state with the shared Viewport
        if !self.pane.alternate_screen()
            && let Some(handle) = ctx.viewport_handle()
        {
            let used = self.pane.max_scrollback();
            let view_height = area.height as usize;
            let total_height = used + view_height;
            handle.set_content_size(area.width as usize, total_height);

            let current_sb = self.pane.scrollback();
            // If scrollback changed internally (keys/output), push to viewport
            if current_sb != self.last_scrollback {
                let new_offset = used.saturating_sub(current_sb);
                handle.scroll_vertical_to(new_offset);
            } else {
                // Otherwise sync from viewport (scrollbar/mouse wheel on container)
                let offset = ctx.viewport().offset_y;
                let target_sb = used.saturating_sub(offset);
                if target_sb != current_sb {
                    self.pane.set_scrollback(target_sb);
                }
            }
        }

        let scrollback_value = self.pane.scrollback();
        self.last_scrollback = scrollback_value;

        let show_cursor = scrollback_value == 0;
        let used = self.pane.max_scrollback();
        let selection_row_base = used.saturating_sub(scrollback_value);
        let selection_range = if self.selection_enabled {
            self.selection
                .selection_range()
                .filter(|r| r.is_non_empty())
                .map(|r| r.normalized())
        } else {
            None
        };
        let buffer = frame.buffer_mut();

        // Optimally only iterate over the visible intersection
        let visible = area.intersection(buffer.area);
        if visible.width == 0 || visible.height == 0 {
            self.link_overlay.clear();
            return;
        }

        // Calculate offset into the PTY screen
        let start_col = visible.x.saturating_sub(area.x);
        let start_row = visible.y.saturating_sub(area.y);

        let bytes_seen = self.pane.bytes_received();
        let screen = self.pane.screen();
        let signature = OverlaySignature::new(
            bytes_seen,
            scrollback_value,
            area.width,
            area.height,
            start_row,
            start_col,
        );
        if !self.link_overlay.is_signature_current(&signature) {
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
            self.link_overlay.update_view(
                signature,
                viewport_height,
                viewport_width,
                &row_data,
                &self.linkifier,
            );
        }

        let focused = ctx.focused();
        for row in start_row..start_row + visible.height {
            for col in start_col..start_col + visible.width {
                let cell_x = area.x + col;
                let cell_y = area.y + row;
                let viewport_row = row.saturating_sub(start_row) as usize;
                let viewport_col = col.saturating_sub(start_col) as usize;

                // If we have a PTY cell, render it
                if let Some(cell) = screen.cell(row, col) {
                    let mut symbol = cell.contents().chars().next().unwrap_or(' ');
                    let (fg, bg) = resolve_colors(cell, screen);
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

                    if self.link_overlay.is_link_cell(viewport_row, viewport_col) {
                        style = decorate_link_style(style);
                    }

                    if let Some(range) = selection_range {
                        let abs_row = selection_row_base.saturating_add(row as usize);
                        let abs_col = col as usize;
                        if range.contains(LogicalPosition::new(abs_row, abs_col)) {
                            style = style
                                .bg(crate::theme::selection_bg())
                                .fg(crate::theme::selection_fg());
                        }
                    }

                    if let Some(buf_cell) = buffer.cell_mut((cell_x, cell_y)) {
                        let mut buf = [0u8; 4];
                        // If background is transparent (None), force it to Reset to clear underlying content
                        if bg.is_none() {
                            buf_cell.reset();
                        }
                        let sym = symbol.encode_utf8(&mut buf);
                        buf_cell.set_symbol(sym).set_style(style);
                    }
                } else if let Some(buf_cell) = buffer.cell_mut((cell_x, cell_y)) {
                    // Otherwise clear the cell so we don't bleed background
                    buf_cell.reset();
                    buf_cell.set_symbol(" ");
                }
            }
        }

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

impl SelectionViewport for TerminalComponent {
    fn selection_viewport(&self) -> Rect {
        self.last_area
    }

    fn logical_position_from_point(&mut self, column: u16, row: u16) -> Option<LogicalPosition> {
        TerminalComponent::logical_position_from_point(self, column, row)
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
        &mut self.selection
    }
}

#[cfg(unix)]
pub fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| crate::constants::DEFAULT_SHELL_FALLBACK.to_string())
}

#[cfg(windows)]
pub fn default_shell() -> String {
    std::env::var("COMSPEC")
        .unwrap_or_else(|_| crate::constants::DEFAULT_SHELL_FALLBACK.to_string())
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
        let current = self.pane.scrollback() as isize;
        let next = (current + delta).clamp(0, self.pane.scrollback_len() as isize) as usize;
        self.pane.set_scrollback(next);
    }

    fn logical_position_from_point(&mut self, column: u16, row: u16) -> Option<LogicalPosition> {
        if self.last_area.width == 0 || self.last_area.height == 0 {
            return None;
        }
        let max_x = self
            .last_area
            .x
            .saturating_add(self.last_area.width)
            .saturating_sub(1);
        let max_y = self
            .last_area
            .y
            .saturating_add(self.last_area.height)
            .saturating_sub(1);
        let clamped_col = column.clamp(self.last_area.x, max_x);
        let clamped_row = row.clamp(self.last_area.y, max_y);
        let local_col = clamped_col.saturating_sub(self.last_area.x) as usize;
        let local_row = clamped_row.saturating_sub(self.last_area.y) as usize;
        let scrollback_value = self.pane.scrollback();
        let used = self.pane.max_scrollback();
        let row_base = used.saturating_sub(scrollback_value);
        Some(LogicalPosition::new(
            row_base.saturating_add(local_row),
            local_col,
        ))
    }

    fn selection_text_for_range(&mut self, range: SelectionRange) -> Option<String> {
        let row_base = self
            .pane
            .max_scrollback()
            .saturating_sub(self.pane.scrollback());
        let screen = self.pane.screen();
        let (rows, cols) = screen.size();
        if rows == 0 || cols == 0 {
            return None;
        }
        let (mut end_row, mut end_col) = (range.end.row, range.end.column);
        if end_col == 0 && end_row > range.start.row {
            end_row = end_row.saturating_sub(1);
            end_col = cols as usize;
        }

        let start_row = range.start.row.saturating_sub(row_base);
        let end_row = end_row.saturating_sub(row_base);
        if start_row >= rows as usize {
            return None;
        }
        let end_row = end_row.min(rows.saturating_sub(1) as usize);
        let start_col = range.start.column.min(cols as usize);
        let end_col = end_col.min(cols as usize);
        if end_row < start_row {
            return None;
        }

        Some(screen.contents_between(
            start_row as u16,
            start_col as u16,
            end_row as u16,
            end_col as u16,
        ))
    }

    /// Terminate the underlying PTY child process.
    pub fn terminate(&mut self) {
        let _ = self.pane.kill_child();
    }

    fn link_at_position(&self, mouse: &MouseEvent) -> Option<String> {
        if self.last_area.width == 0 || self.last_area.height == 0 {
            return None;
        }
        if mouse.column < self.last_area.x
            || mouse.column >= self.last_area.x.saturating_add(self.last_area.width)
            || mouse.row < self.last_area.y
            || mouse.row >= self.last_area.y.saturating_add(self.last_area.height)
        {
            return None;
        }
        let local_x = (mouse.column - self.last_area.x) as usize;
        let local_y = (mouse.row - self.last_area.y) as usize;
        self.link_overlay.link_at(local_y, local_x)
    }

    fn try_handle_link_click(&mut self, mouse: &MouseEvent) -> bool {
        if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            return false;
        }

        if let Some(url) = self.link_at_position(mouse)
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

fn key_to_bytes(key: KeyEvent) -> Vec<u8> {
    match key.code {
        KeyCode::Char(c) => {
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && let Some(byte) = ctrl_char(c)
            {
                return vec![byte];
            }
            c.to_string().into_bytes()
        }
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        _ => Vec::new(),
    }
}

fn ctrl_char(c: char) -> Option<u8> {
    let c = c.to_ascii_lowercase();
    if c.is_ascii_lowercase() {
        Some((c as u8) - b'a' + 1)
    } else {
        None
    }
}

// Only forward events that match the active mouse reporting mode.
fn mouse_event_allowed(mode: MouseProtocolMode, kind: MouseEventKind) -> bool {
    use MouseEventKind::*;
    match mode {
        MouseProtocolMode::None => false,
        MouseProtocolMode::Press => matches!(kind, Down(_)),
        MouseProtocolMode::PressRelease => matches!(kind, Down(_) | Up(_)),
        MouseProtocolMode::ButtonMotion => matches!(kind, Down(_) | Up(_) | Drag(_)),
        MouseProtocolMode::AnyMotion => true,
    }
}

fn mouse_event_to_bytes(mouse: MouseEvent, encoding: MouseProtocolEncoding) -> Vec<u8> {
    let (mut code, release): (u8, bool) = match mouse.kind {
        MouseEventKind::Down(button) => (
            match button {
                MouseButton::Left => 0,
                MouseButton::Middle => 1,
                MouseButton::Right => 2,
            },
            false,
        ),
        MouseEventKind::Up(button) => (
            match button {
                MouseButton::Left => 0,
                MouseButton::Middle => 1,
                MouseButton::Right => 2,
            },
            true,
        ),
        MouseEventKind::Drag(button) => (
            32 + match button {
                MouseButton::Left => 0,
                MouseButton::Middle => 1,
                MouseButton::Right => 2,
            },
            false,
        ),
        MouseEventKind::Moved => (35, false),
        MouseEventKind::ScrollUp => (64, false),
        MouseEventKind::ScrollDown => (65, false),
        MouseEventKind::ScrollLeft => (66, false),
        MouseEventKind::ScrollRight => (67, false),
    };
    if mouse.modifiers.contains(KeyModifiers::SHIFT) {
        code |= 4;
    }
    if mouse.modifiers.contains(KeyModifiers::ALT) {
        code |= 8;
    }
    if mouse.modifiers.contains(KeyModifiers::CONTROL) {
        code |= 16;
    }
    let col = mouse.column.saturating_add(1);
    let row = mouse.row.saturating_add(1);

    // Two encodings are supported:
    // - SGR (`CSI < Cb ; Cx ; Cy M/m`) which conveys presses/releases
    //   with explicit button numbers and is preferred by many modern apps.
    // - Legacy/X11 (`CSI M Cb Cx Cy`) used by older apps; for releases the
    //   canonical base button code is 3. Modifier bits are preserved when
    //   constructing the Cb byte for legacy encoding.
    match encoding {
        MouseProtocolEncoding::Sgr => {
            let action = if release { 'm' } else { 'M' };
            format!("\x1b[<{};{};{}{}", code, col, row, action).into_bytes()
        }
        MouseProtocolEncoding::Default => {
            // X11 (CSI M) encoding: Cb Cx Cy. For button releases the base
            // button code is 3; preserve modifier bits when constructing Cb.
            let x11_code = if release {
                let mods = code & (4 | 8 | 16);
                3 | mods
            } else {
                code
            };

            let cb = x11_code.saturating_add(32);
            let cx = mouse.column.saturating_add(33);
            let cy = mouse.row.saturating_add(33);

            if cx > 255 || cy > 255 {
                return Vec::new();
            }

            vec![0x1b, b'[', b'M', cb, cx as u8, cy as u8]
        }
        _ => Vec::new(),
    }
}

fn resolve_colors(cell: &vt100::Cell, screen: &vt100::Screen) -> (Option<TColor>, Option<TColor>) {
    let mut fg = resolve_color(cell.fgcolor(), screen.fgcolor());
    let bg = resolve_color(cell.bgcolor(), screen.bgcolor());
    if cell.bold() {
        fg = brighten_indexed(fg);
    }
    (fg, bg)
}

fn vt_color_to_ratatui(color: vt100::Color) -> Option<TColor> {
    use crate::term_color::map_rgb_to_color;
    match color {
        vt100::Color::Default => None,
        vt100::Color::Idx(idx) => Some(TColor::Indexed(idx)),
        vt100::Color::Rgb(r, g, b) => Some(map_rgb_to_color(r, g, b)),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{
        KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use vt100::MouseProtocolMode;

    fn key(k: KeyCode, mods: KeyModifiers) -> KeyEvent {
        let mut ev = KeyEvent::new(k, mods);
        ev.kind = KeyEventKind::Press;
        ev
    }

    #[test]
    fn key_to_bytes_char_and_controls() {
        let b = key_to_bytes(key(KeyCode::Char('x'), KeyModifiers::NONE));
        assert_eq!(b, b"x".to_vec());

        let enter = key_to_bytes(key(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(enter, vec![b'\r']);

        let back = key_to_bytes(key(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(back, vec![0x7f]);

        let ctrl_a = key_to_bytes(key(KeyCode::Char('a'), KeyModifiers::CONTROL));
        assert_eq!(ctrl_a, vec![1u8]);
    }

    #[test]
    fn ctrl_char_edges() {
        assert_eq!(ctrl_char('a'), Some(1));
        assert_eq!(ctrl_char('z'), Some(26));
        assert_eq!(ctrl_char('A'), Some(1));
        assert_eq!(ctrl_char('1'), None);
    }

    #[test]
    fn mouse_event_allowed_modes() {
        use MouseEventKind::*;
        assert!(!mouse_event_allowed(
            MouseProtocolMode::None,
            Down(MouseButton::Left)
        ));
        assert!(mouse_event_allowed(
            MouseProtocolMode::Press,
            Down(MouseButton::Left)
        ));
        assert!(!mouse_event_allowed(
            MouseProtocolMode::Press,
            Up(MouseButton::Left)
        ));
        assert!(mouse_event_allowed(
            MouseProtocolMode::PressRelease,
            Up(MouseButton::Left)
        ));
        assert!(mouse_event_allowed(
            MouseProtocolMode::ButtonMotion,
            MouseEventKind::Drag(MouseButton::Left)
        ));
        assert!(mouse_event_allowed(
            MouseProtocolMode::AnyMotion,
            MouseEventKind::Moved
        ));
    }

    #[test]
    fn mouse_event_to_bytes_format_and_mods() {
        let m = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 2,
            row: 3,
            modifiers: KeyModifiers::NONE,
        };
        // Test SGR
        let bytes = mouse_event_to_bytes(m, MouseProtocolEncoding::Sgr);
        let s = String::from_utf8(bytes).unwrap();
        assert!(s.starts_with("\x1b[<0;3;4M"));

        // Test Default encoding
        let bytes_def = mouse_event_to_bytes(m, MouseProtocolEncoding::Default);
        // CSI M Cb Cx Cy
        // Cb = 0 + 32 = 32 (' ')
        // Cx = 2 + 33 = 35 ('#')
        // Cy = 3 + 33 = 36 ('$')
        assert_eq!(bytes_def, vec![0x1b, b'[', b'M', 32, 35, 36]);

        let m2 = MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Right),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::SHIFT | KeyModifiers::ALT,
        };
        let s2 = String::from_utf8(mouse_event_to_bytes(m2, MouseProtocolEncoding::Sgr)).unwrap();
        // code should include modifier bits
        assert!(s2.contains(';'));
        assert!(s2.ends_with('m'));
    }

    #[test]
    fn mouse_event_x11_release_and_modifiers() {
        // 1. Simple Release (Left Up) -> Code 3
        let m_up = MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        // Cb = 3 + 32 = 35 ('#'). Cx, Cy = 0 + 33 = 33 ('!')
        let bytes = mouse_event_to_bytes(m_up, MouseProtocolEncoding::Default);
        assert_eq!(bytes, vec![0x1b, b'[', b'M', 35, 33, 33]);

        // 2. Release with Shift -> Code 3 + 4 = 7
        let m_up_shift = MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::SHIFT,
        };
        // Cb = 7 + 32 = 39 ('\'')
        let bytes = mouse_event_to_bytes(m_up_shift, MouseProtocolEncoding::Default);
        assert_eq!(bytes, vec![0x1b, b'[', b'M', 39, 33, 33]);

        // 3. Press Right with Control -> Code 2 + 16 = 18
        let m_down_ctrl = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::CONTROL,
        };
        // Cb = 18 + 32 = 50 ('2')
        let bytes = mouse_event_to_bytes(m_down_ctrl, MouseProtocolEncoding::Default);
        assert_eq!(bytes, vec![0x1b, b'[', b'M', 50, 33, 33]);
    }

    #[test]
    fn vt_color_and_resolve_color() {
        assert_eq!(vt_color_to_ratatui(vt100::Color::Default), None);
        assert_eq!(
            vt_color_to_ratatui(vt100::Color::Idx(5)),
            Some(TColor::Indexed(5))
        );
        assert_eq!(
            vt_color_to_ratatui(vt100::Color::Rgb(1, 2, 3)),
            Some(crate::term_color::map_rgb_to_color(1, 2, 3))
        );

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
}
