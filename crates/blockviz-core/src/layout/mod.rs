//! Bottom-up layout of the whole scene tree.
//!
//! Each node that owns an interior is laid out once; its size in the parent
//! then follows from that interior. Small control-flow regions are drawn
//! inline at the parent's scale (like the "N×" box in an architecture figure);
//! named abstractions (files, classes, functions, sections, calls into project
//! code) become boxes whose interior is fitted at a reduced scale and revealed
//! by zooming.

pub mod layered;

use crate::ir::*;
use crate::text::{mono_width, sans_width};
use layered::{LEdge, LNode, Pin};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

pub const LEAF_TITLE: f64 = 13.;
pub const LEAF_META: f64 = 11.;
pub const BOX_HEADER: f64 = 30.;
pub const BOX_PAD: f64 = 8.;
pub const REGION_HEADER: f64 = 26.;
pub const REGION_PAD: f64 = 12.;
pub const ROW_H: f64 = 17.;
pub const HEADING_H: f64 = 22.;
const TARGET_SCALE: f64 = 0.24;
const MAX_INNER_W: f64 = 400.;
const MAX_INNER_H: f64 = 300.;
const INLINE_TABLE_ROWS: usize = 8;

#[derive(Clone, Copy, PartialEq)]
enum State {
    Todo,
    Busy,
    Done,
}

/// How diagrams are drawn, where there is a choice of style.
#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    /// All values a block sends leave on one trunk that branches to each
    /// reader, every branch labelled where it arrives. Off: one line per
    /// value, labelled under the block it leaves.
    pub shared_trunks: bool,
}

pub fn layout(b: &mut Builder, root: Id, opts: Options) {
    let mut st = vec![State::Todo; b.nodes.len()];
    content(b, root, &mut st, opts);
    let r = b.get_mut(root);
    r.w = r.cw;
    r.h = r.ch;
}

/// Whether a node is drawn as a named, zoomable box rather than an inline region.
fn wants_box(n: &Node) -> bool {
    match n.kind {
        Kind::Repo | Kind::Dir | Kind::File | Kind::Notebook | Kind::Class | Kind::Function | Kind::Section => true,
        Kind::Table => n.rows.len() > INLINE_TABLE_ROWS,
        Kind::Loop | Kind::Branch | Kind::Lane | Kind::Group => false,
        _ => n.reference.is_some() || n.has_interior(),
    }
}

/// Visible units inside a would-be inline region (boxes count once).
fn units(b: &Builder, id: Id) -> usize {
    let n = b.get(id);
    n.children
        .iter()
        .map(|&c| {
            let c = b.get(c);
            if c.is_port() {
                0
            } else if wants_box(c) || !c.has_interior() {
                1
            } else {
                units(b, c.id) + 1
            }
        })
        .sum()
}

fn decide_shape(b: &mut Builder, id: Id) {
    let n = b.get(id);
    if n.shape == Shape::Glyph || n.shape == Shape::Port {
        return;
    }
    let inline = match n.kind {
        Kind::Lane | Kind::Group => true,
        Kind::Branch => units(b, id) <= 18,
        Kind::Loop => units(b, id) <= 10,
        Kind::Table => n.rows.len() <= INLINE_TABLE_ROWS,
        _ => false,
    };
    let has = n.has_interior() || n.reference.is_some();
    let n = b.get_mut(id);
    n.hints.inline = inline && has;
    n.shape = if n.hints.inline { Shape::Region } else { Shape::Box };
}

