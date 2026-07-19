use std::collections::HashMap;

use crate::actions::{EventResult, TermWmAction};
use crate::components::{ComponentContext, WmComponent};
use crate::events::Event;
use crate::hitbox_registry::HitboxId;
use crate::hitbox_registry::HitboxRegistry;
use term_wm_layout_engine::LayoutRect;
use term_wm_render::RenderBackend;

use slotmap::DefaultKey;

/// Opaque identifier for a layer component (singleton, overlay, panel).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LayerId(pub u64);

impl Default for LayerId {
    fn default() -> Self {
        Self::new()
    }
}

impl LayerId {
    pub fn new() -> Self {
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self(NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

/// Z-plane classification for interleaved render and dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZPlane {
    /// Panels — rendered before windows (bottom of Z-stack).
    Background,
    /// Overlays, FAB, command palette — rendered after windows (top of Z-stack).
    Foreground,
}

/// Singular macro-focus authority. Replaces `FocusRing<WindowKey>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacroFocus {
    /// Window-level focus via existing FocusRing.
    FocusRing(DefaultKey),
    /// Layer-level focus — a singleton component has keyboard focus.
    Layer(LayerId),
}

/// Semantic tag for identifying layer components without hardcoded fields.
/// Used with `WindowManager::semantic_registry` for programmatic lookup.
/// Third-party plugins register via `Custom(&str)` — no core modification needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ComponentTag {
    TopPanel,
    BottomPanel,
    CommandPalette,
    FloatingActionButton,
    NotificationArea,
    /// Open-ended extension vector for third-party plugins.
    Custom(&'static str),
}

/// Unified storage for singleton UI components (panels, FAB, overlays, etc.).
///
/// Components are partitioned into `ZPlane::Background` (panels) and
/// `ZPlane::Foreground` (overlays, FAB, command palette). This guarantees
/// deterministic interleaving with the window rendering pipeline:
///
/// ```text
/// Background Layers → Windows → Foreground Layers
/// ```
///
/// Dispatch order is reversed (front-to-back):
/// ```text
/// Foreground Layers → Windows → Background Layers
/// ```
pub struct LayerManager {
    layers: HashMap<LayerId, Box<dyn WmComponent>>,
    background_order: Vec<LayerId>,
    foreground_order: Vec<LayerId>,
    /// Isolated keyboard focus for foreground layers.
    /// Prevents split-brain with `Window.active_keyboard_focus`.
    pub active_keyboard_focus: Option<HitboxId>,
}

impl LayerManager {
    pub fn new() -> Self {
        Self {
            layers: HashMap::new(),
            background_order: Vec::new(),
            foreground_order: Vec::new(),
            active_keyboard_focus: None,
        }
    }

    /// Insert a component into the specified Z-plane.
    #[allow(dead_code)]
    pub fn insert(&mut self, comp: Box<dyn WmComponent>, plane: ZPlane) -> LayerId {
        let id = LayerId::new();
        self.layers.insert(id, comp);
        match plane {
            ZPlane::Background => self.background_order.push(id),
            ZPlane::Foreground => self.foreground_order.push(id),
        }
        id
    }

    /// Remove a component by ID.
    pub fn remove(&mut self, id: LayerId) {
        self.layers.remove(&id);
        self.background_order.retain(|l| *l != id);
        self.foreground_order.retain(|l| *l != id);
    }

    /// Get an immutable reference to a layer component.
    pub fn get(&self, id: LayerId) -> Option<&dyn WmComponent> {
        self.layers.get(&id).map(|c| c.as_ref())
    }

    /// Get a mutable reference to a layer component.
    pub fn get_mut(&mut self, id: LayerId) -> Option<&mut dyn WmComponent> {
        self.layers.get_mut(&id).map(|c| c.as_mut())
    }

    /// Dispatch foreground layers (front-to-back = reverse render order).
    /// Called first — foreground has highest Z.
    pub fn dispatch_foreground(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        let mut layer_ctx = ctx.clone();
        if let Some(focus_id) = self.active_keyboard_focus {
            layer_ctx = layer_ctx.with_keyboard_focus_id(focus_id);
        }
        for id in self.foreground_order.iter().rev() {
            if let Some(comp) = self.layers.get_mut(id) {
                let result = comp.handle_events(event, &layer_ctx);
                if !result.is_ignored() {
                    return result;
                }
            }
        }
        EventResult::Ignored
    }

    /// Dispatch background layers (panels).
    pub fn dispatch_background(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        let mut layer_ctx = ctx.clone();
        if let Some(focus_id) = self.active_keyboard_focus {
            layer_ctx = layer_ctx.with_keyboard_focus_id(focus_id);
        }
        for id in self.background_order.iter().rev() {
            if let Some(comp) = self.layers.get_mut(id) {
                let result = comp.handle_events(event, &layer_ctx);
                if !result.is_ignored() {
                    return result;
                }
            }
        }
        EventResult::Ignored
    }

    /// Render background layers (panels) — back-to-front.
    pub fn render_background(
        &mut self,
        backend: &mut dyn RenderBackend,
        area: LayoutRect,
        ctx: &ComponentContext,
        registry: &mut HitboxRegistry,
    ) {
        for id in &self.background_order {
            if let Some(comp) = self.layers.get_mut(id) {
                comp.render(backend, area, ctx, registry);
            }
        }
    }

    /// Render foreground layers (overlays, command palette, FAB) — back-to-front.
    pub fn render_foreground(
        &mut self,
        backend: &mut dyn RenderBackend,
        area: LayoutRect,
        ctx: &ComponentContext,
        registry: &mut HitboxRegistry,
    ) {
        for id in &self.foreground_order {
            if let Some(comp) = self.layers.get_mut(id) {
                comp.render(backend, area, ctx, registry);
            }
        }
    }
}

impl Default for LayerManager {
    fn default() -> Self {
        Self::new()
    }
}
