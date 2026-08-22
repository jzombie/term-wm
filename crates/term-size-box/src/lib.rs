#![doc = include_str!("../README.md")]

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    self, DisableLineWrap, EnableLineWrap, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

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

struct App {
    live_size: Size,
}

pub fn run() -> std::io::Result<()> {
    terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen, DisableLineWrap)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let current = terminal::size().unwrap_or((80, 24));
    let mut app = App {
        live_size: Size::new(current.0, current.1),
    };

    let res = run_loop(&mut terminal, &mut app);

    let _ = crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen, EnableLineWrap);
    let _ = terminal::disable_raw_mode();

    res
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
) -> std::io::Result<()> {
    loop {
        terminal.draw(|f| {
            let area = f.area();
            let dim = app.live_size;
            draw_overlay(f, area, dim.width, dim.height);
        })?;

        if !event::poll(std::time::Duration::from_millis(50))? {
            continue;
        }

        match event::read()? {
            Event::Resize(w, h) => app.live_size = Size::new(w, h),
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Char('q' | 'Q') | KeyCode::Esc => break,
                KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => break,
                _ => {}
            },
            _ => {}
        }
    }
    Ok(())
}

/// Draw the full-screen overlay.
///
/// - Background fill (dark blue) so both viewers see a uniform canvas.
/// - Box-drawing border (`┌─┐│└─┘`) edge-to-edge — a deliberate measured box
///   that is visually distinct from the terminal boundary.
/// - Centered text shows the live terminal dimensions.
fn draw_overlay(f: &mut ratatui::Frame, full: Rect, w: u16, h: u16) {
    if full.width < 4 || full.height < 3 {
        return;
    }

    let line_style = Style::default()
        .fg(Color::Cyan)
        .bg(BG)
        .add_modifier(Modifier::BOLD);

    let bg_style = Style::default().bg(BG);

    let buf = f.buffer_mut();

    for y in full.top()..full.bottom() {
        for x in full.left()..full.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_style(bg_style);
                cell.set_symbol(" ");
            }
        }
    }

    let bx = full.left() as i32 + (full.width as i32 - w as i32) / 2;
    let by = full.top() as i32 + (full.height as i32 - h as i32) / 2;
    let bw = w as i32;
    let bh = h as i32;

    let x_lo = bx.max(full.left() as i32);
    let x_hi = (bx + bw).min(full.right() as i32);
    let y_lo = by.max(full.top() as i32);
    let y_hi = (by + bh).min(full.bottom() as i32);

    let set_cell = |buf: &mut ratatui::buffer::Buffer, x: i32, y: i32, ch: char| {
        if x >= full.left() as i32
            && x < full.right() as i32
            && y >= full.top() as i32
            && y < full.bottom() as i32
            && let Some(cell) = buf.cell_mut((x as u16, y as u16))
        {
            let mut s = [0u8; 4];
            let s = ch.encode_utf8(&mut s);
            cell.set_symbol(s).set_style(line_style);
        }
    };

    if x_lo < x_hi && y_lo < y_hi {
        let ty = by;
        if ty >= full.top() as i32 && ty < full.bottom() as i32 {
            for x in x_lo..x_hi {
                let ch = if x == bx {
                    '┌'
                } else if x == bx + bw - 1 {
                    '┐'
                } else {
                    '─'
                };
                set_cell(buf, x, ty, ch);
            }
        }
        let ty2 = by + bh - 1;
        if ty2 >= full.top() as i32 && ty2 < full.bottom() as i32 {
            for x in x_lo..x_hi {
                let ch = if x == bx {
                    '└'
                } else if x == bx + bw - 1 {
                    '┘'
                } else {
                    '─'
                };
                set_cell(buf, x, ty2, ch);
            }
        }
        let lx = bx;
        if lx >= full.left() as i32 && lx < full.right() as i32 {
            for y in (by + 1).max(y_lo)..(by + bh - 1).min(y_hi) {
                set_cell(buf, lx, y, '│');
            }
        }
        let rx = bx + bw - 1;
        if rx >= full.left() as i32 && rx < full.right() as i32 {
            for y in (by + 1).max(y_lo)..(by + bh - 1).min(y_hi) {
                set_cell(buf, rx, y, '│');
            }
        }
    }

    let label = format!("{}x{}", w, h);

    let text_style = Style::default()
        .fg(Color::White)
        .bg(BG)
        .add_modifier(Modifier::BOLD);

    let text_x = (bx + bw / 2 - label.len() as i32 / 2).max(full.left() as i32) as u16;
    let text_y =
        (by + bh / 2).clamp(full.top() as i32, full.bottom().saturating_sub(1) as i32) as u16;
    let max_x = full.right();

    for (i, ch) in label.chars().enumerate() {
        let cx = text_x.saturating_add(i as u16);
        if cx >= max_x {
            break;
        }
        let Some(cell) = buf.cell_mut((cx, text_y)) else {
            continue;
        };
        let mut s = [0u8; 4];
        let s = ch.encode_utf8(&mut s);
        cell.set_symbol(s).set_style(text_style);
    }
}
