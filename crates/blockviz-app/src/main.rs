//! blockviz as a native app: a folder on disk analyzed in process and drawn
//! with GPUI. Drag pans, scrolling zooms at the pointer, a click selects, a
//! double click (or Enter) opens; F fits the block the view is in, H or 0 shows
//! the whole project. Breadcrumbs name the blocks the view is in, and a
//! panel shows the selected block's details and source. `/` or ⌘K searches.
//! The diagram follows the folder: saving a file analyzes the project again.
//!
//! `blockviz-app [folder] [--svg out.svg]`: with `--svg`, the whole view is
//! written as SVG, drawn as the window would draw it, and no window opens.

mod camera;
mod code;
mod crumbs;
mod input;
mod live;
mod model;
mod paint;
mod panel;
mod theme;

use camera::{Camera, Framing, Plan};
use gpui::{
    canvas, div, prelude::*, px, size, App, Application, Bounds, Context, FocusHandle, Focusable, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollWheelEvent, Window, WindowBounds, WindowOptions,
};
use model::Model;
use paint::{Fonts, Hit, Walk};
use panel::Inspector;
use std::rc::Rc;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// A press that may still be a click: where it started, and whether it has moved.
struct Press {
    at: Point<Pixels>,
    last: Point<Pixels>,
    moved: bool,
}

pub struct Viewer {
    m: Rc<Model>,
    camera: Camera,
    theme: theme::Theme,
    fonts: Fonts,
    focus: FocusHandle,
    /// The canvas size as of the last frame (0 until the first).
    width: f64,
    height: f64,
    /// Blocks drawn in the last frame, for clicks.
    hits: Vec<Hit>,
    selected: Option<u32>,
    /// The instance path the selection was made through (the call it was reached by).
    selected_path: Option<Vec<u32>>,
    press: Option<Press>,
    /// A camera move under way, and when it started.
    flight: Option<(Instant, Plan)>,
    ui: theme::Ui,
    /// The blocks the view is in, root first, as of the last frame.
    crumbs: Vec<u32>,
    /// The menu of folded breadcrumbs is open.
    crumb_menu: bool,
    inspector: gpui::Entity<Inspector>,
    search: Option<Search>,
}

/// The details panel's width, and how far it is from the window's edges.
const PANEL_W: f64 = 380.0;

/// The search palette while it is open: its field, the results for what is
/// in it, and the one Enter picks.
struct Search {
    input: gpui::Entity<input::TextInput>,
    query: String,
    results: Vec<u32>,
    active: usize,
    scroll: gpui::ScrollHandle,
    _edits: gpui::Subscription,
}

