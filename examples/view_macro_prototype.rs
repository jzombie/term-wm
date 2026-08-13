//! Demonstrates the declarative `view!` macro.
//!
//! `MyWindow` hosts a real terminal: the stateful `ScrollViewComponent` is held
//! by the app and injected into the per-frame view by `{ &mut self.terminal }`.
//! Layout tags (`VerticalStack`, `Grid`, `Center`, `Box`) and stateless leaves
//! (`Label`, `Button`) are constructed declaratively. The terminal is wired
//! into the live event loop via `TermWmApp::run_with_setup`.
//!
//! Run: `cargo run --example view_demo`

use term_wm::components::AppRootComponent;
use term_wm::prelude::*;
use term_wm::view;
use term_wm_core::impl_view_component;

struct MyWindow {
    terminal: term_wm::ScrollViewComponent<term_wm::TerminalComponent>,
}

impl MyWindow {
    fn view(&mut self) -> impl Component<TermWmAction> + '_ {
        view! {
            <VerticalStack gap=1>
                <Label text="term-wm view! macro prototype" />
                <Center width=80 height=12>
                    <Box>
                    <Grid cols="1fr 2fr" rows="1fr">
                        <Label text="left cell" />
                        <Box title="Embedded terminal">
                            { &mut self.terminal }
                        </Box>
                    </Grid>
                    </Box>
                </Center>
                <Button label=" Quit " action={TermWmAction::Quit} />
            </VerticalStack>
        }
    }
}

// `view(&mut self)` injects `{ &mut self.terminal }`, so `desired_height(&self)`
// can't build the view; a window root stretches (`0`).
impl_view_component!(MyWindow, height = 0);

fn main() -> std::io::Result<()> {
    let mut app: TermWmApp<MyWindow> = TermWmApp::new_custom(AppContext::new("view-demo", "0.0.0"));

    let size = term_wm::TerminalComponent::default_pty_size();
    let pty = term_wm_pty_engine::Pty::spawn_with_scrollback(
        term_wm::default_shell_command(),
        size,
        2000,
    )
    .map_err(std::io::Error::other)?;
    let tracker = pty.direct_input_tracker();
    let mut scroll =
        term_wm::ScrollViewComponent::new(term_wm::TerminalComponent::from_pane(Box::new(pty)));
    scroll.set_keyboard_mode(term_wm::ScrollKeyMode::PaginationOnly);

    let key = app.open_window(AppRootComponent::Custom(MyWindow { terminal: scroll }));
    app.wm().set_window_tracker(key, tracker);
    app.set_window_title(key, "view! demo");

    // Wire the terminal hosted inside the Custom window: PTY output wakes the
    // event loop and direct-mode transitions reach the WM. `run()` only
    // auto-wires `CoreWmComponent::Terminal` windows, so `run_with_setup` is
    // how a terminal inside a `view!` window gets wired.
    app.run_with_setup(|app, ctx| {
        if let Some(AppRootComponent::Custom(MyWindow { terminal, .. })) =
            app.wm().component_for_key_mut(key)
        {
            ctx.wire_terminal(&mut terminal.content.borrow_mut(), key);
        }
    })
}
