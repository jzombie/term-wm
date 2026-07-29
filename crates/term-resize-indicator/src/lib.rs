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
    /// In locked mode this gets immediately snapped back to the locked size.
    live_size: Size,
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
        }
    } else {
        App {
            mode: Mode::Interactive,
            live_size: Size::new(current.0, current.1),
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
        terminal.draw(|f| {
            let area = f.area();
            let dim = app.live_size;
            let locked = matches!(app.mode, Mode::Locked(_));
            draw_overlay(f, area, dim.width, dim.height, locked);
        })?;

        if !event::poll(std::time::Duration::from_millis(100))? {
            continue;
        }

        match event::read()? {
            Event::Resize(w, h) => on_resize(app, terminal, w, h),
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Char('l' | 'L') => toggle_lock(app),
                KeyCode::Char('q' | 'Q') | KeyCode::Esc => break,
                KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => break,
                _ => {}
            },
            _ => {}
        }
    }
    Ok(())
}

fn on_resize(
    app: &mut App,
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    w: u16,
    h: u16,
) {
    app.live_size = Size::new(w, h);
    if let Mode::Locked(locked) = app.mode {
        let _ = crossterm::execute!(terminal.backend_mut(), SetSize(locked.width, locked.height));
        app.live_size = locked;
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

    for y in full.top()..full.bottom() {
        for x in full.left()..full.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_style(bg_style);
                cell.set_symbol(" ");
            }
        }
    }

    let x0 = full.left();
    let x1 = full.right().saturating_sub(1);
    let y0 = full.top();
    let y1 = full.bottom().saturating_sub(1);

    // Corners
    if let Some(cell) = buf.cell_mut((x0, y0)) { cell.set_symbol("┌").set_style(line_style); }
    if let Some(cell) = buf.cell_mut((x1, y0)) { cell.set_symbol("┐").set_style(line_style); }
    if let Some(cell) = buf.cell_mut((x0, y1)) { cell.set_symbol("└").set_style(line_style); }
    if let Some(cell) = buf.cell_mut((x1, y1)) { cell.set_symbol("┘").set_style(line_style); }

    // Top and bottom edges
    for x in (x0 + 1)..x1 {
        if let Some(cell) = buf.cell_mut((x, y0)) { cell.set_symbol("─").set_style(line_style); }
        if let Some(cell) = buf.cell_mut((x, y1)) { cell.set_symbol("─").set_style(line_style); }
    }

    // Left and right edges
    for y in (y0 + 1)..y1 {
        if let Some(cell) = buf.cell_mut((x0, y)) { cell.set_symbol("│").set_style(line_style); }
        if let Some(cell) = buf.cell_mut((x1, y)) { cell.set_symbol("│").set_style(line_style); }
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

    let text_x = full.left() + (full.width.saturating_sub(label.len() as u16)) / 2;
    let text_y = full.top() + full.height / 2;
    let max_x = full.left() + full.width;

    for (i, ch) in label.chars().enumerate() {
        let cx = text_x + i as u16;
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