impl Viewer {
    /// Takes in new analyses as the thread watching the folder makes them.
    fn listen(&mut self, mut updates: futures::channel::mpsc::UnboundedReceiver<live::Update>, cx: &mut Context<Self>) {
        use futures::StreamExt;
        cx.spawn(async move |this, cx| {
            while let Some(u) = updates.next().await {
                if this.update(cx, |v, cx| v.replace(u, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Shows a new analysis of the project.
    fn replace(&mut self, u: live::Update, cx: &mut Context<Self>) {
        let files = if u.changed.len() <= 3 { u.changed.join(", ") } else { format!("{} files", u.changed.len()) };
        println!("{files} changed: analyzed again in {:.0} ms, {} files' parse reused", u.took.as_secs_f64() * 1000.0, u.reused);
        let m = Rc::new(u.model);
        self.m = m.clone();
        self.inspector.update(cx, |i, cx| i.set_model(m, cx));
        // Ids are the new scene's: start again from the whole project.
        self.selected = None;
        self.selected_path = None;
        self.search = None;
        self.flight = None;
        self.hits.clear();
        self.crumbs.clear();
        self.camera.path = vec![self.m.root()];
        self.camera.insets.right = 12.0;
        self.width = 0.0;
        cx.notify();
    }

    fn new(m: Rc<Model>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
        let camera = Camera::new(&m);
        let me = cx.entity().downgrade();
        let inspector = cx.new(|_| Inspector::new(m.clone(), me));
        Viewer {
            m,
            camera,
            theme: theme::light(),
            fonts: Fonts::new(),
            focus,
            width: 0.0,
            height: 0.0,
            hits: vec![],
            selected: None,
            selected_path: None,
            press: None,
            flight: None,
            ui: theme::ui(),
            crumbs: vec![],
            crumb_menu: false,
            inspector,
            search: None,
        }
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        self.camera.reanchor(&self.m, self.width, self.height);
        cx.notify();
    }

    /// Flies to the node at the end of an instance path: `Open` to read it, `Fit` to see all of it.
    fn fly_to(&mut self, path: &[u32], how: Framing, pad: f64, cx: &mut Context<Self>) {
        let plan = self.camera.flight(&self.m, path, self.width, self.height, how, pad);
        self.flight = Some((Instant::now(), plan));
        cx.notify();
    }

    pub(crate) fn select(&mut self, id: Option<u32>, path: Option<Vec<u32>>, cx: &mut Context<Self>) {
        self.selected = id;
        self.selected_path = path;
        // Framing keeps blocks clear of the panel while it is open.
        self.camera.insets.right = if id.is_some() { PANEL_W + 24.0 } else { 12.0 };
        self.inspector.update(cx, |i, cx| i.show(id, cx));
        cx.notify();
    }

    /// Selects a block and flies to where it is defined (a link in the panel).
    pub(crate) fn go_to(&mut self, id: u32, cx: &mut Context<Self>) {
        let path = self.m.canonical_path(id);
        self.select(Some(id), Some(path.clone()), cx);
        self.fly_to(&path, Framing::Open, 0.08, cx);
    }

    /// Selects the block drawn for a line of source, staying where the reader is.
    pub(crate) fn pick(&mut self, id: u32, cx: &mut Context<Self>) {
        let near = self.selected_path.clone().unwrap_or_else(|| self.crumbs.clone());
        let path = self.m.instance_path(id, &near);
        self.select(path.last().copied(), Some(path.clone()), cx);
        self.reveal(&path, cx);
    }

    /// Brings a block into view unless it is already on screen and legible.
    fn reveal(&mut self, path: &[u32], cx: &mut Context<Self>) {
        let id = *path.last().unwrap();
        let i = self.camera.insets;
        let seen = self.hits.iter().any(|h| {
            let r = h.rect;
            h.id == id && h.clear && r.x >= i.left && r.y >= i.top && r.x + r.w <= self.width - i.right && r.y + r.h <= self.height - i.bottom
        });
        if !seen {
            self.fly_to(path, Framing::Open, 0.08, cx);
        }
    }

    /// Opens the selected block (the panel's Zoom in, or Enter).
    pub(crate) fn open_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.selected_path.clone() {
            self.fly_to(&path, Framing::Open, 0.08, cx);
        }
    }

    /// Frames all of the block the view is in.
    fn fit(&mut self, cx: &mut Context<Self>) {
        let path = self.camera.focus(&self.m, self.width, self.height);
        if path.len() <= 1 {
            self.home(cx);
        } else {
            self.fly_to(&path, Framing::Fit, 0.04, cx);
        }
    }

    fn home(&mut self, cx: &mut Context<Self>) {
        self.fly_to(&[self.m.root()], Framing::Fit, 0.05, cx);
    }

    /// Opens the block at a point (a double click): zooms into it, or goes to its definition.
    fn open(&mut self, x: f64, y: f64, cx: &mut Context<Self>) {
        let i = paint::hit_at(&self.hits, x, y, true);
        if i < 0 {
            self.flight = None;
            self.camera.zoom(&self.m, 2.0, x, y);
            self.changed(cx);
            return;
        }
        let n = self.m.node(self.hits[i as usize].id);
        // A block with nothing inside but a definition elsewhere goes there.
        match n.link {
            Some(link) if self.m.owner(n).is_none() && link != n.id => {
                let path = self.m.canonical_path(link);
                self.select(Some(link), Some(path.clone()), cx);
                self.fly_to(&path, Framing::Open, 0.08, cx);
            }
            _ => {
                let path = paint::path_of(&self.hits, self.m.root(), i);
                self.fly_to(&path, Framing::Open, 0.08, cx);
            }
        }
    }

    fn scroll(&mut self, e: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.flight = None;
        let d = e.delta.pixel_delta(px(16.0));
        // Scrolling zooms, as on the web: AppKit's deltas are the opposite way round to a browser's.
        let factor = (f64::from(d.y).clamp(-160.0, 160.0) * 0.0022).exp();
        self.camera.zoom(&self.m, factor, f64::from(e.position.x), f64::from(e.position.y));
        self.changed(cx);
    }

    fn down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        self.flight = None;
        if e.click_count == 2 {
            self.press = None;
            self.open(f64::from(e.position.x), f64::from(e.position.y), cx);
            return;
        }
        self.press = Some(Press { at: e.position, last: e.position, moved: false });
    }

    fn moved(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.press.as_mut() else { return };
        if !e.dragging() {
            self.press = None;
            return;
        }
        let (dx, dy) = (f64::from(e.position.x - p.last.x), f64::from(e.position.y - p.last.y));
        p.last = e.position;
        if f64::from(e.position.x - p.at.x).hypot(f64::from(e.position.y - p.at.y)) > 4.0 {
            p.moved = true;
        }
        self.camera.pan(dx, dy);
        self.changed(cx);
    }

    fn up(&mut self, e: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.press.take() else { return };
        if !p.moved {
            let i = paint::hit_at(&self.hits, f64::from(e.position.x), f64::from(e.position.y), false);
            let path = (i >= 0).then(|| paint::path_of(&self.hits, self.m.root(), i));
            self.select(path.as_ref().and_then(|p| p.last().copied()), path, cx);
        }
    }

    fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ui = theme::ui();
        let input = cx.new(|cx| input::TextInput::new("Jump to a file, class, function or block…", &ui, cx));
        let edits = cx.observe(&input, |v, input, cx| {
            let query = input.read(cx).content.to_string();
            if let Some(s) = v.search.as_mut() {
                if s.query != query {
                    s.results = v.m.find(&query);
                    s.query = query;
                    s.active = 0;
                    s.scroll.scroll_to_item(0);
                    cx.notify();
                }
            }
        });
        window.focus(&input.focus_handle(cx));
        self.search = Some(Search { input, query: String::new(), results: self.m.find(""), active: 0, scroll: gpui::ScrollHandle::new(), _edits: edits });
        cx.notify();
    }

    fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search = None;
        window.focus(&self.focus);
        cx.notify();
    }

    /// Picks a search result: flies to it and selects it.
    fn pick_result(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(&id) = self.search.as_ref().and_then(|s| s.results.get(i)) else { return };
        self.close_search(window, cx);
        self.go_to(id, cx);
    }

    fn search_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.search.as_mut() else { return };
        let k = &e.keystroke;
        if k.key == "k" && (k.modifiers.platform || k.modifiers.control) {
            cx.stop_propagation();
            return self.close_search(window, cx);
        }
        match k.key.as_str() {
            "down" => s.active = (s.active + 1).min(s.results.len().saturating_sub(1)),
            "up" => s.active = s.active.saturating_sub(1),
            "enter" => {
                let i = s.active;
                cx.stop_propagation();
                return self.pick_result(i, window, cx);
            }
            "escape" => {
                cx.stop_propagation();
                return self.close_search(window, cx);
            }
            _ => return,
        }
        s.scroll.scroll_to_item(s.active);
        cx.stop_propagation();
        cx.notify();
    }

    fn key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        if self.search.is_some() {
            return;
        }
        if k.key == "k" && (k.modifiers.platform || k.modifiers.control) || k.key == "/" && !k.modifiers.platform {
            // Handled here, the key is not typed into the field it opens.
            cx.stop_propagation();
            return self.open_search(window, cx);
        }
        if k.modifiers.platform || k.modifiers.control || k.modifiers.alt {
            return;
        }
        match k.key.as_str() {
            "escape" => self.select(None, None, cx),
            "enter" => self.open_selected(cx),
            "f" => self.fit(cx),
            "h" | "0" => self.home(cx),
            _ => {}
        }
    }
}