/// Lays out the interior owned by `id` (children or rows) and records cw/ch.
fn content(b: &mut Builder, id: Id, st: &mut Vec<State>, opts: Options) {
    if st[id as usize] != State::Todo {
        return;
    }
    st[id as usize] = State::Busy;
    if !b.get(id).rows.is_empty() {
        let (w, h) = table_size(&b.get(id).rows);
        let n = b.get_mut(id);
        n.cw = w;
        n.ch = h;
        st[id as usize] = State::Done;
        return;
    }
    let children = b.get(id).children.clone();
    let inline_parent = b.get(id).hints.inline;
    for &c in &children {
        let is_port = b.get(c).is_port();
        if is_port {
            let n = b.get_mut(c);
            n.hints.hidden = inline_parent;
        }
        size(b, c, st, opts);
    }
    let lanes = b.get(id).hints.lanes;
    let lnodes: Vec<LNode> = children
        .iter()
        .map(|&c| {
            let n = b.get(c);
            LNode {
                w: n.w,
                h: n.h,
                pin: match n.kind {
                    Kind::Input => Pin::Top,
                    Kind::Output => Pin::Bottom,
                    _ => Pin::Free,
                },
                order: n.hints.order as f64,
                rank: if lanes && !n.is_port() { Some(n.hints.order) } else { None },
                converge: n.shape == Shape::Glyph || (n.is_port() && n.hints.hidden),
                pack: n.hints.pack,
            }
        })
        .collect();
    let index: std::collections::HashMap<Id, usize> = children.iter().enumerate().map(|(i, &c)| (c, i)).collect();
    // The edges are taken, laid out and put back; only those between children are drawn.
    let edges = std::mem::take(&mut b.get_mut(id).edges);
    let mut edges = bundle(b, edges);
    edges.retain(|e| index.contains_key(&e.from) && index.contains_key(&e.to));
    let mut ledges = vec![];
    for e in &edges {
        let (from, to) = (index[&e.from], index[&e.to]);
        let to_x = e.to_port.and_then(|p| port_offset(b, e.to, p));
        let from_x = e.from_port.and_then(|p| port_offset(b, e.from, p));
        let src = b.get(e.from);
        // Every arrow says what it carries, unless the port it leaves already shows the name.
        let show_label = !e.label.is_empty() && !(src.is_port() && !src.hints.hidden && src.title == e.label);
        let mut h = DefaultHasher::new();
        // Values a block sends share a line when they are one value, or always
        // with shared trunks.
        if opts.shared_trunks {
            (e.kind as u8, from_x.map(|v| v.to_bits())).hash(&mut h);
        } else {
            (e.label.as_str(), e.kind as u8, from_x.map(|v| v.to_bits())).hash(&mut h);
        }
        ledges.push(LEdge {
            from,
            to,
            group: h.finish(),
            from_x,
            to_x,
            weight: match e.kind {
                EdgeKind::Control => 0.3,
                _ => 1.,
            },
            draw: true,
            label: show_label,
            label_w: if show_label { mono_width(&e.label, 11.) } else { 0. },
        });
    }
    for &c in &children {
        for &a in &b.get(c).hints.after.clone() {
            if let (Some(&from), Some(&to)) = (index.get(&a), index.get(&c)) {
                ledges.push(LEdge { from, to, group: 0, from_x: None, to_x: None, weight: 0.5, draw: false, label: false, label_w: 0. });
            }
        }
    }
    let out = layered::layout(&lnodes, &ledges, opts.shared_trunks);
    for (i, &c) in children.iter().enumerate() {
        let n = b.get_mut(c);
        n.x = out.pos[i].0;
        n.y = out.pos[i].1;
    }
    let (cw, ch) = (out.width.max(1.), out.height.max(1.));
    let mut new_edges = Vec::with_capacity(edges.len());
    // Routes and labels follow the drawn edges, then the layout-only ones.
    for ((mut e, mut pts), label) in edges.into_iter().zip(out.routes).zip(out.labels) {
        if pts.len() < 2 {
            continue;
        }
        // Hidden pass-through ports continue the line through the region border.
        if b.get(e.from).is_port() && b.get(e.from).hints.hidden {
            pts.insert(0, (pts[0].0, -region_header(b.get(id))));
        }
        if b.get(e.to).is_port() && b.get(e.to).hints.hidden {
            let last = *pts.last().unwrap();
            pts.push((last.0, ch + REGION_PAD));
            e.open = true;
        }
        if let Some(p) = e.to_port {
            if b.get(e.to).hints.inline && b.get(p).hints.hidden {
                e.open = true;
            }
        }
        e.points = pts.iter().flat_map(|&(x, y)| [round(x), round(y)]).collect();
        e.label_at = label.map(|(x, y)| [round(x), round(y)]);
        new_edges.push(e);
    }
    let n = b.get_mut(id);
    n.edges = new_edges;
    n.cw = cw;
    n.ch = ch;
    st[id as usize] = State::Done;
}

