/// Shared application identity information.
///
/// Created once at application startup (typically in `main.rs`) and
/// shared via [`Arc`] so that every [`ComponentContext`] created by the
/// window manager can cheaply reference the same data without copying.
///
/// # Static vs. dynamic branding (#284)
///
/// `AppContext::new(name, ...)` means "the host application explicitly set
/// this name": embedders (term-wm-as-a-library) get that exact name in every
/// UI surface, unchanged. The bundled binary instead opts into dynamic Menu /
/// FAB branding via [`AppContext::with_dynamic_label`], which resolves a
/// display label per frame through
/// [`AppContext::resolve_display_label`] with the priority:
/// explicit host name (when static) → workspace name → launch-directory
/// name → app name.
///
/// [`Arc`]: std::sync::Arc
/// [`ComponentContext`]: crate::component_context::ComponentContext
#[derive(Debug, Clone)]
pub struct AppContext {
    pub app_name: String,
    pub app_version: String,
    pub hostname: Option<String>,
    /// Dynamic branding enabled? Only the bundled binary sets this; embedders
    /// keep their explicit `app_name` verbatim.
    dynamic_label: bool,
    /// Sanitized launch-directory basename used as the workspace-name fallback.
    directory_label: Option<String>,
}

impl AppContext {
    pub fn new(app_name: &str, app_version: &str) -> Self {
        Self {
            app_name: app_name.to_string(),
            app_version: app_version.to_string(),
            hostname: None,
            dynamic_label: false,
            directory_label: None,
        }
    }

    pub fn with_hostname(mut self, hostname: &str) -> Self {
        self.hostname = Some(hostname.to_string());
        self
    }

    /// Opt into dynamic Menu/FAB branding with the given launch-directory
    /// fallback label (see [#284]). The explicit `app_name` passed to
    /// [`AppContext::new`] remains the last-resort fallback.
    ///
    /// [#284]: https://github.com/jzombie/term-wm/issues/284
    pub fn with_dynamic_label(mut self, directory_label: Option<String>) -> Self {
        self.dynamic_label = true;
        self.directory_label = directory_label.filter(|s| !s.is_empty());
        self
    }

    /// Whether dynamic branding is active for this context.
    pub fn has_dynamic_label(&self) -> bool {
        self.dynamic_label
    }

    /// Resolve the label the Main Menu / FAB should render this frame.
    ///
    /// Priority: current workspace name → launch-directory name → app name.
    /// When dynamic branding is off this always returns the explicit
    /// `app_name`, preserving the embedder contract.
    ///
    /// `workspace` is the caller's current-workspace hint (empty/absent when
    /// unknown or session persistence is inactive).
    pub fn resolve_display_label(&self, workspace: Option<&str>) -> String {
        if !self.dynamic_label {
            return self.app_name.clone();
        }
        let workspace = workspace.map(str::trim).filter(|s| !s.is_empty());
        match workspace {
            Some(ws) => ws.to_string(),
            None => self
                .directory_label
                .clone()
                .unwrap_or_else(|| self.app_name.clone()),
        }
    }
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_context_always_returns_explicit_name() {
        let ctx = AppContext::new("myapp", "1.0");
        assert!(!ctx.has_dynamic_label());
        assert_eq!(ctx.resolve_display_label(Some("dev")), "myapp");
        assert_eq!(ctx.resolve_display_label(None), "myapp");
    }

    #[test]
    fn dynamic_context_prioritizes_workspace_over_directory_and_app() {
        let ctx = AppContext::new("term-wm", "0.1").with_dynamic_label(Some("proj".to_string()));
        assert!(ctx.has_dynamic_label());
        assert_eq!(ctx.resolve_display_label(Some("ws-42")), "ws-42");
        assert_eq!(
            ctx.resolve_display_label(Some("  dev  ")),
            "dev",
            "workspace hint must be trimmed"
        );
        assert_eq!(
            ctx.resolve_display_label(None),
            "proj",
            "directory label is second priority"
        );
    }

    #[test]
    fn dynamic_context_empty_hint_treated_as_absent() {
        let ctx = AppContext::new("term-wm", "0.1").with_dynamic_label(Some("proj".to_string()));
        assert_eq!(ctx.resolve_display_label(Some("")), "proj");
        assert_eq!(ctx.resolve_display_label(Some("   ")), "proj");
    }

    #[test]
    fn dynamic_context_without_directory_falls_back_to_app_name() {
        let ctx = AppContext::new("term-wm", "0.1").with_dynamic_label(None);
        assert_eq!(ctx.resolve_display_label(None), "term-wm");
    }

    #[test]
    fn with_hostname_preserves_branding_fields() {
        // Builder chaining must not reset branding state.
        let ctx = AppContext::new("term-wm", "0.1")
            .with_hostname("h")
            .with_dynamic_label(Some("d".to_string()));
        assert!(ctx.has_dynamic_label());
        assert_eq!(ctx.resolve_display_label(None), "d");
    }
}
