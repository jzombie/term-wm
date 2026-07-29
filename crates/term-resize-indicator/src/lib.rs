use std::time::Instant;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen, SetSize};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::Terminal;

/// Dark background to distinguish the overlay from normal terminal content.
const BG: Color = Color::Rgb(20, 20, 48);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub width: u16,
    pub height: u16,
}

impl Size {
    pub fn new(width: u16, height: u16) -> Self {
        Self { width, height }
    }
}

/// Only two modes: unlocked (interactive) and locked.
/// Both viewers see the same session, so they always see identical output.
enum Mode {
    Interactive,
    Locked(Size),
}

struct App {
    mode: Mode,
    /// Current live terminal dimensions (updated on every Resize event).
    /// In locked mode this tracks actual size; SetSize is deferred via pending_snap.
    live_size: Size,
    /// When armed, the SetSize command will fire after this instant elapses.
    /// Reset on each new Resize event to implement debounce.
    pending_snap: Option<Instant>,
}

pub fn run(initial_lock: Option<Size>) -> std::io::Result<()> {
    terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let current = terminal::size().unwrap_or((80, 24));
    let mut app = if let Some(size) = initial_lock {
        let _ = crossterm::execute!(terminal.backend_mut(), SetSize(size.width, size.height));
        App {
            mode: Mode::Locked(size),
            live_size: Size::new(current.0, current.1),
            pending_snap: None,
        }
    } else {
        App {
            mode: Mode::Interactive,
            live_size: Size::new(current.0, current.1),
            pending_snap: None,
        }
    };

    let res = run_loop(&mut terminal, &mut app);

    let _ = crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal::disable_raw_mode();

    res
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
) -> std::io::Result<()> {
    loop {
        // Fire deferred SetSize if debounce timer has elapsed
        if let Some(snap_time) = app.pending_snap
            && Instant::now() >= snap_time
        {
            app.pending_snap = None;
            if let Mode::Locked(locked) = app.mode {
                let _ = crossterm::execute!(
                    terminal.backend_mut(),
                    SetSize(locked.width, locked.height)
                );
                app.live_size = locked;
            }
        }

        terminal.draw(|f| {
            let area = f.area();
            let dim = app.live_size;
            let locked = matches!(app.mode, Mode::Locked(_));
            draw_overlay(f, area, dim.width, dim.height, locked);
        })?;

        if !event::poll(std::time::Duration::from_millis(50))? {
            continue;
        }

        match event::read()? {
            Event::Resize(w, h) => on_resize(app, w, h),
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Char('l' | 'L') => {
                    app.pending_snap = None;
                    toggle_lock(app);
                }
                KeyCode::Char('q' | 'Q') | KeyCode::Esc => break,
                KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => break,
                _ => {}
            },
            _ => {}
        }
    }
    Ok(())
}

fn on_resize(app: &mut App, w: u16, h: u16) {
    app.live_size = Size::new(w, h);
    if matches!(app.mode, Mode::Locked(_)) {
        // Defer SetSize — arm the debounce timer.
        // Every new resize event pushes the deadline forward, so SetSize
        // only fires after 250ms of silence from the OS window manager.
        app.pending_snap = Some(Instant::now() + std::time::Duration::from_millis(250));
    }
}

fn toggle_lock(app: &mut App) {
    match app.mode {
        Mode::Interactive => {
            if let Ok((w, h)) = terminal::size() {
                let locked = Size::new(w, h);
                app.live_size = locked;
                app.mode = Mode::Locked(locked);
            }
        }
        Mode::Locked(locked) => {
            app.live_size = locked;
            app.mode = Mode::Interactive;
        }
    }
}

