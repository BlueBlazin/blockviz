//! A port of the core of src/canvas/renderer.ts: nested diagrams drawn
//! immediate-mode, walking only what is visible. Containers are culled
//! against the window, interiors are entered once they are large enough to
//! show something, and text fades in by its size on screen. Hit regions are
//! recorded as a side effect of drawing.
//!
//! Drawing is two passes: `Walk` turns the visible scene into a list of
//! primitives without touching GPUI, then `emit` hands them to the window (or
//! `svg` writes them out, to check a frame without a screen).

use crate::camera::{box_of, Rect, Xf};
use crate::model::Model;
use crate::theme::{mix, swatch, Theme};
use blockviz_core::ir::{EdgeKind, Kind, Node, Shape};
use gpui::{point, px, size, App, BorderStyle, Bounds, Corners, Edges, Font, FontWeight, Hsla, PathBuilder, SharedString, TextRun, Window};

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
/// The smallest name, in pixels, a reader can make out.
const LEGIBLE: f64 = 7.0;

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

/// A block drawn this frame. `clear` when the reader can see what it is (its
/// name is legible, or the box is large), so clicks never land on a block too
/// small to make out. `face` when its name is on its face: it is closed, or
/// what is drawn inside it is only a thumbnail. `parent` is the hit of the
/// block it is drawn in (-1: the root).
#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub id: u32,
    pub rect: Rect,
    pub parent: i32,
    pub clear: bool,
    pub face: bool,
}

/// The innermost block under a point that the reader can make out; a block
/// too small to see is passed over for the one around it, unless `any`
/// (double-clicking a speck zooms into it). -1: none.
pub fn hit_at(hits: &[Hit], x: f64, y: f64, any: bool) -> i32 {
    let mut fallback = -1;
    for i in (0..hits.len()).rev() {
        if hits[i].rect.contains(x, y) {
            if any {
                return shown(hits, i as i32);
            }
            if hits[i].clear {
                return i as i32;
            }
            if fallback < 0 {
                fallback = i as i32;
            }
        }
    }
    fallback
}

/// Hit `i`, or the closed block it is drawn inside: a block in the thumbnail
/// of a block whose name is still on its face is part of that block's picture.
fn shown(hits: &[Hit], i: i32) -> i32 {
    let mut out = i;
    let mut j = hits[i as usize].parent;
    while j >= 0 {
        if hits[j as usize].face {
            out = j;
        }
        j = hits[j as usize].parent;
    }
    out
}

/// Instance path (root first) of a hit.
pub fn path_of(hits: &[Hit], root: u32, i: i32) -> Vec<u32> {
    let mut out = vec![];
    let mut at = i;
    while at >= 0 {
        out.push(hits[at as usize].id);
        at = hits[at as usize].parent;
    }
    out.push(root);
    out.reverse();
    out
}

pub struct Walk<'a> {
    pub m: &'a Model,
    pub theme: &'a Theme,
    pub width: f64,
    pub height: f64,
    /// The camera's anchor path (root first) and the interior transform of each.
    pub path: &'a [u32],
    pub xfs: &'a [Xf],
    pub selected: Option<u32>,
    pub out: Vec<Prim>,
    pub hits: Vec<Hit>,
}

impl<'a> Walk<'a> {
    pub fn new(m: &'a Model, theme: &'a Theme, width: f64, height: f64, path: &'a [u32], xfs: &'a [Xf]) -> Self {
        Walk { m, theme, width, height, path, xfs, selected: None, out: Vec::with_capacity(4096), hits: vec![] }
    }

