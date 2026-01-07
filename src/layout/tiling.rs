use ratatui::Frame;
use ratatui::prelude::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};

use super::{FloatingPane, RegionMap, gap_size, rect_contains};

#[derive(Debug, Clone)]
pub enum LayoutNode<Id: Copy + Eq + Ord> {
    Leaf(Id),
    Split {
        direction: Direction,
        children: Vec<LayoutNode<Id>>,
        weights: Vec<f32>,
        constraints: Vec<Constraint>,
        resizable: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertPosition {
    Left,
    Right,
    Top,
    Bottom,
}

impl<Id: Copy + Eq + Ord> LayoutNode<Id> {
    pub fn leaf(id: Id) -> Self {
        Self::Leaf(id)
    }

    pub fn split(
        direction: Direction,
        constraints: Vec<Constraint>,
        children: Vec<LayoutNode<Id>>,
    ) -> Self {
        Self::Split {
            direction,
            children,
            weights: Vec::new(),
            constraints,
            resizable: true,
        }
    }

    pub fn split_resizable(
        direction: Direction,
        constraints: Vec<Constraint>,
        children: Vec<LayoutNode<Id>>,
        resizable: bool,
    ) -> Self {
        Self::Split {
            direction,
            children,
            weights: Vec::new(),
            constraints,
            resizable,
        }
    }

    pub fn unwrap_leaf(&self) -> Option<Id> {
        match self {
            LayoutNode::Leaf(id) => Some(*id),
            _ => None,
        }
    }

    pub fn layout(&self, area: Rect) -> Vec<(Id, Rect)> {
        let (regions, _) = self.layout_with_handles(area);
        regions
    }

    pub fn layout_with_handles(&self, area: Rect) -> (Vec<(Id, Rect)>, Vec<SplitHandle>) {
        let mut regions = Vec::new();
        let mut handles = Vec::new();
        self.layout_recursive(area, &mut regions, &mut handles, &mut Vec::new());
        (regions, handles)
    }

    pub fn node_at_path(&self, path: &[usize]) -> Option<&LayoutNode<Id>> {
        let mut current = self;
        for &idx in path {
            let LayoutNode::Split { children, .. } = current else {
                return None;
            };
            current = children.get(idx)?;
        }
        Some(current)
    }

    pub fn subtree_any<F>(&self, mut predicate: F) -> bool
    where
        F: FnMut(Id) -> bool,
    {
        fn walk<Id: Copy + Eq + Ord, F: FnMut(Id) -> bool>(
            node: &LayoutNode<Id>,
            predicate: &mut F,
        ) -> bool {
            match node {
                LayoutNode::Leaf(id) => predicate(*id),
                LayoutNode::Split { children, .. } => {
                    children.iter().any(|child| walk(child, predicate))
                }
            }
        }

        walk(self, &mut predicate)
    }

    pub fn hit_test_handle(&self, area: Rect, column: u16, row: u16) -> Option<SplitHandle> {
        let (_, handles) = self.layout_with_handles(area);
        handles
            .into_iter()
            .find(|handle| rect_contains(handle.rect, column, row))
    }

    pub fn apply_drag(
        &mut self,
        area: Rect,
        path: &[usize],
        index: usize,
        direction: Direction,
        delta: i16,
    ) -> bool {
        let Some(split_area) = split_area_for_path(self, area, path) else {
            return false;
        };
        let Some(split) = split_at_path_mut(self, path) else {
            return false;
        };
        let LayoutNode::Split {
            weights,
            children,
            constraints,
            resizable,
            ..
        } = split
        else {
            return false;
        };
        if !*resizable || children.len() < 2 || index + 1 >= children.len() {
            return false;
        }
        let sizes = split_sizes(
            split_area,
            direction,
            weights,
            constraints,
            children.len(),
            *resizable,
        );
        if sizes.is_empty() {
            return false;
        }
        let mut sizes = sizes.into_iter().map(|v| v as i16).collect::<Vec<_>>();
        let min_size: i16 = 4;
        let total_pair = sizes[index] + sizes[index + 1];
        let mut left = sizes[index] + delta;
        let min_left = min_size;
        let max_left = (total_pair - min_size).max(min_size);
        left = left.clamp(min_left, max_left);
        let right = total_pair - left;
        sizes[index] = left;
        sizes[index + 1] = right;
        *weights = sizes.iter().map(|v| (*v).max(1) as f32).collect();
        true
    }

    pub fn remove_leaf(&mut self, id: Id) -> bool {
        match self {
            LayoutNode::Leaf(_) => false,
            LayoutNode::Split {
                children,
                weights,
                constraints,
                ..
            } => {
                let mut removed = false;
                let mut index = 0;
                while index < children.len() {
                    let is_target = match &children[index] {
                        LayoutNode::Leaf(i) => *i == id,
                        _ => false,
                    };

                    if is_target {
                        children.remove(index);
                        if index < weights.len() {
                            weights.remove(index);
                        }
                        if index < constraints.len() {
                            constraints.remove(index);
                        }
                        removed = true;
                        break;
                    }

                    if children[index].remove_leaf(id) {
                        removed = true;
                        // If the child split created an empty split, remove it
                        let is_empty_split = match &children[index] {
                            LayoutNode::Split { children: s, .. } => s.is_empty(),
                            _ => false,
                        };
                        if is_empty_split {
                            children.remove(index);
                            if index < weights.len() {
                                weights.remove(index);
                            }
                            if index < constraints.len() {
                                constraints.remove(index);
                            }
                        }
                        break;
                    }

                    index += 1;
                }
                if removed && children.len() == 1 {
                    let only = children.remove(0);
                    *self = only;
                }
                removed
            }
        }
    }

    pub fn insert_leaf(&mut self, target: Id, insert: Id, position: InsertPosition) -> bool {
        match self {
            LayoutNode::Leaf(current) => {
                if *current != target {
                    return false;
                }
                let (direction, children) = match position {
                    InsertPosition::Left => (
                        Direction::Horizontal,
                        vec![LayoutNode::leaf(insert), LayoutNode::leaf(*current)],
                    ),
                    InsertPosition::Right => (
                        Direction::Horizontal,
                        vec![LayoutNode::leaf(*current), LayoutNode::leaf(insert)],
                    ),
                    InsertPosition::Top => (
                        Direction::Vertical,
                        vec![LayoutNode::leaf(insert), LayoutNode::leaf(*current)],
                    ),
                    InsertPosition::Bottom => (
                        Direction::Vertical,
                        vec![LayoutNode::leaf(*current), LayoutNode::leaf(insert)],
                    ),
                };
                *self = LayoutNode::Split {
                    direction,
                    children,
                    weights: vec![1.0, 1.0],
                    constraints: Vec::new(),
                    resizable: true,
                };
                true
            }
            LayoutNode::Split { children, .. } => {
                for child in children.iter_mut() {
                    if child.insert_leaf(target, insert, position) {
                        return true;
                    }
                }
                false
            }
        }
    }

    fn layout_recursive(
        &self,
        area: Rect,
        regions: &mut Vec<(Id, Rect)>,
        handles: &mut Vec<SplitHandle>,
        path: &mut Vec<usize>,
    ) {
        match self {
            LayoutNode::Leaf(id) => {
                regions.push((*id, area));
            }
            LayoutNode::Split {
                direction,
                children,
                weights,
                constraints,
                resizable,
            } => {
                let (rects, gaps) = split_rects_with_gaps(
                    *direction,
                    area,
                    weights,
                    constraints,
                    children.len(),
                    *resizable,
                );
                for (idx, (child, rect)) in children.iter().zip(rects.iter().copied()).enumerate() {
                    path.push(idx);
                    child.layout_recursive(rect, regions, handles, path);
                    path.pop();
                }
                if *resizable && children.len() > 1 {
                    for (index, handle_rect) in gaps.into_iter().enumerate() {
                        handles.push(SplitHandle {
                            rect: handle_rect,
                            path: path.clone(),
                            index,
                            direction: *direction,
                        });
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct SplitHandle {
    pub rect: Rect,
    pub path: Vec<usize>,
    pub index: usize,
    pub direction: Direction,
}

#[derive(Debug)]
pub struct DragState {
    pub path: Vec<usize>,
    pub index: usize,
    pub direction: Direction,
    pub last_col: u16,
    pub last_row: u16,
}

#[derive(Debug)]
pub struct TilingLayout<Id: Copy + Eq + Ord> {
    root: LayoutNode<Id>,
    drag: Option<DragState>,
    hover: Option<(u16, u16)>,
}

impl<Id: Copy + Eq + Ord> TilingLayout<Id> {
    pub fn new(root: LayoutNode<Id>) -> Self {
        Self {
            root,
            drag: None,
            hover: None,
        }
    }

    pub fn root(&self) -> &LayoutNode<Id> {
        &self.root
    }

    pub fn root_mut(&mut self) -> &mut LayoutNode<Id> {
        &mut self.root
    }

    pub fn split_root(&mut self, insert: Id, position: InsertPosition) {
        let (direction, children) = match position {
            InsertPosition::Left => (
                Direction::Horizontal,
                vec![LayoutNode::leaf(insert), self.root.clone()],
            ),
            InsertPosition::Right => (
                Direction::Horizontal,
                vec![self.root.clone(), LayoutNode::leaf(insert)],
            ),
            InsertPosition::Top => (
                Direction::Vertical,
                vec![LayoutNode::leaf(insert), self.root.clone()],
            ),
            InsertPosition::Bottom => (
                Direction::Vertical,
                vec![self.root.clone(), LayoutNode::leaf(insert)],
            ),
        };
        self.root = LayoutNode::Split {
            direction,
            children,
            weights: vec![1.0, 1.0],
            constraints: Vec::new(),
            resizable: true,
        };
    }

    pub fn regions(&self, area: Rect) -> Vec<(Id, Rect)> {
        self.root.layout(area)
    }

    pub fn handles(&self, area: Rect) -> Vec<SplitHandle> {
        let (_, handles) = self.root.layout_with_handles(area);
        handles
    }

    pub fn hovered_handle(&self, area: Rect) -> Option<SplitHandle> {
        let (column, row) = self.hover?;
        self.root.hit_test_handle(area, column, row)
    }

    pub fn render_handles(&self, frame: &mut Frame, area: Rect) {
        let handles = self.handles(area);
        let hovered = self.hovered_handle(area);
        render_handles(frame, &handles, hovered.as_ref());
    }

    pub fn handle_event(&mut self, event: &crossterm::event::Event, area: Rect) -> bool {
        use crossterm::event::MouseEventKind;
        let crossterm::event::Event::Mouse(mouse) = event else {
            return false;
        };
        self.hover = Some((mouse.column, mouse.row));
        match mouse.kind {
            MouseEventKind::Down(_) => {
                if let Some(handle) = self.root.hit_test_handle(area, mouse.column, mouse.row) {
                    self.drag = Some(DragState {
                        path: handle.path,
                        index: handle.index,
                        direction: handle.direction,
                        last_col: mouse.column,
                        last_row: mouse.row,
                    });
                    return true;
                }
            }
            MouseEventKind::Drag(_) => {
                if let Some(state) = self.drag.as_mut() {
                    let delta = match state.direction {
                        Direction::Horizontal => mouse.column as i16 - state.last_col as i16,
                        Direction::Vertical => mouse.row as i16 - state.last_row as i16,
                    };
                    state.last_col = mouse.column;
                    state.last_row = mouse.row;
                    return self.root.apply_drag(
                        area,
                        &state.path,
                        state.index,
                        state.direction,
                        delta,
                    );
                }
            }
            MouseEventKind::Moved => {}
            MouseEventKind::Up(_) => {
                if self.drag.is_some() {
                    self.drag = None;
                    return true;
                }
            }
            _ => {}
        }
        false
    }
}

#[derive(Debug, Clone)]
pub struct LayoutPlan<Id: Copy + Eq + Ord> {
    pub root: LayoutNode<Id>,
    pub floating: Vec<FloatingPane<Id>>,
}

impl<Id: Copy + Eq + Ord> LayoutPlan<Id> {
    pub fn new(root: LayoutNode<Id>) -> Self {
        Self {
            root,
            floating: Vec::new(),
        }
    }

    pub fn regions(&self, area: Rect) -> RegionMap<Id> {
        let mut regions = RegionMap::default();
        for (id, rect) in self.root.layout(area) {
            regions.set(id, rect);
        }
        for floating in &self.floating {
            regions.set(floating.id, floating.rect.resolve(area));
        }
        regions
    }
}

fn split_rects(
    direction: Direction,
    area: Rect,
    weights: &[f32],
    constraints: &[Constraint],
    child_count: usize,
) -> Vec<Rect> {
    if weights.len() == child_count && weights.iter().any(|value| *value > 0.0) {
        return split_rects_weighted(direction, area, weights, child_count);
    }
    if constraints.len() == child_count {
        return Layout::default()
            .direction(direction)
            .constraints(constraints.to_vec())
            .split(area)
            .to_vec();
    }
    split_rects_weighted(direction, area, weights, child_count)
}

fn split_rects_with_gaps(
    direction: Direction,
    area: Rect,
    weights: &[f32],
    constraints: &[Constraint],
    child_count: usize,
    resizable: bool,
) -> (Vec<Rect>, Vec<Rect>) {
    let gap = gap_size(direction, area, child_count, resizable);
    if gap == 0 || child_count < 2 {
        return (
            split_rects(direction, area, weights, constraints, child_count),
            Vec::new(),
        );
    }
    let gap_total = gap.saturating_mul(child_count.saturating_sub(1) as u16);
    let mut shrunk = area;
    match direction {
        Direction::Horizontal => {
            shrunk.width = area.width.saturating_sub(gap_total);
        }
        Direction::Vertical => {
            shrunk.height = area.height.saturating_sub(gap_total);
        }
    }
    let raw = split_rects(direction, shrunk, weights, constraints, child_count);
    let mut rects = Vec::with_capacity(raw.len());
    for (idx, rect) in raw.into_iter().enumerate() {
        let offset = gap.saturating_mul(idx as u16);
        let shifted = match direction {
            Direction::Horizontal => Rect {
                x: rect.x.saturating_add(offset),
                ..rect
            },
            Direction::Vertical => Rect {
                y: rect.y.saturating_add(offset),
                ..rect
            },
        };
        rects.push(shifted);
    }
    let mut gaps = Vec::with_capacity(child_count.saturating_sub(1));
    for rect in rects.iter().take(child_count.saturating_sub(1)) {
        let rect = match direction {
            Direction::Horizontal => Rect {
                x: rect.x.saturating_add(rect.width),
                y: area.y,
                width: gap,
                height: area.height,
            },
            Direction::Vertical => Rect {
                x: area.x,
                y: rect.y.saturating_add(rect.height),
                width: area.width,
                height: gap,
            },
        };
        gaps.push(rect);
    }
    (rects, gaps)
}

fn split_rects_weighted(
    direction: Direction,
    area: Rect,
    weights: &[f32],
    child_count: usize,
) -> Vec<Rect> {
    let count = child_count.max(1);
    let weights = if weights.len() == child_count {
        weights.to_vec()
    } else {
        vec![1.0; child_count]
    };
    let total_weight: f32 = weights.iter().sum::<f32>().max(1.0);
    let total = match direction {
        Direction::Horizontal => area.width,
        Direction::Vertical => area.height,
    };
    let mut sizes = Vec::with_capacity(count);
    let mut used: u16 = 0;
    for (idx, weight) in weights.iter().enumerate() {
        let size = if idx + 1 == count {
            total.saturating_sub(used)
        } else {
            let portion = ((*weight / total_weight) * total as f32).floor() as u16;
            used = used.saturating_add(portion);
            portion
        };
        sizes.push(size);
    }
    build_rects_from_sizes(direction, area, &sizes)
}

fn split_sizes(
    area: Rect,
    direction: Direction,
    weights: &[f32],
    constraints: &[Constraint],
    child_count: usize,
    resizable: bool,
) -> Vec<u16> {
    let (rects, _) = split_rects_with_gaps(
        direction,
        area,
        weights,
        constraints,
        child_count,
        resizable,
    );
    rects
        .iter()
        .map(|rect| match direction {
            Direction::Horizontal => rect.width,
            Direction::Vertical => rect.height,
        })
        .collect()
}

fn build_rects_from_sizes(direction: Direction, area: Rect, sizes: &[u16]) -> Vec<Rect> {
    let mut rects = Vec::with_capacity(sizes.len());
    let mut cursor_x = area.x;
    let mut cursor_y = area.y;
    for size in sizes {
        let rect = match direction {
            Direction::Horizontal => {
                let rect = Rect {
                    x: cursor_x,
                    y: area.y,
                    width: *size,
                    height: area.height,
                };
                cursor_x = cursor_x.saturating_add(*size);
                rect
            }
            Direction::Vertical => {
                let rect = Rect {
                    x: area.x,
                    y: cursor_y,
                    width: area.width,
                    height: *size,
                };
                cursor_y = cursor_y.saturating_add(*size);
                rect
            }
        };
        rects.push(rect);
    }
    rects
}

fn split_area_for_path<Id: Copy + Eq + Ord>(
    node: &LayoutNode<Id>,
    area: Rect,
    path: &[usize],
) -> Option<Rect> {
    let mut area = area;
    let mut current = node;
    for &idx in path {
        let LayoutNode::Split {
            direction,
            children,
            weights,
            constraints,
            resizable,
            ..
        } = current
        else {
            return None;
        };
        let (rects, _) = split_rects_with_gaps(
            *direction,
            area,
            weights,
            constraints,
            children.len(),
            *resizable,
        );
        area = *rects.get(idx)?;
        current = children.get(idx)?;
    }
    Some(area)
}

fn split_at_path_mut<'a, Id: Copy + Eq + Ord>(
    node: &'a mut LayoutNode<Id>,
    path: &[usize],
) -> Option<&'a mut LayoutNode<Id>> {
    let mut current = node;
    for &idx in path {
        let LayoutNode::Split { children, .. } = current else {
            return None;
        };
        current = children.get_mut(idx)?;
    }
    Some(current)
}

pub fn render_handles(frame: &mut Frame, handles: &[SplitHandle], hovered: Option<&SplitHandle>) {
    render_handles_masked(frame, handles, hovered, |_, _| false);
}

pub fn render_handles_masked<F>(
    frame: &mut Frame,
    handles: &[SplitHandle],
    hovered: Option<&SplitHandle>,
    is_obscured: F,
) where
    F: Fn(u16, u16) -> bool,
{
    let buffer = frame.buffer_mut();
    let hover_rect = hovered.map(|handle| handle.rect);
    for handle in handles {
        if handle.rect.width == 0 || handle.rect.height == 0 {
            continue;
        }
        let is_hovered = hover_rect == Some(handle.rect);
        let style = if is_hovered {
            Style::default()
                .fg(crate::theme::menu_selected_bg())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(crate::theme::menu_bg())
                .add_modifier(Modifier::DIM)
        };
        let clip = handle.rect.intersection(buffer.area);
        if clip.width > 0 && clip.height > 0 {
            for y in clip.y..clip.y.saturating_add(clip.height) {
                for x in clip.x..clip.x.saturating_add(clip.width) {
                    if is_obscured(x, y) {
                        continue;
                    }
                    if let Some(cell) = buffer.cell_mut((x, y)) {
                        cell.reset();
                        cell.set_symbol("·");
                        cell.set_style(style);
                    }
                }
            }
        }
        match handle.direction {
            Direction::Horizontal => {
                let x = handle.rect.x + handle.rect.width / 2;
                let y_center = handle.rect.y + handle.rect.height / 2;
                for offset in 0..3 {
                    let y = y_center.saturating_sub(1).saturating_add(offset);
                    if y < handle.rect.y || y >= handle.rect.y.saturating_add(handle.rect.height) {
                        continue;
                    }
                    if is_obscured(x, y) {
                        continue;
                    }
                    if let Some(cell) = buffer.cell_mut((x, y)) {
                        cell.set_symbol(if is_hovered { "O" } else { "o" });
                        cell.set_style(style);
                    }
                }
            }
            Direction::Vertical => {
                let y = handle.rect.y + handle.rect.height / 2;
                let x_center = handle.rect.x + handle.rect.width / 2;
                for offset in 0..3 {
                    let x = x_center.saturating_sub(1).saturating_add(offset);
                    if x < handle.rect.x || x >= handle.rect.x.saturating_add(handle.rect.width) {
                        continue;
                    }
                    if is_obscured(x, y) {
                        continue;
                    }
                    if let Some(cell) = buffer.cell_mut((x, y)) {
                        cell.set_symbol(if is_hovered { "O" } else { "o" });
                        cell.set_style(style);
                    }
                }
            }
        }
        if is_hovered {
            let border_style = Style::default()
                .fg(crate::theme::accent_alt())
                .add_modifier(Modifier::BOLD);
            let max_x = handle
                .rect
                .x
                .saturating_add(handle.rect.width.saturating_sub(1));
            let max_y = handle
                .rect
                .y
                .saturating_add(handle.rect.height.saturating_sub(1));
            for x in handle.rect.x..=max_x {
                if is_obscured(x, handle.rect.y) {
                    continue;
                }
                if let Some(cell) = buffer.cell_mut((x, handle.rect.y)) {
                    cell.set_symbol("-");
                    cell.set_style(border_style);
                }
                if is_obscured(x, max_y) {
                    continue;
                }
                if let Some(cell) = buffer.cell_mut((x, max_y)) {
                    cell.set_symbol("-");
                    cell.set_style(border_style);
                }
            }
            for y in handle.rect.y..=max_y {
                if is_obscured(handle.rect.x, y) {
                    continue;
                }
                if let Some(cell) = buffer.cell_mut((handle.rect.x, y)) {
                    cell.set_symbol("|");
                    cell.set_style(border_style);
                }
                if is_obscured(max_x, y) {
                    continue;
                }
                if let Some(cell) = buffer.cell_mut((max_x, y)) {
                    cell.set_symbol("|");
                    cell.set_style(border_style);
                }
            }
            if !is_obscured(handle.rect.x, handle.rect.y)
                && let Some(cell) = buffer.cell_mut((handle.rect.x, handle.rect.y))
            {
                cell.set_symbol("+");
                cell.set_style(border_style);
            }
            if !is_obscured(max_x, handle.rect.y)
                && let Some(cell) = buffer.cell_mut((max_x, handle.rect.y))
            {
                cell.set_symbol("+");
                cell.set_style(border_style);
            }
            if !is_obscured(handle.rect.x, max_y)
                && let Some(cell) = buffer.cell_mut((handle.rect.x, max_y))
            {
                cell.set_symbol("+");
                cell.set_style(border_style);
            }
            if !is_obscured(max_x, max_y)
                && let Some(cell) = buffer.cell_mut((max_x, max_y))
            {
                cell.set_symbol("+");
                cell.set_style(border_style);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::prelude::{Direction, Rect};

    #[test]
    fn build_rects_from_sizes_horizontal() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 3,
        };
        let sizes = [3u16, 7u16];
        let rects = build_rects_from_sizes(Direction::Horizontal, area, &sizes);
        assert_eq!(rects.len(), 2);
        assert_eq!(rects[0].width, 3);
        assert_eq!(rects[1].width, 7);
        assert_eq!(rects[0].x, 0);
        assert_eq!(rects[1].x, 3);
    }

    #[test]
    fn build_rects_from_sizes_vertical() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 4,
            height: 9,
        };
        let sizes = [2u16, 3u16, 4u16];
        let rects = build_rects_from_sizes(Direction::Vertical, area, &sizes);
        assert_eq!(rects.len(), 3);
        assert_eq!(rects[0].height, 2);
        assert_eq!(rects[1].height, 3);
        assert_eq!(rects[2].height, 4);
        assert_eq!(rects[2].y, 5);
    }

    #[test]
    fn split_rects_weighted_even() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 11,
            height: 1,
        };
        let weights = [1.0f32, 1.0f32];
        let rects = split_rects_weighted(Direction::Horizontal, area, &weights, 2);
        assert_eq!(rects.len(), 2);
        // floor division: first portion floor((1/2)*11)=5, remainder 6
        assert_eq!(rects[0].width, 5);
        assert_eq!(rects[1].width, 6);
    }

    #[test]
    fn insert_and_remove_leaf_and_split_area_for_path() {
        let mut node: LayoutNode<usize> = LayoutNode::leaf(1);
        // insert a new leaf to the right of 1
        assert!(node.insert_leaf(1, 2, InsertPosition::Right));
        // now root should be a split
        if let LayoutNode::Split { children, .. } = &node {
            assert_eq!(children.len(), 2);
            // first child should be leaf(1)
            assert_eq!(children[0].unwrap_leaf(), Some(1));
            assert_eq!(children[1].unwrap_leaf(), Some(2));
        } else {
            panic!("expected split after insert");
        }

        // compute area for the second child (path [1])
        let area = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 4,
        };
        let sub = split_area_for_path(&node, area, &[1]).expect("should get area for path");
        // since weights default to equal, second rect x should be > 0
        assert!(sub.x > 0);

        // remove leaf 2
        assert!(node.remove_leaf(2));
        // after removal, node should simplify back to leaf(1)
        assert_eq!(node.unwrap_leaf(), Some(1));
    }
}
