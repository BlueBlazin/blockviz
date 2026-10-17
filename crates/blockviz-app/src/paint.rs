//! A port of the core of src/canvas/renderer.ts: nested diagrams drawn
//! immediate-mode, walking only what is visible. Containers are culled
//! against the window, interiors are entered once they are large enough to
//! show something, and text fades in by its size on screen.
//!
//! Drawing is two passes: `Walk` turns the visible scene into a list of
//! primitives without touching GPUI, then `emit` hands them to the window (or
//! `svg` writes them out, to check a frame without a screen).

use crate::theme::{mix, swatch, Theme};
use blockviz_core::ir::{EdgeKind, Kind, Node, Scene, Shape};
use gpui::{
    point, px, size, App, BorderStyle, Bounds, Corners, Edges, Font, FontWeight, Hsla, PathBuilder, SharedString,
    TextRun, Window,
};

/// Maps a content coordinate system to the screen: screen = (x + u·s, y + v·s).
#[derive(Clone, Copy, Debug)]
pub struct Xf {
    pub x: f64,
    pub y: f64,
    pub s: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

pub fn box_of(t: Xf, n: &Node) -> Rect {
    Rect { x: t.x + n.x * t.s, y: t.y + n.y * t.s, w: n.w * t.s, h: n.h * t.s }
}

/// The node whose interior is drawn inside `n`: itself, or the definition it references.
pub fn owner<'a>(scene: &'a Scene, n: &'a Node) -> Option<&'a Node> {
    let content = |n: &Node| !n.children.is_empty() || !n.rows.is_empty();
    if content(n) {
        return (n.inner.is_some() || n.id == scene.root).then_some(n);
    }
    let r = &scene.nodes[n.reference? as usize];
    (n.inner.is_some() && content(r)).then_some(r)
}

/// Transform of `n`'s interior, given the transform of the space `n` lives in.
pub fn interior(t: Xf, n: &Node, owner: &Node) -> Option<Xf> {
    let inner = n.inner?;
    let (cw, ch) = (if owner.cw > 0.0 { owner.cw } else { 1.0 }, if owner.ch > 0.0 { owner.ch } else { 1.0 });
    let k = (inner.w / cw.max(1e-9)).min(inner.h / ch.max(1e-9));
    let ox = inner.x + (inner.w - cw * k) / 2.0;
    let oy = inner.y + (inner.h - ch * k) / 2.0;
    Some(Xf { x: t.x + (n.x + ox) * t.s, y: t.y + (n.y + oy) * t.s, s: t.s * k })
}

fn smoothstep(a: f64, b: f64, x: f64) -> f64 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Interior scale at which a box starts / finishes opening.
const OPEN_FROM: f64 = 0.028;
const OPEN_TO: f64 = 0.075;
/// The shortest side, in pixels, of a block drawn as more than a spot of its color.
const SPECK: f64 = 2.0;
const ROW_H: f64 = 17.0;
const ROW_MAX: f64 = 15.0;

#[derive(Clone, Copy, PartialEq)]
pub enum Face {
    Sans,
    SansMedium,
    SansBold,
    Mono,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Align {
    Left,
    Center,
}

pub enum Prim {
    Quad { b: Rect, radii: [f32; 4], fill: Hsla, border: f32, border_color: Hsla },
    /// A filled triangle (an arrowhead).
    Tri([[f64; 2]; 3], Hsla),
    /// A stroked polyline that is not axis-aligned.
    Line(Vec<[f64; 2]>, f32, Hsla),
    /// `x, y` is the left end (or center) of the baseline.
    Text { text: SharedString, x: f64, y: f64, size: f64, face: Face, color: Hsla, align: Align, max: f64 },
}

pub struct Walk<'a> {
    pub scene: &'a Scene,
    pub titles: &'a [SharedString],
    pub theme: &'a Theme,
    pub width: f64,
    pub height: f64,
    pub out: Vec<Prim>,
}

impl<'a> Walk<'a> {
    pub fn root(&mut self, t: Xf) {
        let root = &self.scene.nodes[self.scene.root as usize];
        self.interior(root, t, &[root.id], 1.0);
    }