/// Height of the bar over the diagram's top, kept clear when framing.
const TOP_BAR: f64 = 48.0;

impl Render for Viewer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.camera.insets.top = TOP_BAR;
        let view = cx.entity();
        div()
            .relative()
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .child(
                div()
                    .absolute()
                    .size_full()
                    .on_scroll_wheel(cx.listener(Self::scroll))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::down))
                    .on_mouse_move(cx.listener(Self::moved))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::up))
                    .child(
                        canvas(
                            |_, _, _| (),
                            move |bounds: Bounds<Pixels>, (), window, cx| {
                                view.update(cx, |v, cx| v.paint(bounds, window, cx));
                            },
                        )
                        .size_full(),
                    ),
            )
            .child(div().absolute().top(px(12.0)).left(px(12.0)).child(self.breadcrumbs(cx)))
            .when_some(self.search_palette(cx), |d, p| d.child(p))
            .when(self.selected.is_some(), |d| {
                d.child(
                    div()
                        .absolute()
                        .top(px(60.0))
                        .right(px(12.0))
                        .bottom(px(64.0))
                        .w(px(PANEL_W as f32))
                        .debug_selector(|| "inspector".into())
                        .child(gpui::AnyView::from(self.inspector.clone()).cached(gpui::StyleRefinement::default().size_full())),
                )
            })
    }
}