/// Values entering or leaving a collapsed box travel as one bundled edge;
/// only inline regions keep one line per port.
fn bundle(b: &Builder, edges: Vec<Edge>) -> Vec<Edge> {
    let mut out: Vec<Edge> = vec![];
    // Edges by their ends, so a region with thousands of them bundles in linear time.
    let mut at: std::collections::HashMap<(Id, Id, Option<Id>, Option<Id>), usize> = std::collections::HashMap::new();
    for mut e in edges {
        if e.to_port.is_some() && !b.get(e.to).hints.inline {
            e.to_port = None;
        }
        if e.from_port.is_some() && !b.get(e.from).hints.inline {
            e.from_port = None;
        }
        let key = (e.from, e.to, e.to_port, e.from_port);
        match at.get(&key).map(|&i| &mut out[i]) {
            Some(o) => {
                if e.kind < o.kind {
                    o.kind = e.kind;
                }
                for l in e.label.split(", ").filter(|l| !l.is_empty()) {
                    if !o.label.split(", ").any(|x| x == l) {
                        if !o.label.is_empty() {
                            o.label.push_str(", ");
                        }
                        o.label.push_str(l);
                    }
                }
            }
            None => {
                at.insert(key, out.len());
                out.push(e);
            }
        }
    }
    for e in &mut out {
        let parts: Vec<&str> = e.label.split(", ").collect();
        if parts.len() > 3 {
            e.label = format!("{}, +{}", parts[..2].join(", "), parts.len() - 2);
        }
    }
    out
}

fn round(v: f64) -> f64 {
    (v * 10.).round() / 10.
}

/// x offset (from the region's left edge) of a hidden pass-through port.
fn port_offset(b: &Builder, region: Id, port: Id) -> Option<f64> {
    let r = b.get(region);
    let p = b.get(port);
    if !r.hints.inline || p.parent != Some(region) || !p.hints.hidden {
        return None;
    }
    let inner = r.inner?;
    Some(inner.x + p.x + p.w / 2.)
}

fn size(b: &mut Builder, id: Id, st: &mut Vec<State>, opts: Options) {
    decide_shape(b, id);
    let n = b.get(id);
    match n.shape {
        Shape::Glyph => {
            let n = b.get_mut(id);
            n.w = 24.;
            n.h = 24.;
            return;
        }
        Shape::Port => {
            let hidden = n.hints.hidden;
            let w = sans_width(&n.title, 12., false) + 6.;
            let n = b.get_mut(id);
            if hidden {
                n.w = 2.;
                n.h = 0.;
            } else {
                n.w = w.max(14.);
                n.h = 18.;
            }
            return;
        }
        _ => {}
    }
    // Which node's interior does this one show?
    let owner = if n.has_interior() { Some(id) } else { n.reference.filter(|&r| b.get(r).has_interior()) };
    if let Some(o) = owner {
        if o != id && st[o as usize] == State::Busy {
            // Recursive reference: draw as a leaf that links to its definition.
            leaf_size(b, id);
            return;
        }
        content(b, o, st, opts);
        let (cw, ch) = (b.get(o).cw, b.get(o).ch);
        if b.get(id).hints.inline {
            region_size(b, id, cw, ch);
        } else {
            box_size(b, id, cw, ch);
        }
    } else {
        leaf_size(b, id);
    }
}