    fn quad(&mut self, b: Rect, radius: f64, fill: Hsla, border: f64, border_color: Hsla, alpha: f64) {
        let r = radius as f32;
        self.out.push(Prim::Quad { b, radii: [r; 4], fill: fill.opacity(alpha as f32), border: border as f32, border_color: border_color.opacity(alpha as f32) });
    }

    #[allow(clippy::too_many_arguments)]
    fn text(&mut self, text: SharedString, x: f64, y: f64, size: f64, face: Face, color: Hsla, align: Align, max: f64, alpha: f64) {
        if alpha < 0.02 || text.is_empty() {
            return;
        }
        self.out.push(Prim::Text { text, x, y, size, face, color: color.opacity(alpha as f32), align, max });
    }

    fn interior(&mut self, owner: &'a Node, t: Xf, chain: &[u32], alpha: f64) {
        if !owner.rows.is_empty() {
            self.rows(owner, t, alpha);
            return;
        }
        let edge_alpha = alpha * smoothstep(0.035, 0.12, t.s);
        let lw = (1.3 * t.s.sqrt()).clamp(0.45, 1.45);
        if edge_alpha > 0.02 {
            for e in &owner.edges {
                self.edge(e, t, edge_alpha, lw);
            }
        }
        for &id in &owner.children {
            let n = &self.scene.nodes[id as usize];
            let b = box_of(t, n);
            let m = 2.0;
            if !(b.x < self.width + m && b.y < self.height + m && b.x + b.w > -m && b.y + b.h > -m) || (b.w < 1.0 && b.h < 1.0) {
                continue;
            }
            self.node(n, b, t, chain, alpha);
        }
        if edge_alpha > 0.02 && t.s * 11.0 >= 4.5 {
            let a = edge_alpha * smoothstep(4.5, 7.0, t.s * 11.0);
            for e in &owner.edges {
                let Some([lx, ly]) = e.label_at else { continue };
                if e.label.is_empty() {
                    continue;
                }
                let (x, y) = (t.x + lx * t.s, t.y + ly * t.s);
                if x < -200.0 || y < -20.0 || x > self.width + 20.0 || y > self.height + 20.0 {
                    continue;
                }
                let size = (11.0 * t.s).min(13.5);
                self.text(e.label.clone().into(), x, y, size, Face::Mono, self.theme.label_ink, Align::Left, f64::INFINITY, a);
            }
        }
    }

    fn node(&mut self, n: &'a Node, b: Rect, t: Xf, chain: &[u32], alpha: f64) {
        if n.shape != Shape::Port && b.w.min(b.h) < SPECK {
            let sw = swatch(n.category);
            self.out.push(Prim::Quad { b, radii: [0.0; 4], fill: mix(sw.fill, self.theme.outline, 0.25).opacity(alpha as f32), border: 0.0, border_color: sw.fill });
            return;
        }
        match n.shape {
            Shape::Port => self.port(n, b, t, alpha),
            Shape::Glyph => self.glyph(n, b, t, alpha),
            Shape::Region => self.region(n, b, t, chain, alpha),
            Shape::Box => self.boxed(n, b, t, chain, alpha),
        }
    }

