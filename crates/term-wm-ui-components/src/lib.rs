extern crate self as term_wm_ui_components;

pub mod ascii_image;
pub mod button;
pub mod canvas_scroll_view;
pub mod center;
pub mod command_palette;
pub mod confirm_overlay;
pub mod dialog_overlay;
pub mod grid;
pub mod helpers;
pub mod hstack;
pub mod label;
pub mod list;
pub mod markdown_viewer;
pub mod menu;
// `box` is a reserved keyword, so the module uses a raw identifier (loads
// `src/box.rs`); consumers only ever see the re-exported `BoxComponent`.
mod r#box;
pub mod scroll_view;

pub mod svg_image;
pub mod tab_bar;
pub mod terminal;
pub mod text_renderer;
pub mod toggle_list;
pub mod vertical_stack;

pub use ascii_image::AsciiImageComponent;
pub use r#box::BoxComponent;
pub use button::ButtonComponent;
pub use canvas_scroll_view::{CanvasScrollView, CanvasSizingPolicy};
pub use center::CenterComponent;
pub use command_palette::CommandPaletteComponent;
pub use confirm_overlay::ConfirmOverlayComponent;
pub use dialog_overlay::DialogOverlayComponent;
pub use grid::{
    FRACTION_COL_MIN_WIDTH, GridComponent, GridConstraint, grid_reflows, resolve_sizes,
};
pub use hstack::HStackComponent;
pub use label::LabelComponent;
pub use list::ListComponent;
pub use markdown_viewer::MarkdownViewerComponent;
pub use menu::MenuComponent;
pub use scroll_view::{
    ScrollKeyMode, ScrollViewComponent, ScrollbarAxis, ScrollbarDrag, render_scrollbar,
    render_scrollbar_oriented,
};

pub use svg_image::SvgImageComponent;
pub use tab_bar::{TabBarComponent, TabBarEvent, TabItem};
pub use terminal::{TerminalComponent, default_shell, default_shell_command};
pub use text_renderer::TextRendererComponent;
pub use toggle_list::{ToggleItem, ToggleListComponent};
pub use vertical_stack::VerticalStackComponent;
