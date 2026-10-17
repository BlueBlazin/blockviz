//! Camera and placement math, ported from src/canvas/geometry.ts and
//! src/canvas/camera.ts.
//!
//! The camera is anchored to the deepest container that fills the window:
//! `xf` maps the anchor's interior to the screen and every other transform is
//! derived from it, so precision never degrades however deep the zoom (a
//! root-relative camera runs out of float precision far down).

use crate::model::Model;
use blockviz_core::ir::Node;

/// Maps a content coordinate system to the screen: screen = (x + u·s, y + v·s).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Xf {
    pub x: f64,
    pub y: f64,
    pub s: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.y + self.h
    }
}

/// A view described in some content frame: its center and visible width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub cx: f64,
    pub cy: f64,
    pub w: f64,
}

/// Screen margins covered by overlays (the top bar, the details panel).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Insets {
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub left: f64,
}

pub fn box_of(t: Xf, n: &Node) -> Rect {
    Rect { x: t.x + n.x * t.s, y: t.y + n.y * t.s, w: n.w * t.s, h: n.h * t.s }
}

pub fn covers(b: Rect, w: f64, h: f64) -> bool {
    b.x <= 0.0 && b.y <= 0.0 && b.x + b.w >= w && b.y + b.h >= h
}

/// Where `n`'s interior goes: content `cw`×`ch` fitted and centered inside `inner`.
fn placement(n: &Node, cw: f64, ch: f64) -> (f64, f64, f64) {
    let inner = n.inner.unwrap_or_default();
    let k = (inner.w / cw.max(1e-9)).min(inner.h / ch.max(1e-9));
    (k, inner.x + (inner.w - cw * k) / 2.0, inner.y + (inner.h - ch * k) / 2.0)
}

pub fn child_xf(t: Xf, n: &Node, cw: f64, ch: f64) -> Xf {
    let (k, ox, oy) = placement(n, cw, ch);
    Xf { x: t.x + (n.x + ox) * t.s, y: t.y + (n.y + oy) * t.s, s: t.s * k }
}

pub fn parent_xf(c: Xf, n: &Node, cw: f64, ch: f64) -> Xf {
    let (k, ox, oy) = placement(n, cw, ch);
    let s = c.s / k;
    Xf { x: c.x - (n.x + ox) * s, y: c.y - (n.y + oy) * s, s }
}

pub fn zoom_at(t: Xf, factor: f64, px: f64, py: f64) -> Xf {
    Xf { x: px - (px - t.x) * factor, y: py - (py - t.y) * factor, s: t.s * factor }
}

pub fn view_of(t: Xf, width: f64, height: f64) -> View {
    View { cx: (width / 2.0 - t.x) / t.s, cy: (height / 2.0 - t.y) / t.s, w: width / t.s }
}

pub fn xf_of(v: View, width: f64, height: f64) -> Xf {
    let s = width / v.w;
    Xf { x: width / 2.0 - v.cx * s, y: height / 2.0 - v.cy * s, s }
}

/// The view that frames a box with padding, keeping the screen's aspect.
pub fn frame(b: Rect, width: f64, height: f64, pad: f64) -> View {
    let aspect = width / height.max(1.0);
    let w = b.w.max(b.h * aspect) * (1.0 + pad * 2.0);
    View { cx: b.x + b.w / 2.0, cy: b.y + b.h / 2.0, w }
}

/// The part of the screen overlays leave visible.
fn visible(width: f64, height: f64, i: Insets) -> (f64, f64) {
    ((width - i.left - i.right).max(width * 0.3), (height - i.top - i.bottom).max(height * 0.3))
}

/// Like `frame`, but centers the box in the part of the screen overlays leave visible.
pub fn frame_in(b: Rect, width: f64, height: f64, pad: f64, i: Insets) -> View {
    let (vw, vh) = visible(width, height, i);
    let v = frame(b, vw, vh, pad);
    let w = v.w * width / vw;
    let k = w / width;
    View { cx: v.cx - (i.left - i.right) / 2.0 * k, cy: v.cy - (i.top - i.bottom) / 2.0 * k, w }
}