    fn boxed(&mut self, n: &'a Node, b: Rect, t: Xf, chain: &[u32], alpha: f64) {
        let owner = owner(self.scene, n);
        let recursive = owner.is_some_and(|o| o.id != n.id && chain.contains(&o.id));
        let inner = owner.filter(|_| !recursive).and_then(|o| interior(t, n, o));
        let open = inner.map_or(0.0, |i| smoothstep(OPEN_FROM, OPEN_TO, i.s) * smoothstep(40.0, 90.0, b.w));
        let sw = swatch(n.category);
        let radius = (6.0 * t.s).clamp(1.2, 10.0);
        let fill = if open > 0.0 { mix(sw.fill, sw.tint, open as f32) } else { sw.fill };
        let lw = (1.25 * t.s.sqrt()).clamp(0.5, 1.4);
        let outline = if n.category == "op" { self.theme.op_outline } else { self.theme.outline };
        self.quad(b, radius, fill, lw, outline, alpha);
        if open > 0.01 {
            // The header band keeps the box's identity once it is open.
            let hh = (30.0 * t.s).min(b.h);
            let r = radius as f32;
            self.out.push(Prim::Quad { b: Rect { h: hh, ..b }, radii: [r, r, 0.0, 0.0], fill: sw.fill.opacity((alpha * open) as f32), border: 0.0, border_color: sw.fill });
        }
        let legible = smoothstep(9.0, 10.5, 13.0 * t.s);
        let face_alpha = (1.0 - open).max(open * (1.0 - legible));
        if open > 0.01 {
            if let (Some(inner), Some(o)) = (inner, owner) {
                self.header(n, b, t, alpha * open * legible);
                let shown = if o.rows.is_empty() { alpha * open } else { alpha * open * legible };
                if o.id == n.id {
                    self.interior(o, inner, chain, shown);
                } else {
                    let mut deeper = chain.to_vec();
                    deeper.push(o.id);
                    self.interior(o, inner, &deeper, shown);
                }
            }
        }
        if face_alpha > 0.01 {
            self.face(n, b, t, alpha * face_alpha, open > 0.2);
        }
    }

    fn face(&mut self, n: &Node, b: Rect, t: Xf, alpha: f64, overlay: bool) {
        let code = n.kind == Kind::Op;
        let natural = if code { 12.0 } else { 13.0 } * t.s;
        let named = matches!(n.kind, Kind::File | Kind::Notebook | Kind::Dir | Kind::Class | Kind::Function | Kind::Section | Kind::Artifact | Kind::Doc | Kind::Table);
        let roomy = b.w >= if named { 44.0 } else { 70.0 } && b.h >= 18.0;
        let prominent = t.s >= if named { 0.08 } else { 0.14 } && roomy;
        // Readable, but never larger than the box holds (an estimate: 0.55 em per character).
        let room = b.w - 14.0 * t.s.min(1.0);
        let fits = room / (n.title.chars().count().max(1) as f64 * 0.55);
        let size = if prominent { natural.clamp(11.5f64.min(b.h * 0.55).min(fits.max(natural)), 18.0) } else { natural };
        if size < 4.2 {
            return;
        }
        let face = if code { Face::Mono } else if matches!(n.kind, Kind::Call | Kind::Symbol) { Face::SansMedium } else { Face::SansBold };
        let (cx, cy) = (b.x + b.w / 2.0, b.y + b.h / 2.0);
        if overlay {
            let w = (n.title.chars().count() as f64 * size * 0.55).min(room) + size * 1.1;
            let h = size * 1.7;
            self.quad(Rect { x: cx - w / 2.0, y: cy - h / 2.0, w, h }, h / 2.6, self.theme.paper, 0.0, self.theme.paper, alpha * 0.92);
        }
        let ta = alpha * smoothstep(4.2, 7.0, size);
        let title = self.titles[n.id as usize].clone();
        let meta = !n.subtitle.is_empty() && b.h >= size + size * 0.82 + 10.0;
        let ms = (size * 0.82).min((10.5 * t.s).max(10.0));
        let y = if meta { cy - ms * 0.15 } else { cy + size * 0.35 };
        self.text(title, cx, y, size, face, self.theme.ink, Align::Center, room, ta);
        if meta {
            self.text(n.subtitle.clone().into(), cx, y + ms + 3.0, ms, Face::Sans, self.theme.muted, Align::Center, room, ta);
        }
    }

    fn header(&mut self, n: &Node, b: Rect, t: Xf, alpha: f64) {
        let size = (13.0 * t.s).min(19.0);
        if size < 4.2 || alpha < 0.02 {
            return;
        }
        let band = 30.0 * t.s;
        let x = b.x + (9.0 * t.s).min(14.0);
        let y = b.y + (band / 2.0 + size * 0.36).min(band - 4.0);
        let title = self.titles[n.id as usize].clone();
        self.text(title, x, y, size, Face::SansBold, self.theme.ink, Align::Left, b.w - 18.0 * t.s, alpha * smoothstep(4.2, 7.0, size));
    }

