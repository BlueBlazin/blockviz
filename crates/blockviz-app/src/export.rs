//! Downloads: a block alone as SVG (src/canvas/svg.ts `blockSvg`), and a view
//! as PNG, its SVG rasterized.

use crate::camera::{READABLE, Xf};
use crate::model::Model;
use crate::render::{Changes, Paint, Renderer, Trace};
use crate::svgpen::SvgPen;
use crate::theme::Theme;

/// The most pixels across a block is exported at: a larger one is drawn smaller, as a screen that size would show it.
const LARGEST: f64 = 4096.0;
/// Room around a block, for the rings and badges drawn over its edge.
const MARGIN: f64 = 12.0;

/// Block `id` alone as SVG in `theme`'s colors, with its diagram at the size
/// names are read at on screen; a change being read or a value being followed
/// is drawn as on screen.
pub fn block_svg(pen: &mut SvgPen, m: &Model, id: u32, theme: &Theme, trace: Option<&Trace>, diff: Option<&dyn Changes>) -> String {
    let n = m.node(id);
    // The scale of the diagram the block is in at which its own diagram reads at READABLE.
    let k = match (m.owner(n), n.inner) {
        (Some(o), Some(inner)) => {
            let (cw, ch) = (if o.cw > 0.0 { o.cw } else { 1.0 }, if o.ch > 0.0 { o.ch } else { 1.0 });
            Some((inner.w / cw.max(1e-9)).min(inner.h / ch.max(1e-9)))
        }
        _ => None,
    };
    let s = k.map_or(1.0, |k| READABLE / k).min(LARGEST / n.w.max(n.h));
    let t = Xf { x: MARGIN - n.x * s, y: MARGIN - n.y * s, s };
    let width = (n.w * s).ceil() + 2.0 * MARGIN;
    let height = (n.h * s).ceil() + 2.0 * MARGIN;
    let path = [n.id];
    let xfs = [t];
    let root = n.id == m.root();
    let p = Paint { width, height, hover: None, selected: None, path: if root { &path } else { &[] }, xfs: if root { &xfs } else { &[] }, related: None, trace, hover_edge: None, diff };
    let mut r = Renderer::default();
    // The project itself is drawn as its diagram, as the view of all of it shows it.
    if root {
        r.draw(pen, m, theme, &p);
    } else {
        r.draw_block(pen, m, theme, n, t, &p);
    }
    pen.svg(width, height)
}

/// An SVG drawn into a PNG at `scale` pixels per point, its text in the system's fonts.
pub fn png(svg: &str, scale: f32) -> Option<Vec<u8>> {
    use resvg::{tiny_skia, usvg};
    let mut opt = usvg::Options::default();
    opt.fontdb_mut().load_system_fonts();
    let tree = usvg::Tree::from_str(svg, &opt).ok()?;
    let size = tree.size().to_int_size().scale_by(scale)?;
    let mut pixmap = tiny_skia::Pixmap::new(size.width(), size.height())?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    pixmap.encode_png().ok()
}