/// Draw the full-screen overlay.
///
/// - Background fill (dark blue) so both viewers see a uniform canvas.
/// - Box-drawing border (`┌─┐│└─┘`) edge-to-edge — a deliberate measured box
///   that is visually distinct from the terminal boundary.
/// - Centered text shows the live terminal dimensions (and lock state).
fn draw_overlay(f: &mut ratatui::Frame, full: Rect, w: u16, h: u16, locked: bool) {
    if full.width < 4 || full.height < 3 {
        return;
    }

    let line_style = Style::default()
        .fg(Color::Cyan)
        .bg(BG)
        .add_modifier(Modifier::BOLD);

    let bg_style = Style::default().bg(BG);

    let buf = f.buffer_mut();

    // Bottom-right cell — writing here triggers VT100 DECAWM auto-wrap hardware
    // scroll.  Exclude it from all rendering to keep the buffer in sync.
    let br_x = full.right().saturating_sub(1);
    let br_y = full.bottom().saturating_sub(1);

    for y in full.top()..full.bottom() {
        for x in full.left()..full.right() {
            if x == br_x && y == br_y {
                continue;
            }
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_style(bg_style);
                cell.set_symbol(" ");
            }
        }
    }

    // Border position in i32 — no .max(0) clamping.  Parts that extend beyond
    // the terminal boundaries simply won't have buffer cells to draw to.
    // Use exact signed centering: (full - w) / 2 to avoid truncation asymmetry.
    let bx = full.left() as i32 + (full.width as i32 - w as i32) / 2;
    let by = full.top() as i32 + (full.height as i32 - h as i32) / 2;
    let bw = w as i32;
    let bh = h as i32;

    // Visible region of the border intersecting the terminal buffer
    let x_lo = bx.max(full.left() as i32);
    let x_hi = (bx + bw).min(full.right() as i32);
    let y_lo = by.max(full.top() as i32);
    let y_hi = (by + bh).min(full.bottom() as i32);

    // Helper to draw a box-drawing character at a buffer cell if in range.
    // Skips the bottom-right cell to avoid VT100 DECAWM auto-wrap scroll.
    let set_cell = |buf: &mut ratatui::buffer::Buffer, x: i32, y: i32, ch: char| {
        if x == br_x as i32 && y == br_y as i32 {
            return;
        }
        if x >= full.left() as i32 && x < full.right() as i32
            && y >= full.top() as i32 && y < full.bottom() as i32
            && let Some(cell) = buf.cell_mut((x as u16, y as u16))
        {
            let mut s = [0u8; 4];
            let s = ch.encode_utf8(&mut s);
            cell.set_symbol(s).set_style(line_style);
        }
    };

    if x_lo < x_hi && y_lo < y_hi {
        // Top edge
        let ty = by;
        if ty >= full.top() as i32 && ty < full.bottom() as i32 {
            for x in x_lo..x_hi {
                let ch = if x == bx { '┌' } else if x == bx + bw - 1 { '┐' } else { '─' };
                set_cell(buf, x, ty, ch);
            }
        }
        // Bottom edge
        let ty2 = by + bh - 1;
        if ty2 >= full.top() as i32 && ty2 < full.bottom() as i32 {
            for x in x_lo..x_hi {
                let ch = if x == bx { '└' } else if x == bx + bw - 1 { '┘' } else { '─' };
                set_cell(buf, x, ty2, ch);
            }
        }
        // Left edge (skip corners — already drawn above)
        let lx = bx;
        if lx >= full.left() as i32 && lx < full.right() as i32 {
            for y in (by + 1).max(y_lo)..(by + bh - 1).min(y_hi) {
                set_cell(buf, lx, y, '│');
            }
        }
        // Right edge
        let rx = bx + bw - 1;
        if rx >= full.left() as i32 && rx < full.right() as i32 {
            for y in (by + 1).max(y_lo)..(by + bh - 1).min(y_hi) {
                set_cell(buf, rx, y, '│');
            }
        }
    }

    let label = if locked {
        format!("{}x{}  [LOCKED]", w, h)
    } else {
        format!("{}x{}", w, h)
    };

    let text_style = Style::default()
        .fg(Color::White)
        .bg(BG)
        .add_modifier(Modifier::BOLD);

    // Text centered on the border rect, clamped to terminal bounds
    let text_x = (bx + bw / 2 - label.len() as i32 / 2).max(full.left() as i32) as u16;
    let text_y = (by + bh / 2)
        .clamp(full.top() as i32, full.bottom().saturating_sub(1) as i32) as u16;
    let max_x = full.right();

    for (i, ch) in label.chars().enumerate() {
        let cx = text_x.saturating_add(i as u16);
        if cx >= max_x { break; }
        if cx == br_x && text_y == br_y { continue; }
        let Some(cell) = buf.cell_mut((cx, text_y)) else { continue; };
        let mut s = [0u8; 4];
        let s = ch.encode_utf8(&mut s);
        cell.set_symbol(s).set_style(text_style);
    }
}
