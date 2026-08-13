//! Ground-truth diagnostic: where do mouse events for the embedded terminal in
//! a `view!` tree actually go?
//!
//! Runs the demo's exact tree (`VStack{Label, Center{Box{Grid{Label, Box{ScrollView<probe>}}}}, Button}`)
//! through the REAL pipeline — `WindowManager` + `render_app` + `dispatch_mouse` —
//! with a probe leaf that mirrors `TerminalComponent` (registers a hitbox over its
//! screen area, reports a scrollable content size) and logs every `handle_events`
//! call it receives. The printed trace answers:
//!   1. does a press on the terminal content reach the probe (selection)?
//!   2. does a press on the scrollbar column reach anything (drag)?
//!   3. what `ctx.screen_area()` does the probe see vs its render rect?

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use term_wm::Component;
use term_wm::actions::{EventResult, TermWmAction};
use term_wm::component_context::ComponentContext;
use term_wm::components::AppRootComponent;
use term_wm::components::SelectionStatus;
use term_wm::events::Event;
use term_wm::events::core_event_to_wm;
use term_wm::events::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind, WmEvent};
use term_wm::hitbox_registry::{ComponentOwner, HitboxId, HitboxRegistry};
use term_wm::layout::tiling::{LayoutNode, TilingLayout};
use term_wm::render_app;
use term_wm::view;
use term_wm::window::{WindowKey, WindowManager};
use term_wm::wm_config::WmConfig;
use term_wm_console::RatatuiBackend;
use term_wm_console::draw_plan_renderer::DrawPlanRenderer;
use term_wm_core::engine::CoreEngine;
use term_wm_core::impl_view_component;
use term_wm_layout_engine::LayoutRect;

const AREA: term_wm_core::Rect = term_wm_core::Rect {
    x: 0,
    y: 0,
    width: 100,
    height: 30,
};

/// Mirrors `TerminalComponent`: registers a hitbox over its screen area and
/// reports a large scrollable content size so the scroll view shows a scrollbar.
/// With `consume_left_press` it returns `Consumed` on a left press (like the
/// terminal's `handle_selection_mouse`), so the WM captures the gesture.
struct ProbeLeaf {
    log: Rc<RefCell<Vec<String>>>,
    render_area: Rc<RefCell<Option<LayoutRect>>>,
    event_area: Rc<RefCell<Option<LayoutRect>>>,
    event_areas: Rc<RefCell<Vec<LayoutRect>>>,
    consume_left_press: bool,
    selection: Rc<RefCell<SelectionStatus>>,
    text: Rc<RefCell<Option<String>>>,
    hitbox: HitboxId,
}

impl Component<TermWmAction> for ProbeLeaf {
    fn hitbox_id(&self) -> Option<HitboxId> {
        Some(self.hitbox)
    }

    fn selection_status(&self) -> SelectionStatus {
        *self.selection.borrow()
    }

    fn selection_text(&self) -> Option<String> {
        self.text.borrow().clone()
    }

    fn render(
        &mut self,
        _b: &mut dyn term_wm::RenderBackend,
        _a: LayoutRect,
        ctx: &ComponentContext,
        registry: &mut HitboxRegistry,
    ) {
        *self.render_area.borrow_mut() = ctx.screen_area();
        if let Some(key) = ctx.window_key() {
            registry.register(
                self.hitbox,
                ComponentOwner::Window(key),
                ctx.screen_area().unwrap_or_default(),
            );
        }
        if let Some(h) = ctx.scroll_handle() {
            h.set_content_size(100, 50);
        }
    }

    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        let desc = match event {
            Event::Mouse(m) => format!("Mouse({:?}, col={}, row={})", m.kind, m.column, m.row),
            other => format!("{other:?}"),
        };
        self.log
            .borrow_mut()
            .push(format!("  {desc} screen_area={:?}", ctx.screen_area()));
        let sa = ctx.screen_area();
        *self.event_area.borrow_mut() = sa;
        if let Some(sa) = sa {
            self.event_areas.borrow_mut().push(sa);
        }
        if self.consume_left_press
            && matches!(
                event,
                Event::Mouse(m)
                    if matches!(m.kind, MouseEventKind::Press(MouseButton::Left))
            )
        {
            return EventResult::Consumed;
        }
        EventResult::Ignored
    }
}

