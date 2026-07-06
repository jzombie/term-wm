use std::cell::RefCell;

use ratatui::layout::Rect;

use crate::actions::TermWmAction;
use crate::components::{Component, ComponentContext};
use crate::hitbox_registry::HitboxRegistry;
use crate::ui::UiFrame;

type RenderFn = Box<dyn Fn(&mut UiFrame<'_>, Rect, &ComponentContext)>;
type StatefulRenderFn<S> = Box<dyn Fn(&mut UiFrame<'_>, Rect, &mut S, &ComponentContext)>;

/// Wraps a closure-based renderer as a term-wm Component.
/// For stateless widgets (Paragraph, Block, Gauge, etc.)
///
/// # Example
/// ```ignore
/// use term_wm::prelude::*;
/// use term_wm::WidgetAdapter;
/// use ratatui::widgets::Paragraph;
///
/// let comp = WidgetAdapter::new(|frame, area, _ctx| {
///     frame.render_widget(Paragraph::new("Hello"), area);
/// });
/// app.register(comp);
/// ```
pub struct WidgetAdapter {
    render_fn: RenderFn,
}

impl WidgetAdapter {
    pub fn new<F>(render_fn: F) -> Self
    where
        F: Fn(&mut UiFrame<'_>, Rect, &ComponentContext) + 'static,
    {
        Self {
            render_fn: Box::new(render_fn),
        }
    }
}

impl Component<TermWmAction> for WidgetAdapter {
    fn render(
        &self,
        frame: &mut UiFrame<'_>,
        area: Rect,
        ctx: &ComponentContext,
        _registry: &mut HitboxRegistry,
    ) {
        (self.render_fn)(frame, area, ctx);
    }
}

/// Wraps a Ratatui StatefulWidget + its State as a term-wm Component.
/// Uses RefCell for interior mutability so render(&self) can pass &mut State
/// to the render closure without unsafe.
///
/// # Example
/// ```ignore
/// use term_wm::prelude::*;
/// use term_wm::StatefulWidgetAdapter;
/// use ratatui::widgets::{List, ListItem};
/// use ratatui::widgets::ListState;
///
/// let mut state = ListState::default();
/// state.select(Some(0));
/// let comp = StatefulWidgetAdapter::new(state, |frame, area, state, _ctx| {
///     let items = vec![ListItem::new("Item 1"), ListItem::new("Item 2")];
///     frame.render_stateful_widget(List::new(items), area, state);
/// });
/// app.register(comp);
/// ```
pub struct StatefulWidgetAdapter<S> {
    state: RefCell<S>,
    render_fn: StatefulRenderFn<S>,
}

impl<S: 'static> StatefulWidgetAdapter<S> {
    pub fn new<F>(initial_state: S, render_fn: F) -> Self
    where
        F: Fn(&mut UiFrame<'_>, Rect, &mut S, &ComponentContext) + 'static,
    {
        Self {
            state: RefCell::new(initial_state),
            render_fn: Box::new(render_fn),
        }
    }

    /// Mutate the widget state within a strictly bounded scope.
    /// The RefMut guard is dropped when the closure returns, preventing
    /// leaked borrows across render ticks.
    pub fn modify_state<R>(&self, f: impl FnOnce(&mut S) -> R) -> R {
        let mut state = self.state.borrow_mut();
        f(&mut *state)
    }

    /// Inspect the widget state within a strictly bounded scope.
    pub fn inspect_state<R>(&self, f: impl FnOnce(&S) -> R) -> R {
        let state = self.state.borrow();
        f(&*state)
    }
}

impl<S: 'static> Component<TermWmAction> for StatefulWidgetAdapter<S> {
    fn render(
        &self,
        frame: &mut UiFrame<'_>,
        area: Rect,
        ctx: &ComponentContext,
        _registry: &mut HitboxRegistry,
    ) {
        let mut state = self.state.borrow_mut();
        (self.render_fn)(frame, area, &mut *state, ctx);
    }
}
