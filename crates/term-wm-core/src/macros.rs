/// Generate a `Component<TermWmAction>` impl that delegates all methods
/// to each variant's inner value via `match self` dispatch.
///
/// # Example
/// ```ignore
/// impl_component_delegate!(MyEnum { VariantA, VariantB });
/// ```
#[macro_export]
macro_rules! impl_component_delegate {
    ($enum_name:ident, param: $param:ident, bound: $bound:path, variants: { $($variant:ident),* $(,)? }) => {
        impl<$param: $bound> $crate::components::Component<$crate::actions::TermWmAction>
            for $enum_name<$param>
        {
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
            fn take_alternate_screen_transition(&mut self) -> Option<bool> {
                match self { $(Self::$variant(c) => c.take_alternate_screen_transition(),)* }
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
            fn take_alternate_screen_transition(&mut self) -> Option<bool> {
                match self { $(Self::$variant(c) => c.take_alternate_screen_transition(),)* }
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
            fn set_menu_items(&mut self, items: Vec<$crate::components::MenuDisplayItem<$crate::actions::TermWmAction>>) {
                match self { $(Self::$variant(c) => c.set_menu_items(items),)* }
            }
            fn set_tab_outline(&mut self, expires_at: Option<std::time::Instant>) {
                match self { $(Self::$variant(c) => c.set_tab_outline(expires_at),)* }
            }
            fn render_area(&self) -> Option<$crate::Rect> {
                match self { $(Self::$variant(c) => c.render_area(),)* }
            }
        }
    };
}

/// Generate a `Component<TermWmAction>` impl that forwards every lifecycle
/// method to a per-frame `view()` method. Use on nameable window / content
/// structs whose content is a `view!` tree (the `*ContentView` pattern), to
/// avoid hand-writing the forwarding boilerplate.
///
/// Three forms:
/// - `impl_view_component!(Ty)` — forwards `desired_height` (and the `&self`
///   queries `hitbox_id`/`selection_status`/`selection_text`) to the view, so
///   height stays fully dynamic. Requires `view()` to take `&self` (an
///   all-owned tree — children are stateless or cheaply cloned, sharing state
///   via `Rc<RefCell<_>>`).
/// - `impl_view_component!(Ty, height = <expr>)` — reports a static height
///   expression for `&mut self` views that cannot be built from
///   `desired_height(&self)` (the `&self`-querying methods then use defaults).
/// - `impl_view_component!(Ty, child: <field>)` — for `&mut self` views that
///   borrow a single stateful child field (e.g.
///   `view! { <Box>{ &mut self.<field> }</Box> }`). Forwards the mutable
///   lifecycle to `self.view()` and delegates the `&self`-queryable metadata —
///   `desired_height`, `hitbox_id`, `selection_status`/`selection_text`,
///   `clear_selection`, `set_selection_enabled`, `paste` — to `self.<field>`.
///   `desired_height` reports the *child's* height (not the composed tree's
///   chrome); correct for full-window roots. This form is fully `$crate`-
///   qualified, so no `Component` trait import is required at the call site.
#[macro_export]
macro_rules! impl_view_component {
    ($ty:ty) => {
        impl $crate::components::Component<$crate::actions::TermWmAction> for $ty {
            fn render(
                &mut self,
                backend: &mut dyn term_wm_render::RenderBackend,
                area: $crate::Rect,
                ctx: &$crate::component_context::ComponentContext,
                registry: &mut $crate::hitbox_registry::HitboxRegistry,
            ) {
                self.view().render(backend, area, ctx, registry);
            }
            fn handle_events(
                &mut self,
                event: &$crate::events::Event,
                ctx: &$crate::component_context::ComponentContext,
            ) -> $crate::actions::EventResult<$crate::actions::TermWmAction> {
                self.view().handle_events(event, ctx)
            }
            fn update(
                &mut self,
                action: $crate::actions::TermWmAction,
                ctx: &$crate::component_context::ComponentContext,
                actions: &mut ::std::collections::VecDeque<(
                    $crate::window::WindowKey,
                    $crate::actions::TermWmAction,
                )>,
            ) {
                self.view().update(action, ctx, actions);
            }
            fn destroy(&mut self) {
                self.view().destroy();
            }
            fn desired_height(&self, width: u16) -> u16 {
                self.view().desired_height(width)
            }
            fn hitbox_id(&self) -> Option<$crate::hitbox_registry::HitboxId> {
                self.view().hitbox_id()
            }
            fn clear_selection(&mut self) {
                self.view().clear_selection();
            }
            fn selection_status(&self) -> $crate::components::SelectionStatus {
                self.view().selection_status()
            }
            fn selection_text(&self) -> Option<String> {
                self.view().selection_text()
            }
            fn take_pending_title(&mut self) -> Option<String> {
                self.view().take_pending_title()
            }
            fn take_alternate_screen_transition(&mut self) -> Option<bool> {
                self.view().take_alternate_screen_transition()
            }
            fn take_teardown_parts(
                &mut self,
            ) -> Option<(
                Box<dyn ::std::any::Any + Send + Sync>,
                ::std::thread::JoinHandle<()>,
            )> {
                self.view().take_teardown_parts()
            }
            fn set_selection_enabled(&mut self, enabled: bool) {
                self.view().set_selection_enabled(enabled);
            }
            fn paste(&mut self, text: &str) -> bool {
                self.view().paste(text)
            }
        }
    };
    ($ty:ty, height = $h:expr) => {
        impl $crate::components::Component<$crate::actions::TermWmAction> for $ty {
            fn render(
                &mut self,
                backend: &mut dyn term_wm_render::RenderBackend,
                area: $crate::Rect,
                ctx: &$crate::component_context::ComponentContext,
                registry: &mut $crate::hitbox_registry::HitboxRegistry,
            ) {
                self.view().render(backend, area, ctx, registry);
            }
            fn handle_events(
                &mut self,
                event: &$crate::events::Event,
                ctx: &$crate::component_context::ComponentContext,
            ) -> $crate::actions::EventResult<$crate::actions::TermWmAction> {
                self.view().handle_events(event, ctx)
            }
            fn update(
                &mut self,
                action: $crate::actions::TermWmAction,
                ctx: &$crate::component_context::ComponentContext,
                actions: &mut ::std::collections::VecDeque<(
                    $crate::window::WindowKey,
                    $crate::actions::TermWmAction,
                )>,
            ) {
                self.view().update(action, ctx, actions);
            }
            fn destroy(&mut self) {
                self.view().destroy();
            }
            // `view()` needs `&mut self`, so `desired_height(&self)` can't build
            // it; a static layout reports a fixed height expression.
            fn desired_height(&self, _width: u16) -> u16 {
                $h
            }
            fn clear_selection(&mut self) {
                self.view().clear_selection();
            }
            fn take_pending_title(&mut self) -> Option<String> {
                self.view().take_pending_title()
            }
            fn take_alternate_screen_transition(&mut self) -> Option<bool> {
                self.view().take_alternate_screen_transition()
            }
            fn take_teardown_parts(
                &mut self,
            ) -> Option<(
                Box<dyn ::std::any::Any + Send + Sync>,
                ::std::thread::JoinHandle<()>,
            )> {
                self.view().take_teardown_parts()
            }
            fn set_selection_enabled(&mut self, enabled: bool) {
                self.view().set_selection_enabled(enabled);
            }
            fn paste(&mut self, text: &str) -> bool {
                self.view().paste(text)
            }
        }
    };
    ($ty:ty, child: $child:ident) => {
        impl $crate::components::Component<$crate::actions::TermWmAction> for $ty {
            fn render(
                &mut self,
                backend: &mut dyn term_wm_render::RenderBackend,
                area: $crate::Rect,
                ctx: &$crate::component_context::ComponentContext,
                registry: &mut $crate::hitbox_registry::HitboxRegistry,
            ) {
                let mut view = self.view();
                $crate::components::Component::render(&mut view, backend, area, ctx, registry);
            }
            fn handle_events(
                &mut self,
                event: &$crate::events::Event,
                ctx: &$crate::component_context::ComponentContext,
            ) -> $crate::actions::EventResult<$crate::actions::TermWmAction> {
                let mut view = self.view();
                $crate::components::Component::handle_events(&mut view, event, ctx)
            }
            fn update(
                &mut self,
                action: $crate::actions::TermWmAction,
                ctx: &$crate::component_context::ComponentContext,
                actions: &mut ::std::collections::VecDeque<(
                    $crate::window::WindowKey,
                    $crate::actions::TermWmAction,
                )>,
            ) {
                let mut view = self.view();
                $crate::components::Component::update(&mut view, action, ctx, actions);
            }
            fn destroy(&mut self) {
                let mut view = self.view();
                $crate::components::Component::destroy(&mut view);
            }
            fn desired_height(&self, width: u16) -> u16 {
                $crate::components::Component::desired_height(&self.$child, width)
            }
            fn hitbox_id(&self) -> Option<$crate::hitbox_registry::HitboxId> {
                $crate::components::Component::hitbox_id(&self.$child)
            }
            fn clear_selection(&mut self) {
                $crate::components::Component::clear_selection(&mut self.$child);
            }
            fn selection_status(&self) -> $crate::components::SelectionStatus {
                $crate::components::Component::selection_status(&self.$child)
            }
            fn selection_text(&self) -> Option<String> {
                $crate::components::Component::selection_text(&self.$child)
            }
            fn take_pending_title(&mut self) -> Option<String> {
                let mut view = self.view();
                $crate::components::Component::take_pending_title(&mut view)
            }
            fn take_alternate_screen_transition(&mut self) -> Option<bool> {
                let mut view = self.view();
                $crate::components::Component::take_alternate_screen_transition(&mut view)
            }
            fn take_teardown_parts(
                &mut self,
            ) -> Option<(
                Box<dyn ::std::any::Any + Send + Sync>,
                ::std::thread::JoinHandle<()>,
            )> {
                let mut view = self.view();
                $crate::components::Component::take_teardown_parts(&mut view)
            }
            fn set_selection_enabled(&mut self, enabled: bool) {
                $crate::components::Component::set_selection_enabled(&mut self.$child, enabled);
            }
            fn paste(&mut self, text: &str) -> bool {
                $crate::components::Component::paste(&mut self.$child, text)
            }
        }
    };
    ($ty:ty, height = $h:expr, child: $child:ident) => {
        impl $crate::components::Component<$crate::actions::TermWmAction> for $ty {
            // A `&mut self` view with a static height (like `height = <expr>`)
            // that ALSO delegates the `&self`-queryable selection/hitbox metadata
            // to a stateful child field (like `child:`). Needed for e.g. a
            // terminal hosted deep inside a `view!` tree: `height = <expr>` can't
            // forward selection (it can't call `view()` from `&self`), so the WM's
            // copy-on-selection-release snapshot would never see the terminal.
            fn render(
                &mut self,
                backend: &mut dyn term_wm_render::RenderBackend,
                area: $crate::Rect,
                ctx: &$crate::component_context::ComponentContext,
                registry: &mut $crate::hitbox_registry::HitboxRegistry,
            ) {
                self.view().render(backend, area, ctx, registry);
            }
            fn handle_events(
                &mut self,
                event: &$crate::events::Event,
                ctx: &$crate::component_context::ComponentContext,
            ) -> $crate::actions::EventResult<$crate::actions::TermWmAction> {
                self.view().handle_events(event, ctx)
            }
            fn update(
                &mut self,
                action: $crate::actions::TermWmAction,
                ctx: &$crate::component_context::ComponentContext,
                actions: &mut ::std::collections::VecDeque<(
                    $crate::window::WindowKey,
                    $crate::actions::TermWmAction,
                )>,
            ) {
                self.view().update(action, ctx, actions);
            }
            fn destroy(&mut self) {
                self.view().destroy();
            }
            fn desired_height(&self, _width: u16) -> u16 {
                $h
            }
            fn hitbox_id(&self) -> Option<$crate::hitbox_registry::HitboxId> {
                $crate::components::Component::hitbox_id(&self.$child)
            }
            fn clear_selection(&mut self) {
                $crate::components::Component::clear_selection(&mut self.$child);
            }
            fn selection_status(&self) -> $crate::components::SelectionStatus {
                $crate::components::Component::selection_status(&self.$child)
            }
            fn selection_text(&self) -> Option<String> {
                $crate::components::Component::selection_text(&self.$child)
            }
            fn take_pending_title(&mut self) -> Option<String> {
                self.view().take_pending_title()
            }
            fn take_alternate_screen_transition(&mut self) -> Option<bool> {
                self.view().take_alternate_screen_transition()
            }
            fn take_teardown_parts(
                &mut self,
            ) -> Option<(
                Box<dyn ::std::any::Any + Send + Sync>,
                ::std::thread::JoinHandle<()>,
            )> {
                self.view().take_teardown_parts()
            }
            fn set_selection_enabled(&mut self, enabled: bool) {
                $crate::components::Component::set_selection_enabled(&mut self.$child, enabled);
            }
            fn paste(&mut self, text: &str) -> bool {
                $crate::components::Component::paste(&mut self.$child, text)
            }
        }
    };
}