/// The demo's window: a terminal inside `<Center><Box><Grid><Box>`.
struct DemoWindow {
    scroll: term_wm::ScrollViewComponent<ProbeLeaf>,
}

impl DemoWindow {
    fn view(&mut self) -> impl Component<TermWmAction> + '_ {
        view! {
            <VStack gap=1>
                <Label text="h" />
                <Center width=80 height=12>
                    <Box>
                        <Grid cols="1fr 2fr" rows="1fr">
                            <Label text="l" />
                            <Box title="t">
                                { &mut self.scroll }
                            </Box>
                        </Grid>
                    </Box>
                </Center>
                <Button label=" Quit " action={TermWmAction::Quit} />
            </VStack>
        }
    }
}

impl_view_component!(DemoWindow, height = 0, child: scroll);

fn make_mouse(kind: MouseEventKind, col: u16, row: u16) -> WmEvent {
    let event = Event::Mouse(MouseEvent {
        kind,
        column: col,
        row,
        modifiers: KeyModifiers::NONE,
    });
    core_event_to_wm(&event).expect("valid mouse event")
}

struct Harness {
    wm: WindowManager<AppRootComponent<DemoWindow>>,
    key: WindowKey,
    probe_hitbox: HitboxId,
    log: Rc<RefCell<Vec<String>>>,
    render_area: Rc<RefCell<Option<LayoutRect>>>,
    event_areas: Rc<RefCell<Vec<LayoutRect>>>,
    handle: term_wm::component_context::ScrollHandle,
}

/// Build the demo window and render it through the real pipeline.
fn setup_probe(consume_left_press: bool) -> Harness {
    let config = WmConfig {
        chrome_enabled: false,
        ..Default::default()
    };
    let mut wm: WindowManager<AppRootComponent<DemoWindow>> = WindowManager::with_config(
        config,
        Arc::new(term_wm::AppContext::new("probe", "0.0.0")),
        None,
        term_wm_core::window::LayerManager::new(),
        std::collections::HashMap::new(),
    );
    wm.set_panel_visible(false);

    let log = Rc::new(RefCell::new(Vec::new()));
    let render_area = Rc::new(RefCell::new(None));
    let event_area = Rc::new(RefCell::new(None));
    let event_areas = Rc::new(RefCell::new(Vec::new()));
    let probe_hitbox = HitboxId::new();
    let probe = ProbeLeaf {
        log: log.clone(),
        render_area: render_area.clone(),
        event_area,
        event_areas: event_areas.clone(),
        consume_left_press,
        selection: Rc::new(RefCell::new(SelectionStatus {
            active: false,
            dragging: false,
        })),
        text: Rc::new(RefCell::new(None)),
        hitbox: probe_hitbox,
    };
    let scroll = term_wm::ScrollViewComponent::new(probe);
    let handle = scroll.scroll_handle();
    let key = wm.create_window(AppRootComponent::Custom(DemoWindow { scroll }));
    wm.set_managed_layout(TilingLayout::new(LayoutNode::Leaf(key)));
    wm.register_managed_layout(AREA);

    // Render through the real pipeline so regions + hitboxes exist.
    let rect = ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: AREA.width,
        height: AREA.height,
    };
    let buf = ratatui::buffer::Buffer::empty(rect);
    let mut backend = RatatuiBackend::new_simple(buf, rect);
    render_app(
        &mut backend,
        &mut wm,
        &mut CoreEngine::new(),
        &mut DrawPlanRenderer::new(),
    );

    Harness {
        wm,
        key,
        probe_hitbox,
        log,
        render_area,
        event_areas,
        handle,
    }
}

