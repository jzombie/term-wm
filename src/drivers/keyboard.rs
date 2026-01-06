use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::io;

pub trait KeyboardDriver {
    fn next_key(&mut self) -> io::Result<KeyEvent>;
}

#[derive(Default)]
pub struct KeyboardNormalizer {
    esc_down: bool,
}

impl KeyboardNormalizer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn normalize(&mut self, evt: Event) -> Option<Event> {
        match evt {
            Event::Key(mut key) => {
                if key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::SHIFT) {
                    key.code = KeyCode::BackTab;
                    key.modifiers.remove(KeyModifiers::SHIFT);
                }
                if cfg!(windows) {
                    match key.kind {
                        KeyEventKind::Release => {
                            if key.code == KeyCode::Esc {
                                self.esc_down = false;
                            }
                            return None;
                        }
                        KeyEventKind::Repeat => return None,
                        KeyEventKind::Press => {}
                    }
                    if key.code == KeyCode::Esc {
                        if self.esc_down {
                            return None;
                        }
                        self.esc_down = true;
                    } else {
                        self.esc_down = false;
                    }
                } else if key.kind == KeyEventKind::Release {
                    return None;
                }
                Some(Event::Key(key))
            }
            other => Some(other),
        }
    }
}
