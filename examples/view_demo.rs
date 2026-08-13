//! Demonstrates the declarative `view!` macro.
//!
//! `MyWindow` hosts a real terminal: the stateful `ScrollViewComponent` is held
//! by the app and injected into the per-frame view by `{ &mut self.terminal }`.
//! Layout tags (`VerticalStack`, `Grid`, `Center`) and stateless leaves
//! (`Label`, `Button`) are constructed declaratively.
//!
//! Run: `cargo run --example view_demo`

use term_wm::prelude::*;
use term_wm::view;
use term_wm::components::AppRootComponent;

struct MyWindow {
    terminal: term_wm::ScrollViewComponent<term_wm::TerminalComponent>,
}

impl MyWindow {
    fn view(&mut self) -> impl Component<TermWmAction> + '_ {
        view! {
            <VerticalStack gap=1>
                <Label text="term-wm view! demo — quit with the button below or Ctrl+Q" />
                <Center width=80 height=12>
                    <Grid cols="1fr 2fr" rows="1fr">
                        <Label text="left cell" />
                        { &mut self.terminal }
                    </Grid>
                </Center>
                <Button label=" Quit " action={TermWmAction::Quit} />
            </VerticalStack>
        }
    }
}

impl Component<TermWmAction> for MyWindow {
    fn render(
        &mut self,
        backend: &mut dyn term_wm::RenderBackend,
        area: term_wm::Rect,
        ctx: &ComponentContext,
        registry: &mut term_wm::hitbox_registry::HitboxRegistry,
    ) {
        self.view().render(backend, area, ctx, registry);
    }

    fn handle_events(&mut self, event: &term_wm::Event, ctx: &ComponentContext) -> EventResult<TermWmAction> {
        self.view().handle_events(event, ctx)
    }

    fn update(
        &mut self,
        action: TermWmAction,
        ctx: &ComponentContext,
        actions: &mut std::collections::VecDeque<(term_wm::window::WindowKey, TermWmAction)>,
    ) {
        self.view().update(action, ctx, actions);
    }

    fn destroy(&mut self) {
        self.view().destroy();
    }
}

fn main() -> std::io::Result<()> {
    let mut app: TermWmApp<MyWindow> =
        TermWmApp::new_custom(AppContext::new("view-demo", "0.0.0"));

    let pty = term_wm::TerminalComponent::spawn_default(term_wm::default_shell_command())
        .map_err(std::io::Error::other)?;
    let mut scroll = term_wm::ScrollViewComponent::new(pty);
    scroll.set_keyboard_mode(term_wm::ScrollKeyMode::PaginationOnly);

    let key = app.open_window(AppRootComponent::Custom(MyWindow { terminal: scroll }));
    app.set_window_title(key, "view! demo");
    app.run()
}
