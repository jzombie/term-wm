use std::collections::VecDeque;

use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::components::{Component, ComponentContext};
use term_wm_core::events::Event;
use term_wm_core::window::WindowKey;
use term_wm_layout_engine::LayoutRect;

/// A grid constraint: a fixed cell size or a share of the remaining space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridConstraint {
    Fixed(u16),
    Fraction(u16),
}

/// Resolve a constraint list against a total dimension into concrete sizes.
/// `Fixed` cells consume their size; `Fraction` cells share the leftover space
/// in proportion to their weight (the last `Fraction` absorbs the remainder).
pub fn resolve_sizes(dim: u16, constraints: &[GridConstraint]) -> Vec<u16> {
    let total = u32::from(dim);
    let mut fixed_total: u32 = 0;
    let mut frac_total: u32 = 0;
    for c in constraints {
        match c {
            GridConstraint::Fixed(n) => fixed_total = fixed_total.saturating_add(u32::from(*n)),
            GridConstraint::Fraction(n) => frac_total = frac_total.saturating_add(u32::from(*n)),
        }
    }
    let remaining = total.saturating_sub(fixed_total);
    let mut allocated: u32 = 0;
    let mut sizes = Vec::with_capacity(constraints.len());
    for (i, c) in constraints.iter().enumerate() {
        let is_last = i + 1 == constraints.len();
        let s = match c {
            GridConstraint::Fixed(n) => u32::from(*n),
            GridConstraint::Fraction(n) => {
                if frac_total == 0 {
                    remaining
                } else if is_last {
                    remaining.saturating_sub(allocated)
                } else {
                    let s = remaining
                        .saturating_mul(u32::from(*n))
                        .checked_div(frac_total)
                        .unwrap_or(0);
                    allocated = allocated.saturating_add(s);
                    s
                }
            }
        };
        sizes.push(s as u16);
    }
    sizes
}

/// The minimum width a `Fraction` column is allotted before a multi-column
/// grid reflows to a single stacked column. Prevents columns from collapsing
/// into unreadable slivers on narrow containers.
pub const FRACTION_COL_MIN_WIDTH: u16 = 10;

/// Minimum total width a constraint list needs to keep multiple columns.
fn min_total_width(cols: &[GridConstraint]) -> u16 {
    cols.iter()
        .map(|c| match c {
            GridConstraint::Fixed(n) => *n,
            GridConstraint::Fraction(_) => FRACTION_COL_MIN_WIDTH,
        })
        .fold(0u16, u16::saturating_add)
}

/// Whether a multi-column grid must reflow to a single stacked column because
/// the available width cannot accommodate the configured columns' minimums.
/// Shared by `GridComponent` and its consumers (e.g. a scroll-content wrapper
/// that must report the same adaptive height).
pub fn grid_reflows(cols: &[GridConstraint], width: u16) -> bool {
    cols.len() > 1 && width < min_total_width(cols)
}

/// A row-major grid container.
///
/// Column widths and row heights come from [`GridConstraint`] lists; children
/// are placed left-to-right, top-to-bottom. If `rows` is empty, one
/// `Fraction` row per child is synthesized (a vertical stack of full-width
/// cells); if `cols` is empty, a single `Fraction` column is used.
///
/// `desired_height` returns `0` (stretch) whenever any row is `Fraction` (or
/// `rows` is empty) — otherwise a parent would allocate only the fixed rows'
/// total and leave no space for the fractional rows — else the sum of fixed
/// row heights.
pub struct GridComponent<C: Component<TermWmAction>> {
    children: Vec<C>,
    cols: Vec<GridConstraint>,
    rows: Vec<GridConstraint>,
}

impl<C: Component<TermWmAction>> GridComponent<C> {
    pub fn new(children: Vec<C>) -> Self {
        Self {
            children,
            cols: Vec::new(),
            rows: Vec::new(),
        }
    }

    pub fn with_cols(mut self, cols: Vec<GridConstraint>) -> Self {
        self.cols = cols;
        self
    }

    pub fn with_rows(mut self, rows: Vec<GridConstraint>) -> Self {
        self.rows = rows;
        self
    }

    fn effective_cols(&self) -> Vec<GridConstraint> {
        if self.cols.is_empty() {
            vec![GridConstraint::Fraction(1)]
        } else {
            self.cols.clone()
        }
    }