/// Interior scale at which names (13 units) read comfortably, and the least that counts as readable.
pub const READABLE: f64 = 1.0;
const READABLE_MIN: f64 = 0.8;

/// The view at screen scale `s` that puts frame point (x, y) at screen point (sx, sy).
fn view_at(x: f64, y: f64, sx: f64, sy: f64, s: f64, width: f64, height: f64) -> View {
    View { cx: x + (width / 2.0 - sx) / s, cy: y + (height / 2.0 - sy) / s, w: width / s }
}

/// Frames a box so the diagram inside it can be read. `k` is the size of one
/// interior unit in frame units. The whole box when that is readable already;
/// otherwise as much as readable text needs, never past the box's full width,
/// starting at its top (diagrams read top to bottom).
pub fn frame_readable(b: Rect, k: f64, width: f64, height: f64, pad: f64, i: Insets) -> View {
    let whole = frame_in(b, width, height, pad, i);
    let s0 = width / whole.w;
    if s0 * k >= READABLE_MIN {
        return whole;
    }
    let (vw, vh) = visible(width, height, i);
    let s = s0.max((READABLE / k).min(vw / (b.w * 1.06)));
    let sy = if b.h * s <= vh - 32.0 { i.top + vh / 2.0 - b.h * s / 2.0 } else { i.top + 16.0 };
    view_at(b.x + b.w / 2.0, b.y, i.left + vw / 2.0, sy, s, width, height)
}

/// Frames a block with nothing inside among its neighbors: centered, at the
/// size its diagram reads at, and never filling more than about half the screen.
pub fn frame_around(b: Rect, k: f64, width: f64, height: f64, i: Insets) -> View {
    let (vw, vh) = visible(width, height, i);
    let s = (READABLE * 1.15 / k).min(vw * 0.55 / b.w.max(1e-9)).min(vh * 0.55 / b.h.max(1e-9));
    view_at(b.x + b.w / 2.0, b.y + b.h / 2.0, i.left + vw / 2.0, i.top + vh / 2.0, s, width, height)
}

/// Smooth and efficient zooming and panning (van Wijk & Nuij, 2003): the
/// perceptually shortest path between two views zooms out, pans, then zooms in.
pub struct SmoothPath {
    pub length: f64,
    a: View,
    dx: f64,
    dy: f64,
    d: f64,
    rho: f64,
    r0: f64,
    /// A pure zoom: log of the width ratio.
    zoom: Option<f64>,
}

impl SmoothPath {
    pub fn new(a: View, b: View) -> Self {
        let rho = 1.35;
        let (dx, dy) = (b.cx - a.cx, b.cy - a.cy);
        let d = dx.hypot(dy);
        let (w0, w1) = (a.w, b.w);
        if d < 1e-9 * w0.max(w1) {
            let zoom = w1.ln() - w0.ln();
            return SmoothPath { length: zoom.abs() / rho, a, dx, dy, d, rho, r0: 0.0, zoom: Some(zoom) };
        }
        // The paper writes -asinh(b) as log(-b + sqrt(b² + 1)), which cancels to
        // log 0 once the views are more than about 10⁸ apart in zoom.
        let r = |wi: f64, sign: f64| -((w1 * w1 - w0 * w0 + sign * rho.powi(4) * d * d) / (2.0 * wi * rho * rho * d)).asinh();
        let (r0, r1) = (r(w0, 1.0), r(w1, -1.0));
        SmoothPath { length: (r1 - r0) / rho, a, dx, dy, d, rho, r0, zoom: None }
    }

