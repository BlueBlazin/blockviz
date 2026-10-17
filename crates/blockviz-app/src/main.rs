//! blockviz as a native app: a folder on disk analyzed in process and drawn
//! with GPUI. Drag pans, scrolling zooms at the pointer, a click selects, a
//! double click (or Enter) opens; F fits the block the view is in, H or 0 shows
//! the whole project.
//!
//! `blockviz-app [folder] [--svg out.svg]`: with `--svg`, the whole view is
//! written as SVG, drawn as the window would draw it, and no window opens.

mod camera;
mod folder;
mod model;
mod paint;
mod theme;

use camera::{Camera, Framing, Plan};
use gpui::{
    canvas, div, prelude::*, px, size, App, Application, Bounds, Context, FocusHandle, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollWheelEvent, Window, WindowBounds, WindowOptions,
};
use model::Model;
use paint::{Fonts, Hit, Walk};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// A press that may still be a click: where it started, and whether it has moved.
struct Press {
    at: Point<Pixels>,
    last: Point<Pixels>,
    moved: bool,
}

struct Viewer {
    m: Model,
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
}

impl Viewer {
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

    fn select(&mut self, id: Option<u32>, path: Option<Vec<u32>>, cx: &mut Context<Self>) {
        self.selected = id;
        self.selected_path = path;
        cx.notify();
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

    fn key(&mut self, e: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        if k.modifiers.platform || k.modifiers.control || k.modifiers.alt {
            return;
        }
        match k.key.as_str() {
            "escape" => self.select(None, None, cx),
            "enter" => {
                if let Some(path) = self.selected_path.clone() {
                    self.fly_to(&path, Framing::Open, 0.08, cx);
                }
            }
            "f" => self.fit(cx),
            "h" | "0" => self.home(cx),
            _ => {}
        }
    }
}

impl Render for Viewer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        div()
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
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
            )
    }
}

impl Viewer {
    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
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
    let texts: HashMap<String, String> = files.iter().filter_map(|f| Some((f.path.clone(), f.text.clone()?))).collect();
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
    let m = Model::new(scene, texts);

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
        let bounds = Bounds::centered(None, size(px(1440.0), px(900.0)), cx);
        let options = WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() };
        cx.open_window(options, |window, cx| {
            cx.new(|cx| {
                let focus = cx.focus_handle();
                window.focus(&focus);
                let camera = Camera::new(&m);
                Viewer { m, camera, theme: theme::light(), fonts: Fonts::new(), focus, width: 0.0, height: 0.0, hits: vec![], selected: None, selected_path: None, press: None, flight: None }
            })
        })
        .unwrap();
        cx.activate(true);
    });
}
