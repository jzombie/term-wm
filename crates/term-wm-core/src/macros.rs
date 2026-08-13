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
/// Four forms:
/// - `impl_view_component!(Ty)` — all-owned `&self` view. Forwards the whole
///   lifecycle (incl. `desired_height`, `hitbox_id`, selection metadata) to
///   `self.view()`. Requires `view()` to take `&self` (children are stateless or
///   cheaply cloned, sharing state via `Rc<RefCell<_>>`); a borrowed
///   `view(&mut self)` fails to compile here, which is the intended contract.
/// - `impl_view_component!(Ty, height = <expr>)` — `&mut self` view with a
///   static height. The `&self`-queryable metadata (`hitbox_id`, selection) use
///   defaults (`None` / `SelectionStatus::default()`); the mutable lifecycle
///   still forwards to `self.view()`.
/// - `impl_view_component!(Ty, child: <field>)` — `&mut self` view that borrows
///   one or more stateful child fields (comma-separated:
///   `child: terminal, log`). Forwards the mutable lifecycle to `self.view()`
///   and delegates the `&self`-queryable metadata to the child fields:
///   `desired_height` to the first field (or `height = <expr>` if given),
///   `selection_status`/`selection_text`/`hitbox_id` to the first field with an
///   active selection / non-`None` hitbox, `clear_selection` and
///   `set_selection_enabled` to every field, and `paste` to the first field
///   that consumes the payload.
/// - `impl_view_component!(Ty, height = <expr>, child: <field>, …)` — static
///   height + child-field metadata delegation (see previous form).
///
/// This is a single TT-muncher implementation: the four entry-point arms below
/// only normalize the syntax; the `@impl` body defines every lifecycle method
/// once, and small `@desired_height` / `@metadata_methods` rules parameterize
/// the two things that vary (height source and selection/hitbox source).
#[macro_export]
macro_rules! impl_view_component {
    // --- Normalized syntax entry points -------------------------------------
    ($ty:ty, height = $h:expr, child: $first:ident $(, $rest:ident)* $(,)?) => {
        $crate::impl_view_component!(@impl $ty, height = static $h, meta = child, head = $first, children = [$first $(, $rest)*]);
    };
    ($ty:ty, child: $first:ident $(, $rest:ident)* $(,)?) => {
        $crate::impl_view_component!(@impl $ty, height = child, meta = child, head = $first, children = [$first $(, $rest)*]);
    };
    ($ty:ty, height = $h:expr $(,)?) => {
        $crate::impl_view_component!(@impl $ty, height = static $h, meta = none, head = none, children = []);
    };
    ($ty:ty $(,)?) => {
        $crate::impl_view_component!(@impl $ty, height = view, meta = view, head = none, children = []);
    };

    // --- Single implementation body -----------------------------------------
    (@impl $ty:ty, height = $h_kind:tt $($h_expr:expr)?, meta = $m_kind:tt, head = $head:tt, children = [$($child:ident),*]) => {
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
                $crate::impl_view_component!(@desired_height self, width, $h_kind $($h_expr)?, head = $head)
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
            $crate::impl_view_component!(@metadata_methods $m_kind, head = $head, children = [$($child),*]);
        }
    };

    // --- Helper: desired_height source --------------------------------------
    (@desired_height $self:ident, $w:ident, view, head = none) => {
        $crate::components::Component::desired_height(&$self.view(), $w)
    };
    (@desired_height $self:ident, $w:ident, static $h:expr, head = $head:tt) => {
        $h
    };
    (@desired_height $self:ident, $w:ident, child, head = $head:ident) => {
        $crate::components::Component::desired_height(&$self.$head, $w)
    };

    // --- Helper: selection / hitbox metadata source -------------------------
    (@metadata_methods view, head = none, children = []) => {
        fn hitbox_id(&self) -> Option<$crate::hitbox_registry::HitboxId> {
            $crate::components::Component::hitbox_id(&self.view())
        }
        fn selection_status(&self) -> $crate::components::SelectionStatus {
            $crate::components::Component::selection_status(&self.view())
        }
        fn selection_text(&self) -> Option<String> {
            $crate::components::Component::selection_text(&self.view())
        }
        fn clear_selection(&mut self) {
            let mut view = self.view();
            $crate::components::Component::clear_selection(&mut view);
        }
        fn set_selection_enabled(&mut self, enabled: bool) {
            let mut view = self.view();
            $crate::components::Component::set_selection_enabled(&mut view, enabled);
        }
        fn paste(&mut self, text: &str) -> bool {
            let mut view = self.view();
            $crate::components::Component::paste(&mut view, text)
        }
    };

    (@metadata_methods none, head = none, children = []) => {
        fn hitbox_id(&self) -> Option<$crate::hitbox_registry::HitboxId> {
            None
        }
        fn selection_status(&self) -> $crate::components::SelectionStatus {
            $crate::components::SelectionStatus::default()
        }
        fn selection_text(&self) -> Option<String> {
            None
        }
        fn clear_selection(&mut self) {
            let mut view = self.view();
            $crate::components::Component::clear_selection(&mut view);
        }
        fn set_selection_enabled(&mut self, enabled: bool) {
            let mut view = self.view();
            $crate::components::Component::set_selection_enabled(&mut view, enabled);
        }
        fn paste(&mut self, text: &str) -> bool {
            let mut view = self.view();
            $crate::components::Component::paste(&mut view, text)
        }
    };

    (@metadata_methods child, head = $head:ident, children = [$($child:ident),+]) => {
        fn hitbox_id(&self) -> Option<$crate::hitbox_registry::HitboxId> {
            $(
                if let Some(hid) = $crate::components::Component::hitbox_id(&self.$child) {
                    return Some(hid);
                }
            )+
            None
        }
        fn selection_status(&self) -> $crate::components::SelectionStatus {
            $(
                let status = $crate::components::Component::selection_status(&self.$child);
                if status.active {
                    return status;
                }
            )+
            $crate::components::SelectionStatus::default()
        }
        fn selection_text(&self) -> Option<String> {
            $(
                let status = $crate::components::Component::selection_status(&self.$child);
                if status.active {
                    return $crate::components::Component::selection_text(&self.$child);
                }
            )+
            None
        }
        fn clear_selection(&mut self) {
            $(
                $crate::components::Component::clear_selection(&mut self.$child);
            )+
        }
        fn set_selection_enabled(&mut self, enabled: bool) {
            $(
                $crate::components::Component::set_selection_enabled(&mut self.$child, enabled);
            )+
        }
        fn paste(&mut self, text: &str) -> bool {
            $(
                if $crate::components::Component::paste(&mut self.$child, text) {
                    return true;
                }
            )+
            false
        }
    };
}
