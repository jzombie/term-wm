use crate::hitbox_registry::HitboxId;
use crate::layout::floating::ResizeEdge;
use crate::window::WindowKey;

/// Strongly-typed payload for chrome element hit-testing.
/// Core defines only the vocabulary — no metrics, no button positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromeTarget {
    /// Floating window resize edge.
    Resize(WindowKey, ResizeEdge),
    /// Window header — initiates drag/detach.
    Drag(WindowKey),
    /// Close button in the window header.
    CloseButton(WindowKey),
    /// Maximize button in the window header.
    MaximizeButton(WindowKey),
    /// Minimize button in the window header.
    MinimizeButton(WindowKey),
    /// Toggle direct mode button in the window header.
    ToggleDirectMode(WindowKey),
    /// Tiling layout split handle seam.
    SplitHandle(HitboxId),
}