    fn region(&mut self, n: &'a Node, b: Rect, t: Xf, chain: &[u32], alpha: f64) {
        let lane = n.kind == Kind::Lane;
        let radius = (9.0 * t.s).clamp(1.5, 14.0);
        let sw = swatch(n.category);
        let lw = (1.1 * t.s.sqrt()).clamp(0.45, 1.2);
        let outline = if lane { self.theme.lane_outline } else { self.theme.region_outline };
        self.quad(b, radius, sw.fill, lw, outline, alpha);
        let size = 11.5 * t.s;
        if !n.title.is_empty() && size >= 4.2 {
            let code = matches!(n.kind, Kind::Op | Kind::Loop | Kind::Branch);
            let title = self.titles[n.id as usize].clone();
            self.text(title, b.x + 10.0 * t.s, b.y + 17.0 * t.s, size, if code { Face::Mono } else { Face::SansBold }, self.theme.muted, Align::Left, b.w - 20.0 * t.s, alpha * smoothstep(4.2, 7.0, size));
        }
        if let Some(inner) = interior(t, n, n) {
            self.interior(n, inner, chain, alpha);
        }
    }

    fn glyph(&mut self, n: &Node, b: Rect, t: Xf, alpha: f64) {
        let r = b.w.min(b.h) / 2.0;
        let (cx, cy) = (b.x + b.w / 2.0, b.y + b.h / 2.0);
        let lw = (1.25 * t.s.sqrt()).clamp(0.5, 1.4);
        self.quad(Rect { x: cx - r, y: cy - r, w: 2.0 * r, h: 2.0 * r }, r, self.theme.paper, lw, self.theme.outline, alpha);
        if r >= 2.5 {
            let title = self.titles[n.id as usize].clone();
            self.text(title, cx, cy + r * 0.4, r * 1.15, Face::SansBold, self.theme.ink, Align::Center, f64::INFINITY, alpha);
        }
    }

    fn port(&mut self, n: &Node, b: Rect, t: Xf, alpha: f64) {
        if n.kind == Kind::Unused || b.h == 0.0 || n.title.is_empty() {
            return;
        }
        let size = (11.5 * t.s).min(15.0);
        if size < 4.2 {
            return;
        }
        let title = self.titles[n.id as usize].clone();
        self.text(title, b.x + b.w / 2.0, b.y + b.h / 2.0 + size * 0.35, size, Face::Mono, self.theme.muted, Align::Center, (b.w + 60.0 * t.s).max(40.0), alpha * smoothstep(4.2, 7.0, size));
    }

    fn rows(&mut self, owner: &Node, t: Xf, alpha: f64) {
        let size = (11.5 * t.s).min(ROW_MAX);
        if size < 3.0 {
            return;
        }
        let f = size / 11.5;
        let mut y = t.y + 6.0 * f;
        let x = t.x + 10.0 * f;
        let ta = alpha * smoothstep(3.5, 6.5, size);
        let width = owner.cw * t.s - 20.0 * f;
        for r in &owner.rows {
            let h = ROW_H * f;
            if y > self.height {
                break;
            }
            if y + h >= 0.0 {
                let line = if r.value.is_empty() { r.key.clone() } else { format!("{}  {}", r.key, r.value) };
                self.text(line.into(), x, y + h * 0.72, size, if r.heading { Face::SansBold } else { Face::Mono }, self.theme.ink, Align::Left, width, ta);
            }
            y += h;
        }
    }

