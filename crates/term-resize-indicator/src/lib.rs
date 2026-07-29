use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen, SetSize};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Paragraph, Widget};
use ratatui::Terminal;

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

enum Mode {
    Interactive,
    Locked(Size),
}

struct App {
    mode: Mode,
    last_size: Size,
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
            last_size: Size::new(current.0, current.1),
        }
    } else {
        App {
            mode: Mode::Interactive,
            last_size: Size::new(current.0, current.1),
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
            let (disp_w, disp_h, locked) = match app.mode {
                Mode::Locked(s) => (s.width, s.height, true),
                Mode::Interactive => (app.last_size.width, app.last_size.height, false),
            };
            draw_overlay(f, area, disp_w, disp_h, locked);
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
    app.last_size = Size::new(w, h);
    if let Mode::Locked(locked) = app.mode {
        let _ = crossterm::execute!(terminal.backend_mut(), SetSize(locked.width, locked.height));
    }
}

fn toggle_lock(app: &mut App) {
    match app.mode {
        Mode::Interactive => {
            let size = app.last_size;
            app.mode = Mode::Locked(size);
        }
        Mode::Locked(locked) => {
            app.last_size = locked;
            app.mode = Mode::Interactive;
        }
    }
}

fn draw_overlay(f: &mut ratatui::Frame, full: Rect, disp_w: u16, disp_h: u16, locked: bool) {
    if full.width < 4 || full.height < 3 {
        return;
    }

    // When locked, draw the border at the locked dimensions (centered).
    // When interactive, draw it at the full terminal size.
    let border = if locked {
        let x = (i32::from(full.left()) + i32::from(full.width) / 2 - i32::from(disp_w) / 2)
            .max(0) as u16;
        let y = (i32::from(full.top()) + i32::from(full.height) / 2 - i32::from(disp_h) / 2)
            .max(0) as u16;
        Rect { x, y, width: disp_w, height: disp_h }
    } else {
        full
    };

    let dot_style = Style::default()
        .fg(Color::Cyan)
        .bg(BG)
        .add_modifier(Modifier::BOLD);

    let bg_style = Style::default().bg(BG);

    let buf = f.buffer_mut();

    // Fill background across the full terminal
    for y in full.top()..full.bottom() {
        for x in full.left()..full.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_style(bg_style);
                cell.set_symbol(" ");
            }
        }
    }

    // Dotted border at the border rect
    if border.width >= 2 && border.height >= 2 {
        for x in border.left()..border.right() {
            if let Some(cell) = buf.cell_mut((x, border.top())) {
                cell.set_symbol("·").set_style(dot_style);
            }
            if let Some(cell) = buf.cell_mut((x, border.bottom() - 1)) {
                cell.set_symbol("·").set_style(dot_style);
            }
        }
        for y in (border.top() + 1)..(border.bottom() - 1) {
            if let Some(cell) = buf.cell_mut((border.left(), y)) {
                cell.set_symbol("·").set_style(dot_style);
            }
            if let Some(cell) = buf.cell_mut((border.right() - 1, y)) {
                cell.set_symbol("·").set_style(dot_style);
            }
        }
    }

    let label = if locked {
        format!("{}x{}  [LOCKED]", disp_w, disp_h)
    } else {
        format!("{}x{}", disp_w, disp_h)
    };

    let text_style = Style::default()
        .fg(Color::White)
        .bg(BG)
        .add_modifier(Modifier::BOLD);

    let inner = Rect {
        x: border.left() + 1,
        y: border.top() + border.height / 2,
        width: border.width.saturating_sub(2),
        height: 1,
    };
    let para = Paragraph::new(label)
        .alignment(Alignment::Center)
        .style(text_style);

    if inner.width > 0 {
        para.render(inner, buf);
    }
}
