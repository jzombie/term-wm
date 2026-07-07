use std::marker::PhantomData;
use std::sync::Arc;

use ratatui::layout::Rect;

use crate::app_context::AppContext;
use crate::bottom_panel_trait::BottomPanel;
use crate::components::{MenuOverlay, WmComponent};
use crate::keybindings::KeyBindings;
use crate::theme::Theme;
use crate::top_panel_trait::TopPanel;
use crate::window::decorator::WindowDecorator;
use crate::window::{WindowKey, WindowManager};
use crate::wm_config::{HintVisibility, WmConfig};

/// Builder for [`WmConfig`] and [`WindowManager`].
///
/// Provides a uniform construction path for standalone and embedded modes,
/// eliminating duplicate boilerplate across `main.rs` and `lib.rs`.
///
/// # Example
///
/// ```ignore
/// let wm = WmBuilder::standalone()
///     .app_ctx(Arc::new(app_ctx))
///     .build(current, top_panel, bottom_panel, menu);
/// ```
pub struct WmBuilder {
    config: WmConfig,
    app_ctx: Option<Arc<AppContext>>,
}

impl WmBuilder {
    /// Start with standalone (full WM) defaults.
    pub fn standalone() -> Self {
        Self {
            config: WmConfig::standalone(),
            app_ctx: None,
        }
    }

    /// Start with embedded (minimal) defaults.
    pub fn embedded() -> Self {
        Self {
            config: WmConfig::embedded(),
            app_ctx: None,
        }
    }

    /// Replace the entire config.
    pub fn with_config(mut self, config: WmConfig) -> Self {
        self.config = config;
        self
    }

    pub fn theme(mut self, theme: Theme) -> Self {
        self.config.theme = theme;
        self
    }

    pub fn keybindings(mut self, kb: KeyBindings) -> Self {
        self.config.keybindings = kb;
        self
    }

    pub fn decorator(mut self, decorator: Arc<dyn WindowDecorator>) -> Self {
        self.config.decorator = Some(decorator);
        self
    }

    pub fn keyboard_focus(mut self, enabled: bool) -> Self {
        self.config.keyboard_focus_enabled = enabled;
        self
    }

    pub fn mouse_focus_click(mut self, enabled: bool) -> Self {
        self.config.mouse_focus_click_enabled = enabled;
        self
    }

    pub fn hint_visibility(mut self, v: HintVisibility) -> Self {
        self.config.hint_visibility = v;
        self
    }

    pub fn chrome_enabled(mut self, enabled: bool) -> Self {
        self.config.chrome_enabled = enabled;
        self
    }

    pub fn floating_windows_enabled(mut self, enabled: bool) -> Self {
        self.config.floating_windows_enabled = enabled;
        self
    }

    pub fn panel_enabled(mut self, enabled: bool) -> Self {
        self.config.panel_enabled = enabled;
        self
    }

    pub fn wm_command_menu_enabled(mut self, enabled: bool) -> Self {
        self.config.wm_command_menu_enabled = enabled;
        self
    }

    pub fn app_ctx(mut self, ctx: Arc<AppContext>) -> Self {
        self.app_ctx = Some(ctx);
        self
    }

    /// Build a [`WindowManager`] from the accumulated configuration.
    pub fn build(
        self,
        top_panel: Option<Box<dyn TopPanel<WindowKey>>>,
        bottom_panel: Option<Box<dyn BottomPanel>>,
        menu_overlay: Option<Box<dyn MenuOverlay<crate::actions::TermWmAction>>>,
    ) -> WindowManager {
        let app_ctx = self.app_ctx.expect("app_ctx must be set before building");
        WindowManager::with_config(self.config, app_ctx, top_panel, bottom_panel, menu_overlay)
    }

    /// Access the underlying [`WmConfig`] for read or inspection.
    pub fn config(&self) -> &WmConfig {
        &self.config
    }
}

// ---------------------------------------------------------------------------
// Typestate markers for AppBuilder
// ---------------------------------------------------------------------------

/// Marker type for standalone (full WM) mode.
pub struct Standalone;

/// Marker type for embedded (nested) mode.
pub struct Embedded;

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::Standalone {}
    impl Sealed for super::Embedded {}
}

/// Trait implemented only by the two mode markers.
pub trait AppMode: sealed::Sealed {}
impl AppMode for Standalone {}
impl AppMode for Embedded {}

/// Error type for [`AppBuilder::build`].
#[derive(Debug)]
pub enum ConfigError {
    MissingAppContext,
}

/// Typestate builder for [`WindowManager`].
///
/// Generic over `M: AppMode` to enforce compile-time separation between
/// standalone and embedded construction paths. Shared configuration methods
/// are available on all modes; mode-specific methods are restricted to the
/// appropriate `impl` block.
pub struct AppBuilder<M: AppMode> {
    config: WmConfig,
    app_ctx: Option<Arc<AppContext>>,
    top_panel: Option<Box<dyn WmComponent>>,
    bottom_panel: Option<Box<dyn WmComponent>>,
    command_menu: Option<Box<dyn WmComponent>>,
    _mode: PhantomData<M>,
}