    fn effective_rows(&self) -> Vec<GridConstraint> {
        if !self.rows.is_empty() {
            return self.rows.clone();
        }
        let ncols = self.effective_cols().len();
        let nrows = self.children.len().div_ceil(ncols).max(1);
        vec![GridConstraint::Fraction(1); nrows]
    }

    fn reflows(&self, width: u16) -> bool {
        grid_reflows(&self.effective_cols(), width)
    }

    fn col_widths(&self, width: u16) -> Vec<u16> {
        if self.reflows(width) {
            // Reflowed to a single stacked column: full width.
            vec![width]
        } else {
            resolve_sizes(width, &self.effective_cols())
        }
    }

    fn row_heights(&self, width: u16, height: u16) -> Vec<u16> {
        if self.reflows(width) {
            // Reflowed to a single stacked column. Non-stretch children keep
            // their own desired_height; stretch children (`desired_height == 0`)
            // share the remaining vertical space.
            let fixed_total: u16 = self.children.iter().fold(0u16, |acc, c| {
                let h = c.desired_height(width);
                if h == 0 { acc } else { acc.saturating_add(h) }
            });
            let remaining = (i32::from(height))
                .saturating_sub(i32::from(fixed_total))
                .max(0) as u16;
            let n_stretch = self
                .children
                .iter()
                .filter(|c| c.desired_height(width) == 0)
                .count() as u16;
            let stretch_h = remaining.checked_div(n_stretch).unwrap_or(0);
            self.children
                .iter()
                .map(|c| {
                    let h = c.desired_height(width);
                    if h == 0 { stretch_h.max(1) } else { h.max(1) }
                })
                .collect()
        } else {
            resolve_sizes(height, &self.effective_rows())
        }
    }
}

/// Walk the grid cells row-major, invoking `f(idx, cell_rect)` for each cell.
/// Cells are placed starting at `area`'s top-left; `idx` indexes children in
/// insertion order (children beyond the grid capacity are never visited).
fn walk_cells<F: FnMut(usize, LayoutRect)>(
    area: LayoutRect,
    col_widths: &[u16],
    row_heights: &[u16],
    mut f: F,
) {
    let mut idx: usize = 0;
    let mut cell_y = area.y;
    for &rh in row_heights {
        let mut cell_x = area.x;
        for &cw in col_widths {
            f(
                idx,
                LayoutRect {
                    x: cell_x,
                    y: cell_y,
                    width: cw,
                    height: rh,
                },
            );
            idx += 1;
            cell_x = cell_x.saturating_add(i32::from(cw));
        }
        cell_y = cell_y.saturating_add(i32::from(rh));
    }
}

impl<C: Component<TermWmAction>> Default for GridComponent<C> {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl<C: Component<TermWmAction>> Component<TermWmAction> for GridComponent<C> {
    fn desired_height(&self, width: u16) -> u16 {
        if self.reflows(width) {
            // Reflowed to a stacked column: a stretching child (0) propagates
            // stretch up; otherwise the height is the sum of the children's
            // own desired_heights (matches the reflowed row layout).
            if self.children.iter().any(|c| c.desired_height(width) == 0) {
                return 0;
            }
            return self
                .children
                .iter()
                .map(|c| c.desired_height(width))
                .fold(0u16, u16::saturating_add);
        }
        let mut fixed_sum: u16 = 0;
        for row in &self.effective_rows() {
            match row {
                GridConstraint::Fraction(_) => return 0,
                GridConstraint::Fixed(h) => fixed_sum = fixed_sum.saturating_add(*h),
            }
        }
        fixed_sum
    }

    fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        ctx: &ComponentContext,
        registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let parent_screen = ctx.screen_area().unwrap_or(area);
        let dx = parent_screen.x.saturating_sub(area.x);
        let dy = parent_screen.y.saturating_sub(area.y);
        let col_widths = self.col_widths(area.width);
        let row_heights = self.row_heights(area.width, area.height);
        walk_cells(area, &col_widths, &row_heights, |idx, cell| {
            if let Some(child) = self.children.get_mut(idx) {
                let screen = LayoutRect {
                    x: cell.x.saturating_add(dx),
                    y: cell.y.saturating_add(dy),
                    width: cell.width,
                    height: cell.height,
                };
                let child_ctx = ctx.clone().with_screen_area(screen);
                child.render(backend, cell, &child_ctx, registry);
            }
        });
    }

    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        match event {
            Event::Mouse(_) => {
                let parent_screen = ctx.screen_area().unwrap_or_default();
                let Event::Mouse(mouse) = event else {
                    return EventResult::Ignored;
                };
                let m_x = i32::from(mouse.column);
                let m_y = i32::from(mouse.row);
                let mut result = EventResult::Ignored;
                let col_widths = self.col_widths(parent_screen.width);
                let row_heights = self.row_heights(parent_screen.width, parent_screen.height);
                walk_cells(parent_screen, &col_widths, &row_heights, |idx, cell| {
                    if result.is_ignored()
                        && m_x >= cell.x
                        && m_x < cell.x.saturating_add(i32::from(cell.width))
                        && m_y >= cell.y
                        && m_y < cell.y.saturating_add(i32::from(cell.height))
                        && let Some(child) = self.children.get_mut(idx)
                    {
                        let child_ctx = ctx.clone().with_screen_area(cell);
                        result = child.handle_events(event, &child_ctx);
                    }
                });
                result
            }
            Event::Key(_) => crate::helpers::route_key_to_focused(&mut self.children, event, ctx),
            _ => crate::helpers::route_broadcast(&mut self.children, event, ctx),
        }
    }

    fn update(
        &mut self,
        action: TermWmAction,
        ctx: &ComponentContext,
        actions: &mut VecDeque<(WindowKey, TermWmAction)>,
    ) {
        for child in &mut self.children {
            child.update(action.clone(), ctx, actions);
        }
    }

    fn destroy(&mut self) {
        for child in &mut self.children {
            child.destroy();
        }
    }
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;
    use term_wm_core::events::{
        KeyCode, KeyEvent, KeyKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use term_wm_core::hitbox_registry::HitboxId;

    fn rect(x: i32, y: i32, w: u16, h: u16) -> LayoutRect {
        LayoutRect {
            x,
            y,
            width: w,
            height: h,
        }
    }

    fn make_backend() -> term_wm_console::RatatuiBackend {
        let buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 40, 20));
        term_wm_console::RatatuiBackend::new_simple(
            buffer,
            ratatui::layout::Rect::new(0, 0, 40, 20),
        )
    }

    fn mouse_event(col: u16, row: u16) -> Event {
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            modifiers: KeyModifiers::NONE,
            column: col,
            row,
        })
    }

    fn key_event() -> Event {
        Event::Key(KeyEvent::new(
            KeyCode::Char('x'),
            KeyModifiers::NONE,
            KeyKind::Press,
        ))
    }

    #[derive(Default)]
    struct SpyChild {
        height: u16,
        hitbox: Option<HitboxId>,
        seen_render: Option<LayoutRect>,
        key_count: u32,
    }

    impl Component<TermWmAction> for SpyChild {
        fn desired_height(&self, _width: u16) -> u16 {
            self.height
        }

        fn hitbox_id(&self) -> Option<HitboxId> {
            self.hitbox
        }

        fn render(
            &mut self,
            _b: &mut dyn term_wm_render::RenderBackend,
            _a: LayoutRect,
            ctx: &ComponentContext,
            _r: &mut term_wm_core::hitbox_registry::HitboxRegistry,
        ) {
            self.seen_render = ctx.screen_area();
        }

        fn handle_events(
            &mut self,
            event: &Event,
            _ctx: &ComponentContext,
        ) -> EventResult<TermWmAction> {
            if matches!(event, Event::Key(_)) {
                self.key_count += 1;
            }
            EventResult::Ignored
        }
    }

    #[test]
    fn resolve_sizes_fixed_and_fraction() {
        assert_eq!(
            resolve_sizes(
                100,
                &[
                    GridConstraint::Fixed(20),
                    GridConstraint::Fraction(1),
                    GridConstraint::Fraction(1),
                ]
            ),
            vec![20, 40, 40]
        );
    }

    #[test]
    fn resolve_sizes_last_fraction_absorbs_remainder() {
        assert_eq!(
            resolve_sizes(
                100,
                &[GridConstraint::Fraction(1), GridConstraint::Fraction(2)]
            ),
            vec![33, 67]
        );
    }

    #[test]
    fn resolve_sizes_all_fixed() {
        assert_eq!(
            resolve_sizes(100, &[GridConstraint::Fixed(30), GridConstraint::Fixed(40)]),
            vec![30, 40]
        );
    }

    #[test]
    fn resolve_sizes_fixed_exceeds_dim() {
        assert_eq!(
            resolve_sizes(
                50,
                &[GridConstraint::Fixed(100), GridConstraint::Fraction(1)]
            ),
            vec![100, 0]
        );
    }

    #[test]
    fn grid_desired_height_fraction_row_stretches() {
        // Stretch regression: a Fraction row must make the grid report 0 so a
        // parent allocates all remaining space (not just the fixed rows' sum).
        let g = GridComponent::<SpyChild>::new(Vec::new())
            .with_cols(vec![GridConstraint::Fraction(1)])
            .with_rows(vec![GridConstraint::Fixed(2), GridConstraint::Fraction(1)]);
        assert_eq!(g.desired_height(40), 0);
    }

    #[test]
    fn grid_desired_height_empty_rows_stretches() {
        let g =
            GridComponent::<SpyChild>::new(Vec::new()).with_cols(vec![GridConstraint::Fraction(1)]);
        assert_eq!(g.desired_height(40), 0);
    }

    #[test]
    fn grid_desired_height_all_fixed_rows_sums() {
        let g = GridComponent::<SpyChild>::new(Vec::new())
            .with_cols(vec![GridConstraint::Fraction(1)])
            .with_rows(vec![GridConstraint::Fixed(3), GridConstraint::Fixed(2)]);
        assert_eq!(g.desired_height(40), 5);
    }

    #[test]
    fn grid_render_places_children_row_major_with_rebound_ctx() {
        let mut g = GridComponent::new(vec![
            SpyChild::default(),
            SpyChild::default(),
            SpyChild::default(),
            SpyChild::default(),
        ])
        .with_cols(vec![
            GridConstraint::Fraction(1),
            GridConstraint::Fraction(1),
        ])
        .with_rows(vec![
            GridConstraint::Fraction(1),
            GridConstraint::Fraction(1),
        ]);
        let mut backend = make_backend();
        let ctx = ComponentContext::new(true).with_screen_area(rect(0, 0, 40, 20));
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        g.render(&mut backend, rect(0, 0, 40, 20), &ctx, &mut registry);
        assert_eq!(g.children[0].seen_render, Some(rect(0, 0, 20, 10)));
        assert_eq!(g.children[1].seen_render, Some(rect(20, 0, 20, 10)));
        assert_eq!(g.children[2].seen_render, Some(rect(0, 10, 20, 10)));
        assert_eq!(g.children[3].seen_render, Some(rect(20, 10, 20, 10)));
    }

    #[test]
    fn grid_handle_mouse_routes_to_cell() {
        let mut g = GridComponent::new(vec![SpyChild::default(), SpyChild::default()])
            .with_cols(vec![
                GridConstraint::Fraction(1),
                GridConstraint::Fraction(1),
            ])
            .with_rows(vec![GridConstraint::Fraction(1)]);
        let ctx = ComponentContext::new(true).with_screen_area(rect(0, 0, 40, 20));
        // Col 25 -> second cell.
        let result = g.handle_events(&mouse_event(25, 5), &ctx);
        assert!(result.is_ignored());
    }

    #[test]
    fn grid_keys_route_to_focused_child_only() {
        let mut g = GridComponent::new(vec![
            SpyChild {
                hitbox: Some(HitboxId::new()),
                ..Default::default()
            },
            SpyChild {
                hitbox: Some(HitboxId::new()),
                ..Default::default()
            },
        ])
        .with_cols(vec![
            GridConstraint::Fraction(1),
            GridConstraint::Fraction(1),
        ])
        .with_rows(vec![GridConstraint::Fraction(1)]);
        let focus = g.children[1].hitbox.unwrap();
        let ctx = ComponentContext::new(true).with_keyboard_focus_id(focus);
        g.handle_events(&key_event(), &ctx);
        assert_eq!(g.children[0].key_count, 0);
        assert_eq!(g.children[1].key_count, 1);
    }

    #[test]
    fn grid_reflows_truth_table() {
        // Single column never reflows.
        assert!(!grid_reflows(&[GridConstraint::Fraction(1)], 5));
        // Fixed + Fraction needs 14 + 10 = 24 columns.
        let two = [GridConstraint::Fixed(14), GridConstraint::Fraction(1)];
        assert!(grid_reflows(&two, 23));
        assert!(!grid_reflows(&two, 24));
        // All-Fixed columns are exact.
        let fixed = [GridConstraint::Fixed(3), GridConstraint::Fixed(5)];
        assert!(grid_reflows(&fixed, 7));
        assert!(!grid_reflows(&fixed, 8));
    }

    #[test]
    fn reflowed_grid_reports_sum_of_child_heights() {
        let g = GridComponent::new(vec![
            SpyChild {
                height: 1,
                ..Default::default()
            },
            SpyChild {
                height: 3,
                ..Default::default()
            },
            SpyChild {
                height: 1,
                ..Default::default()
            },
        ])
        .with_cols(vec![GridConstraint::Fixed(14), GridConstraint::Fraction(1)])
        .with_rows(vec![GridConstraint::Fixed(3), GridConstraint::Fixed(3)]);
        // Narrow (below 24) => reflowed stack: 1 + 3 + 1 = 5.
        assert_eq!(g.desired_height(20), 5);
        // Wide (>= 24) => non-reflowed rows "3 3" = 6.
        assert_eq!(g.desired_height(30), 6);
    }

    #[test]
    fn reflowed_grid_places_one_child_per_full_width_row() {
        let mut g = GridComponent::new(vec![
            SpyChild {
                height: 1,
                ..Default::default()
            },
            SpyChild {
                height: 3,
                ..Default::default()
            },
        ])
        .with_cols(vec![GridConstraint::Fixed(14), GridConstraint::Fraction(1)])
        .with_rows(vec![GridConstraint::Fixed(3), GridConstraint::Fixed(3)]);
        let mut backend = make_backend();
        let ctx = ComponentContext::new(true).with_screen_area(rect(0, 0, 20, 10));
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        g.render(&mut backend, rect(0, 0, 20, 10), &ctx, &mut registry);
        // Reflowed: full-width rows at y=0 and y=1.
        assert_eq!(g.children[0].seen_render, Some(rect(0, 0, 20, 1)));
        assert_eq!(g.children[1].seen_render, Some(rect(0, 1, 20, 3)));
    }

    #[test]
    fn reflowed_grid_stretch_child_propagates_stretch() {
        let g = GridComponent::new(vec![
            SpyChild {
                height: 1,
                ..Default::default()
            },
            SpyChild {
                height: 0,
                ..Default::default()
            }, // stretch
        ])
        .with_cols(vec![GridConstraint::Fixed(14), GridConstraint::Fraction(1)]);
        // A stretching child forces the reflowed grid to stretch.
        assert_eq!(g.desired_height(20), 0);
    }

    #[test]
    fn reflowed_grid_gives_stretch_child_remaining_height() {
        let mut g = GridComponent::new(vec![
            SpyChild {
                height: 1,
                ..Default::default()
            },
            SpyChild {
                height: 0,
                ..Default::default()
            }, // stretch
        ])
        .with_cols(vec![GridConstraint::Fixed(14), GridConstraint::Fraction(1)]);
        let mut backend = make_backend();
        let ctx = ComponentContext::new(true).with_screen_area(rect(0, 0, 20, 9));
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        g.render(&mut backend, rect(0, 0, 20, 9), &ctx, &mut registry);
        // Fixed child (1) first, stretch child gets the remaining 8.
        assert_eq!(g.children[0].seen_render, Some(rect(0, 0, 20, 1)));
        assert_eq!(g.children[1].seen_render, Some(rect(0, 1, 20, 8)));
    }

    #[test]
    fn render_geometry_stays_within_local_area() {
        // Containment: a 30-col grid hosted where the (un-rebound) screen area
        // claims 120 cols must still render every cell inside the 30-col area.
        let mut g = GridComponent::new(vec![
            SpyChild {
                height: 1,
                ..Default::default()
            },
            SpyChild {
                height: 3,
                ..Default::default()
            },
        ])
        .with_cols(vec![GridConstraint::Fixed(14), GridConstraint::Fraction(1)]);
        let mut backend = term_wm_console::RatatuiBackend::new_simple(
            ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 30, 10)),
            ratatui::layout::Rect::new(0, 0, 30, 10),
        );
        let ctx = ComponentContext::new(true).with_screen_area(rect(0, 0, 120, 10));
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        g.render(&mut backend, rect(0, 0, 30, 10), &ctx, &mut registry);
        for c in &g.children {
            let r = c.seen_render.expect("child rendered");
            assert!(
                r.x >= 0 && r.x + i32::from(r.width) <= 30,
                "cell {r:?} must stay within the 30-col local area"
            );
        }
    }
}