    fn edge(&mut self, e: &blockviz_core::ir::Edge, t: Xf, alpha: f64, lw: f64) {
        let pts = &e.points;
        if pts.len() < 4 {
            return;
        }
        let screen: Vec<[f64; 2]> = pts.chunks_exact(2).map(|p| [t.x + p[0] * t.s, t.y + p[1] * t.s]).collect();
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for p in &screen {
            x0 = x0.min(p[0]);
            y0 = y0.min(p[1]);
            x1 = x1.max(p[0]);
            y1 = y1.max(p[1]);
        }
        if x0 > self.width + 10.0 || y0 > self.height + 10.0 || x1 < -10.0 || y1 < -10.0 {
            return;
        }
        let control = e.kind == EdgeKind::Control;
        let color = match e.kind {
            EdgeKind::Control => self.theme.region_outline,
            EdgeKind::Artifact => self.theme.artifact,
            EdgeKind::Exec => self.theme.op_outline,
            _ => self.theme.edge,
        };
        let a = (alpha * if control { 0.8 } else { 0.95 }) as f32;
        let width = if control { lw * 0.85 } else { lw };
        let head = if e.open { 0.0 } else { (7.0 * t.s.sqrt()).clamp(3.0, 8.0) * if control { 0.8 } else { 1.0 } };
        let n = screen.len();
        let [ex, ey] = screen[n - 1];
        let (dx, dy) = (ex - screen[n - 2][0], ey - screen[n - 2][1]);
        let len = dx.hypot(dy).max(1e-9);
        let (ux, uy) = (dx / len, dy / len);
        let mut line = screen.clone();
        line[n - 1] = [ex - ux * head * 0.8, ey - uy * head * 0.8];
        // Layered layout routes orthogonally: each straight run is a thin quad.
        let mut slanted = false;
        for w in line.windows(2) {
            let ([ax, ay], [bx, by]) = (w[0], w[1]);
            if (ax - bx).abs() < 0.01 || (ay - by).abs() < 0.01 {
                let b = Rect { x: ax.min(bx) - width / 2.0, y: ay.min(by) - width / 2.0, w: (ax - bx).abs() + width, h: (ay - by).abs() + width };
                self.out.push(Prim::Quad { b, radii: [0.0; 4], fill: color.opacity(a), border: 0.0, border_color: color });
            } else {
                slanted = true;
            }
        }
        if slanted {
            self.out.push(Prim::Line(line, width as f32, color.opacity(a)));
        }
        if head > 0.0 {
            self.out.push(Prim::Tri(
                [[ex, ey], [ex - ux * head - uy * head * 0.42, ey - uy * head + ux * head * 0.42], [ex - ux * head + uy * head * 0.42, ey - uy * head - ux * head * 0.42]],
                color.opacity(a),
            ));
        }
    }
}

pub struct Fonts {
    pub sans: Font,
    pub medium: Font,
    pub bold: Font,
    pub mono: Font,
}

impl Fonts {
    pub fn new() -> Self {
        let sans = gpui::font("Helvetica Neue");
        Fonts {
            medium: Font { weight: FontWeight::MEDIUM, ..sans.clone() },
            bold: Font { weight: FontWeight::SEMIBOLD, ..sans.clone() },
            sans,
            mono: gpui::font("Menlo"),
        }
    }
}

/// Font sizes are rounded to this fraction of a pixel, so that a zoom does not
/// shape and rasterize every name again at a new size in every frame.
pub const SIZE_STEP: f64 = 0.25;

pub fn emit(prims: &[Prim], fonts: &Fonts, window: &mut Window, cx: &mut App) {
    let f = |v: f64| px(v as f32);
    for p in prims {
        match p {
            Prim::Quad { b, radii, fill, border, border_color } => {
                let bounds = Bounds::new(point(f(b.x), f(b.y)), size(f(b.w), f(b.h)));
                let radii = Corners { top_left: px(radii[0]), top_right: px(radii[1]), bottom_right: px(radii[2]), bottom_left: px(radii[3]) };
                window.paint_quad(gpui::quad(bounds, radii, *fill, Edges::all(px(*border)), *border_color, BorderStyle::Solid));
            }
            Prim::Tri(pts, color) => {
                let mut pb = PathBuilder::fill();
                pb.move_to(point(f(pts[0][0]), f(pts[0][1])));
                pb.line_to(point(f(pts[1][0]), f(pts[1][1])));
                pb.line_to(point(f(pts[2][0]), f(pts[2][1])));
                pb.close();
                if let Ok(path) = pb.build() {
                    window.paint_path(path, *color);
                }
            }
            Prim::Line(pts, width, color) => {
                let mut pb = PathBuilder::stroke(px(*width));
                pb.move_to(point(f(pts[0][0]), f(pts[0][1])));
                for p in &pts[1..] {
                    pb.line_to(point(f(p[0]), f(p[1])));
                }
                if let Ok(path) = pb.build() {
                    window.paint_path(path, *color);
                }
            }
            Prim::Text { text, x, y, size, face, color, align, max } => {
                let size = (size / SIZE_STEP).round() * SIZE_STEP;
                let font = match face {
                    Face::Sans => &fonts.sans,
                    Face::SansMedium => &fonts.medium,
                    Face::SansBold => &fonts.bold,
                    Face::Mono => &fonts.mono,
                };
                let ts = window.text_system().clone();
                let run = |len| TextRun { len, font: font.clone(), color: *color, background_color: None, underline: None, strikethrough: None };
                let shape = |t: SharedString, len| ts.shape_line(t, px(size as f32), &[run(len)], None);
                let mut line = shape(text.clone(), text.len());
                let width = f64::from(line.width);
                if width > *max && *max > 0.0 {
                    // Cut short at about where it fits, its end marked.
                    let keep = ((text.chars().count() as f64 * max / width).floor() as usize).saturating_sub(1);
                    if keep == 0 {
                        continue;
                    }
                    let cut: String = text.chars().take(keep).chain(std::iter::once('…')).collect();
                    let len = cut.len();
                    line = shape(cut.into(), len);
                }
                let width = f64::from(line.width);
                let left = match align {
                    Align::Left => *x,
                    Align::Center => x - width / 2.0,
                };
                let ascent = f64::from(line.ascent);
                let height = line.ascent + line.descent;
                let _ = line.paint(point(f(left), f(y - ascent)), height, window, cx);
            }
        }
    }
}

/// The display list as SVG, to check what a frame draws without a screen.
pub fn svg(prims: &[Prim], width: f64, height: f64) -> String {
    let color = |c: &Hsla| {
        let r = c.to_rgb();
        format!("rgba({:.0},{:.0},{:.0},{:.3})", r.r * 255.0, r.g * 255.0, r.b * 255.0, r.a)
    };
    let esc = |t: &str| t.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let mut out = format!("<svg xmlns='http://www.w3.org/2000/svg' width='{width}' height='{height}'><rect width='100%' height='100%' fill='white'/>");
    for p in prims {
        match p {
            Prim::Quad { b, radii, fill, border, border_color } => out.push_str(&format!(
                "<rect x='{:.2}' y='{:.2}' width='{:.2}' height='{:.2}' rx='{:.2}' fill='{}' stroke='{}' stroke-width='{}'/>",
                b.x, b.y, b.w, b.h, radii[0], color(fill), if *border > 0.0 { color(border_color) } else { "none".into() }, border
            )),
            Prim::Tri(pts, c) => out.push_str(&format!("<polygon points='{:.2},{:.2} {:.2},{:.2} {:.2},{:.2}' fill='{}'/>", pts[0][0], pts[0][1], pts[1][0], pts[1][1], pts[2][0], pts[2][1], color(c))),
            Prim::Line(pts, w, c) => out.push_str(&format!("<polyline points='{}' fill='none' stroke='{}' stroke-width='{w}'/>", pts.iter().map(|p| format!("{:.2},{:.2}", p[0], p[1])).collect::<Vec<_>>().join(" "), color(c))),
            Prim::Text { text, x, y, size, face, color: c, align, .. } => out.push_str(&format!(
                "<text x='{x:.2}' y='{y:.2}' font-size='{size:.2}' font-family='{}' font-weight='{}' text-anchor='{}' fill='{}'>{}</text>",
                if *face == Face::Mono { "Menlo" } else { "Helvetica Neue" },
                match face { Face::SansBold => 600, Face::SansMedium => 500, _ => 400 },
                if *align == Align::Center { "middle" } else { "start" },
                color(c),
                esc(text)
            )),
        }
    }
    out.push_str("</svg>");
    out
}

/// The interior transform that fits the root's diagram in the window.
pub fn fit(scene: &Scene, width: f64, height: f64) -> Xf {
    let root = &scene.nodes[scene.root as usize];
    let (cw, ch) = (root.cw.max(1.0), root.ch.max(1.0));
    let s = (width / cw).min(height / ch) * 0.92;
    Xf { x: (width - cw * s) / 2.0, y: (height - ch * s) / 2.0, s }
}