impl Viewer {
    fn search_palette(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let s = self.search.as_ref()?;
        let ui = &self.ui;
        let rows = s.results.iter().enumerate().map(|(i, &id)| {
            let n = self.m.node(id);
            div()
                .id(("result", i))
                .flex()
                .flex_col()
                .gap(px(1.0))
                .px(px(10.0))
                .py(px(7.0))
                .rounded(px(8.0))
                .cursor_pointer()
                .when(i == s.active, |d| d.bg(ui.accent_bg))
                .on_hover(cx.listener(move |v, hovered: &bool, _, cx| {
                    if let Some(s) = v.search.as_mut().filter(|_| *hovered) {
                        s.active = i;
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(move |v, _, window, cx| v.pick_result(i, window, cx)))
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(8.0))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child(n.title.clone())
                        .when(!n.subtitle.is_empty(), |d| d.child(div().font_weight(gpui::FontWeight::NORMAL).text_size(px(12.0)).text_color(ui.faint).child(n.subtitle.clone()))),
                )
                .child(div().text_size(px(11.5)).text_color(ui.faint).truncate().child(self.m.trail(id)))
        });
        let list = div()
            .id("search-results")
            .max_h(px((self.height * 0.52) as f32))
            .overflow_y_scroll()
            .track_scroll(&s.scroll)
            .p(px(6.0))
            .when(s.results.is_empty(), |d| d.child(div().p(px(18.0)).text_color(ui.faint).flex().justify_center().child("No matches")))
            .children(rows);
        let palette = div()
            .w(px((self.width - 32.0).min(640.0) as f32))
            .bg(ui.panel)
            .border_1()
            .border_color(ui.line)
            .rounded(px(14.0))
            .shadow_lg()
            .overflow_hidden()
            .debug_selector(|| "search".into())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_key_down(cx.listener(Self::search_key))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .px(px(14.0))
                    .py(px(12.0))
                    .border_b_1()
                    .border_color(ui.line)
                    .text_size(px(15.0))
                    .text_color(ui.ink)
                    .child(s.input.clone())
                    .child(
                        div()
                            .font_family(theme::MONO_FONT)
                            .text_size(px(11.0))
                            .text_color(ui.faint)
                            .border_1()
                            .border_color(ui.line)
                            .rounded(px(4.0))
                            .px(px(5.0))
                            .py(px(1.0))
                            .bg(ui.code_bg)
                            .child("esc"),
                    ),
            )
            .child(list);
        Some(
            div()
                .absolute()
                .size_full()
                .bg(ui.scrim)
                .flex()
                .justify_center()
                .items_start()
                .pt(px((self.height * 0.12) as f32))
                .font_family(theme::UI_FONT)
                .text_size(px(13.0))
                .text_color(ui.ink)
                .occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|v, _, window, cx| v.close_search(window, cx)))
                .child(palette)
                .into_any_element(),
        )
    }

    /// Flies out to fit crumb `i`.
    fn go_crumb(&mut self, i: usize, cx: &mut Context<Self>) {
        self.crumb_menu = false;
        let path = if i >= 1 { self.crumbs[..=i].to_vec() } else { vec![self.m.root()] };
        self.fly_to(&path, Framing::Fit, 0.06, cx);
    }

    fn breadcrumbs(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let path: Vec<usize> = (0..self.crumbs.len()).filter(|&i| !self.m.node(self.crumbs[i]).title.is_empty()).collect();
        let titles: Vec<gpui::SharedString> = path.iter().map(|&i| self.m.titles[self.crumbs[i] as usize].clone()).collect();
        let ui = &self.ui;
        if titles.len() <= 1 {
            return crumbs::bar(ui).child(div().px(px(7.0)).py(px(4.0)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(ui.ink).child(self.m.scene.title.clone())).into_any_element();
        }
        let room = self.width - 24.0 - self.camera.insets.right;
        let (shown, hidden) = crumbs::fold(&titles, room);
        let go = |k: usize| {
            let this = cx.entity();
            let i = path[k];
            move |_: usize, _: &mut Window, cx: &mut App| this.update(cx, |v, cx| v.go_crumb(i, cx))
        };
        let mut bar = crumbs::bar(ui).debug_selector(|| "crumbs".into());
        for (j, c) in shown.iter().enumerate() {
            if j > 0 {
                bar = bar.child(crumbs::chevron(ui));
            }
            match *c {
                Some(k) => bar = bar.child(crumbs::crumb(ui, k, titles[k].clone(), k == titles.len() - 1, go(k))),
                None => {
                    let mut fold = div().relative().child(
                        div()
                            .id("crumb-fold")
                            .px(px(7.0))
                            .py(px(4.0))
                            .rounded(px(6.0))
                            .cursor_pointer()
                            .text_color(ui.muted)
                            .hover(|s| s.bg(ui.hover))
                            .on_click(cx.listener(|v, _, _, cx| {
                                v.crumb_menu = !v.crumb_menu;
                                cx.notify();
                            }))
                            .child("…"),
                    );
                    if self.crumb_menu {
                        let mut menu = div()
                            .absolute()
                            .top(px(30.0))
                            .left_0()
                            .flex()
                            .flex_col()
                            .min_w(px(180.0))
                            .max_w(px(360.0))
                            .p(px(4.0))
                            .bg(ui.panel)
                            .border_1()
                            .border_color(ui.line)
                            .rounded(px(10.0))
                            .shadow_md()
                            .occlude();
                        for &k in &hidden {
                            menu = menu.child(crumbs::crumb(ui, k, titles[k].clone(), false, go(k)));
                        }
                        fold = fold.child(menu);
                    }
                    bar = bar.child(fold);
                }
            }
        }
        bar.into_any_element()
    }
}