// --- Constructors (unconditional) ---

impl AppBuilder<Standalone> {
    /// Bare standalone — no default chrome. Consumer injects via IoC.
    pub fn bare_standalone() -> Self {
        Self {
            config: WmConfig::standalone(),
            app_ctx: None,
            top_panel: None,
            bottom_panel: None,
            command_menu: None,
            _mode: PhantomData,
        }
    }
}

impl AppBuilder<Embedded> {
    /// Embedded mode — configures the engine for nested operation.
    /// Geometry is supplied dynamically on every layout pass via
    /// `register_managed_layout(area)`, not cached in the builder.
    pub fn embedded() -> Self {
        Self {
            config: WmConfig::embedded(),
            app_ctx: None,
            top_panel: None,
            bottom_panel: None,
            command_menu: None,
            _mode: PhantomData,
        }
    }
}

// --- Shared methods (all modes) ---

impl<M: AppMode> AppBuilder<M> {
    pub fn app_ctx(mut self, ctx: Arc<AppContext>) -> Self {
        self.app_ctx = Some(ctx);
        self
    }

    pub fn theme(mut self, theme: Theme) -> Self {
        self.config.theme = theme;
        self
    }

    pub fn keybindings(mut self, kb: KeyBindings) -> Self {
        self.config.keybindings = kb;
        self
    }

    pub fn decorator(mut self, decorator: Arc<dyn WindowDecorator>) -> Self {
        self.config.decorator = Some(decorator);
        self
    }

    pub fn hint_visibility(mut self, v: HintVisibility) -> Self {
        self.config.hint_visibility = v;
        self
    }

    pub fn top_panel(mut self, panel: Box<dyn WmComponent>) -> Self {
        self.top_panel = Some(panel);
        self
    }

    pub fn bottom_panel(mut self, panel: Box<dyn WmComponent>) -> Self {
        self.bottom_panel = Some(panel);
        self
    }

    pub fn command_menu(mut self, menu: Box<dyn WmComponent>) -> Self {
        self.command_menu = Some(menu);
        self
    }

    /// Build a [`WindowManager`] from the accumulated configuration.
    ///
    /// Currently delegates to the existing `WmBuilder` path by wrapping
    /// the `WmComponent` trait objects into the old trait object slots.
    /// Once the `WindowManager` is migrated to use `WmComponent` directly
    /// (Phase 6), this will call `WindowManager::with_config` with the new
    /// component fields.
    pub fn build(self) -> Result<WindowManager, ConfigError> {
        let app_ctx = self.app_ctx.ok_or(ConfigError::MissingAppContext)?;

        // Wrap WmComponent trait objects into the old TopPanel/BottomPanel/MenuOverlay
        // trait objects by boxing them as `Box<dyn MenuOverlay<TermWmAction>>`.
        // This is a temporary bridge until Phase 6 migrates the WindowManager.
        let top = self.top_panel.map(|p| {
            let b: Box<dyn TopPanel<WindowKey>> =
                Box::new(WmComponentTopPanelBridge(p));
            b
        });
        let bottom = self.bottom_panel.map(|p| {
            let b: Box<dyn BottomPanel> = Box::new(WmComponentBottomPanelBridge(p));
            b
        });
        let menu = self.command_menu.map(|p| {
            let b: Box<dyn MenuOverlay<crate::actions::TermWmAction>> =
                Box::new(WmComponentMenuBridge(p));
            b
        });

        Ok(WindowManager::with_config(
            self.config, app_ctx, top, bottom, menu,
        ))
    }
}

// --- Mode-specific methods ---

impl AppBuilder<Standalone> {
    pub fn mouse_capture(mut self, enabled: bool) -> Self {
        self.config.mouse_capture_enabled = enabled;
        self
    }

    pub fn floating_windows(mut self, enabled: bool) -> Self {
        self.config.floating_windows_enabled = enabled;
        self
    }

    pub fn chrome(mut self, enabled: bool) -> Self {
        self.config.chrome_enabled = enabled;
        self
    }

    pub fn panel(mut self, enabled: bool) -> Self {
        self.config.panel_enabled = enabled;
        self
    }
}

// ---------------------------------------------------------------------------
// Bridge types: adapt WmComponent to old trait objects (temporary)
// ---------------------------------------------------------------------------

use crate::actions::TermWmAction;
use crate::ui::UiFrame;

struct WmComponentTopPanelBridge(Box<dyn WmComponent>);