    pub fn at(&self, t: f64) -> View {
        let a = self.a;
        if let Some(zoom) = self.zoom {
            return View { cx: a.cx, cy: a.cy, w: a.w * (zoom * t).exp() };
        }
        let (rho, r0) = (self.rho, self.r0);
        let s = rho * t * self.length;
        // cosh(r0)·tanh(s + r0) − sinh(r0) as one ratio: its two terms are huge
        // and nearly equal at a deep start, and their difference is noise.
        let u = a.w / (rho * rho * self.d) * s.sinh() / (s + r0).cosh();
        let w = a.w * (r0.cosh() / (s + r0).cosh());
        View { cx: a.cx + self.dx * u, cy: a.cy + self.dy * u, w }
    }
}

/// How a flight frames its target: `Open` reads it (zooming until its diagram
/// is legible), `Fit` shows all of it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Framing {
    Open,
    Fit,
}

/// A planned camera move: how long it takes, where it starts from (the part
/// of the path both ends share) and the views it passes.
pub struct Plan {
    pub duration: f64,
    /// How far it goes (0: nowhere).
    pub length: f64,
    base: Vec<u32>,
    path: SmoothPath,
}

pub struct Camera {
    pub path: Vec<u32>,
    pub xf: Xf,
    /// Overlays covering the screen's edges; framing centers in what is left.
    pub insets: Insets,
}

impl Camera {
    pub fn new(m: &Model) -> Self {
        Camera { path: vec![m.root()], xf: Xf { x: 0.0, y: 0.0, s: 1.0 }, insets: Insets::default() }
    }

    /// Interior transforms along the anchor path, root first.
    pub fn transforms(&self, m: &Model) -> Vec<Xf> {
        let mut out = vec![self.xf; self.path.len()];
        for i in (1..self.path.len()).rev() {
            out[i - 1] = m.exterior(out[i], m.node(self.path[i]));
        }
        out
    }

    pub fn zoom(&mut self, m: &Model, factor: f64, px: f64, py: f64) {
        // Out, relative to the whole scene; in, relative to the innermost
        // diagram under the point, past which there is nothing to see.
        let root_scale = self.transforms(m)[0].s * factor;
        if root_scale < 0.02 && factor < 1.0 {
            return;
        }
        if factor > 1.0 && self.scale_at(m, px, py) * factor > 50.0 {
            return;
        }
        self.xf = zoom_at(self.xf, factor, px, py);
    }

    pub fn pan(&mut self, dx: f64, dy: f64) {
        self.xf.x += dx;
        self.xf.y += dy;
    }

    /// The scale of the innermost diagram under a screen point.
    fn scale_at(&self, m: &Model, px: f64, py: f64) -> f64 {
        let mut owners: Vec<Option<u32>> = self.path.iter().map(|&id| m.owner(m.node(id)).map(|o| o.id)).collect();
        let mut t = self.xf;
        let mut owner = *owners.last().unwrap();
        while let Some(o) = owner {
            let (u, v) = ((px - t.x) / t.s, (py - t.y) / t.s);
            let hit = m.node(o).children.iter().copied().find(|&c| {
                let n = m.node(c);
                if u < n.x || v < n.y || u > n.x + n.w || v > n.y + n.h {
                    return false;
                }
                m.owner(n).is_some_and(|o| !owners.contains(&Some(o.id)))
            });
            let Some(c) = hit else { break };
            let n = m.node(c);
            t = m.interior(t, n).unwrap();
            owner = m.owner(n).map(|o| o.id);
            owners.push(owner);
        }
        t.s
    }