fn meta_line(n: &Node) -> String {
    let mut parts: Vec<&str> = vec![];
    if !n.subtitle.is_empty() {
        parts.push(&n.subtitle);
    }
    for badge in &n.badges {
        parts.push(badge);
    }
    parts.join(" · ")
}

fn leaf_size(b: &mut Builder, id: Id) {
    let n = b.get(id);
    // Code-like steps are drawn in monospace.
    let title = if matches!(n.kind, Kind::Op | Kind::Table) { mono_width(&n.title, 12.) } else { sans_width(&n.title, LEAF_TITLE, true) };
    let meta = meta_line(n);
    let meta_w = sans_width(&meta, LEAF_META, false);
    // Files on disk and other named things read as proper blocks, not chips.
    let min_w = if matches!(n.kind, Kind::Artifact | Kind::File | Kind::Doc | Kind::Asset | Kind::Symbol) { 110. } else { 64. };
    let w = (title.max(meta_w) + 28.).clamp(min_w, 260.);
    let h = if meta.is_empty() { 36. } else { 48. };
    let n = b.get_mut(id);
    n.w = w.round();
    n.h = h;
    n.inner = None;
}

fn header_width(n: &Node) -> f64 {
    let title = sans_width(&n.title, 13., true);
    let meta = meta_line(n);
    let meta_w = if meta.is_empty() { 0. } else { sans_width(&meta, 11., false) + 10. };
    title + meta_w + 2. * BOX_PAD + 16.
}

fn box_size(b: &mut Builder, id: Id, cw: f64, ch: f64) {
    let n = b.get(id);
    let min_inner_w = (header_width(n).min(320.) - 2. * BOX_PAD).max(110.);
    let mut s = TARGET_SCALE;
    if cw * s < min_inner_w {
        s = (min_inner_w / cw).min(0.85);
    }
    if cw * s > MAX_INNER_W {
        s = MAX_INNER_W / cw;
    }
    if ch * s > MAX_INNER_H {
        s = MAX_INNER_H / ch;
    }
    // Keep tiny interiors from dominating their parent, and very tall ones
    // from becoming slivers (the interior is centred in a wider frame).
    let ih = (ch * s).max(26.);
    let iw = (cw * s).max(min_inner_w).max((ih * 0.62).min(260.));
    let n = b.get_mut(id);
    n.w = (iw + 2. * BOX_PAD).round();
    n.h = (BOX_HEADER + ih + BOX_PAD).round();
    n.inner = Some(Rect { x: BOX_PAD, y: BOX_HEADER, w: n.w - 2. * BOX_PAD, h: n.h - BOX_HEADER - BOX_PAD });
}

/// Untitled regions (a branch holding titled lanes) need no header strip.
pub fn region_header(n: &Node) -> f64 {
    if n.title.is_empty() {
        REGION_PAD
    } else {
        REGION_HEADER
    }
}

fn region_size(b: &mut Builder, id: Id, cw: f64, ch: f64) {
    let n = b.get(id);
    let top = region_header(n);
    let header = if n.title.is_empty() { 0. } else { header_width(n) - 2. * BOX_PAD + 2. * REGION_PAD };
    let w = (cw + 2. * REGION_PAD).max(header);
    let n = b.get_mut(id);
    n.w = w.round();
    n.h = (top + ch + REGION_PAD).round();
    let x = ((n.w - cw) / 2.).max(REGION_PAD);
    n.inner = Some(Rect { x, y: top, w: cw, h: ch });
}

fn table_size(rows: &[Row]) -> (f64, f64) {
    let mut w: f64 = 120.;
    let mut h = 6.;
    for r in rows {
        if r.heading {
            w = w.max(sans_width(&r.key, 11., true) + 20.);
            h += HEADING_H;
        } else {
            let text = format!("{} = {}", r.key, r.value);
            w = w.max(mono_width(&text, 11.5) + 20.);
            h += ROW_H;
        }
    }
    (w.min(480.).round(), h + 6.)
}