#[test]
fn trace_embedded_terminal_mouse() {
    let mut h = setup_probe(false);
    let Harness {
        ref mut wm,
        key,
        probe_hitbox,
        ref log,
        ref render_area,
        ref event_areas,
        ref handle,
    } = h;
    let r = *render_area.borrow().as_ref().expect("probe must render");
    println!("=== PROBE render_area = {r:?} (screen)");
    println!(
        "=== WM region(key) = {:?} (the event-dispatch screen_area base)",
        wm.region(key)
    );
    println!("=== managed_area = {:?}", wm.managed_area());

    use term_wm_layout_engine::{CoordSpace, MousePosition};
    let pos = |col: i16, row: i16| MousePosition {
        column: col,
        row,
        space: CoordSpace::Screen,
    };
    let reg = wm.hitbox_registry_mut();
    println!("=== registry len = {}", reg.len());
    println!(
        "=== hitboxes under CONTENT ({},{}):",
        r.x + i32::from(r.width / 2),
        r.y + i32::from(r.height / 2)
    );
    for (id, owner, a) in reg.hit_test_all(pos(
        (r.x + i32::from(r.width / 2)) as i16,
        (r.y + i32::from(r.height / 2)) as i16,
    )) {
        println!(
            "    id={id:?} owner={owner:?} area={a:?}{}",
            if id == probe_hitbox {
                "  <-- PROBE"
            } else {
                ""
            }
        );
    }
    let sb_x = r.x + i32::from(r.width) - 1;
    println!("=== hitboxes under SCROLLBAR column ({}):", sb_x);
    for (id, owner, a) in reg.hit_test_all(pos(sb_x as i16, (r.y + i32::from(r.height / 2)) as i16))
    {
        println!(
            "    id={id:?} owner={owner:?} area={a:?}{}",
            if id == probe_hitbox {
                "  <-- PROBE"
            } else {
                ""
            }
        );
    }
    let probe_registered = reg
        .hit_test_all(pos(
            (r.x + i32::from(r.width / 2)) as i16,
            (r.y + i32::from(r.height / 2)) as i16,
        ))
        .any(|(id, _, _)| id == probe_hitbox);
    println!("=== PROBE hitbox registered (under content) = {probe_registered}");
    let _ = reg;

    // 1. Press in the middle of the terminal content (selection).
    let cx = (r.x + i32::from(r.width / 2)) as u16;
    let cy = (r.y + i32::from(r.height / 2)) as u16;
    wm.dispatch_mouse(&make_mouse(
        MouseEventKind::Press(MouseButton::Left),
        cx,
        cy,
    ));
    let after_content = log.borrow().len();
    println!("=== after CONTENT press at ({cx},{cy}) -> {after_content} events");
    for l in log.borrow().iter() {
        println!("{l}");
    }

    // 2. Press on the scrollbar column (rightmost column of the terminal area).
    let sb_col = sb_x as u16;
    let res_press = wm.dispatch_mouse(&make_mouse(
        MouseEventKind::Press(MouseButton::Left),
        sb_col,
        cy,
    ));
    let after_sb = log.borrow().len();
    println!(
        "=== after SCROLLBAR press at ({sb_col},{cy}) -> {after_sb} events (dispatch: {res_press:?})"
    );
    for l in log.borrow().iter() {
        println!("{l}");
    }

    // 3. A drag on the scrollbar column must scroll (not reach the probe).
    let before_off = handle.info().offset_y;
    let res_drag = wm.dispatch_mouse(&make_mouse(
        MouseEventKind::Drag(MouseButton::Left),
        sb_col,
        (cy + 2).min(r.y as u16 + r.height),
    ));
    let after_drag = log.borrow().len();
    let after_off = handle.info().offset_y;
    println!(
        "=== after SCROLLBAR DRAG -> events={after_drag} offset {before_off} -> {after_off} (dispatch: {res_drag:?})"
    );
    wm.dispatch_mouse(&make_mouse(
        MouseEventKind::Release(MouseButton::Left),
        sb_col,
        cy,
    ));

    // Diagnostic summary: the leaf's event screen_area must equal its render rect.
    println!(
        "=== DIAGNOSIS: render_area={r:?} | event screen_areas={:?}",
        *event_areas.borrow()
    );
    assert!(
        probe_registered,
        "the probe's own hitbox must be registered under its content (it was culled by the scroll view's clip?)"
    );
    assert!(
        after_content > 0,
        "content press must reach the probe (selection) — got 0 events"
    );
    assert_eq!(
        event_areas.borrow().first().copied(),
        Some(r),
        "leaf event screen_area must equal its render rect (geometry parity)"
    );
    assert!(
        after_sb == after_content,
        "scrollbar-column press must be handled by the scroll view, not reach the probe — got {after_sb} events"
    );
    assert!(
        after_off > before_off,
        "scrollbar drag must scroll (offset {before_off} -> {after_off})"
    );
}