    /// Moves the anchor up while it no longer fills the screen, and down into any child that does.
    pub fn reanchor(&mut self, m: &Model, width: f64, height: f64) {
        while self.path.len() > 1 {
            let xfs = self.transforms(m);
            let n = m.node(*self.path.last().unwrap());
            if covers(box_of(xfs[xfs.len() - 2], n), width, height) {
                break;
            }
            self.xf = xfs[xfs.len() - 2];
            self.path.pop();
        }
        for _ in 0..96 {
            let here = m.node(*self.path.last().unwrap());
            let Some(owner) = m.owner(here) else { break };
            let next = owner.children.iter().copied().find(|&c| {
                let n = m.node(c);
                let Some(o) = m.owner(n) else { return false };
                // Not back into code the path is already in (a recursive call).
                let again = self.path.iter().any(|&id| m.owner(m.node(id)).is_some_and(|p| p.id == o.id) && o.id != n.id);
                !again && covers(box_of(self.xf, n), width, height)
            });
            let Some(next) = next else { break };
            self.xf = m.interior(self.xf, m.node(next)).unwrap();
            self.path.push(next);
        }
    }

    /// The node at the end of `path` in the interior frame of `path[up_to]`:
    /// its box, the scale of the diagram it sits in, and of its own interior.
    fn place(&self, m: &Model, path: &[u32], up_to: usize) -> (Rect, f64, Option<f64>) {
        let mut t = Xf { x: 0.0, y: 0.0, s: 1.0 };
        for &id in &path[up_to + 1..path.len() - 1] {
            t = m.interior(t, m.node(id)).unwrap_or(t);
        }
        let node = m.node(*path.last().unwrap());
        (box_of(t, node), t.s, m.interior(t, node).map(|i| i.s))
    }

    /// Plans a smooth flight framing the node at the end of `target` (an instance path).
    pub fn flight(&self, m: &Model, target: &[u32], width: f64, height: f64, how: Framing, pad: f64) -> Plan {
        let mut common = self.path.iter().zip(target).take_while(|(a, b)| a == b).count();
        // Fitting a block shows all of it, its title too: it is framed in the diagram around it.
        if how == Framing::Fit && target.len() > 1 {
            common = common.min(target.len() - 1);
        }
        let lca = common.saturating_sub(1);
        let from = view_of(self.transforms(m)[lca], width, height);
        let to = self.framing(m, target, lca, width, height, how, pad);
        Self::plan(target[..=lca].to_vec(), SmoothPath::new(from, to))
    }

    fn plan(base: Vec<u32>, path: SmoothPath) -> Plan {
        Plan { duration: (path.length * 260.0).clamp(320.0, 1400.0), length: path.length, base, path }
    }

    /// Plans a flight back to a saved camera state (on the same scene).
    pub fn flight_to(&self, m: &Model, path: &[u32], xf: Xf, width: f64, height: f64) -> Plan {
        let common = self.path.iter().zip(path).take_while(|(a, b)| a == b).count();
        let lca = common.saturating_sub(1);
        // The saved transform, carried up to the common ancestor's interior.
        let mut t = xf;
        for i in (lca + 1..path.len()).rev() {
            t = m.exterior(t, m.node(path[i]));
        }
        let from = view_of(self.transforms(m)[lca], width, height);
        Self::plan(path[..=lca].to_vec(), SmoothPath::new(from, view_of(t, width, height)))
    }

    /// Positions the camera `t` (0…1) of the way along a plan.
    pub fn step(&mut self, m: &Model, plan: &Plan, t: f64, width: f64, height: f64) {
        self.path = plan.base.clone();
        self.xf = xf_of(plan.path.at(t), width, height);
        self.reanchor(m, width, height);
    }

    /// Where a flight to `target` ends, in the interior frame of `target[lca]`.
    #[allow(clippy::too_many_arguments)]
    fn framing(&self, m: &Model, target: &[u32], lca: usize, width: f64, height: f64, how: Framing, pad: f64) -> View {
        if target.len() - 1 <= lca {
            if how == Framing::Fit {
                return self.content_view(m, target[lca], width, height, pad);
            }
            let n = m.node(target[lca]);
            let owner = m.owner(n).unwrap_or(n);
            return frame_readable(Rect { x: 0.0, y: 0.0, w: owner.cw.max(1.0), h: owner.ch.max(1.0) }, 1.0, width, height, pad, self.insets);
        }
        let (b, outer, inner) = self.place(m, target, lca);
        let node = m.node(*target.last().unwrap());
        // A block with nothing inside is shown among its neighbors, not blown up alone.
        if m.owner(node).is_none() {
            return frame_around(b, outer, width, height, self.insets);
        }
        match (how, inner) {
            (Framing::Open, Some(inner)) => frame_readable(b, inner, width, height, pad, self.insets),
            _ => frame_in(b, width, height, pad, self.insets),
        }
    }

