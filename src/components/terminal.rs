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

use crate::components::{Component, scroll_view::ScrollViewComponent};
use crate::layout::rect_contains;
use crate::linkifier::{
    LinkHandler, LinkOverlay, Linkifier, OverlaySignature, decorate_link_style,
};
use crate::pty::Pty;
use crate::ui::UiFrame;

// This controls the scrollback buffer size in the vt100 parser.
// It determines how many lines you can scroll up to see.
const DEFAULT_SCROLLBACK_LEN: usize = 2000;

pub struct TerminalComponent {
    pane: Pty,
    last_size: (u16, u16),
    scroll_view: ScrollViewComponent,
    last_area: Rect,
    linkifier: Linkifier,
    link_overlay: LinkOverlay,
    link_handler: Option<LinkHandler>,
}

impl Component for TerminalComponent {
    fn resize(&mut self, area: Rect) {
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

    fn render(&mut self, frame: &mut UiFrame<'_>, area: Rect, focused: bool) {
        if area.height == 0 || area.width == 0 {
            self.last_area = Rect::default();
            return;
        }
        self.last_area = area;
        let _exited = self.pane.has_exited();
        self.render_screen(frame, area, focused);
    }

    fn handle_event(&mut self, event: &Event) -> bool {
        match event {
            Event::Key(key) => {
                if key.kind == KeyEventKind::Release {
                    return false;
                }
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
                if self.try_handle_link_click(mouse) {
                    return true;
                }
                if !self.pane.alternate_screen() && self.handle_scrollbar_event(event) {
                    return true;
                }
                if !rect_contains(self.last_area, mouse.column, mouse.row) {
                    return false;
                }
                // Only forward mouse events when the nested app opted in to SGR mouse reporting.
                let screen = self.pane.screen();
                if screen.mouse_protocol_encoding() != MouseProtocolEncoding::Sgr {
                    return false;
                }
                let mode = screen.mouse_protocol_mode();
                // Avoid emitting sequences for modes that the app didn't request.
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
                let bytes = mouse_event_to_bytes(local);
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
}

impl TerminalComponent {
    pub fn spawn(command: CommandBuilder, size: PtySize) -> crate::pty::PtyResult<Self> {
        let pane = Pty::spawn_with_scrollback(command, size, DEFAULT_SCROLLBACK_LEN)?;
        let mut comp = Self {
            pane,
            last_size: (size.cols, size.rows),
            scroll_view: ScrollViewComponent::new(),
            last_area: Rect::default(),
            linkifier: Linkifier::new(),
            link_overlay: LinkOverlay::new(),
            link_handler: None,
        };
        // Terminal scroll view must not hijack keyboard input; disable by default.
        comp.scroll_view.set_keyboard_enabled(false);
        Ok(comp)
    }

    pub fn write_bytes(&mut self, input: &[u8]) -> std::io::Result<()> {
        self.pane.write_bytes(input)
    }

    pub fn has_exited(&mut self) -> bool {
        self.pane.has_exited()
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

    fn render_screen(&mut self, frame: &mut UiFrame<'_>, area: Rect, focused: bool) {
        let scrollback_value = self.pane.scrollback();
        let show_cursor = scrollback_value == 0;
        let used = self.pane.max_scrollback();
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

        if !screen.alternate_screen() && used > 0 {
            let view = area.height as usize;
            if view > 0 {
                let total = used.saturating_add(view);
                let offset = used.saturating_sub(scrollback_value);
                self.scroll_view.update(area, total, view);
                self.scroll_view.set_offset(offset);
                self.scroll_view.render(frame);
            }
        }
    }
}

#[cfg(unix)]
pub fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "bash".to_string())
}

#[cfg(windows)]
pub fn default_shell() -> String {
    std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())
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

    fn handle_scrollbar_event(&mut self, event: &Event) -> bool {
        let used = self.pane.max_scrollback();
        if used == 0 {
            return false;
        }
        let response = self.scroll_view.handle_event(event);
        if let Some(offset) = response.v_offset {
            let scrollback = used.saturating_sub(offset);
            self.pane.set_scrollback(scrollback);
        }
        response.handled
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

fn mouse_event_to_bytes(mouse: MouseEvent) -> Vec<u8> {
    let (mut code, release) = match mouse.kind {
        MouseEventKind::Down(button) => (
            match button {
                MouseButton::Left => 0,
                MouseButton::Middle => 1,
                MouseButton::Right => 2,
            },
            false,
        ),
        MouseEventKind::Up(_) => (3, true),
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
    let action = if release { 'm' } else { 'M' };
    let col = mouse.column.saturating_add(1);
    let row = mouse.row.saturating_add(1);
    format!("\x1b[<{};{};{}{}", code, col, row, action).into_bytes()
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
        let bytes = mouse_event_to_bytes(m);
        let s = String::from_utf8(bytes).unwrap();
        assert!(s.starts_with("\x1b[<0;3;4M"));

        let m2 = MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Right),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::SHIFT | KeyModifiers::ALT,
        };
        let s2 = String::from_utf8(mouse_event_to_bytes(m2)).unwrap();
        // code should include modifier bits
        assert!(s2.contains(';'));
        assert!(s2.ends_with('m'));
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
