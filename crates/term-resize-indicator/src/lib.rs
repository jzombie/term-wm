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
        let _ = crossterm::execute!(std::io::stdout(), SetSize(size.width, size.height));
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
            let display = match app.mode {
                Mode::Locked(s) => s,
                Mode::Interactive => app.last_size,
            };
            draw_overlay(f, area, display.width, display.height, matches!(app.mode, Mode::Locked(_)));
        })?;

        if !event::poll(std::time::Duration::from_millis(100))? {
            continue;
        }

        match event::read()? {
            Event::Resize(w, h) => on_resize(app, w, h),
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

fn on_resize(app: &mut App, w: u16, h: u16) {
    app.last_size = Size::new(w, h);
    if let Mode::Locked(locked) = app.mode {
        let _ = crossterm::execute!(std::io::stdout(), SetSize(locked.width, locked.height));
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

fn draw_overlay(f: &mut ratatui::Frame, area: Rect, width: u16, height: u16, locked: bool) {
    if area.width < 4 || area.height < 3 {
        return;
    }

    let dot_style = Style::default()
        .fg(Color::Cyan)
        .bg(BG)
        .add_modifier(Modifier::BOLD);

    let bg_style = Style::default().bg(BG);

    let buf = f.buffer_mut();

    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_style(bg_style);
                cell.set_symbol(" ");
            }
        }
    }

    for x in area.left()..area.right() {
        if let Some(cell) = buf.cell_mut((x, area.top())) {
            cell.set_symbol("·").set_style(dot_style);
        }
        if let Some(cell) = buf.cell_mut((x, area.bottom() - 1)) {
            cell.set_symbol("·").set_style(dot_style);
        }
    }

    for y in (area.top() + 1)..(area.bottom() - 1) {
        if let Some(cell) = buf.cell_mut((area.left(), y)) {
            cell.set_symbol("·").set_style(dot_style);
        }
        if let Some(cell) = buf.cell_mut((area.right() - 1, y)) {
            cell.set_symbol("·").set_style(dot_style);
        }
    }

    let label = if locked {
        format!("{}x{}  [LOCKED]", width, height)
    } else {
        format!("{}x{}", width, height)
    };

    let text_style = Style::default()
        .fg(Color::White)
        .bg(BG)
        .add_modifier(Modifier::BOLD);

    let inner = Rect {
        x: area.left() + 1,
        y: area.top() + area.height / 2,
        width: area.width.saturating_sub(2),
        height: 1,
    };
    let para = Paragraph::new(label)
        .alignment(Alignment::Center)
        .style(text_style);
    para.render(inner, buf);
}