    /// View framing a node's whole interior in its own interior frame.
    pub fn content_view(&self, m: &Model, id: u32, width: f64, height: f64, pad: f64) -> View {
        let n = m.node(id);
        let owner = m.owner(n).unwrap_or(n);
        frame_in(Rect { x: 0.0, y: 0.0, w: owner.cw.max(1.0), h: owner.ch.max(1.0) }, width, height, pad, self.insets)
    }

    pub fn fit_root(&mut self, m: &Model, width: f64, height: f64) {
        self.path = vec![m.root()];
        self.xf = xf_of(self.content_view(m, m.root(), width, height, 0.05), width, height);
    }

    /// The anchor path extended into the container that dominates the view: it
    /// contains the center of what the overlays leave visible and fills a good
    /// part of it, or all of it in one direction (as fitting it does). What
    /// the breadcrumbs show.
    pub fn focus(&self, m: &Model, width: f64, height: f64) -> Vec<u32> {
        let mut path = self.path.clone();
        let mut t = self.xf;
        let i = self.insets;
        let (x0, y0) = (i.left, i.top);
        let x1 = (width - i.right).max(x0 + width * 0.3);
        let y1 = (height - i.bottom).max(y0 + height * 0.3);
        let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        let area = (x1 - x0) * (y1 - y0);
        for _ in 0..64 {
            let Some(owner) = m.owner(m.node(*path.last().unwrap())) else { break };
            let next = owner.children.iter().copied().find(|&c| {
                let n = m.node(c);
                if m.owner(n).is_none() {
                    return false;
                }
                let b = box_of(t, n);
                let w = ((b.x + b.w).min(x1) - b.x.max(x0)).max(0.0);
                let h = ((b.y + b.h).min(y1) - b.y.max(y0)).max(0.0);
                let fills = w * h >= area * 0.3 || (w * h >= area * 0.15 && (w >= (x1 - x0) * 0.85 || h >= (y1 - y0) * 0.85));
                b.contains(cx, cy) && fills
            });
            let Some(next) = next else { break };
            let Some(inner) = m.interior(t, m.node(next)) else { break };
            t = inner;
            path.push(next);
        }
        path
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, eps: f64) -> bool {
        (a - b).abs() <= eps * a.abs().max(b.abs()).max(1.0)
    }

    #[test]
    fn zooming_keeps_the_point_under_the_cursor_fixed() {
        let t = Xf { x: 100.0, y: 50.0, s: 2.0 };
        let z = zoom_at(t, 3.0, 400.0, 300.0);
        let u = (400.0 - t.x) / t.s;
        assert!(close(z.x + u * z.s, 400.0, 1e-12));
    }

    #[test]
    fn views_round_trip_through_transforms() {
        let t = Xf { x: -120.0, y: 33.0, s: 1.7 };
        let back = xf_of(view_of(t, 1200.0, 800.0), 1200.0, 800.0);
        assert!(close(back.x, t.x, 1e-9) && close(back.y, t.y, 1e-9) && close(back.s, t.s, 1e-12));
    }

    #[test]
    fn frames_a_box_with_padding_and_the_screen_aspect() {
        let v = frame(Rect { x: 0.0, y: 0.0, w: 100.0, h: 100.0 }, 1600.0, 800.0, 0.1);
        assert_eq!(v.cx, 50.0);
        assert!(close(v.w, 200.0 * 1.2, 1e-9));
    }