#[test]
fn selection_gesture_routes_through_capture() {
    // The terminal's text selection is a gesture: a left Press starts a drag
    // (handle_selection_mouse returns Consumed -> the WM captures), then Drag
    // extends it and Release finishes it. Simulate that gesture on the content
    // and assert every event reaches the leaf with geometry parity (its
    // screen_area equals the render rect at each step).
    let mut h = setup_probe(true);
    let Harness {
        ref mut wm,
        ref log,
        ref render_area,
        ref event_areas,
        ..
    } = h;
    let r = *render_area.borrow().as_ref().expect("probe must render");

    let cx = (r.x + i32::from(r.width / 2)) as u16;
    let cy = (r.y + i32::from(r.height / 2)) as u16;
    let dy = (cy + 2).min((r.y + i32::from(r.height) - 1) as u16);

    let res_press = wm.dispatch_mouse(&make_mouse(
        MouseEventKind::Press(MouseButton::Left),
        cx,
        cy,
    ));
    let res_drag = wm.dispatch_mouse(&make_mouse(MouseEventKind::Drag(MouseButton::Left), cx, dy));
    let res_release = wm.dispatch_mouse(&make_mouse(
        MouseEventKind::Release(MouseButton::Left),
        cx,
        dy,
    ));

    println!(
        "=== selection gesture: press={res_press:?} drag={res_drag:?} release={res_release:?}"
    );
    for l in log.borrow().iter() {
        println!("{l}");
    }

    assert!(
        res_press.is_consumed(),
        "selection press must be consumed (WM captures): {res_press:?}"
    );
    assert_eq!(
        log.borrow().len(),
        3,
        "press + drag + release must all reach the leaf (selection gesture) — got {:?}",
        log.borrow().len()
    );
    let areas = event_areas.borrow();
    assert_eq!(
        areas.len(),
        3,
        "each gesture event must record a screen_area"
    );
    for (i, a) in areas.iter().enumerate() {
        assert_eq!(
            *a, r,
            "gesture event {i} screen_area must equal the render rect (geometry parity)"
        );
    }
}

#[test]
fn window_root_reports_embedded_terminal_selection() {
    // The WM's copy-on-selection-release path (`update_selection_snapshot`)
    // reads the FOCUSED WINDOW's `selection_status()`/`selection_text()`. With
    // `impl_view_component!(DemoWindow, height = 0, child: scroll)`, that
    // metadata must come from the embedded terminal (the scroll view), not the
    // default (no selection). Set a selection on the probe and assert the window
    // root reports it.
    let mut h = setup_probe(true);
    let Harness {
        ref mut wm,
        key,
        ref render_area,
        ref event_areas,
        ..
    } = h;
    let r = *render_area.borrow().as_ref().expect("probe must render");
    let _ = event_areas;

    // Reach into the probe to simulate a completed selection.
    let (sel, text) = {
        let AppRootComponent::Custom(DemoWindow { scroll }) = wm
            .component_for_key_mut(key)
            .expect("window root component")
        else {
            panic!("expected custom window");
        };
        let content = scroll.content.borrow();
        let sel = content.selection.clone();
        let text = content.text.clone();
        (sel, text)
    };
    *sel.borrow_mut() = SelectionStatus {
        active: true,
        dragging: false,
    };
    *text.borrow_mut() = Some("selected text".to_string());

    // The window root must surface the embedded terminal's selection + text.
    let status = wm.component_for_key_mut(key).unwrap().selection_status();
    let sel_text = wm.component_for_key_mut(key).unwrap().selection_text();
    println!(
        "=== window-root selection_status={status:?} selection_text={sel_text:?} (render rect {r:?})"
    );
    assert!(
        status.active,
        "window root must report the embedded terminal's active selection"
    );
    assert_eq!(
        sel_text.as_deref(),
        Some("selected text"),
        "window root must report the embedded terminal's selection text"
    );
}
