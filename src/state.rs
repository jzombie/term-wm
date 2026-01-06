#[derive(Debug, Default, Clone, Copy)]
pub struct AppState {
    mouse_capture_enabled: bool,
    mouse_capture_dirty: bool,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            mouse_capture_enabled: true,
            mouse_capture_dirty: false,
        }
    }

    pub fn mouse_capture_enabled(&self) -> bool {
        self.mouse_capture_enabled
    }

    pub fn set_mouse_capture_enabled(&mut self, enabled: bool) {
        if self.mouse_capture_enabled == enabled {
            return;
        }
        self.mouse_capture_enabled = enabled;
        self.mouse_capture_dirty = true;
    }

    pub fn toggle_mouse_capture(&mut self) {
        let enabled = !self.mouse_capture_enabled;
        self.set_mouse_capture_enabled(enabled);
    }

    pub fn take_mouse_capture_change(&mut self) -> Option<bool> {
        if self.mouse_capture_dirty {
            self.mouse_capture_dirty = false;
            Some(self.mouse_capture_enabled)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_capture_toggle_and_take_change() {
        let mut s = AppState::new();
        assert!(s.mouse_capture_enabled());
        s.set_mouse_capture_enabled(true);
        // no change -> None
        assert!(s.take_mouse_capture_change().is_none());
        s.set_mouse_capture_enabled(false);
        // now change recorded
        assert_eq!(s.take_mouse_capture_change(), Some(false));
        // consumed
        assert!(s.take_mouse_capture_change().is_none());
        s.toggle_mouse_capture();
        assert!(s.mouse_capture_enabled());
    }
}
