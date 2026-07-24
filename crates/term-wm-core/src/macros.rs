/// Generate a `Component<TermWmAction>` impl that delegates all methods
/// to each variant's inner value via `match self` dispatch.
///
/// # Example
/// ```ignore
/// impl_component_delegate!(MyEnum { VariantA, VariantB });
/// ```
#[macro_export]
macro_rules! impl_component_delegate {
    ($enum_name:ident { $($variant:ident),* $(,)? }) => {
        impl $crate::components::Component<$crate::actions::TermWmAction> for $enum_name {
            fn init(&mut self) {
                match self { $(Self::$variant(c) => c.init(),)* }
            }
            fn on_mount(&mut self, key: $crate::window::WindowKey, app: &$crate::app_context::AppContext) {
                match self { $(Self::$variant(c) => c.on_mount(key, app),)* }
            }
            fn hitbox_id(&self) -> Option<$crate::hitbox_registry::HitboxId> {
                match self { $(Self::$variant(c) => c.hitbox_id(),)* }
            }
            fn handle_events(&mut self, event: &$crate::events::Event, ctx: &$crate::component_context::ComponentContext) -> $crate::actions::EventResult<$crate::actions::TermWmAction> {
                match self { $(Self::$variant(c) => c.handle_events(event, ctx),)* }
            }
            fn on_mouse_press(
                &mut self, col: u16, row: u16, button: $crate::events::MouseButton,
                modifiers: $crate::events::KeyModifiers, ctx: &$crate::component_context::ComponentContext,
            ) -> $crate::actions::EventResult<$crate::actions::TermWmAction> {
                match self { $(Self::$variant(c) => c.on_mouse_press(col, row, button, modifiers, ctx),)* }
            }
            fn on_mouse_release(
                &mut self, col: u16, row: u16, button: $crate::events::MouseButton,
                modifiers: $crate::events::KeyModifiers, ctx: &$crate::component_context::ComponentContext,
            ) -> $crate::actions::EventResult<$crate::actions::TermWmAction> {
                match self { $(Self::$variant(c) => c.on_mouse_release(col, row, button, modifiers, ctx),)* }
            }
            fn on_mouse_drag(
                &mut self, col: u16, row: u16, button: $crate::events::MouseButton,
                modifiers: $crate::events::KeyModifiers, ctx: &$crate::component_context::ComponentContext,
            ) -> $crate::actions::EventResult<$crate::actions::TermWmAction> {
                match self { $(Self::$variant(c) => c.on_mouse_drag(col, row, button, modifiers, ctx),)* }
            }
            fn on_mouse_scroll(
                &mut self, col: u16, row: u16, kind: $crate::events::MouseEventKind,
                modifiers: $crate::events::KeyModifiers, ctx: &$crate::component_context::ComponentContext,
            ) -> $crate::actions::EventResult<$crate::actions::TermWmAction> {
                match self { $(Self::$variant(c) => c.on_mouse_scroll(col, row, kind, modifiers, ctx),)* }
            }
            fn on_mouse_move(
                &mut self, col: u16, row: u16, modifiers: $crate::events::KeyModifiers,
                ctx: &$crate::component_context::ComponentContext,
            ) -> $crate::actions::EventResult<$crate::actions::TermWmAction> {
                match self { $(Self::$variant(c) => c.on_mouse_move(col, row, modifiers, ctx),)* }
            }
            fn on_key(&mut self, event: &$crate::events::Event, ctx: &$crate::component_context::ComponentContext) -> $crate::actions::EventResult<$crate::actions::TermWmAction> {
                match self { $(Self::$variant(c) => c.on_key(event, ctx),)* }
            }
            fn update(&mut self, action: $crate::actions::TermWmAction, ctx: &$crate::component_context::ComponentContext, actions: &mut std::collections::VecDeque<($crate::window::WindowKey, $crate::actions::TermWmAction)>) {
                match self { $(Self::$variant(c) => c.update(action, ctx, actions),)* }
            }
            fn render(&mut self, backend: &mut dyn term_wm_render::RenderBackend, area: $crate::Rect, ctx: &$crate::component_context::ComponentContext, registry: &mut $crate::hitbox_registry::HitboxRegistry) {
                match self { $(Self::$variant(c) => $crate::components::Component::<$crate::actions::TermWmAction>::render(c, backend, area, ctx, registry),)* }
            }
            fn destroy(&mut self) {
                match self { $(Self::$variant(c) => c.destroy(),)* }
            }
            fn clear_selection(&mut self) {
                match self { $(Self::$variant(c) => c.clear_selection(),)* }
            }
            fn selection_status(&self) -> $crate::components::SelectionStatus {
                match self { $(Self::$variant(c) => c.selection_status(),)* }
            }
            fn selection_text(&self) -> Option<String> {
                match self { $(Self::$variant(c) => c.selection_text(),)* }
            }
            fn desired_height(&self, width: u16) -> u16 {
                match self { $(Self::$variant(c) => c.desired_height(width),)* }
            }
            fn take_pending_title(&mut self) -> Option<String> {
                match self { $(Self::$variant(c) => c.take_pending_title(),)* }
            }
            fn take_teardown_parts(&mut self) -> Option<(Box<dyn std::any::Any + Send + Sync>, std::thread::JoinHandle<()>)> {
                match self { $(Self::$variant(c) => c.take_teardown_parts(),)* }
            }
            fn set_selection_enabled(&mut self, enabled: bool) {
                match self { $(Self::$variant(c) => c.set_selection_enabled(enabled),)* }
            }
            fn paste(&mut self, text: &str) -> bool {
                match self { $(Self::$variant(c) => c.paste(text),)* }
            }
        }
    };
}