impl Viewer {
    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let (width, height) = (f64::from(bounds.size.width), f64::from(bounds.size.height));
        if self.width == 0.0 {
            self.camera.fit_root(&self.m, width, height);
        } else if (width, height) != (self.width, self.height) {
            // A resized window keeps what was at its center there.
            self.camera.pan((width - self.width) / 2.0, (height - self.height) / 2.0);
            self.camera.reanchor(&self.m, width, height);
        }
        (self.width, self.height) = (width, height);
        if let Some((start, plan)) = &self.flight {
            let t = (start.elapsed().as_secs_f64() * 1000.0 / plan.duration).clamp(0.0, 1.0);
            self.camera.step(&self.m, plan, camera::ease(t), width, height);
            if t >= 1.0 {
                self.flight = None;
            } else {
                window.request_animation_frame();
            }
        }
        window.paint_quad(gpui::fill(bounds, self.theme.background));
        let xfs = self.camera.transforms(&self.m);
        let mut walk = Walk::new(&self.m, &self.theme, width, height, &self.camera.path, &xfs);
        walk.selected = self.selected;
        walk.draw();
        paint::emit(&walk.out, &self.fonts, window, cx);
        self.hits = walk.hits;
        // The breadcrumbs follow the view; they are drawn again only when it is somewhere else.
        let focus = self.camera.focus(&self.m, width, height);
        if focus != self.crumbs {
            self.crumbs = focus;
            self.crumb_menu = false;
            cx.notify();
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let svg = args.iter().position(|a| a == "--svg").and_then(|i| args.get(i + 1)).cloned();
    let folder = args.iter().find(|a| !a.starts_with("--") && Some(*a) != svg.as_ref()).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let folder = folder.canonicalize().unwrap_or(folder);

    let reading = Instant::now();
    let files = live::read(&folder);
    let read = reading.elapsed();
    let name = folder.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let analyzing = Instant::now();
    let mut cache = blockviz_core::project::Cache::default();
    let scene = blockviz_core::project::build_cached(&name, live::inputs(&files), blockviz_core::layout::Options::default(), &mut cache);
    let analyzed = analyzing.elapsed();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    println!("{} — {} files, {} lines, {} nodes", scene.title, scene.stats.files, scene.stats.lines, scene.stats.nodes);
    println!("{:>9.1} ms  read files", ms(read));
    for (stage, d) in blockviz_core::timing::take() {
        println!("{:>9.1} ms  {stage}", ms(d));
    }
    println!("{:>9.1} ms  analysis in all", ms(analyzed));
    let m = Rc::new(Model::new(scene, live::texts(&files)));

    if let Some(out) = svg {
        let (width, height) = (1440.0, 900.0);
        let theme = theme::light();
        let mut camera = Camera::new(&m);
        camera.fit_root(&m, width, height);
        let xfs = camera.transforms(&m);
        let mut walk = Walk::new(&m, &theme, width, height, &camera.path, &xfs);
        walk.draw();
        std::fs::write(&out, paint::svg(&walk.out, width, height)).unwrap();
        println!("wrote {out}");
        return;
    }

    Application::new().run(move |cx: &mut App| {
        input::bind_keys(cx);
        let bounds = Bounds::centered(None, size(px(1440.0), px(900.0)), cx);
        let options = WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() };
        let (send, updates) = futures::channel::mpsc::unbounded();
        live::watch(folder, name, files, cache, move |u| send.unbounded_send(u).is_ok());
        cx.open_window(options, |window, cx| {
            cx.new(|cx| {
                let mut v = Viewer::new(m, window, cx);
                v.listen(updates, cx);
                v
            })
        })
        .unwrap();
        cx.activate(true);
    });
}

