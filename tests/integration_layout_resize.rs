#[cfg(test)]
mod tests {
    use ratatui::prelude::{Direction, Rect};
    use term_wm::layout::tiling::{InsertPosition, LayoutNode};

    #[test]
    fn test_vertical_resize_precision() {
        // This test ensures that 1-pixel resize operations are not lost due to
        // floating point quantization errors (e.g. floor(20.99) -> 20).

        let area = Rect {
            x: 0,
            y: 0,
            width: 20,
            height: 20,
        };

        // Create a vertical split with 2 leaves (Top=1, Bottom=2)
        let mut node = LayoutNode::leaf(1);
        node.insert_leaf(1, 2, InsertPosition::Bottom);

        // Initial layout: 10 vs 10 (assuming gap=0 or handled internally)
        // With gap=1 (default handle thickness), height is 19.
        // It splits as 9 and 10 usually, or 10 and 9.

        // Let's check initial state
        let (regions, _) = node.layout_with_handles(area);
        let h1_start = regions.iter().find(|(id, _)| *id == 1).unwrap().1.height;

        let mut current_height = h1_start;

        // Perform 5 separate 1-pixel drag operations
        for _ in 0..5 {
            // Drag handle #0 (vertical) by +1
            let success = node.apply_drag(area, &[], 0, Direction::Vertical, 1);
            assert!(success, "apply_drag returned false");

            let (regions, _) = node.layout_with_handles(area);
            let h1_new = regions.iter().find(|(id, _)| *id == 1).unwrap().1.height;

            assert!(
                h1_new > current_height,
                "Resize got stuck! Expected growth from {}, got {}. Precision loss suspected.",
                current_height,
                h1_new
            );
            current_height = h1_new;
        }
    }
}