/// Generate a `WmComponent` impl (including `Debug`) that delegates all
/// methods to each variant's inner value via `match self` dispatch.
#[macro_export]
macro_rules! impl_wm_component_delegate {
    ($enum_name:ident { $($variant:ident),* $(,)? }) => {
        impl std::fmt::Debug for $enum_name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self { $(Self::$variant(c) => std::fmt::Debug::fmt(c, f),)* }
            }
        }
        impl $crate::components::WmComponent for $enum_name {
            fn consume_area(&mut self, available: $crate::Rect) -> ($crate::Rect, $crate::Rect) {
                match self { $(Self::$variant(c) => c.consume_area(available),)* }
            }
            fn process_action(&mut self, action: &$crate::components::ComponentAction) {
                match self { $(Self::$variant(c) => c.process_action(action),)* }
            }
            fn query(&self, query: &$crate::components::ComponentQuery) -> $crate::components::ComponentResponse {
                match self { $(Self::$variant(c) => c.query(query),)* }
            }
            fn hit_test(&self, x: u16, y: u16) -> bool {
                match self { $(Self::$variant(c) => c.hit_test(x, y),)* }
            }
            fn begin_frame(&mut self) {
                match self { $(Self::$variant(c) => c.begin_frame(),)* }
            }
            fn visible(&self) -> bool {
                match self { $(Self::$variant(c) => c.visible(),)* }
            }
            fn set_visible(&mut self, visible: bool) {
                match self { $(Self::$variant(c) => c.set_visible(visible),)* }
            }
        }
    };
}

/// Generate an `Overlay<TermWmAction>` impl that delegates all methods
/// to each variant's inner value via `match self` dispatch.
#[macro_export]
macro_rules! impl_overlay_delegate {
    ($enum_name:ident { $($variant:ident),* $(,)? }) => {
        impl $crate::components::Overlay<$crate::actions::TermWmAction> for $enum_name {
            fn visible(&self) -> bool {
                match self { $(Self::$variant(c) => c.visible(),)* }
            }
            fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
                match self { $(Self::$variant(c) => c.as_any_mut(),)* }
            }
            fn shadow_rect(&self, area: $crate::Rect) -> Option<$crate::Rect> {
                match self { $(Self::$variant(c) => c.shadow_rect(area),)* }
            }
            fn handle_confirm_event(&mut self, event: &$crate::events::Event) -> Option<$crate::actions::ConfirmAction> {
                match self { $(Self::$variant(c) => c.handle_confirm_event(event),)* }
            }
            fn mark_dirty(&mut self) {
                match self { $(Self::$variant(c) => c.mark_dirty(),)* }
            }
            fn set_menu_items(&mut self, items: Vec<$crate::components::MenuItem<$crate::actions::TermWmAction>>) {
                match self { $(Self::$variant(c) => c.set_menu_items(items),)* }
            }
            fn set_tab_outline(&mut self, expires_at: Option<std::time::Instant>) {
                match self { $(Self::$variant(c) => c.set_tab_outline(expires_at),)* }
            }
        }
    };
}
