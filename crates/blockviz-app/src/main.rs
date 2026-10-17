//! blockviz as a native app: a folder on disk analyzed in process and drawn
//! with GPUI. For now a spike: it opens one folder, drag pans and scrolling
//! zooms at the pointer.
//!
//! `blockviz-app [folder] [--svg out.svg]`: with `--svg`, the whole view is
//! written as SVG, drawn as the window would draw it, and no window opens.

mod folder;
mod paint;
mod theme;

use blockviz_core::ir::Scene;
use gpui::{
    canvas, div, prelude::*, px, size, App, Application, Bounds, Context, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, Point, ScrollWheelEvent, SharedString, Window, WindowBounds, WindowOptions,
};
use paint::{Fonts, Walk, Xf};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

struct State {
    scene: Scene,
    titles: Vec<SharedString>,
    theme: theme::Theme,
    fonts: Fonts,
    /// The root diagram's transform; fitted to the window on the first frame.
    xf: Option<Xf>,
    drag: Option<Point<Pixels>>,
}

struct Viewer {
    state: Rc<RefCell<State>>,
}

impl Viewer {
    fn scroll(&mut self, e: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let mut st = self.state.borrow_mut();
        let Some(xf) = st.xf.as_mut() else { return };
        let d = e.delta.pixel_delta(px(16.0));
        let factor = (-f64::from(d.y) * 0.004).exp();
        let (x, y) = (f64::from(e.position.x), f64::from(e.position.y));
        *xf = Xf { x: x - (x - xf.x) * factor, y: y - (y - xf.y) * factor, s: xf.s * factor };
        cx.notify();
    }

    fn down(&mut self, e: &MouseDownEvent, _: &mut Window, _: &mut Context<Self>) {
        self.state.borrow_mut().drag = Some(e.position);
    }

    fn moved(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let mut st = self.state.borrow_mut();
        let Some(from) = st.drag else { return };
        if !e.dragging() {
            st.drag = None;
            return;
        }
        if let Some(xf) = st.xf.as_mut() {
            xf.x += f64::from(e.position.x - from.x);
            xf.y += f64::from(e.position.y - from.y);
        }
        st.drag = Some(e.position);
        cx.notify();
    }

    fn up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.state.borrow_mut().drag = None;
    }
}

impl Render for Viewer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.clone();
        div()
            .size_full()
            .on_scroll_wheel(cx.listener(Self::scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::down))
            .on_mouse_move(cx.listener(Self::moved))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::up))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds: Bounds<Pixels>, (), window, cx| {
                        let st = &mut *state.borrow_mut();
                        let (width, height) = (f64::from(bounds.size.width), f64::from(bounds.size.height));
                        let xf = *st.xf.get_or_insert_with(|| paint::fit(&st.scene, width, height));
                        window.paint_quad(gpui::fill(bounds, st.theme.background));
                        let mut walk = Walk { scene: &st.scene, titles: &st.titles, theme: &st.theme, width, height, out: Vec::with_capacity(4096) };
                        walk.root(xf);
                        paint::emit(&walk.out, &st.fonts, window, cx);
                    },
                )
                .size_full(),
            )
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let svg = args.iter().position(|a| a == "--svg").and_then(|i| args.get(i + 1)).cloned();
    let folder = args.iter().find(|a| !a.starts_with("--") && Some(*a) != svg.as_ref()).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let folder = folder.canonicalize().unwrap_or(folder);

    let reading = Instant::now();
    let files = folder::read(&folder);
    let read = reading.elapsed();
    let name = folder.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let analyzing = Instant::now();
    let scene = blockviz_core::analyze(&name, files);
    let analyzed = analyzing.elapsed();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    println!("{} — {} files, {} lines, {} nodes", scene.title, scene.stats.files, scene.stats.lines, scene.stats.nodes);
    println!("{:>9.1} ms  read files", ms(read));
    for (stage, d) in blockviz_core::timing::take() {
        println!("{:>9.1} ms  {stage}", ms(d));
    }
    println!("{:>9.1} ms  analysis in all", ms(analyzed));
    let titles: Vec<SharedString> = scene.nodes.iter().map(|n| SharedString::from(n.title.clone())).collect();

    if let Some(out) = svg {
        let (width, height) = (1440.0, 900.0);
        let theme = theme::light();
        let mut walk = Walk { scene: &scene, titles: &titles, theme: &theme, width, height, out: vec![] };
        walk.root(paint::fit(&scene, width, height));
        std::fs::write(&out, paint::svg(&walk.out, width, height)).unwrap();
        println!("wrote {out}");
        return;
    }

    Application::new().run(move |cx: &mut App| {
        let state = Rc::new(RefCell::new(State { scene, titles, theme: theme::light(), fonts: Fonts::new(), xf: None, drag: None }));
        let bounds = Bounds::centered(None, size(px(1440.0), px(900.0)), cx);
        let options = WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() };
        cx.open_window(options, |_, cx| cx.new(|_| Viewer { state })).unwrap();
        cx.activate(true);
    });
}