#[cfg(test)]
mod ui_tests {
    use super::*;
    use gpui::{point, Modifiers, TestAppContext, VisualTestContext};

    const MODEL_PY: &str = "class Block:\n    def __init__(self):\n        self.attn = Attention()\n\n    def forward(self, x):\n        x = x + self.attn(x)\n        x = x + self.mlp(x)\n        return x\n\n\nclass Attention:\n    def __call__(self, x):\n        return x\n";

    fn open(cx: &mut TestAppContext) -> (gpui::Entity<Viewer>, &mut VisualTestContext) {
        let files = [("model.py", MODEL_PY), ("train.py", "from model import Block\nb = Block()\nb.forward(1)\n")];
        let inputs = files.iter().map(|(p, t)| blockviz_core::project::InFile { path: p.to_string(), text: Some(t.to_string()), bytes: t.len() as u64 }).collect();
        let m = Rc::new(Model::new(blockviz_core::analyze("t", inputs), files.iter().map(|(p, t)| (p.to_string(), t.to_string())).collect()));
        cx.update(input::bind_keys);
        let (view, cx) = cx.add_window_view(|window, cx| Viewer::new(m, window, cx));
        frame(cx);
        (view, cx)
    }

    /// Draws the window as the next frame would (the test platform draws only on input),
    /// twice: a frame can ask for another, as the breadcrumbs do when the view moved.
    fn frame(cx: &mut VisualTestContext) {
        for _ in 0..2 {
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear());
        }
        cx.run_until_parked();
    }

    fn titled(v: &Viewer, title: &str) -> u32 {
        v.m.scene.nodes.iter().find(|n| n.title == title).unwrap().id
    }

    fn center(b: gpui::Bounds<Pixels>) -> Point<Pixels> {
        point(b.origin.x + b.size.width / 2.0, b.origin.y + b.size.height / 2.0)
    }

    /// Finishes the flight under way, as its last frame would.
    fn land(view: &gpui::Entity<Viewer>, cx: &mut VisualTestContext) {
        view.update(cx, |v, cx| {
            if let Some((_, plan)) = v.flight.take() {
                let (w, h) = (v.width, v.height);
                v.camera.step(&v.m, &plan, 1.0, w, h);
                cx.notify();
            }
        });
        frame(cx);
    }

    #[gpui::test]
    fn a_click_opens_the_panel_and_a_line_of_source_selects_its_block(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        assert!(cx.debug_bounds("inspector").is_none());
        // Click the middle of model.py's box.
        let (file, at) = view.read_with(cx, |v, _| {
            let file = titled(v, "model.py");
            let r = v.hits.iter().find(|h| h.id == file).unwrap().rect;
            (file, point(px((r.x + r.w / 2.0) as f32), px((r.y + 6.0) as f32)))
        });
        cx.simulate_click(at, Modifiers::none());
        frame(cx);
        assert_eq!(view.read_with(cx, |v, _| v.selected), Some(file));
        let panel = cx.debug_bounds("inspector").expect("the panel is open");
        assert_eq!(panel.size.width, px(PANEL_W as f32));

        // A step is shown in its function, its own line marked; clicking that line selects the block drawn for it.
        let step = view.update(cx, |v, cx| {
            let forward = titled(v, "forward");
            let step = v.m.block_at(forward, 6);
            assert_ne!(step, forward);
            let path = v.m.canonical_path(step);
            v.select(Some(step), Some(path), cx);
            step
        });
        frame(cx);
        let line = cx.debug_bounds("marked-line").expect("its line is marked");
        assert!(panel.contains(&line.origin), "{line:?} in {panel:?}");
        view.update(cx, |v, cx| v.select(None, None, cx));
        frame(cx);
        // (GPUI keeps elements' debug bounds from earlier frames, so a closed panel is checked by state.)
        assert_eq!(view.read_with(cx, |v, _| v.selected), None);
        view.update(cx, |v, cx| {
            let path = v.m.canonical_path(step);
            v.select(Some(step), Some(path), cx);
        });
        frame(cx);
        let line = cx.debug_bounds("marked-line").unwrap();
        cx.simulate_click(center(line), Modifiers::none());
        frame(cx);
        assert_eq!(view.read_with(cx, |v, _| v.selected), Some(step));

        // Its close button closes it, and so does Esc.
        let close = cx.debug_bounds("panel-close").expect("a close button");
        cx.simulate_click(center(close), Modifiers::none());
        frame(cx);
        assert_eq!(view.read_with(cx, |v, _| v.selected), None);
        view.update(cx, |v, cx| v.select(Some(step), None, cx));
        frame(cx);
        cx.simulate_keystrokes("escape");
        frame(cx);
        assert_eq!(view.read_with(cx, |v, _| v.selected), None);
    }

    #[gpui::test]
    fn breadcrumbs_follow_a_flight_and_take_the_view_back_out(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        assert!(cx.debug_bounds("crumbs").is_none(), "the whole project shows only its name");
        view.update(cx, |v, cx| {
            let path = v.m.canonical_path(titled(v, "forward"));
            v.fly_to(&path, Framing::Open, 0.08, cx);
        });
        land(&view, cx);
        let crumbs = view.read_with(cx, |v, _| v.crumbs.iter().map(|&c| v.m.node(c).title.clone()).collect::<Vec<_>>());
        assert_eq!(crumbs.last().map(String::as_str), Some("forward"), "{crumbs:?}");
        assert!(crumbs.iter().any(|c| c == "model.py"));
        let root = cx.debug_bounds("crumb-root").expect("crumbs are shown");
        cx.simulate_click(center(root), Modifiers::none());
        frame(cx);
        land(&view, cx);
        assert_eq!(view.read_with(cx, |v, _| v.camera.path.clone()), [view.read_with(cx, |v, _| v.m.root())]);
        // Keys: H goes home from anywhere, F fits the block the view is in.
        cx.simulate_keystrokes("f");
        frame(cx);
        assert!(view.read_with(cx, |v, _| v.flight.is_some()));
    }

    #[gpui::test]
    fn search_finds_a_method_by_its_qualified_name_and_flies_there(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        cx.simulate_keystrokes("/");
        frame(cx);
        assert!(cx.debug_bounds("search").is_some());
        // The key that opened the palette is not typed into it.
        assert_eq!(view.read_with(cx, |v, cx| v.search.as_ref().unwrap().input.read(cx).content.to_string()), "");
        cx.simulate_input("Block.forward");
        frame(cx);
        let forward = view.read_with(cx, |v, _| titled(v, "forward"));
        assert_eq!(view.read_with(cx, |v, _| v.search.as_ref().unwrap().results.first().copied()), Some(forward));
        // Typing never runs the diagram's keys (F fits, H goes home).
        assert!(view.read_with(cx, |v, _| v.flight.is_none()));
        cx.simulate_keystrokes("down up enter");
        frame(cx);
        view.read_with(cx, |v, _| {
            assert!(v.search.is_none());
            assert_eq!(v.selected, Some(forward));
            assert!(v.flight.is_some());
        });
        // ⌘K opens it and closes it again; so does Esc.
        cx.simulate_keystrokes("cmd-k");
        frame(cx);
        assert!(view.read_with(cx, |v, _| v.search.is_some()));
        cx.simulate_keystrokes("cmd-k");
        frame(cx);
        assert!(view.read_with(cx, |v, _| v.search.is_none()));
        cx.simulate_keystrokes("/");
        frame(cx);
        cx.simulate_keystrokes("escape");
        frame(cx);
        assert!(view.read_with(cx, |v, _| v.search.is_none()));
    }
}
