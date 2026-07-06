use crossterm::event::{KeyEvent, KeyModifiers, MouseEventKind};

use crate::mouse_coord::MousePosition;

/// Typed window manager event, replacing raw `crossterm::Event` in the
/// component dispatch path.
///
/// Key property: `Mouse` events always carry `MousePosition` with
/// `CoordSpace::Screen`. Coordinates are never mutated during dispatch.
/// Components that need local coordinates call `position.to_local(area)`.
#[derive(Debug, Clone, PartialEq)]
pub enum WmEvent {
    /// Keyboard event (unchanged from crossterm).
    Key(KeyEvent),
    /// Mouse event with typed, immutable screen-space position.
    Mouse {
        kind: MouseEventKind,
        modifiers: KeyModifiers,
        position: MousePosition,
    },
    /// Terminal resize.
    Resize(u16, u16),
    /// Window gained focus.
    FocusGained,
    /// Window lost focus.
    FocusLost,
    /// Paste from clipboard.
    Paste(String),
}

/// A mouse event with coordinates localized to the component's top-left (0, 0).
pub struct LocalMouseEvent {
    pub col: u16,
    pub row: u16,
    pub kind: crossterm::event::MouseEventKind,
    pub modifiers: crossterm::event::KeyModifiers,
}

/// Convert a crossterm `Event` into a `WmEvent`, returning `None` for
/// unrecognized event types (e.g. crossterm-internal paste events when
/// the feature is not enabled).
pub fn crossterm_event_to_wm(event: &crossterm::event::Event) -> Option<WmEvent> {
    match event {
        crossterm::event::Event::Key(key) => Some(WmEvent::Key(*key)),
        crossterm::event::Event::Mouse(mouse) => Some(WmEvent::Mouse {
            kind: mouse.kind,
            modifiers: mouse.modifiers,
            position: MousePosition {
                column: mouse.column as i16,
                row: mouse.row as i16,
                space: crate::mouse_coord::CoordSpace::Screen,
            },
        }),
        crossterm::event::Event::Resize(w, h) => Some(WmEvent::Resize(*w, *h)),
        crossterm::event::Event::FocusGained => Some(WmEvent::FocusGained),
        crossterm::event::Event::FocusLost => Some(WmEvent::FocusLost),
        crossterm::event::Event::Paste(text) => Some(WmEvent::Paste(text.clone())),
    }
}
