use super::{FloatRectSpec, SystemWindowId, WindowId};

#[derive(Debug, Clone)]
pub struct Window {
    pub title: Option<String>,
    pub title_set_order: Option<usize>,
    pub minimized: bool,
    pub floating_rect: Option<FloatRectSpec>,
    pub prev_floating_rect: Option<FloatRectSpec>,
    pub creation_order: usize,
    pub direct_mode: bool,
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
        }
    }

    pub fn title_or_default<Id: Copy + Eq + Ord + std::fmt::Debug>(
        &self,
        id: WindowId<Id>,
    ) -> String {
        self.title.clone().unwrap_or_else(|| match id {
            WindowId::App(app_id) => format!("{:?}", app_id),
            WindowId::System(SystemWindowId::DebugLog) => "Debug Log".to_string(),
        })
    }

    pub fn is_floating(&self) -> bool {
        self.floating_rect.is_some()
    }
}
