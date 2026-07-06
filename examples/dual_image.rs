use std::io;
use std::sync::Arc;

use term_wm::SvgImageComponent;
use term_wm::actions::TermWmAction;
use term_wm::components::{Component, component_downcast_mut};
use term_wm::io::console_event_source::ConsoleEventSource;
use term_wm::io::{ConsoleRenderTarget, RenderTarget};
use term_wm::runner::{WindowManagerHost, WindowProvider, run_window_app};
use term_wm::window::{WindowKey, WindowManager};
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
    pending_paths: Vec<String>,
    loaded_count: usize,
    left_key: Option<WindowKey>,
    right_key: Option<WindowKey>,
}

impl App {
    fn new(mut paths: Vec<String>) -> io::Result<Self> {
        let mut left = SvgImageComponent::new();
        let mut right = SvgImageComponent::new();
        left.set_keep_aspect(true);
        right.set_keep_aspect(true);
        left.set_colorize(true);
        right.set_colorize(true);
        if paths.is_empty() {
            paths.push("assets/zenOSmosis-logo.svg".to_string());
        }
        if paths.len() == 1 {
            paths.push(paths[0].clone());
        }
        let hostname = None;
        let top_panel: Box<dyn term_wm_core::top_panel_trait::TopPanel<WindowKey>> = Box::new(
            term_wm_sys_ui_components::WmTopPanelComponent::new("example"),
        );
        let bottom_panel: Box<dyn term_wm_core::bottom_panel_trait::BottomPanel> = Box::new(
            term_wm_sys_ui_components::WmBottomPanelComponent::new("example", "0.0.0", hostname),
        );
        let menu_overlay: Box<
            dyn term_wm_core::components::MenuOverlay<term_wm_core::actions::TermWmAction>,
        > = Box::new(term_wm_sys_ui_components::WmMenuOverlay::new());
        let mut wm = WindowManager::with_config(
            WmConfig::standalone(),
            Arc::new(term_wm::AppContext::new("example", "0.0.0")),
            Some(top_panel),
            Some(bottom_panel),
            Some(menu_overlay),
        );
        // Box the image components and hand ownership to the WindowManager.
        let left_key = Some(wm.create_window(Box::new(left)));
        let right_key = Some(wm.create_window(Box::new(right)));
        let mut app = Self {
            wm,
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
        let key = match self.loaded_count {
            0 => self.left_key,
            1 => self.right_key,
            _ => None,
        };
        if let Some(key) = key
            && let Some(comp) = self.wm.component_for_key_mut(key)
            && let Some(img) = component_downcast_mut::<SvgImageComponent>(comp)
        {
            img.load_from_path(path).map_err(io::Error::other)?;
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

    fn empty_window_message(&self) -> &str {
        "no images loaded"
    }

    fn window_component(&mut self, key: WindowKey) -> Option<&mut dyn Component<TermWmAction>> {
        self.wm.component_for_key_mut(key)
    }
}