    #[test]
    fn smooth_paths_start_and_end_at_the_given_views_and_zoom_out_between() {
        let a = View { cx: 0.0, cy: 0.0, w: 1000.0 };
        let b = View { cx: 5000.0, cy: -300.0, w: 20.0 };
        let p = SmoothPath::new(a, b);
        let (s, e) = (p.at(0.0), p.at(1.0));
        assert!(close(s.cx, a.cx, 1e-9) && close(s.w, a.w, 1e-9));
        assert!(close(e.cx, b.cx, 1e-6) && close(e.w, b.w, 1e-6) && close(e.cy, b.cy, 1e-6));
        assert!([0.25, 0.5, 0.75].iter().map(|&t| p.at(t).w).fold(0.0, f64::max) > a.w);
    }

    #[test]
    fn smooth_paths_stay_finite_between_views_far_apart_in_zoom() {
        let deep = View { cx: 12.3456789, cy: 45.678, w: 1e-17 };
        let far = View { cx: 300.0, cy: -40.0, w: 1e3 };
        for (a, b) in [(deep, far), (far, deep)] {
            let p = SmoothPath::new(a, b);
            for k in 0..=20 {
                let v = p.at(k as f64 / 20.0);
                assert!(v.cx.is_finite() && v.cy.is_finite() && v.w.is_finite() && v.w > 0.0, "{v:?}");
            }
            assert!(close(p.at(0.0).cx, a.cx, 1e-12) && close(p.at(1.0).w, b.w, 1e-6));
        }
    }

    #[test]
    fn a_flight_opens_a_block_and_home_comes_back_out() {
        let files = [
            ("model.py", "class Block:\n    def forward(self, x):\n        x = x + self.attn(x)\n        x = x + self.mlp(x)\n        return x\n"),
            ("train.py", "from model import Block\nfor step in range(10):\n    b = Block()\n    b.forward(step)\n"),
        ];
        let inputs = files.iter().map(|(p, t)| blockviz_core::project::InFile { path: p.to_string(), text: Some(t.to_string()), bytes: t.len() as u64 }).collect();
        let m = Model::new(blockviz_core::analyze("t", inputs), files.iter().map(|(p, t)| (p.to_string(), t.to_string())).collect());
        let (w, h) = (1200.0, 800.0);
        let mut cam = Camera::new(&m);
        cam.fit_root(&m, w, h);
        let forward = m.scene.nodes.iter().find(|n| n.title == "forward").unwrap().id;
        let target = m.canonical_path(forward);
        let plan = cam.flight(&m, &target, w, h, Framing::Open, 0.08);
        assert!(plan.duration >= 320.0 && plan.duration <= 1400.0);
        cam.step(&m, &plan, 1.0, w, h);
        assert_eq!(cam.focus(&m, w, h).last(), Some(&forward), "{:?}", cam.focus(&m, w, h));
        let home = cam.flight(&m, &[m.root()], w, h, Framing::Fit, 0.05);
        cam.step(&m, &home, 1.0, w, h);
        assert_eq!(cam.path, [m.root()]);
        let mut fitted = Camera::new(&m);
        fitted.fit_root(&m, w, h);
        assert!(close(cam.xf.s, fitted.xf.s, 1e-9) && close(cam.xf.x, fitted.xf.x, 1e-6));
    }

    #[test]
    fn readable_framing_starts_at_the_top_of_a_tall_box() {
        let b = Rect { x: 0.0, y: 0.0, w: 100.0, h: 10_000.0 };
        let v = frame_readable(b, 0.001, 1200.0, 800.0, 0.08, Insets::default());
        let t = xf_of(v, 1200.0, 800.0);
        // The box's top is just under the top of the screen, its interior readable or as wide as it can be.
        assert!(close(t.y + b.y * t.s, 16.0, 1e-9));
    }
}
