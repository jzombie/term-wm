/// Shared application identity information.
///
/// Created once at application startup (typically in `main.rs`) and
/// shared via [`Arc`] so that every [`ComponentContext`] created by the
/// window manager can cheaply reference the same data without copying.
///
/// [`Arc`]: std::sync::Arc
/// [`ComponentContext`]: crate::component_context::ComponentContext
#[derive(Debug, Clone)]
pub struct AppContext {
    pub app_name: String,
    pub app_version: String,
    pub hostname: Option<String>,
}

impl AppContext {
    pub fn new(app_name: &str, app_version: &str) -> Self {
        Self {
            app_name: app_name.to_string(),
            app_version: app_version.to_string(),
            hostname: None,
        }
    }

    pub fn with_hostname(mut self, hostname: &str) -> Self {
        self.hostname = Some(hostname.to_string());
        self
    }
}
