//! blockviz as a native app: a folder on disk analyzed in process and drawn
//! with GPUI, the same diagram and reading tools as the web app, following
//! the folder live as files change, and comparing it with any git revision.
//!
//! `blockviz-app [folder]` opens a folder (or asks for one).
//! `blockviz-app folder --svg out.svg|out.png [--at file/Class/method] [--dark] [--size 1440x900]`
//! draws a view into an SVG (or PNG) without a window, as the window would draw it.

mod anchor;
mod camera;
mod code;
mod compare;
mod crumbs;
mod diff;
mod export;
mod git;
mod gpuipen;
mod history;
mod icons;
mod input;
mod linediff;
mod lines;
mod live;
mod model;
mod panel;
mod pen;
mod render;
mod settings;
mod shell;
mod svgpen;
#[cfg(test)]
mod tests;
mod theme;
mod trace;
mod viewer;

use gpui::{px, size, App, AppContext, Application, Bounds, KeyBinding, WindowBounds, WindowOptions};
use std::path::PathBuf;
use viewer::Viewer;

gpui::actions!(blockviz, [Quit]);

/// Draws a view of `folder` into an SVG, without a window.
fn draw_svg(folder: &std::path::Path, out: &str, at: Option<&str>, dark: bool, (w, h): (f64, f64)) {
    let files = live::read(folder);
    let name = folder.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let scene = blockviz_core::analyze(&name, live::inputs(&files));
    let m = model::Model::new(scene, live::texts(&files));
    let mut cam = camera::Camera::new(&m);
    cam.insets = camera::Insets { top: 56.0, right: 12.0, bottom: 52.0, left: 12.0 };
    cam.fit_root(&m, w, h);
    if let Some(at) = at {
        // A path of titles from the top: `model.py/Block/forward`.
        let mut id = m.root();
        for part in at.split('/') {
            let o = m.owner(m.node(id)).map_or(id, |o| o.id);
            match m.node(o).children.iter().copied().find(|&c| m.node(c).title == part) {
                Some(c) => id = c,
                None => return eprintln!("no {part} in {}", m.node(o).title),
            }
        }
        let plan = cam.flight(&m, &m.canonical_path(id), w, h, camera::Framing::Open, 0.08);
        cam.step(&m, &plan, 1.0, w, h);
    }
    let theme = if dark { theme::Theme::dark() } else { theme::Theme::light() };
    let xfs = cam.transforms(&m);
    let p = render::Paint { width: w, height: h, hover: None, selected: None, path: &cam.path, xfs: &xfs, related: None, trace: None, hover_edge: None, diff: None };
    let mut pen = svgpen::SvgPen::estimating();
    render::Renderer::default().draw(&mut pen, &m, &theme, &p);
    let svg = pen.svg(w, h);
    // A PNG is the SVG rasterized, as the Download menu makes one.
    if out.ends_with(".png") {
        std::fs::write(out, export::png(&svg, 1.0).expect("rasterized")).unwrap();
    } else {
        std::fs::write(out, svg).unwrap();
    }
    println!("wrote {out}");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
    let svg = value("--svg");
    let at = value("--at");
    let sized = value("--size");
    let flags_with_values = [svg.as_ref(), at.as_ref(), sized.as_ref()];
    let folder = args.iter().find(|a| !a.starts_with("--") && !flags_with_values.contains(&Some(*a))).map(PathBuf::from);

    if let Some(out) = svg {
        let folder = folder.unwrap_or_else(|| PathBuf::from("."));
        let (w, h) = sized.and_then(|s| s.split_once('x').and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))).unwrap_or((1440.0, 900.0));
        return draw_svg(&folder, &out, at.as_deref(), args.iter().any(|a| a == "--dark"), (w, h));
    }

    Application::new().with_assets(icons::Icons).run(move |cx: &mut App| {
        input::bind_keys(cx);
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        let bounds = Bounds::centered(None, size(px(1440.0), px(900.0)), cx);
        let options = WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() };
        cx.open_window(options, |window, cx| {
            cx.new(|cx| {
                let mut v = Viewer::new(window, cx);
                if let Some(f) = folder {
                    v.open_folder(f, cx);
                }
                v
            })
        })
        .unwrap();
        cx.activate(true);
    });
}
