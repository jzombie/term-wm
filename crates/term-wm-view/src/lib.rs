//! The `view!` proc-macro — declarative construction of term-wm component trees.
//!
//! Expands to fully-monomorphized Rust that a developer could hand-write:
//! container constructors, leaf component constructors, and generated delegate
//! enums for heterogeneous sibling sets. The result is a value implementing
//! [`Component`] — there is no runtime tree, no reactivity, no reconciliation,
//! no `Box`/`dyn`.
//!
//! ```text
//! view! {
//!     <VerticalStack gap=1>
//!         <Label text="Status" />
//!         <Button label="Refresh" action={TermWmAction::OpenCommandPalette} />
//!         { &mut self.terminal }                       // borrowed component
//!     </VerticalStack>
//! }
//! ```
//!
//! Tiers:
//! 1. **Layout-primitive tags**: `<VerticalStack>`/`<Column>`, `<HStack>`/`<Row>`,
//!    `<Center width height>`, `<Grid cols rows>` (constraint strings parsed at
//!    compile time into `GridConstraint`s).
//! 2. **Built-in component tags**: `<Label text>`, `<Button label action|onClick>`.
//! 3. **Expression braces** `{ expr }`: any expression yielding a `Component`
//!    (owned, or `&mut C` via the blanket impl) — no registry needed for
//!    third-party or fallible components.
//!
//! # Path resolution
//!
//! Generated code resolves the component/core/render crates via
//! [`proc_macro_crate`], so it works from both the umbrella and the leaf
//! crates without circular dependencies:
//! - Consumers that depend on the `term-wm` umbrella (or the crate itself)
//!   get `::term_wm::` paths (the umbrella re-exports `term_wm_core::*` +
//!   `term_wm_ui_components::*` and `RenderBackend`).
//! - Leaf consumers (e.g. `term-wm-sys-ui-components`, which the umbrella
//!   depends on and therefore cannot re-depend on it) get
//!   `::term_wm_ui_components::`, `::term_wm_core::` and
//!   `::term_wm_render::` paths. Renamed dependencies are honored via the name
//!   `crate_name` reports.

use proc_macro::TokenStream;
use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use rstml::node::{Infallible, Node, NodeAttribute, NodeElement};
use syn::{Expr, Ident, Lit};

/// rstml parses `<VerticalStack>` elements as `NodeElement<Infallible>`.
type Element = NodeElement<Infallible>;

/// Qualified-crate prefixes for the generated code.
struct Paths {
    /// Component types (`Label`, `Button`, containers, `GridConstraint`).
    comp: TokenStream2,
    /// Core types (`Component`, `TermWmAction`, `Rect`, …).
    core: TokenStream2,
    /// The `RenderBackend` path (`::term_wm::RenderBackend` or
    /// `::term_wm_render::RenderBackend`).
    backend: TokenStream2,
}