impl std::fmt::Debug for WmComponentTopPanelBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl TopPanel<WindowKey> for WmComponentTopPanelBridge {
    fn begin_frame(&mut self) {
        self.0.begin_frame();
    }
    fn visible(&self) -> bool {
        self.0.visible()
    }
    fn height(&self) -> u16 {
        1
    }
    fn area(&self) -> Rect {
        Rect::default()
    }
    fn set_visible(&mut self, visible: bool) {
        self.0.set_visible(visible);
    }
    fn set_height(&mut self, _height: u16) {}
    fn split_area(&mut self, _active: bool, area: Rect) -> (Rect, Rect) {
        self.0.consume_area(area)
    }
    fn render(
        &mut self,
        _frame: &mut UiFrame<'_>,
        _active: bool,
        _focus_current: WindowKey,
        _display_order: &[WindowKey],
        _status_line: Option<&str>,
        _mouse_capture_enabled: bool,
        _clipboard_enabled: bool,
        _window_selection_enabled: bool,
        _selection_active: bool,
        _selection_dragging: bool,
        _selection_copy_available: bool,
        _selection_copied: bool,
        _menu_open: bool,
        _label_for: &dyn Fn(WindowKey) -> String,
        _theme: &crate::theme::Theme,
    ) {
        // Render is handled via the WmComponent::render path in Phase 6
    }
    fn menu_icon_rect(&self) -> Option<Rect> {
        None
    }
    fn menu_icon_contains_point(&self, _column: u16, _row: u16) -> bool {
        false
    }
    fn hit_test_mouse_capture(&self, _event: &crossterm::event::Event) -> bool {
        false
    }
    fn hit_test_selection(&self, _event: &crossterm::event::Event) -> bool {
        false
    }
    fn hit_test_clipboard(&self, _event: &crossterm::event::Event) -> bool {
        false
    }
    fn hit_test_copy(&self, _event: &crossterm::event::Event) -> bool {
        false
    }
    fn hit_test_window(&self, _event: &crossterm::event::Event) -> Option<WindowKey> {
        None
    }
    fn hit_test_menu(&self, _event: &crossterm::event::Event) -> bool {
        false
    }
}

struct WmComponentBottomPanelBridge(Box<dyn WmComponent>);

impl std::fmt::Debug for WmComponentBottomPanelBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl BottomPanel for WmComponentBottomPanelBridge {
    fn begin_frame(&mut self) {
        self.0.begin_frame();
    }
    fn area(&self) -> Rect {
        Rect::default()
    }
    fn set_keybinding_hints(&mut self, hints: Vec<(TermWmAction, Vec<String>)>) {
        self.0
            .process_action(&crate::components::ComponentAction::SetKeybindingHints(hints));
    }
    fn keybinding_hints(&self) -> &[(TermWmAction, Vec<String>)] {
        &[]
    }
    fn split_bottom_area(&mut self, area: Rect, height: u16) -> (Rect, Rect) {
        let bottom = Rect {
            x: area.x,
            y: area.y.saturating_add(area.height).saturating_sub(height),
            width: area.width,
            height,
        };
        let managed = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: area.height.saturating_sub(height),
        };
        (bottom, managed)
    }
    fn render(
        &mut self,
        _frame: &mut UiFrame<'_>,
        _active: bool,
        _theme: &crate::theme::Theme,
    ) {
        // Render is handled via the WmComponent::render path in Phase 6
    }
    fn hit_test_hint(&self, _event: &crossterm::event::Event) -> Option<TermWmAction> {
        None
    }
    fn set_power_profile(&mut self, profile: crate::power_profile::PowerProfile) {
        self.0
            .process_action(&crate::components::ComponentAction::SetPowerProfile(profile));
    }
}

struct WmComponentMenuBridge(Box<dyn WmComponent>);

impl std::fmt::Debug for WmComponentMenuBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl crate::components::Component<TermWmAction> for WmComponentMenuBridge {
    fn render(
        &self,
        _frame: &mut UiFrame<'_>,
        _area: Rect,
        _ctx: &crate::component_context::ComponentContext,
        _registry: &mut crate::hitbox_registry::HitboxRegistry,
    ) {
    }
    fn handle_events(
        &mut self,
        _event: &crossterm::event::Event,
        _ctx: &crate::component_context::ComponentContext,
    ) -> crate::actions::EventResult<TermWmAction> {
        crate::actions::EventResult::Ignored
    }
    fn update(
        &mut self,
        _action: TermWmAction,
        _ctx: &crate::component_context::ComponentContext,
        _actions: &mut std::collections::VecDeque<(crate::window::WindowKey, TermWmAction)>,
    ) {
    }
    fn destroy(&mut self) {}
}

impl crate::components::Overlay<TermWmAction> for WmComponentMenuBridge {
    fn visible(&self) -> bool {
        self.0.visible()
    }
}

impl MenuOverlay<TermWmAction> for WmComponentMenuBridge {
    fn outline(&mut self) {
        self.0
            .process_action(&crate::components::ComponentAction::Outline);
    }
    fn restore(&mut self) {
        self.0
            .process_action(&crate::components::ComponentAction::Restore);
    }
    fn set_items(&mut self, items: Vec<crate::components::MenuItem<TermWmAction>>) {
        self.0
            .process_action(&crate::components::ComponentAction::SetMenuItems(items));
    }
    fn set_timeout(&mut self, _timeout: std::time::Duration) {}
    fn selected_action(&self) -> Option<&TermWmAction> {
        None
    }
    fn set_anchor(&mut self, pos: Option<(u16, u16)>) {
        self.0
            .process_action(&crate::components::ComponentAction::SetMenuAnchor(pos));
    }
    fn set_managed_area(&mut self, area: Rect) {
        self.0
            .process_action(&crate::components::ComponentAction::SetManagedArea(area));
    }
}
