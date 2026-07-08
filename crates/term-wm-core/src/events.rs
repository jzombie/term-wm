use crate::mouse_coord::MousePosition;

// ============================================================================
// Core-owned event types — NO crossterm dependency
// ============================================================================

/// Core-owned keyboard event (independent of crossterm)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
    pub kind: KeyKind,
}

impl KeyEvent {
    /// Create a new KeyEvent with the given code, modifiers, and kind.
    pub fn new(code: KeyCode, modifiers: KeyModifiers, kind: KeyKind) -> Self {
        Self {
            code,
            modifiers,
            kind,
        }
    }
}

/// Core-owned key code (independent of crossterm)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyCode {
    Char(char),
    Enter,
    Tab,
    Backspace,
    Esc,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Delete,
    Insert,
    F(u8),
    // Media keys
    MediaPlayPause,
    MediaStop,
    MediaTrackNext,
    MediaTrackPrevious,
}

/// Core-owned key modifiers (independent of crossterm)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
}

impl KeyModifiers {
    pub const NONE: Self = Self {
        shift: false,
        control: false,
        alt: false,
    };
}

// TODO: Rename to `KeyEventKind`?
/// Core-owned key event kind (independent of crossterm)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    Press,
    Repeat,
    Release,
}

/// Core-owned mouse event (independent of crossterm)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseEvent {
    pub kind: MouseEventKind,
    pub modifiers: KeyModifiers,
    pub column: u16,
    pub row: u16,
}

/// Core-owned mouse event kind (independent of crossterm)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEventKind {
    Press(MouseButton),
    Release(MouseButton),
    Drag(MouseButton),
    Moved,
    ScrollUp,
    ScrollDown,
    ScrollLeft,
    ScrollRight,
}

/// Core-owned mouse button (independent of crossterm)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

// ============================================================================
// Core-owned Event enum — the top-level event type
// ============================================================================

/// Core-owned event enum (independent of crossterm)
#[derive(Debug, Clone)]
pub enum Event {
    /// Keyboard event
    Key(KeyEvent),
    /// Mouse event
    Mouse(MouseEvent),
    /// Terminal resize
    Resize(u16, u16),
    /// Window gained focus
    FocusGained,
    /// Window lost focus
    FocusLost,
    /// Paste from clipboard
    Paste(String),
}

// ============================================================================
// Typed window manager event (for component dispatch)
// ============================================================================

/// Typed window manager event, used in the component dispatch path.
///
/// Key property: `Mouse` events always carry `MousePosition` with
/// `CoordSpace::Screen`. Coordinates are never mutated during dispatch.
/// Components that need local coordinates call `position.to_local(area)`.
#[derive(Debug, Clone, PartialEq)]
pub enum WmEvent {
    /// Keyboard event
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
    pub kind: MouseEventKind,
    pub modifiers: KeyModifiers,
}

// ============================================================================
// EventResult — return type from component event handling
// ============================================================================

/// Result of handling an event in a component.
#[derive(Debug)]
pub enum EventResult<Msg> {
    /// Event was not handled
    Ignored,
    /// Event was consumed (handled)
    Consumed,
    /// Event produced a message/action
    Action(Msg),
}

// ============================================================================
// Translation functions — convert core Event to WmEvent
// ============================================================================

/// Convert a core `Event` into a `WmEvent`, returning `None` for
/// unrecognized event types.
pub fn core_event_to_wm(event: &Event) -> Option<WmEvent> {
    match event {
        Event::Key(key) => Some(WmEvent::Key(*key)),
        Event::Mouse(mouse) => Some(WmEvent::Mouse {
            kind: mouse.kind,
            modifiers: mouse.modifiers,
            position: MousePosition {
                column: mouse.column as i16,
                row: mouse.row as i16,
                space: crate::mouse_coord::CoordSpace::Screen,
            },
        }),
        Event::Resize(w, h) => Some(WmEvent::Resize(*w, *h)),
        Event::FocusGained => Some(WmEvent::FocusGained),
        Event::FocusLost => Some(WmEvent::FocusLost),
        Event::Paste(text) => Some(WmEvent::Paste(text.clone())),
    }
}