/// Resolve the crate path for a package, honoring renames and self-aliases.
fn crate_path(pkg: &str) -> TokenStream2 {
    let canonical = format_ident!("{}", pkg.replace('-', "_"));
    match crate_name(pkg) {
        Ok(FoundCrate::Itself) => quote!(::#canonical::),
        Ok(FoundCrate::Name(name)) => {
            let ident = format_ident!("{name}");
            quote!(::#ident::)
        }
        // Not a dependency here — use the canonical name; if it is genuinely
        // absent the compiler reports a clear unresolved-crate error.
        Err(_) => quote!(::#canonical::),
    }
}

/// Pick umbrella vs. leaf paths once per expansion.
fn detect_paths() -> Paths {
    if crate_name("term-wm").is_ok() {
        let prefix = crate_path("term-wm");
        return Paths {
            comp: prefix.clone(),
            core: prefix.clone(),
            backend: quote!(#prefix RenderBackend),
        };
    }
    let ui = crate_path("term-wm-ui-components");
    let core = crate_path("term-wm-core");
    let render = crate_path("term-wm-render");
    Paths {
        comp: ui,
        core,
        backend: quote!(#render RenderBackend),
    }
}

/// Declaratively build a term-wm component tree. See the crate docs.
#[proc_macro]
pub fn view(input: TokenStream) -> TokenStream {
    let tokens = TokenStream2::from(input);
    match expand(tokens) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// Accumulates block-scoped generated items (`enum __ViewN` + `impl`) and the
/// unique-name counter.
struct Generator {
    items: Vec<TokenStream2>,
    counter: usize,
    paths: Paths,
}

impl Generator {
    fn new(paths: Paths) -> Self {
        Self {
            items: Vec::new(),
            counter: 0,
            paths,
        }
    }

    fn next_name(&mut self) -> Ident {
        let n = self.counter;
        self.counter += 1;
        format_ident!("__View{n}")
    }

    fn push_items(&mut self, mut items: Vec<TokenStream2>) {
        self.items.append(&mut items);
    }

    /// `::<comp>::<Name>` for a component type.
    fn comp(&self, name: &str) -> TokenStream2 {
        let ident = format_ident!("{name}");
        let prefix = &self.paths.comp;
        quote!(#prefix #ident)
    }

    /// `::<core>::<module>::<Name>` for a core type.
    fn core_mod(&self, module: &str, name: &str) -> TokenStream2 {
        let module = format_ident!("{module}");
        let ident = format_ident!("{name}");
        let prefix = &self.paths.core;
        quote!(#prefix #module::#ident)
    }

    /// `::<core>::<Name>` for a core-root type (`Rect`).
    fn core_root(&self, name: &str) -> TokenStream2 {
        let ident = format_ident!("{name}");
        let prefix = &self.paths.core;
        quote!(#prefix #ident)
    }
}

fn expand(tokens: TokenStream2) -> syn::Result<TokenStream2> {
    let mut g = Generator::new(detect_paths());
    let nodes = rstml::parse2(tokens.clone())?;
    if nodes.len() != 1 {
        return Err(syn::Error::new_spanned(
            tokens,
            "view! requires exactly one root element — wrap multiple nodes in a container like <VerticalStack>",
        ));
    }
    let root = g.node(&nodes[0])?;
    let items = &g.items;
    let core = &g.paths.core;
    Ok(quote! {{
        use #core components::Component;
        use #core actions::TermWmAction;
        #(#items)*
        #root
    }})
}

/// Filter out whitespace-only text nodes and comments so they never count as
/// children.
fn meaningful(nodes: &[Node]) -> Vec<&Node> {
    nodes
        .iter()
        .filter(|n| match n {
            Node::Text(t) => !t.value_string().trim().is_empty(),
            Node::Comment(_) => false,
            _ => true,
        })
        .collect()
}

fn tag_ident(el: &Element) -> syn::Result<Ident> {
    match el.name() {
        rstml::node::NodeName::Path(p) => p
            .path
            .segments
            .last()
            .map(|seg| seg.ident.clone())
            .ok_or_else(|| syn::Error::new_spanned(el.name(), "empty tag name")),
        _ => Err(syn::Error::new_spanned(
            el.name(),
            "tag names must be plain identifiers (e.g. <VerticalStack>)",
        )),
    }
}

fn attr_key(key: &rstml::node::NodeName) -> Option<String> {
    match key {
        rstml::node::NodeName::Path(p) => p.path.segments.last().map(|s| s.ident.to_string()),
        _ => None,
    }
}

fn attr_expr<'a>(el: &'a Element, name: &str) -> Option<&'a Expr> {
    el.attributes().iter().find_map(|a| match a {
        NodeAttribute::Attribute(kv) if attr_key(&kv.key).as_deref() == Some(name) => kv.value(),
        _ => None,
    })
}

fn required_attr_expr(el: &Element, name: &str) -> syn::Result<Expr> {
    attr_expr(el, name)
        .cloned()
        .map(strip_block)
        .ok_or_else(|| {
            let tag = tag_ident(el).unwrap_or_else(|_| format_ident!("element"));
            syn::Error::new_spanned(el.name(), format!("<{tag}> requires a `{name}` attribute"))
        })
}

/// Unwrap a braced single-expression value (`{ expr }`) to just `expr`, so the
/// emitted code avoids the `unused_braces` lint.
fn strip_block(e: Expr) -> Expr {
    if let Expr::Block(b) = &e
        && let [syn::Stmt::Expr(inner, None)] = b.block.stmts.as_slice()
    {
        return inner.clone();
    }
    e
}

impl Generator {
    fn node(&mut self, node: &Node) -> syn::Result<TokenStream2> {
        match node {
            Node::Block(b) => {
                let Some(block) = b.try_block() else {
                    return Err(syn::Error::new_spanned(
                        b,
                        "invalid expression block in view!",
                    ));
                };
                // Emit a single-expression block without its braces to avoid
                // the `unused_braces` lint at the construction site.
                match block.stmts.as_slice() {
                    [syn::Stmt::Expr(e, None)] => Ok(quote!(#e)),
                    _ => Ok(quote!(#block)),
                }
            }
            Node::Text(t) => Err(syn::Error::new_spanned(
                t,
                "text nodes are not allowed — use <Label text=..> or { expr }",
            )),
            Node::Element(el) => self.element(el),
            other => Err(syn::Error::new_spanned(other, "unsupported node in view!")),
        }
    }

    fn element(&mut self, el: &Element) -> syn::Result<TokenStream2> {
        let tag = tag_ident(el)?;
        match tag.to_string().as_str() {
            "VerticalStack" | "Column" => self.stack_container(el, ContainerKind::Vertical),
            "HStack" | "Row" => self.stack_container(el, ContainerKind::Horizontal),
            "Center" => self.center_container(el),
            "Grid" => self.grid_container(el),
            "Label" => {
                let text = required_attr_expr(el, "text")?;
                let label = self.comp("LabelComponent");
                Ok(quote! {
                    #label::new(#text)
                })
            }
            "Button" => {
                let label = required_attr_expr(el, "label")?;
                let action = attr_expr(el, "action")
                    .or_else(|| attr_expr(el, "onClick"))
                    .cloned()
                    .map(strip_block)
                    .ok_or_else(|| {
                        syn::Error::new_spanned(
                            el.name(),
                            "<Button> requires an `action` (or `onClick`) attribute",
                        )
                    })?;
                let button = self.comp("ButtonComponent");
                Ok(quote! {
                    #button::new(#label, #action)
                })
            }
            other => Err(syn::Error::new_spanned(
                el.name(),
                format!(
                    "unknown tag <{other}>; available tags: VerticalStack, Column, HStack, Row, \
                     Center, Grid, Label, Button. Use {{ expr }} to inject an arbitrary component."
                ),
            )),
        }
    }

    /// Expand a container's children into `.add()` / `vec!` entries, generating
    /// a delegate enum for heterogeneous sibling sets.
    ///
    /// Returns `(items, adds, has_children)`.
    fn children(
        &mut self,
        el: &Element,
    ) -> syn::Result<(Vec<TokenStream2>, Vec<TokenStream2>, bool)> {
        let children = meaningful(el.children());
        match children.len() {
            0 => Ok((Vec::new(), Vec::new(), false)),
            1 => {
                let expr = self.node(children[0])?;
                Ok((Vec::new(), vec![expr], true))
            }
            n => {
                let mut exprs = Vec::with_capacity(n);
                for child in children {
                    exprs.push(self.node(child)?);
                }
                let (enum_item, constructions) = self.sibling_enum(&exprs)?;
                Ok((vec![enum_item], constructions, true))
            }
        }
    }

    /// Generate a block-scoped generic delegate enum + `Component` impl and the
    /// per-child variant constructions. One type param per child, all inferred
    /// at the construction site; no lifetime params — borrowed `&mut C` children
    /// satisfy `Component` via the blanket impl in `term-wm-core`.
    fn sibling_enum(
        &mut self,
        exprs: &[TokenStream2],
    ) -> syn::Result<(TokenStream2, Vec<TokenStream2>)> {
        let name = self.next_name();
        let n = exprs.len();
        let tys: Vec<Ident> = (0..n).map(|i| format_ident!("T{i}")).collect();
        let variants: Vec<Ident> = (0..n).map(|i| format_ident!("V{i}")).collect();

        let bounds = tys.iter().map(|t| quote!(#t: Component<TermWmAction>));

        let enum_def = quote! {
            #[allow(clippy::large_enum_variant, clippy::type_complexity)]
            enum #name<#(#tys),*> { #(#variants(#tys)),* }
        };

        // `render` uses UFCS to avoid inherent-`render` shadowing (AGENTS.md);
        // the blanket `impl Component for &'a mut C` makes UFCS resolve for
        // borrowed fields too. The remaining methods use method-call syntax.
        let render = variants.iter().map(|v| {
            quote!(Self::#v(x) => Component::<TermWmAction>::render(x, backend, area, ctx, registry))
        });
        let handle_events = variants
            .iter()
            .map(|v| quote!(Self::#v(x) => x.handle_events(event, ctx)));
        let update = variants
            .iter()
            .map(|v| quote!(Self::#v(x) => x.update(action, ctx, actions)));
        let destroy = variants.iter().map(|v| quote!(Self::#v(x) => x.destroy()));
        let desired_height = variants
            .iter()
            .map(|v| quote!(Self::#v(x) => x.desired_height(width)));
        let hitbox_id = variants
            .iter()
            .map(|v| quote!(Self::#v(x) => x.hitbox_id()));
        let clear_selection = variants
            .iter()
            .map(|v| quote!(Self::#v(x) => x.clear_selection()));
        let selection_status = variants
            .iter()
            .map(|v| quote!(Self::#v(x) => x.selection_status()));
        let selection_text = variants
            .iter()
            .map(|v| quote!(Self::#v(x) => x.selection_text()));
        let take_pending_title = variants
            .iter()
            .map(|v| quote!(Self::#v(x) => x.take_pending_title()));
        let take_alternate = variants
            .iter()
            .map(|v| quote!(Self::#v(x) => x.take_alternate_screen_transition()));
        let take_teardown = variants
            .iter()
            .map(|v| quote!(Self::#v(x) => x.take_teardown_parts()));
        let set_selection = variants
            .iter()
            .map(|v| quote!(Self::#v(x) => x.set_selection_enabled(enabled)));
        let paste = variants
            .iter()
            .map(|v| quote!(Self::#v(x) => x.paste(text)));
        let init = variants.iter().map(|v| quote!(Self::#v(x) => x.init()));
        let on_mount = variants
            .iter()
            .map(|v| quote!(Self::#v(x) => x.on_mount(key, app)));
        let on_mouse_press = variants
            .iter()
            .map(|v| quote!(Self::#v(x) => x.on_mouse_press(col, row, button, modifiers, ctx)));
        let on_mouse_release = variants
            .iter()
            .map(|v| quote!(Self::#v(x) => x.on_mouse_release(col, row, button, modifiers, ctx)));
        let on_mouse_drag = variants
            .iter()
            .map(|v| quote!(Self::#v(x) => x.on_mouse_drag(col, row, button, modifiers, ctx)));
        let on_mouse_scroll = variants
            .iter()
            .map(|v| quote!(Self::#v(x) => x.on_mouse_scroll(col, row, kind, modifiers, ctx)));
        let on_mouse_move = variants
            .iter()
            .map(|v| quote!(Self::#v(x) => x.on_mouse_move(col, row, modifiers, ctx)));
        let on_key = variants
            .iter()
            .map(|v| quote!(Self::#v(x) => x.on_key(event, ctx)));

        // Core type paths for the generated impl signatures.
        let window_key = self.core_mod("window", "WindowKey");
        let app_context = self.core_mod("app_context", "AppContext");
        let hitbox_id_ty = self.core_mod("hitbox_registry", "HitboxId");
        let hitbox_registry_ty = self.core_mod("hitbox_registry", "HitboxRegistry");
        let event_ty = self.core_mod("events", "Event");
        let mouse_button_ty = self.core_mod("events", "MouseButton");
        let key_modifiers_ty = self.core_mod("events", "KeyModifiers");
        let mouse_event_kind_ty = self.core_mod("events", "MouseEventKind");
        let ctx_ty = self.core_mod("component_context", "ComponentContext");
        let event_result_ty = self.core_mod("actions", "EventResult");
        let term_wm_action_ty = self.core_mod("actions", "TermWmAction");
        let selection_status_ty = self.core_mod("components", "SelectionStatus");
        let rect_ty = self.core_root("Rect");
        let backend_ty = &self.paths.backend;

        let impl_def = quote! {
            impl<#(#bounds),*> Component<TermWmAction> for #name<#(#tys),*> {
                fn init(&mut self) { match self { #(#init),* } }
                fn on_mount(&mut self, key: #window_key, app: &#app_context) {
                    match self { #(#on_mount),* }
                }
                fn hitbox_id(&self) -> Option<#hitbox_id_ty> {
                    match self { #(#hitbox_id),* }
                }
                fn handle_events(
                    &mut self,
                    event: &#event_ty,
                    ctx: &#ctx_ty,
                ) -> #event_result_ty<#term_wm_action_ty> {
                    match self { #(#handle_events),* }
                }
                fn on_mouse_press(
                    &mut self, col: u16, row: u16, button: #mouse_button_ty,
                    modifiers: #key_modifiers_ty, ctx: &#ctx_ty,
                ) -> #event_result_ty<#term_wm_action_ty> {
                    match self { #(#on_mouse_press),* }
                }
                fn on_mouse_release(
                    &mut self, col: u16, row: u16, button: #mouse_button_ty,
                    modifiers: #key_modifiers_ty, ctx: &#ctx_ty,
                ) -> #event_result_ty<#term_wm_action_ty> {
                    match self { #(#on_mouse_release),* }
                }
                fn on_mouse_drag(
                    &mut self, col: u16, row: u16, button: #mouse_button_ty,
                    modifiers: #key_modifiers_ty, ctx: &#ctx_ty,
                ) -> #event_result_ty<#term_wm_action_ty> {
                    match self { #(#on_mouse_drag),* }
                }
                fn on_mouse_scroll(
                    &mut self, col: u16, row: u16, kind: #mouse_event_kind_ty,
                    modifiers: #key_modifiers_ty, ctx: &#ctx_ty,
                ) -> #event_result_ty<#term_wm_action_ty> {
                    match self { #(#on_mouse_scroll),* }
                }
                fn on_mouse_move(
                    &mut self, col: u16, row: u16, modifiers: #key_modifiers_ty,
                    ctx: &#ctx_ty,
                ) -> #event_result_ty<#term_wm_action_ty> {
                    match self { #(#on_mouse_move),* }
                }
                fn on_key(
                    &mut self, event: &#event_ty, ctx: &#ctx_ty,
                ) -> #event_result_ty<#term_wm_action_ty> {
                    match self { #(#on_key),* }
                }
                fn update(
                    &mut self, action: #term_wm_action_ty,
                    ctx: &#ctx_ty,
                    actions: &mut ::std::collections::VecDeque<(
                        #window_key, #term_wm_action_ty,
                    )>,
                ) {
                    match self { #(#update),* }
                }
                fn render(
                    &mut self, backend: &mut dyn #backend_ty, area: #rect_ty,
                    ctx: &#ctx_ty,
                    registry: &mut #hitbox_registry_ty,
                ) {
                    match self { #(#render),* }
                }
                fn destroy(&mut self) { match self { #(#destroy),* } }
                fn clear_selection(&mut self) { match self { #(#clear_selection),* } }
                fn selection_status(&self) -> #selection_status_ty {
                    match self { #(#selection_status),* }
                }
                fn selection_text(&self) -> Option<String> { match self { #(#selection_text),* } }
                fn desired_height(&self, width: u16) -> u16 { match self { #(#desired_height),* } }
                fn take_pending_title(&mut self) -> Option<String> { match self { #(#take_pending_title),* } }
                fn take_alternate_screen_transition(&mut self) -> Option<bool> { match self { #(#take_alternate),* } }
                fn take_teardown_parts(
                    &mut self,
                ) -> Option<(Box<dyn ::std::any::Any + Send + Sync>, ::std::thread::JoinHandle<()>)> {
                    match self { #(#take_teardown),* }
                }
                fn set_selection_enabled(&mut self, enabled: bool) { match self { #(#set_selection),* } }
                fn paste(&mut self, text: &str) -> bool { match self { #(#paste),* } }
            }
        };

        let constructions = variants
            .iter()
            .zip(exprs)
            .map(|(v, e)| quote!(#name::#v(#e)))
            .collect();
        Ok((quote!(#enum_def #impl_def), constructions))
    }

    fn stack_container(&mut self, el: &Element, kind: ContainerKind) -> syn::Result<TokenStream2> {
        let ctor = match kind {
            ContainerKind::Vertical => self.comp("VerticalStackComponent"),
            ContainerKind::Horizontal => self.comp("HStackComponent"),
        };
        let gap = attr_expr(el, "gap").cloned();
        let (items, adds, has_children) = self.children(el)?;
        self.push_items(items);
        let noop = self.core_mod("components", "NoopComponent");

        let init = match &gap {
            Some(g) => quote!(#ctor::new().with_gap(#g)),
            None => quote!(#ctor::new()),
        };
        if has_children {
            Ok(quote! {{
                let mut __stack = #init;
                #(__stack.add(#adds);)*
                __stack
            }})
        } else {
            let init = match &gap {
                Some(g) => quote!(#ctor::<#noop>::new().with_gap(#g)),
                None => quote!(#ctor::<#noop>::new()),
            };
            Ok(init)
        }
    }

    fn center_container(&mut self, el: &Element) -> syn::Result<TokenStream2> {
        let width = required_attr_expr(el, "width")?;
        let height = required_attr_expr(el, "height")?;
        let children = meaningful(el.children());
        if children.len() != 1 {
            return Err(syn::Error::new_spanned(
                el.name(),
                "<Center> requires exactly one child",
            ));
        }
        let child = self.node(children[0])?;
        let center = self.comp("CenterComponent");
        Ok(quote! {
            #center::new(#child, #width, #height)
        })
    }

    fn grid_container(&mut self, el: &Element) -> syn::Result<TokenStream2> {
        let cols = grid_constraint_attr(&self.paths, el, "cols")?;
        let rows = grid_constraint_attr(&self.paths, el, "rows")?;
        let (items, adds, has_children) = self.children(el)?;
        self.push_items(items);
        let grid = self.comp("GridComponent");
        let noop = self.core_mod("components", "NoopComponent");

        let base = if has_children {
            quote!(#grid::new(vec![#(#adds),*]))
        } else {
            quote!(#grid::<#noop>::new(Vec::new()))
        };
        let base = match cols {
            Some(c) => quote!(#base.with_cols(#c)),
            None => base,
        };
        let base = match rows {
            Some(r) => quote!(#base.with_rows(#r)),
            None => base,
        };
        Ok(base)
    }
}

enum ContainerKind {
    Vertical,
    Horizontal,
}

/// Parse a `<Grid cols="200px 1fr">` attribute string literal into
/// `vec![GridConstraint::Fixed(..), GridConstraint::Fraction(..)]` at compile
/// time.
fn grid_constraint_attr(
    paths: &Paths,
    el: &Element,
    name: &str,
) -> syn::Result<Option<TokenStream2>> {
    let Some(attr) = el.attributes().iter().find(
        |a| matches!(a, NodeAttribute::Attribute(kv) if attr_key(&kv.key).as_deref() == Some(name)),
    ) else {
        return Ok(None);
    };
    let NodeAttribute::Attribute(kv) = attr else {
        return Ok(None);
    };
    let value = kv.value().ok_or_else(|| {
        syn::Error::new_spanned(
            kv,
            format!("<Grid> `{name}` must be a string like \"200px 1fr\""),
        )
    })?;
    let s = match value {
        Expr::Lit(lit) => match &lit.lit {
            Lit::Str(s) => s.value(),
            _ => {
                return Err(syn::Error::new_spanned(
                    value,
                    format!("<Grid> `{name}` must be a string literal like \"200px 1fr\""),
                ));
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                value,
                format!("<Grid> `{name}` must be a string literal like \"200px 1fr\""),
            ));
        }
    };
    let constraints =
        parse_grid_constraints(paths, &s).map_err(|msg| syn::Error::new_spanned(value, msg))?;
    Ok(Some(quote!(vec![#(#constraints),*])))
}

/// Parse `"200px 1fr"` into `GridConstraint` construction expressions.
fn parse_grid_constraints(paths: &Paths, s: &str) -> Result<Vec<TokenStream2>, String> {
    let constraint = |name: &str, n: u16| {
        let ident = format_ident!("{name}");
        let prefix = &paths.comp;
        quote!(#prefix GridConstraint::#ident(#n))
    };
    s.split_whitespace()
        .map(|tok| {
            if let Some(px) = tok.strip_suffix("px") {
                let n: u16 = px
                    .parse()
                    .map_err(|_| format!("invalid fixed size '{tok}' (expected like '200px')"))?;
                Ok(constraint("Fixed", n))
            } else if let Some(fr) = tok.strip_suffix("fr") {
                let n: u16 = fr
                    .parse()
                    .map_err(|_| format!("invalid fraction '{tok}' (expected like '1fr')"))?;
                Ok(constraint("Fraction", n))
            } else if let Ok(n) = tok.parse::<u16>() {
                // A bare number is shorthand for a fixed (`px`) size.
                Ok(constraint("Fixed", n))
            } else {
                Err(format!(
                    "invalid grid constraint '{tok}' (expected 'Npx', 'N' or 'Nfr')"
                ))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_constraints_parse_fixed_and_fraction() {
        let paths = Paths {
            comp: quote!(::term_wm::),
            core: quote!(::term_wm::),
            backend: quote!(::term_wm::RenderBackend),
        };
        let toks = parse_grid_constraints(&paths, "200px 1fr").unwrap();
        assert_eq!(toks.len(), 2);
    }

    #[test]
    fn grid_constraints_reject_bad_token() {
        let paths = Paths {
            comp: quote!(::term_wm::),
            core: quote!(::term_wm::),
            backend: quote!(::term_wm::RenderBackend),
        };
        assert!(parse_grid_constraints(&paths, "200px nonsense").is_err());
        assert!(parse_grid_constraints(&paths, "").unwrap().is_empty());
    }
}
