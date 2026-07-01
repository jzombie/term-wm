use std::io;
use std::sync::Arc;

use ratatui::prelude::Rect;
use ratatui::widgets::Clear;

use term_wm::SvgImageComponent;
use term_wm::components::{Component, ComponentContext};
use term_wm::io::RenderTarget;
use term_wm::io::console::{ConsoleEventSource, ConsoleRenderTarget};
use term_wm::runner::{WindowManagerHost, WindowProvider, run_window_app};
use term_wm::ui::UiFrame;
use term_wm::window::{WindowDrawContext, WindowKey, WindowManager};
use term_wm::wm_config::WmConfig;

fn main() -> io::Result<()> {
    let mut app = App::new(std::env::args().skip(1).collect())?;
    let mut output = ConsoleRenderTarget::new()?;
    output.enter()?;
    let mut input = ConsoleEventSource::new();

    let result = run_window_app(&mut output, &mut input, &mut app);

    output.exit()?;

    result
}

struct App {
    wm: WindowManager,
    left: SvgImageComponent,
    right: SvgImageComponent,
    pending_paths: Vec<String>,
    loaded_count: usize,
    left_key: Option<WindowKey>,
    right_key: Option<WindowKey>,
}

impl App {
    fn new(paths: Vec<String>) -> io::Result<Self> {
        let mut left = SvgImageComponent::new();
        let mut right = SvgImageComponent::new();
        let hostname = None;
        let top_panel: Box<
            dyn term_wm_core::top_panel_trait::TopPanel<WindowKey>,
        > = Box::new(term_wm_sys_ui_components::WmTopPanelComponent::new(
            "example",
        ));
        let bottom_panel: Box<dyn term_wm_core::bottom_panel_trait::BottomPanel> = Box::new(
            term_wm_sys_ui_components::WmBottomPanelComponent::new("example", "0.0.0", hostname),
        );
        let menu_overlay: Box<
            dyn term_wm_core::components::MenuOverlay<term_wm_core::window::WmMenuAction>,
        > = Box::new(term_wm_sys_ui_components::WmMenuOverlay::new());
        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(term_wm::AppContext::new("example", "0.0.0")),
            Some(top_panel),
            Some(bottom_panel),
            Some(menu_overlay),
        );
        let left_key = Some(wm.create_window());
        let right_key = Some(wm.create_window());
        let mut app = Self {
            wm,
            left,
            right,
            pending_paths: paths,
            loaded_count: 0,
            left_key,
            right_key,
        };
        app.wm_new_window()?;
        app.wm_new_window()?;
        Ok(app)
    }
}

impl WindowManagerHost for App {
    fn windows(&mut self) -> &mut WindowManager {
        &mut self.wm
    }

    fn wm_new_window(&mut self) -> io::Result<()> {
        if self.loaded_count >= self.pending_paths.len() {
            return Ok(());
        }
        let path = &self.pending_paths[self.loaded_count];
        match self.loaded_count {
            0 => load_into(&mut self.left, path)?,
            1 => load_into(&mut self.right, path)?,
            _ => {}
        }
        self.loaded_count += 1;
        Ok(())
    }
}

impl WindowProvider for App {
    fn enumerate_windows(&mut self) -> Vec<WindowKey> {
        let mut keys = Vec::new();
        if let Some(k) = self.left_key {
            keys.push(k);
        }
        if let Some(k) = self.right_key {
            keys.push(k);
        }
        keys
    }

    fn render_window(
        &mut self,
        frame: &mut UiFrame<'_>,
        window: WindowDrawContext,
        _ctx: &ComponentContext,
    ) {
        let area = window.surface.inner;
        if area.width == 0 || area.height == 0 {
            return;
        }
        if Some(window.id) == self.left_key {
            frame.render_widget(Clear, area);
            self.left.render(frame, area, &ComponentContext::new(window.focused));
        } else if Some(window.id) == self.right_key {
            frame.render_widget(Clear, area);
            self.right.render(frame, area, &ComponentContext::new(window.focused));
        }
    }

    fn empty_window_message(&self) -> &str {
        "no images loaded"
    }

    fn window_component(&mut self, key: WindowKey) -> Option<&mut dyn Component> {
        if Some(key) == self.left_key {
            return Some(&mut self.left);
        }
        if Some(key) == self.right_key {
            return Some(&mut self.right);
        }
        None
    }
}

fn load_into(component: &mut SvgImageComponent, path: &str) -> io::Result<()> {
    component.load_from_path(path)
}