    pub fn draw(&mut self) {
        let root = self.m.node(self.m.root());
        self.interior(root, self.xfs[0], -1, 0, &[root.id], 1.0);
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

    fn hit(&mut self, n: &Node, b: Rect, parent: i32, clear: bool) -> i32 {
        if b.w < 5.0 || b.h < 4.0 {
            return parent;
        }
        self.hits.push(Hit { id: n.id, rect: b, parent, clear: clear || (b.w >= 160.0 && b.h >= 100.0), face: false });
        self.hits.len() as i32 - 1
    }

    /// Marks the hit just recorded for `n` as one the reader can make out.
    fn seen(&mut self, at: i32, n: &Node, size: f64, alpha: f64) {
        if let Some(h) = usize::try_from(at).ok().and_then(|i| self.hits.get_mut(i)) {
            if h.id == n.id && size >= LEGIBLE && alpha >= 0.5 {
                h.clear = true;
            }
        }
    }

    fn interior(&mut self, owner: &'a Node, t: Xf, parent_hit: i32, path_pos: i32, chain: &[u32], alpha: f64) {
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
            let n = self.m.node(id);
            let b = box_of(t, n);
            let m = 2.0;
            if !(b.x < self.width + m && b.y < self.height + m && b.x + b.w > -m && b.y + b.h > -m) || (b.w < 1.0 && b.h < 1.0) {
                continue;
            }
            let next = path_pos + 1;
            let next_pos = if path_pos >= 0 && self.path.get(next as usize) == Some(&id) { next } else { -1 };
            self.node(n, b, t, parent_hit, next_pos, chain, alpha);
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

    #[allow(clippy::too_many_arguments)]
    fn node(&mut self, n: &'a Node, b: Rect, t: Xf, parent_hit: i32, path_pos: i32, chain: &[u32], alpha: f64) {
        if n.shape != Shape::Port && b.w.min(b.h) < SPECK {
            let sw = swatch(n.category);
            self.out.push(Prim::Quad { b, radii: [0.0; 4], fill: mix(sw.fill, self.theme.outline, 0.25).opacity(alpha as f32), border: 0.0, border_color: sw.fill });
            return;
        }
        match n.shape {
            Shape::Port => self.port(n, b, t, alpha),
            Shape::Glyph => {
                self.glyph(n, b, t, alpha);
                self.hit(n, b, parent_hit, b.w.min(b.h) >= 12.0);
            }
            Shape::Region => self.region(n, b, t, parent_hit, path_pos, chain, alpha),
            Shape::Box => self.boxed(n, b, t, parent_hit, path_pos, chain, alpha),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn boxed(&mut self, n: &'a Node, b: Rect, t: Xf, parent_hit: i32, path_pos: i32, chain: &[u32], alpha: f64) {
        let owner = self.m.owner(n);
        let recursive = owner.is_some_and(|o| o.id != n.id && chain.contains(&o.id));
        let inner = owner.filter(|_| !recursive).and_then(|o| if path_pos >= 0 { Some(self.xfs[path_pos as usize]) } else { self.m.interior_of(t, n, o) });
        let open = inner.map_or(0.0, |i| smoothstep(OPEN_FROM, OPEN_TO, i.s) * smoothstep(40.0, 90.0, b.w));
        let sw = swatch(n.category);
        let radius = (6.0 * t.s).clamp(1.2, 10.0);
        let fill = if open > 0.0 { mix(sw.fill, sw.tint, open as f32) } else { sw.fill };
        let lw = (1.25 * t.s.sqrt()).clamp(0.5, 1.4);
        let selected = self.selected == Some(n.id);
        let width = if selected { 2.2 } else { lw };
        let outline = if selected { self.theme.accent } else if n.category == "op" { self.theme.op_outline } else { self.theme.outline };
        self.quad(b, radius, fill, width, outline, alpha);
        if open > 0.01 {
            // The header band keeps the box's identity once it is open.
            let hh = (30.0 * t.s).min(b.h);
            let r = radius as f32;
            self.out.push(Prim::Quad { b: Rect { h: hh, ..b }, radii: [r, r, 0.0, 0.0], fill: sw.fill.opacity((alpha * open) as f32), border: 0.0, border_color: sw.fill });
        }
        let me = self.hit(n, b, parent_hit, false);
        // While the header is too small to read, the name stays centered on a
        // pill over the faint interior; it moves to the header once legible.
        let legible = smoothstep(9.0, 10.5, 13.0 * t.s);
        let face_alpha = (1.0 - open).max(open * (1.0 - legible));
        let thumbnail = face_alpha >= 0.5;
        if let Some(h) = usize::try_from(me).ok().and_then(|i| self.hits.get_mut(i)) {
            if h.id == n.id {
                h.face = thumbnail;
            }
        }
        if open > 0.01 {
            if let (Some(inner), Some(o)) = (inner, owner) {
                let size = (13.0 * t.s).min(19.0);
                self.seen(me, n, size, alpha * open * legible * smoothstep(4.2, 7.0, size));
                self.header(n, b, t, alpha * open * legible);
                let shown = if o.rows.is_empty() { alpha * open } else { alpha * open * legible };
                if o.id == n.id {
                    self.interior(o, inner, me, path_pos, chain, shown);
                } else {
                    let mut deeper = chain.to_vec();
                    deeper.push(o.id);
                    self.interior(o, inner, me, path_pos, &deeper, shown);
                }
            }
        }
        if face_alpha > 0.01 {
            let (size, shown) = self.face(n, b, t, alpha * face_alpha, open > 0.2);
            self.seen(me, n, size, shown);
        }
    }

    /// Draws a closed block's name; returns its size and how visible it is.
    fn face(&mut self, n: &Node, b: Rect, t: Xf, alpha: f64, overlay: bool) -> (f64, f64) {
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
            return (0.0, 0.0);
        }
        let face = if code { Face::Mono } else if matches!(n.kind, Kind::Call | Kind::Symbol) { Face::SansMedium } else { Face::SansBold };
        let (cx, cy) = (b.x + b.w / 2.0, b.y + b.h / 2.0);
        if overlay {
            let w = (n.title.chars().count() as f64 * size * 0.55).min(room) + size * 1.1;
            let h = size * 1.7;
            self.quad(Rect { x: cx - w / 2.0, y: cy - h / 2.0, w, h }, h / 2.6, self.theme.paper, 0.0, self.theme.paper, alpha * 0.92);
        }
        let ta = alpha * smoothstep(4.2, 7.0, size);
        let title = self.m.titles[n.id as usize].clone();
        let meta = !n.subtitle.is_empty() && b.h >= size + size * 0.82 + 10.0;
        let ms = (size * 0.82).min((10.5 * t.s).max(10.0));
        let y = if meta { cy - ms * 0.15 } else { cy + size * 0.35 };
        self.text(title, cx, y, size, face, self.theme.ink, Align::Center, room, ta);
        if meta {
            self.text(n.subtitle.clone().into(), cx, y + ms + 3.0, ms, Face::Sans, self.theme.muted, Align::Center, room, ta);
        }
        (if n.title.is_empty() { 0.0 } else { size }, ta)
    }

    fn header(&mut self, n: &Node, b: Rect, t: Xf, alpha: f64) {
        let size = (13.0 * t.s).min(19.0);
        if size < 4.2 || alpha < 0.02 {
            return;
        }
        let band = 30.0 * t.s;
        let x = b.x + (9.0 * t.s).min(14.0);
        let y = b.y + (band / 2.0 + size * 0.36).min(band - 4.0);
        let title = self.m.titles[n.id as usize].clone();
        self.text(title, x, y, size, Face::SansBold, self.theme.ink, Align::Left, b.w - 18.0 * t.s, alpha * smoothstep(4.2, 7.0, size));
    }

    #[allow(clippy::too_many_arguments)]
    fn region(&mut self, n: &'a Node, b: Rect, t: Xf, parent_hit: i32, path_pos: i32, chain: &[u32], alpha: f64) {
        let lane = n.kind == Kind::Lane;
        let radius = (9.0 * t.s).clamp(1.5, 14.0);
        let sw = swatch(n.category);
        let lw = (1.1 * t.s.sqrt()).clamp(0.45, 1.2);
        let selected = self.selected == Some(n.id);
        let outline = if selected { self.theme.accent } else if lane { self.theme.lane_outline } else { self.theme.region_outline };
        self.quad(b, radius, sw.fill, if selected { 2.0 } else { lw }, outline, alpha);
        let me = self.hit(n, b, parent_hit, false);
        let size = 11.5 * t.s;
        if !n.title.is_empty() {
            self.seen(me, n, size, alpha * smoothstep(4.2, 7.0, size));
        }
        if !n.title.is_empty() && size >= 4.2 {
            let code = matches!(n.kind, Kind::Op | Kind::Loop | Kind::Branch);
            let title = self.m.titles[n.id as usize].clone();
            self.text(title, b.x + 10.0 * t.s, b.y + 17.0 * t.s, size, if code { Face::Mono } else { Face::SansBold }, self.theme.muted, Align::Left, b.w - 20.0 * t.s, alpha * smoothstep(4.2, 7.0, size));
        }
        let inner = if path_pos >= 0 { Some(self.xfs[path_pos as usize]) } else { self.m.interior_of(t, n, n) };
        if let Some(inner) = inner {
            self.interior(n, inner, me, path_pos, chain, alpha);
        }
    }

    fn glyph(&mut self, n: &Node, b: Rect, t: Xf, alpha: f64) {
        let r = b.w.min(b.h) / 2.0;
        let (cx, cy) = (b.x + b.w / 2.0, b.y + b.h / 2.0);
        let lw = (1.25 * t.s.sqrt()).clamp(0.5, 1.4);
        let selected = self.selected == Some(n.id);
        let outline = if selected { self.theme.accent } else { self.theme.outline };
        self.quad(Rect { x: cx - r, y: cy - r, w: 2.0 * r, h: 2.0 * r }, r, self.theme.paper, if selected { lw + 0.8 } else { lw }, outline, alpha);
        if r >= 2.5 {
            let title = self.m.titles[n.id as usize].clone();
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
        let title = self.m.titles[n.id as usize].clone();
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



#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Camera;

    #[test]
    fn clicks_land_on_legible_blocks_and_their_paths_run_from_the_root() {
        let files = [("model.py", "class Block:\n    def forward(self, x):\n        return self.attn(x)\n"), ("train.py", "from model import Block\nb = Block()\n")];
        let inputs = files.iter().map(|(p, t)| blockviz_core::project::InFile { path: p.to_string(), text: Some(t.to_string()), bytes: t.len() as u64 }).collect();
        let m = Model::new(blockviz_core::analyze("t", inputs), files.iter().map(|(p, t)| (p.to_string(), t.to_string())).collect());
        let (w, h) = (1200.0, 800.0);
        let mut cam = Camera::new(&m);
        cam.fit_root(&m, w, h);
        let xfs = cam.transforms(&m);
        let theme = crate::theme::light();
        let mut walk = Walk::new(&m, &theme, w, h, &cam.path, &xfs);
        walk.draw();
        // Every file is hit at its center, and its path is root → file.
        for f in m.node(m.root()).children.iter().copied().filter(|&c| m.node(c).kind == Kind::File) {
            let at = walk.hits.iter().position(|h| h.id == f).expect("file drawn");
            let r = walk.hits[at].rect;
            let i = hit_at(&walk.hits, r.x + r.w / 2.0, r.y + 4.0, false);
            assert!(i >= 0);
            assert_eq!(path_of(&walk.hits, m.root(), i)[..2], [m.root(), f]);
        }
        assert_eq!(hit_at(&walk.hits, -10.0, -10.0, false), -1);
    }
}
