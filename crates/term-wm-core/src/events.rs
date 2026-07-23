use crate::mouse_coord::MousePosition;
use term_wm_layout_engine::LayoutRect;

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

impl MouseEvent {
    /// Translate a screen-space mouse event into local/virtual space relative
    /// to a bounding screen area, scroll offsets, and virtual content dimensions.
    ///
    /// CATEGORY 1 — Viewport Spatial Gate.
    /// Point-trigger events (Press, Moved, Scroll*) outside bounds return None.
    /// Continuous lifecycle events (Drag, Release) are retained and clamped to
    /// valid virtual coordinates to preserve input state machines.
    /// This prevents click leakage across tiled panes.
    pub fn to_local_offset(
        &self,
        screen_area: LayoutRect,
        offset_x: usize,
        offset_y: usize,
        content_width: u16,
        content_height: u16,
    ) -> Option<Self> {
        if screen_area.width == 0
            || screen_area.height == 0
            || content_width == 0
            || content_height == 0
        {
            return None;
        }

        let m_x = i32::from(self.column);
        let m_y = i32::from(self.row);

        // Physical screen viewport bounds check
        let in_physical_x =
            m_x >= screen_area.x && m_x < screen_area.x + i32::from(screen_area.width);
        let in_physical_y =
            m_y >= screen_area.y && m_y < screen_area.y + i32::from(screen_area.height);

        // Virtual coordinate translation
        let v_x_raw = m_x - screen_area.x + offset_x as i32;
        let v_y_raw = m_y - screen_area.y + offset_y as i32;

        // Virtual content bounds check
        let in_virtual_x = v_x_raw >= 0 && v_x_raw < i32::from(content_width);
        let in_virtual_y = v_y_raw >= 0 && v_y_raw < i32::from(content_height);

        let is_point_trigger = matches!(
            self.kind,
            MouseEventKind::Press(_)
                | MouseEventKind::Moved
                | MouseEventKind::ScrollUp
                | MouseEventKind::ScrollDown
                | MouseEventKind::ScrollLeft
                | MouseEventKind::ScrollRight
        );

        if is_point_trigger && (!in_physical_x || !in_physical_y || !in_virtual_x || !in_virtual_y)
        {
            return None;
        }

        // Clamp virtual coordinates for drag/release events drifting outside bounds
        let v_x = v_x_raw.clamp(0, i32::from(content_width.saturating_sub(1))) as u16;
        let v_y = v_y_raw.clamp(0, i32::from(content_height.saturating_sub(1))) as u16;

        Some(Self {
            kind: self.kind,
            modifiers: self.modifiers,
            column: v_x,
            row: v_y,
        })
    }

    /// Unculled origin translation: converts screen-space coordinates relative
    /// to (origin_x, origin_y) and clamps to [0, u16::MAX] without dropping any
    /// events.
    ///
    /// CATEGORY 2 — Unculled Coordinate Transformer.
    /// Never returns None. All events are accepted and clamped to valid u16
    /// bounds. Intended for window manager event dispatching (localize_event,
    /// localize_event_content) where PTY sessions must receive all inputs
    /// even when they land on window chrome or borders.
    pub fn to_clamped_origin(&self, origin_x: i32, origin_y: i32) -> Self {
        let col = (i32::from(self.column) - origin_x).clamp(0, i32::from(u16::MAX)) as u16;
        let row = (i32::from(self.row) - origin_y).clamp(0, i32::from(u16::MAX)) as u16;
        Self {
            kind: self.kind,
            modifiers: self.modifiers,
            column: col,
            row,
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use term_wm_layout_engine::LayoutRect;

    fn make_screen() -> LayoutRect {
        LayoutRect {
            x: 10,
            y: 5,
            width: 80,
            height: 24,
        }
    }

    fn mouse(col: u16, row: u16, kind: MouseEventKind) -> MouseEvent {
        MouseEvent {
            kind,
            modifiers: KeyModifiers::NONE,
            column: col,
            row,
        }
    }

    #[test]
    fn press_inside_returns_translated() {
        let m = mouse(15, 8, MouseEventKind::Press(MouseButton::Left));
        let result = m.to_local_offset(make_screen(), 0, 0, 80, 24);
        assert_eq!(result.map(|r| (r.column, r.row)), Some((5, 3)));
    }

    #[test]
    fn press_outside_physical_returns_none() {
        let m = mouse(0, 0, MouseEventKind::Press(MouseButton::Left));
        assert!(m.to_local_offset(make_screen(), 0, 0, 80, 24).is_none());
    }

    #[test]
    fn press_outside_virtual_returns_none() {
        // virtual x = 85 (exceeds content_width=80)
        let m = mouse(95, 8, MouseEventKind::Press(MouseButton::Left));
        assert!(m.to_local_offset(make_screen(), 0, 0, 80, 24).is_none());
    }

    #[test]
    fn scroll_outside_returns_none() {
        let m = mouse(0, 0, MouseEventKind::ScrollUp);
        assert!(m.to_local_offset(make_screen(), 0, 0, 80, 24).is_none());
    }

    #[test]
    fn drag_outside_returns_clamped() {
        let m = mouse(0, 0, MouseEventKind::Drag(MouseButton::Left));
        let result = m.to_local_offset(make_screen(), 0, 0, 80, 24);
        assert_eq!(result.map(|r| (r.column, r.row)), Some((0, 0)));
    }

    #[test]
    fn release_outside_returns_clamped() {
        let m = mouse(120, 40, MouseEventKind::Release(MouseButton::Left));
        let result = m.to_local_offset(make_screen(), 0, 0, 80, 24);
        assert_eq!(result.map(|r| (r.column, r.row)), Some((79, 23)));
    }

    #[test]
    fn negative_screen_origin() {
        let screen = LayoutRect {
            x: -5,
            y: -5,
            width: 80,
            height: 24,
        };
        let m = mouse(2, 2, MouseEventKind::Press(MouseButton::Left));
        let result = m.to_local_offset(screen, 0, 0, 80, 24);
        assert_eq!(result.map(|r| (r.column, r.row)), Some((7, 7)));
    }


    #[test]
    fn clamped_origin_inside_bounds() {
        let m = mouse(15, 20, MouseEventKind::Press(MouseButton::Left));
        let result = m.to_clamped_origin(10, 5);
        assert_eq!(result.column, 5);
        assert_eq!(result.row, 15);
    }

    #[test]
    fn clamped_origin_negative_origin_clamps_lower() {
        let m = mouse(2, 2, MouseEventKind::Press(MouseButton::Left));
        let result = m.to_clamped_origin(10, 10);
        assert_eq!(result.column, 0);
        assert_eq!(result.row, 0);
    }
    #[test]
    fn with_scroll_offset() {
        let m = mouse(15, 8, MouseEventKind::Press(MouseButton::Left));
        let result = m.to_local_offset(make_screen(), 10, 5, 80, 24);
        assert_eq!(result.map(|r| (r.column, r.row)), Some((15, 8)));
    }
}
