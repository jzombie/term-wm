use super::FloatRectSpec;
use crate::components::Component;

/// A window entry in the SlotMap — the single source of truth for all
/// window data, including the optional renderable component.
/// Process teardown is handled by the `Reaper`, not by `Drop`.
pub struct Window {
    pub title: Option<String>,
    pub title_set_order: Option<usize>,
    pub minimized: bool,
    pub floating_rect: Option<FloatRectSpec>,
    pub prev_floating_rect: Option<FloatRectSpec>,
    pub creation_order: usize,
    pub direct_mode: bool,
    /// The renderable component (terminal, debug log, etc.).
    /// `None` for chrome-only windows or windows whose component is
    /// managed by the application via `WindowProvider::window_component`.
    pub component: Option<Box<dyn Component>>,
}

impl Window {
    pub fn new(creation_order: usize) -> Self {
        Self {
            title: None,
            title_set_order: None,
            minimized: false,
            floating_rect: None,
            prev_floating_rect: None,
            creation_order,
            direct_mode: false,
            component: None,
        }
    }

    pub fn title_or_default(&self, key: super::WindowKey) -> String {
        self.title.clone().unwrap_or_else(|| format!("{:?}", key))
    }

    pub fn is_floating(&self) -> bool {
        self.floating_rect.is_some()
    }
}
