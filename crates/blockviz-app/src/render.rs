//! Immediate-mode renderer for nested diagrams, ported line for line from
//! src/canvas/renderer.ts. Every frame walks only what is visible: containers
//! are culled against the viewport, interiors are entered only when they are
//! large enough to show something, and text fades in by its on-screen size.
//! Hit regions are recorded as a side effect of drawing.

use crate::camera::{box_of, Rect, Xf};
use crate::lines::{rect_line, visible_runs};
use crate::model::Model;
use crate::pen::{Align, Family, Font, Pen};
use crate::theme::{mix, Color, FlowRole, Theme};
use blockviz_core::ir::{Edge, EdgeKind, Kind, Node, Shape};
use std::collections::{HashMap, HashSet};
use std::f64::consts::PI;

/// How a change touched a block (a change being read).
pub use crate::diff::Status as Mark;

/// An edge, as the block whose diagram it is in and its index among that block's edges.
pub type EdgeId = (u32, usize);

use crate::diff::RemovedEdge as GoneEdge;

/// How the code on screen differs from a base version (a change being read).
pub trait Changes {
    fn status(&self, id: u32) -> Option<Mark>;
    /// Whether a block changed or has changes inside.
    fn touched(&self, id: u32) -> bool;
    /// Changes inside a block, not counting its own.
    fn inside(&self, id: u32) -> usize;
    /// Changes in the code a block leads to.
    fn through(&self, id: u32) -> usize;
    /// A connection the change made.
    fn added(&self, edge: EdgeId) -> bool;
    /// Connections the change removed from a block's diagram.
    fn removed(&self, owner: u32) -> &[GoneEdge];
}

impl Changes for crate::diff::SceneDiff {
    fn status(&self, id: u32) -> Option<Mark> {
        crate::diff::SceneDiff::status(self, id)
    }
    fn touched(&self, id: u32) -> bool {
        crate::diff::SceneDiff::touched(self, id)
    }
    fn inside(&self, id: u32) -> usize {
        crate::diff::SceneDiff::inside(self, id)
    }
    fn through(&self, id: u32) -> usize {
        crate::diff::SceneDiff::through(self, id)
    }
    fn added(&self, edge: EdgeId) -> bool {
        self.added.contains(&edge)
    }
    fn removed(&self, owner: u32) -> &[GoneEdge] {
        self.removed.get(&owner).map_or(&[], Vec::as_slice)
    }
}

/// A value being followed: the diagrams it is traced in and the edges carrying it.
#[derive(Clone, Debug, Default)]
pub struct Trace {
    pub scope: HashSet<u32>,
    pub edges: HashSet<EdgeId>,
}

/// A block drawn this frame. `clear` when the reader can see what it is (its
/// name is legible, or the box is large), so clicks never land on a block
/// too small to make out. `face` when its name is on its face: it is closed,
/// or what is drawn inside it is only a thumbnail.
#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub id: u32,
    pub rect: Rect,
    pub parent: i32,
    pub clear: bool,
    pub face: bool,
}

/// An edge drawn this frame: the hit of the box whose interior it is in (-1
/// for the root), and its route on screen.
#[derive(Clone, Debug)]
pub struct EdgeHit {
    pub edge: EdgeId,
    pub container: i32,
    pub pts: Vec<f64>,
}

/// What is drawn: the view, and what is in focus, followed or changed in it.
pub struct Paint<'a> {
    pub width: f64,
    pub height: f64,
    pub hover: Option<u32>,
    pub selected: Option<u32>,
    /// Anchor path (root first) and the interior transform of each element.
    pub path: &'a [u32],
    pub xfs: &'a [Xf],
    /// Blocks that use the one in focus, outlined.
    pub related: Option<&'a HashSet<u32>>,
    /// A value followed through its diagram: its edges lit, the rest faded.
    pub trace: Option<&'a Trace>,
    /// The edge under the pointer.
    pub hover_edge: Option<EdgeId>,
    /// How the code differs from a base version: what changed is colored, the rest faded.
    pub diff: Option<&'a dyn Changes>,
}

/// How faint what a change did not touch is drawn.
const UNTOUCHED: f64 = 0.4;
const ROW_H: f64 = 17.0;
/// Largest on-screen size of a table's rows, as for names.
const ROW_MAX: f64 = 15.0;
const HEADING_H: f64 = 22.0;
/// Interior scale at which a box starts / finishes opening.
const OPEN_FROM: f64 = 0.028;
const OPEN_TO: f64 = 0.075;
/// The smallest name, in pixels, a reader can make out.
const LEGIBLE: f64 = 7.0;
/// The most lines a name is broken over.
const MAX_LINES: usize = 4;
/// The shortest side, in pixels, of a block drawn as more than a spot of its color.
const SPECK: f64 = 2.0;
/// How far past the screen a line is kept: far enough that the corners of what is cut off stay out of sight.
const CUT: f64 = 24.0;

fn code_kind(k: Kind) -> bool {
    matches!(k, Kind::Op | Kind::Loop | Kind::Branch)
}

/// Boxes whose names are navigation landmarks and stay readable longest.
fn named_kind(k: Kind) -> bool {
    matches!(k, Kind::File | Kind::Notebook | Kind::Dir | Kind::Class | Kind::Function | Kind::Section | Kind::Artifact | Kind::Doc | Kind::Table)
}

/// Keywords that open a loop's or a branch's title, and what they do. A
/// keyword that starts another (`else if`, `else`) comes first.
const FLOW_KEYWORDS: [(&str, FlowRole); 19] = [
    ("else if", FlowRole::Branch),
    ("for", FlowRole::Loop),
    ("while", FlowRole::Loop),
    ("repeat", FlowRole::Loop),
    ("loop", FlowRole::Loop),
    ("if", FlowRole::Branch),
    ("elif", FlowRole::Branch),
    ("else", FlowRole::Branch),
    ("unless", FlowRole::Branch),
    ("match", FlowRole::Branch),
    ("switch", FlowRole::Branch),
    ("select", FlowRole::Branch),
    ("case", FlowRole::Branch),
    ("default", FlowRole::Branch),
    ("otherwise", FlowRole::Branch),
    ("try", FlowRole::Attempt),
    ("finally", FlowRole::Attempt),
    ("except", FlowRole::Catch),
    ("catch", FlowRole::Catch),
];

/// A loop's or a branch's title as its keyword, what follows it and what it
/// does: `for block in blocks` is `for`, `block in blocks`, a loop.
pub fn flow_keyword(title: &str) -> Option<(&'static str, &str, FlowRole)> {
    FLOW_KEYWORDS.iter().find_map(|&(k, role)| {
        let rest = title.strip_prefix(k)?;
        (rest.is_empty() || rest.starts_with(' ')).then(|| (k, rest.trim_start(), role))
    })
}

/// What a word of code on the diagram is, for its color.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tok {
    Plain,
    /// A word joining a condition's or a loop's parts: `in`, `is not`, `and`.
    Keyword,
    /// `None`, `true`, `3`.
    Literal,
    Str,
}

/// Words that join the parts of a condition or a loop's header, in the languages read.
const JOINERS: [&str; 16] = ["in", "of", "is", "not", "and", "or", "if", "else", "for", "as", "await", "instanceof", "typeof", "let", "unless", "until"];
const LITERALS: [&str; 10] = ["None", "True", "False", "true", "false", "null", "nil", "undefined", "NaN", "Infinity"];

/// Code cut into the runs drawn in one color each.
pub fn code_tokens(text: &str) -> Vec<(&str, Tok)> {
    let mut runs: Vec<(usize, usize, Tok)> = vec![];
    let mut chars = text.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        let tok = if matches!(c, '"' | '\'' | '`') {
            let mut escaped = false;
            for (_, d) in chars.by_ref() {
                if escaped {
                    escaped = false;
                } else if d == '\\' {
                    escaped = true;
                } else if d == c {
                    break;
                }
            }
            Tok::Str
        } else if c.is_alphanumeric() || c == '_' {
            while chars.next_if(|&(_, d)| d.is_alphanumeric() || d == '_').is_some() {}
            let word = &text[start..chars.peek().map_or(text.len(), |&(j, _)| j)];
            if c.is_ascii_digit() || LITERALS.contains(&word) {
                Tok::Literal
            } else if JOINERS.contains(&word) {
                Tok::Keyword
            } else {
                Tok::Plain
            }
        } else {
            Tok::Plain
        };
        let end = chars.peek().map_or(text.len(), |&(j, _)| j);
        match runs.last_mut() {
            Some(last) if last.2 == tok && tok == Tok::Plain => last.1 = end,
            _ => runs.push((start, end, tok)),
        }
    }
    runs.into_iter().map(|(a, b, t)| (&text[a..b], t)).collect()
}

/// Where a caption's guards (`if ready`, `unless user`) have their keyword, as byte ranges.
pub fn guard_keywords(caption: &str) -> Vec<(usize, usize)> {
    let mut out = vec![];
    let mut at = 0;
    for part in caption.split(" · ") {
        if let Some(k) = ["if ", "unless "].into_iter().find(|k| part.starts_with(k)) {
            out.push((at, at + k.len() - 1));
        }
        at += part.len() + " · ".len();
    }
    out
}

/// The band at the top of a loop or a branch that holds its title: the layout's region header.
const FLOW_BAND: f64 = 26.0;

/// As the web's `clamp`: the low bound wins when the bounds cross.
pub fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    lo.max(hi.min(v))
}

pub fn smoothstep(a: f64, b: f64, v: f64) -> f64 {
    let t = clamp((v - a) / (b - a), 0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn intersects(b: Rect, w: f64, h: f64, margin: f64) -> bool {
    b.x < w + margin && b.y < h + margin && b.x + b.w > -margin && b.y + b.h > -margin
}

/// The size text of `px` pixels is laid out at: one of a ladder of sizes a
/// quarter of an octave apart, 10 px among them, so a name passes through a
/// few of them as the view zooms rather than a new one each frame.
pub fn font_size(px: f64) -> f64 {
    10.0 * 2f64.powf((4.0 * (px / 10.0).log2()).round() / 4.0)
}

/// Whether a block's side effects are marked with a dot: ones that reach outside it.
pub fn marked(n: &Node) -> bool {
    n.effects.iter().any(|e| ["write", "network", "exec", "mutat", "distributed", "process", "raises", "reads", "filesystem", "state"].iter().any(|w| e.contains(w)))
}

/// What a scene draws, for its legend: the roles blocks are colored by, the kinds of arrow, and whether there are adds and marked side effects.
#[derive(Default)]
pub struct Drawn {
    pub roles: HashSet<&'static str>,
    pub edges: HashSet<EdgeKind>,
    pub add: bool,
    pub effects: bool,
}

pub fn drawn_in(m: &Model) -> Drawn {
    let mut out = Drawn::default();
    for n in &m.scene.nodes {
        // As `fill_of` colors them: boxes by their role, tables as settings.
        if n.shape == Shape::Box {
            out.roles.insert(n.category);
        } else if n.shape == Shape::Region && n.kind == Kind::Table {
            out.roles.insert("config");
        }
        if n.shape == Shape::Glyph && n.title == "+" {
            out.add = true;
        }
        if marked(n) {
            out.effects = true;
        }
        for e in &n.edges {
            out.edges.insert(e.kind);
        }
    }
    out
}

/// A block's color, tinted by how a change touched it.
pub fn fill_of(theme: &Theme, n: &Node, status: Option<Mark>) -> Color {
    if n.shape == Shape::Glyph {
        return theme.paper;
    }
    if n.shape == Shape::Region {
        let fill = if n.kind == Kind::Table { theme.swatch("config").tint } else { theme.swatch(if n.kind == Kind::Lane { "lane" } else { "region" }).fill };
        return status.map_or(fill, |s| mix(fill, theme.change(s).tint, 0.5));
    }
    let fill = theme.swatch(n.category).fill;
    status.map_or(fill, |s| mix(fill, theme.change(s).tint, 0.7))
}

/// Where a name may break across lines: after a space, `_`, `-`, `/` or `.`,
/// and before a capital after a lowercase letter (`get|User`); never before a
/// file's extension, nor between separators and nothing (`__init__`).
pub fn name_parts(title: &str) -> Vec<String> {
    let chars: Vec<char> = title.chars().collect();
    let mut parts = vec![];
    let mut start = 0;
    let alnum = |c: char| c.is_ascii_alphanumeric();
    for i in 1..chars.len() {
        let (before, at) = (chars[i - 1], chars[i]);
        let word = alnum(at) && chars[start..i].iter().any(|&c| alnum(c));
        let joint = " _-/".contains(before) || (before == '.' && chars[i..].iter().any(|&c| " _-/.".contains(c))) || (before.is_ascii_lowercase() && at.is_ascii_uppercase());
        if word && joint {
            parts.push(chars[start..i].iter().collect());
            start = i;
        }
    }
    parts.push(chars[start..].iter().collect());
    parts
}

/// `parts` in at most `k` lines, the widest as narrow as it can be (by `width_of`): the lines, trimmed, and that width.
pub fn balance(parts: &[String], k: usize, width_of: &mut dyn FnMut(&str) -> f64) -> (Vec<String>, f64) {
    let n = parts.len();
    let mut span = |i: usize, j: usize| width_of(parts[i..j].concat().trim());
    // best[j][i]: the narrowest widest line for the first i parts in j lines, and where its last line starts.
    let mut best: Vec<Vec<(f64, usize)>> = vec![(0..=n).map(|i| (if i == 0 { 0.0 } else { f64::INFINITY }, 0)).collect()];
    for j in 1..=k {
        let mut row = vec![(0.0, 0)];
        for i in 1..=n {
            let mut cell = (f64::INFINITY, 0);
            for f in j - 1..i {
                let w = best[j - 1][f].0.max(span(f, i));
                if w < cell.0 {
                    cell = (w, f);
                }
            }
            row.push(cell);
        }
        best.push(row);
    }
    let mut lines = vec![];
    let (mut j, mut i) = (k, n);
    while j > 0 && i > 0 {
        let from = best[j][i].1;
        lines.insert(0, parts[from..i].concat().trim().to_string());
        i = from;
        j -= 1;
    }
    (lines, best[k][n].0)
}

/// What the renderer keeps between frames: text widths and name wrappings, by font.
#[derive(Default)]
pub struct Renderer {
    widths: HashMap<(u64, Family, u16, String), f64>,
    wraps: HashMap<(Family, u16, String), Vec<(Vec<String>, f64)>>,
}

/// What a frame drew: the blocks and edges that can be pointed at.
#[derive(Default)]
pub struct Drew {
    pub hits: Vec<Hit>,
    pub edge_hits: Vec<EdgeHit>,
}

impl Renderer {
    pub fn draw(&mut self, pen: &mut dyn Pen, m: &Model, theme: &Theme, p: &Paint) -> Drew {
        pen.set_fill(theme.background);
        pen.set_alpha(1.0);
        pen.fill_rect(0.0, 0.0, p.width, p.height);
        let mut d = Draw::new(self, pen, m, theme, p);
        let root = m.node(m.root());
        d.interior(root, p.xfs[0], -1, 0, &[root.id], 1.0);
        Drew { hits: d.hits, edge_hits: d.edge_hits }
    }

    /// Draws block `n` alone, as `draw` would draw it in a diagram with transform `t`.
    pub fn draw_block(&mut self, pen: &mut dyn Pen, m: &Model, theme: &Theme, n: &Node, t: Xf, p: &Paint) {
        let mut d = Draw::new(self, pen, m, theme, p);
        d.node(n, box_of(t, n), t, -1, -1, &[], 1.0);
    }
}

struct Draw<'a, 'p> {
    r: &'a mut Renderer,
    ctx: &'a mut dyn Pen,
    m: &'a Model,
    theme: &'a Theme,
    p: &'a Paint<'p>,
    hits: Vec<Hit>,
    edge_hits: Vec<EdgeHit>,
    /// How many closed blocks the one being drawn is in the thumbnail of.
    thumbnails: usize,
    /// How many blocks a change did not touch the one being drawn is in (faded once).
    faded: usize,
    font_key: (u64, Family, u16),
    font_scale: f64,
}

impl<'a, 'p> Draw<'a, 'p> {
    fn new(r: &'a mut Renderer, ctx: &'a mut dyn Pen, m: &'a Model, theme: &'a Theme, p: &'a Paint<'p>) -> Self {
        Draw { r, ctx, m, theme, p, hits: vec![], edge_hits: vec![], thumbnails: 0, faded: 0, font_key: (0, Family::Sans, 400), font_scale: 1.0 }
    }

    fn status(&self, id: u32) -> Option<Mark> {
        self.p.diff.and_then(|d| d.status(id))
    }

    fn touched(&self, id: u32) -> bool {
        self.p.diff.is_none_or(|d| d.touched(id))
    }

    // ----- containers -------------------------------------------------------------------

    fn interior(&mut self, owner: &'a Node, t: Xf, parent_hit: i32, path_pos: i32, chain: &[u32], alpha: f64) {
        if !owner.rows.is_empty() {
            self.rows(owner, t, alpha);
            return;
        }
        let kids = &owner.children;
        let edge_alpha = alpha * smoothstep(0.035, 0.12, t.s);
        let lw = clamp(1.3 * t.s.sqrt(), 0.45, 1.45);
        let focus = self.p.hover.or(self.p.selected);
        let focused = focus.is_some_and(|f| kids.contains(&f));
        // A value followed here outshines the block in focus.
        let traced = self.p.trace.filter(|tr| tr.scope.contains(&owner.id));
        let p = self.p;
        let lit = |i: usize, e: &Edge| -> Option<bool> {
            let id = (owner.id, i);
            if p.hover_edge == Some(id) {
                return Some(true);
            }
            if let Some(tr) = traced {
                return Some(tr.edges.contains(&id));
            }
            if focused {
                let f = focus.unwrap();
                return Some(e.from == f || e.to == f);
            }
            None
        };
        // A connection the change made is green; one between blocks it did not touch fades with them.
        let faded_now = self.faded;
        let change = |i: usize, e: &Edge| -> Option<bool> {
            let d = p.diff?;
            if d.added((owner.id, i)) {
                Some(true)
            } else if faded_now == 0 && !d.touched(e.from) && !d.touched(e.to) {
                Some(false)
            } else {
                None
            }
        };
        if edge_alpha > 0.02 {
            for (i, e) in owner.edges.iter().enumerate() {
                self.edge(owner.id, i, e, t, edge_alpha, lw, lit(i, e), parent_hit, change(i, e));
            }
            if let Some(d) = p.diff {
                for r in d.removed(owner.id) {
                    self.removed_edge(r, t, edge_alpha, lw);
                }
            }
        }
        for &id in kids {
            let n = self.m.node(id);
            let b = box_of(t, n);
            if !intersects(b, p.width, p.height, 2.0) || (b.w < 1.0 && b.h < 1.0) {
                continue;
            }
            let next = path_pos + 1;
            let next_pos = if path_pos >= 0 && p.path.get(next as usize) == Some(&id) { next } else { -1 };
            self.node(n, b, t, parent_hit, next_pos, chain, alpha);
        }
        if edge_alpha > 0.02 && t.s * 11.0 >= 4.5 {
            let mut placed: Vec<Rect> = vec![];
            // Labels of lit edges are placed first, so they are the ones shown.
            let mut order: Vec<usize> = (0..owner.edges.len()).collect();
            if focused || traced.is_some() || p.hover_edge.is_some() {
                order.sort_by_key(|&i| !lit(i, &owner.edges[i]).unwrap_or(false));
            }
            let a = edge_alpha * smoothstep(4.5, 7.0, t.s * 11.0);
            for i in order {
                let e = &owner.edges[i];
                self.edge_label(e, t, a, lit(i, e), &mut placed, change(i, e));
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn node(&mut self, n: &'a Node, b: Rect, t: Xf, parent_hit: i32, path_pos: i32, chain: &[u32], alpha: f64) {
        if n.shape != Shape::Port && b.w.min(b.h) < SPECK {
            self.speck(n, b, t, alpha);
            return;
        }
        match n.shape {
            Shape::Port => self.port(n, b, t, alpha, self.status(n.id)),
            Shape::Glyph => {
                let a = if self.p.diff.is_some() && self.faded == 0 && !self.touched(n.id) { alpha * UNTOUCHED } else { alpha };
                self.glyph(n, b, t, a);
                self.hit(n, b, parent_hit, b.w.min(b.h) >= 12.0);
            }
            Shape::Region => self.region(n, b, t, parent_hit, path_pos, chain, alpha),
            Shape::Box => self.boxed(n, b, t, parent_hit, path_pos, chain, alpha),
        }
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

    #[allow(clippy::too_many_arguments)]
    fn boxed(&mut self, n: &'a Node, b: Rect, t: Xf, parent_hit: i32, path_pos: i32, chain: &[u32], mut alpha: f64) {
        let owner = self.m.owner(n);
        let recursive = owner.is_some_and(|o| o.id != n.id && chain.contains(&o.id));
        let inner = owner.filter(|_| !recursive).and_then(|o| if path_pos >= 0 { self.p.xfs.get(path_pos as usize).copied() } else { self.m.interior_of(t, n, o) });
        let open = inner.map_or(0.0, |i| smoothstep(OPEN_FROM, OPEN_TO, i.s) * smoothstep(40.0, 90.0, b.w));
        let theme = self.theme;
        let sw = theme.swatch(n.category);
        let radius = clamp(6.0 * t.s, 1.2, 10.0);
        let selected = self.p.selected == Some(n.id);
        let hovered = self.p.hover == Some(n.id);
        // A change colors what it added or changed; what it did not touch fades.
        let status = self.status(n.id);
        let unfaded = if self.faded > 0 { (alpha / UNTOUCHED).min(1.0) } else { alpha };
        let fade = self.p.diff.is_some() && self.faded == 0 && !self.touched(n.id);
        if fade {
            alpha *= UNTOUCHED;
            self.faded += 1;
        }
        let fill = fill_of(theme, n, status);
        // Opened, a changed block keeps its color on the header band; the diagram inside stays plain.
        let tint = status.map_or(sw.tint, |s| mix(sw.tint, theme.change(s).tint, 0.12));
        let ctx = &mut *self.ctx;
        ctx.set_alpha(alpha);
        ctx.begin_path();
        ctx.round_rect(b.x, b.y, b.w, b.h, radius);
        ctx.set_fill(if open > 0.0 { mix(fill, tint, open) } else { fill });
        ctx.fill();
        if open > 0.01 {
            // Header band keeps the box's identity once it is open.
            let hh = (30.0 * t.s).min(b.h);
            ctx.save();
            ctx.clip();
            ctx.set_alpha(alpha * open);
            ctx.set_fill(fill);
            ctx.fill_rect(b.x, b.y, b.w, hh);
            ctx.restore();
            ctx.set_alpha(alpha);
            // The clip's path was the box's outline: draw it again for the stroke.
            ctx.begin_path();
            ctx.round_rect(b.x, b.y, b.w, b.h, radius);
        }
        let lw = clamp(1.25 * t.s.sqrt(), 0.5, 1.4);
        ctx.set_line_width(if selected {
            2.2
        } else if status.is_some() {
            (lw + 0.5).max(1.5) + if hovered { 0.5 } else { 0.0 }
        } else if hovered {
            lw + 0.6
        } else {
            lw
        });
        ctx.set_stroke(if selected {
            theme.accent
        } else if let Some(s) = status {
            theme.change(s).ink
        } else if n.category == "op" {
            theme.op_outline
        } else {
            theme.outline
        });
        ctx.stroke();
        self.ring(n, b, radius);
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
                let hs = (13.0 * t.s).min(19.0);
                self.seen(me, n, hs, alpha * open * legible * smoothstep(4.2, 7.0, hs));
                self.header(n, b, t, alpha * open * legible);
                let shown = if o.rows.is_empty() { alpha * open } else { alpha * open * legible };
                if thumbnail {
                    self.thumbnails += 1;
                }
                if o.id == n.id {
                    self.interior(o, inner, me, path_pos, chain, shown);
                } else {
                    let mut deeper = chain.to_vec();
                    deeper.push(o.id);
                    self.interior(o, inner, me, path_pos, &deeper, shown);
                }
                if thumbnail {
                    self.thumbnails -= 1;
                }
            }
        }
        if face_alpha > 0.01 {
            let (size, shown) = self.face(n, b, t, alpha * face_alpha, owner.is_some() && !recursive, open > 0.2);
            self.seen(me, n, size, shown);
        }
        self.effects_mark(n, b, t, alpha);
        if self.p.diff.is_some() {
            self.change_marks(n, b, t, alpha, unfaded, status);
        }
        if fade {
            self.faded -= 1;
        }
        self.ctx.set_alpha(1.0);
    }

    /// Rings a block in focus: the one selected, or a use of the one in focus.
    fn ring(&mut self, n: &Node, b: Rect, radius: f64) {
        if self.p.selected == Some(n.id) {
            let ctx = &mut *self.ctx;
            ctx.begin_path();
            ctx.round_rect(b.x - 4.0, b.y - 4.0, b.w + 8.0, b.h + 8.0, radius + 4.0);
            ctx.set_stroke(self.theme.accent_soft);
            ctx.set_line_width(6.0);
            ctx.stroke();
        } else if self.p.related.is_some_and(|r| r.contains(&n.id)) {
            // A use of the block in focus: outlined so the reader sees where it is used.
            self.ctx.set_stroke(self.theme.accent);
            self.ctx.set_line_width(1.6);
            self.ctx.set_dash(&[5.0, 3.0]);
            self.outline(Rect { x: b.x - 3.0, y: b.y - 3.0, w: b.w + 6.0, h: b.h + 6.0 }, radius + 3.0);
            self.ctx.set_dash(&[]);
        }
    }

    /// A block too small to make out: a spot of its color, with no outline, name,
    /// marks or interior; ringed while it is in focus, so it can still be found.
    fn speck(&mut self, n: &Node, b: Rect, t: Xf, alpha: f64) {
        let a = if self.p.diff.is_some() && self.faded == 0 && !self.touched(n.id) { alpha * UNTOUCHED } else { alpha };
        self.ctx.set_alpha(a);
        self.ctx.set_fill(fill_of(self.theme, n, self.status(n.id)));
        self.ctx.fill_rect(b.x, b.y, b.w, b.h);
        self.ring(n, b, clamp(6.0 * t.s, 1.2, 10.0));
        self.ctx.set_alpha(1.0);
    }

    /// How many changes a block holds, on a badge at its top edge; a block that
    /// holds changes without changing itself (a folder) is ringed. A block that
    /// leads into changed code gets a dashed ring and an arrowed count, drawn
    /// even where its surroundings are faded.
    fn change_marks(&mut self, n: &Node, b: Rect, t: Xf, alpha: f64, unfaded: f64, status: Option<Mark>) {
        let Some(diff) = self.p.diff else { return };
        let inside = diff.inside(n.id);
        let through = if inside > 0 || status == Some(Mark::Added) { 0 } else { diff.through(n.id) };
        // In a thumbnail the tint shows where changes are; the closed block's badge counts them.
        if (inside == 0 && through == 0) || b.w < 14.0 || b.h < 10.0 || self.thumbnails > 0 {
            return;
        }
        let ink = self.theme.changed.ink;
        let radius = clamp(6.0 * t.s, 1.2, 10.0) + 3.0;
        if status.is_none() || through > 0 {
            self.ctx.set_alpha(if through > 0 { unfaded } else { alpha });
            self.ctx.set_stroke(ink);
            self.ctx.set_line_width(1.5);
            if through > 0 {
                self.ctx.set_dash(&[4.0, 3.0]);
            }
            self.outline(Rect { x: b.x - 3.0, y: b.y - 3.0, w: b.w + 6.0, h: b.h + 6.0 }, radius);
            self.ctx.set_dash(&[]);
        }
        let count = if inside > 0 { inside } else { through };
        let words = if b.w >= 130.0 { format!(" change{}", if count == 1 { "" } else { "s" }) } else { String::new() };
        let text = if through > 0 { format!("→ {count}{words}") } else { format!("{count}{words}") };
        self.font(10.0, Family::Sans, 600);
        let w = self.width(&text) + 10.0;
        let h = 15.0;
        let x = (b.x + 2.0).max(b.x + b.w - w - 6.0);
        let y = b.y - h / 2.0 - if status.is_some() { 0.0 } else { 3.0 };
        let (paper, a) = (self.theme.paper, if through > 0 { unfaded } else { alpha });
        let ctx = &mut *self.ctx;
        ctx.set_alpha(a);
        ctx.begin_path();
        ctx.round_rect(x, y, w, h, h / 2.0);
        ctx.set_fill(if through > 0 { paper } else { ink });
        ctx.fill();
        if through > 0 {
            ctx.set_stroke(ink);
            ctx.set_line_width(1.0);
            ctx.stroke();
        }
        ctx.set_fill(if through > 0 { ink } else { paper });
        ctx.set_align(Align::Center);
        self.text(&text, x + w / 2.0, y + 11.0, false);
        self.ctx.set_align(Align::Left);
    }

    /// Draws a closed box's name; returns the title's size and opacity.
    fn face(&mut self, n: &Node, b: Rect, t: Xf, alpha: f64, deep: bool, overlay: bool) -> (f64, f64) {
        if alpha < 0.02 {
            return (0.0, 0.0);
        }
        // An operation's title is code; a table's is a name ("External packages").
        let code = n.kind == Kind::Op;
        // Names of boxes big enough to hold them stay readable at any zoom.
        let natural = if code { 12.0 } else { 13.0 } * t.s;
        let named = named_kind(n.kind);
        let family = if code { Family::Mono } else { Family::Sans };
        let weight = if code {
            400
        } else if matches!(n.kind, Kind::Call | Kind::Symbol) {
            500
        } else {
            600
        };
        // Readable minimum, but never larger than what fits the box.
        let room = b.w - 14.0 * t.s.min(1.0);
        let mut at10 = |d: &mut Self, text: &str| {
            d.font(10.0, family, weight);
            d.width(text)
        };
        let size_for = |d: &mut Self, at10: &mut dyn FnMut(&mut Self, &str) -> f64, text: &str, share: f64| {
            let fits = room / (at10(d, text) / 10.0).max(1.0);
            clamp(natural, 11.5f64.min(b.h * share).min(fits.max(natural)), 18.0)
        };
        // A box with room keeps a readable name; so does a landmark (a file, a
        // class, a file on disk) whose name still fits it at a legible size,
        // however narrow it is, unless it is in the thumbnail of a closed block
        // (whose own name is the landmark there). Other names grow and shrink
        // with the zoom, so they are never measured.
        let roomy = b.w >= if named { 44.0 } else { 70.0 } && b.h >= 18.0;
        let landmark = named && self.thumbnails == 0 && b.h >= 14.0;
        let prominent = t.s >= if named { 0.08 } else { 0.14 } && (roomy || landmark);
        let mut lines = vec![n.title.clone()];
        let mut size = if prominent { size_for(self, &mut at10, &n.title, 0.55) } else { natural };
        if prominent && size < 10.0 {
            // A long name in a narrow box goes over as many lines as the box is
            // tall enough for, broken between its words, rather than shrinking.
            let wraps = self.wrapped(&n.title, family, weight);
            for k in 2..=wraps.len() {
                let s = clamp(natural, 11.5f64.min(b.h * 0.6 / k as f64).min((room / (wraps[k - 1].1 / 10.0)).max(natural)), 18.0);
                if s > size + 0.5 {
                    lines = wraps[k - 1].0.clone();
                    size = s;
                }
            }
        }
        if prominent && size < LEGIBLE && self.thumbnails == 0 {
            // Still too long to read whole: what the box holds of it at a size
            // that can be read, its end cut short, rather than all of it at one that can't.
            size = LEGIBLE;
            self.font(size, family, weight);
            lines = self.fill(&name_parts(&n.title), room, clamp((b.h * 0.6 / size).floor(), 1.0, MAX_LINES as f64) as usize);
        } else if !roomy && size < LEGIBLE {
            // A landmark whose name no longer fits at a legible size is not one.
            lines = vec![n.title.clone()];
            size = natural;
        }
        if size < 4.2 {
            return (0.0, 0.0);
        }
        let meta = std::iter::once(n.subtitle.as_str()).chain(n.badges.iter().map(String::as_str)).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
        let text_alpha = alpha * smoothstep(4.2, 7.0, size);
        // The caption under a name is always smaller than the name.
        let ms = (size * 0.82).min((10.5 * t.s).max(10.0));
        let lh = size * 1.15;
        let extra = (lines.len() - 1) as f64 * lh;
        let two_lines = !meta.is_empty() && b.h >= size + extra + ms + 10.0;
        let (cx, cy) = (b.x + b.w / 2.0, b.y + b.h / 2.0);
        let max_w = room;
        self.font(size, family, weight);
        let titles: Vec<String> = lines.iter().map(|l| self.fit(l, max_w)).collect();
        let title = titles.join(" ").trim().to_string();
        if overlay && !title.is_empty() {
            // A soft backing keeps the name legible over the interior preview.
            let w = titles.iter().map(|l| self.width(l)).fold(0.0, f64::max) + size * 1.1;
            let h = if two_lines { size + extra + ms + 6.0 } else { size + extra } + size * 0.7;
            let ctx = &mut *self.ctx;
            ctx.set_alpha(alpha * 0.92);
            ctx.set_fill(self.theme.paper);
            ctx.begin_path();
            ctx.round_rect(cx - w / 2.0, cy - h / 2.0, w, h, h.min(size * 1.7) / 2.6);
            ctx.fill();
        }
        self.ctx.set_alpha(text_alpha);
        self.ctx.set_align(Align::Center);
        self.ctx.set_fill(self.theme.ink);
        let title_y = if two_lines { cy - ms * 0.15 } else { cy + size * 0.35 } - extra / 2.0;
        for (i, l) in titles.iter().enumerate() {
            self.text(l, cx, title_y + i as f64 * lh, false);
        }
        if two_lines {
            self.font(ms, Family::Sans, 400);
            let m = self.fit(&meta, max_w);
            self.caption(&m, cx, title_y + extra + ms + 3.0, ms, self.theme.muted, true);
        }
        if deep && !overlay && b.w > 46.0 && size > 6.0 {
            // Something lives inside: a small corner mark invites zooming.
            let s = clamp(4.0 * t.s, 2.0, 5.0);
            let ctx = &mut *self.ctx;
            ctx.set_alpha(text_alpha * 0.6);
            ctx.set_stroke(self.theme.muted);
            ctx.set_line_width(1.0);
            ctx.begin_path();
            ctx.move_to(b.x + b.w - 3.0 * s, b.y + s);
            ctx.line_to(b.x + b.w - s, b.y + s);
            ctx.line_to(b.x + b.w - s, b.y + 3.0 * s);
            ctx.stroke();
        }
        self.ctx.set_align(Align::Left);
        (if title.is_empty() { 0.0 } else { size }, text_alpha)
    }

    fn header(&mut self, n: &Node, b: Rect, t: Xf, alpha: f64) {
        let size = (13.0 * t.s).min(19.0);
        if size < 4.2 || alpha < 0.02 {
            return;
        }
        self.ctx.set_alpha(alpha * smoothstep(4.2, 7.0, size));
        let band = 30.0 * t.s;
        let x = b.x + (9.0 * t.s).min(14.0);
        let y = b.y + (band / 2.0 + size * 0.36).min(band - 4.0);
        let max_w = b.w - 18.0 * t.s;
        self.font(size, Family::Sans, 600);
        self.ctx.set_fill(self.theme.ink);
        let title = self.fit(&n.title, max_w);
        self.text(&title, x, y, false);
        let used = self.width(&title) + 8.0 * t.s;
        let meta = std::iter::once(n.subtitle.as_str()).chain(n.badges.iter().map(String::as_str)).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
        if !meta.is_empty() && used < max_w - 20.0 * t.s {
            self.font(size * 0.82, Family::Sans, 400);
            let m = self.fit(&meta, max_w - used);
            self.caption(&m, x + used, y, size * 0.82, self.theme.muted, false);
        }
    }

    fn effects_mark(&mut self, n: &Node, b: Rect, t: Xf, alpha: f64) {
        if b.w < 30.0 || !marked(n) {
            return;
        }
        let r = clamp(3.0 * t.s, 1.5, 4.0);
        let ctx = &mut *self.ctx;
        ctx.set_alpha(alpha * 0.9);
        ctx.set_fill(self.theme.effect);
        ctx.begin_path();
        ctx.arc(b.x + b.w - r * 2.2, b.y + b.h - r * 2.2, r, 0.0, PI * 2.0);
        ctx.fill();
    }

    #[allow(clippy::too_many_arguments)]
    fn region(&mut self, n: &'a Node, b: Rect, t: Xf, parent_hit: i32, path_pos: i32, chain: &[u32], mut alpha: f64) {
        let lane = n.kind == Kind::Lane;
        let radius = clamp(9.0 * t.s, 1.5, 14.0);
        let status = self.status(n.id);
        let unfaded = if self.faded > 0 { (alpha / UNTOUCHED).min(1.0) } else { alpha };
        let fade = self.p.diff.is_some() && self.faded == 0 && !self.touched(n.id);
        if fade {
            alpha *= UNTOUCHED;
            self.faded += 1;
        }
        let theme = self.theme;
        let selected = self.p.selected == Some(n.id);
        // A loop, a branch and its arms, a try and its handlers are tinted and
        // outlined in the color of what they do, under a band of it that holds their title.
        let role = self.flow_role(n);
        let flow = role.map(|r| theme.flow(r));
        let mut fill = fill_of(theme, n, status);
        let mut outline = if lane { theme.lane_outline } else { theme.region_outline };
        if let (Some(f), None) = (flow, status) {
            // The role's color toward the page; an arm lighter than the branch around it.
            fill = mix(f.fill, theme.paper, if lane { 0.62 } else { 0.42 });
            outline = mix(f.fill, f.ink, if lane { 0.4 } else { 0.5 });
        }
        let ctx = &mut *self.ctx;
        ctx.set_alpha(alpha);
        ctx.begin_path();
        ctx.round_rect(b.x, b.y, b.w, b.h, radius);
        ctx.set_fill(fill);
        ctx.fill();
        if let (Some(f), false) = (flow, n.title.is_empty()) {
            ctx.save();
            ctx.clip();
            ctx.set_fill(if let Some(s) = status { mix(f.fill, theme.change(s).tint, 0.5) } else { f.fill });
            ctx.fill_rect(b.x, b.y, b.w, (FLOW_BAND * t.s).min(b.h));
            ctx.restore();
            ctx.set_alpha(alpha);
            // The clip's path was the region's outline: draw it again for the stroke.
            ctx.begin_path();
            ctx.round_rect(b.x, b.y, b.w, b.h, radius);
        }
        ctx.set_line_width(if selected {
            2.0
        } else if status.is_some() {
            1.5
        } else if flow.is_some() {
            clamp(1.3 * t.s.sqrt(), 0.6, 1.5)
        } else {
            clamp(1.1 * t.s.sqrt(), 0.45, 1.2)
        });
        ctx.set_stroke(if selected {
            theme.accent
        } else if let Some(s) = status {
            theme.change(s).ink
        } else {
            outline
        });
        if !lane {
            ctx.stroke();
        } else if 4.0 * t.s < 1.0 {
            // Dashes finer than a pixel blend into a fainter line: drawn as one.
            ctx.set_alpha(alpha * 4.0 / 7.0);
            ctx.stroke();
            ctx.set_alpha(alpha);
        } else {
            ctx.set_dash(&[4.0 * t.s, 3.0 * t.s]);
            self.outline(b, radius);
            self.ctx.set_dash(&[]);
        }
        let me = self.hit(n, b, parent_hit, false);
        let size = 11.5 * t.s;
        if !n.title.is_empty() {
            self.seen(me, n, size, alpha * smoothstep(4.2, 7.0, size));
        }
        // A loop's or a branch's title is drawn over what is inside it, below.
        let titled_flow = !n.title.is_empty() && flow.is_some();
        if !titled_flow && !n.title.is_empty() && size >= 4.2 {
            self.ctx.set_alpha(alpha * smoothstep(4.2, 7.0, size));
            let code = code_kind(n.kind);
            let badges = n.badges.join("  ");
            let mut reserve = 0.0;
            if !badges.is_empty() {
                self.font(12.0 * t.s, Family::Sans, 700);
                reserve = self.width(&badges) + 18.0 * t.s;
                self.ctx.set_fill(self.theme.ink);
                self.ctx.set_align(Align::Right);
                self.text(&badges, b.x + b.w - 10.0 * t.s, b.y + 17.0 * t.s, false);
                self.ctx.set_align(Align::Left);
            }
            self.font(size, if code { Family::Mono } else { Family::Sans }, if code { 400 } else { 600 });
            self.ctx.set_fill(self.theme.muted);
            let title = self.fit(&n.title, b.w - 20.0 * t.s - reserve);
            self.text(&title, b.x + 10.0 * t.s, b.y + 17.0 * t.s, false);
        }
        let inner = if path_pos >= 0 { self.p.xfs.get(path_pos as usize).copied() } else { self.m.interior_of(t, n, n) };
        if let Some(inner) = inner {
            self.interior(n, inner, me, path_pos, chain, alpha);
        }
        if let (Some(f), true) = (flow, titled_flow) {
            // Over the lines that come in through its top, which pass behind it.
            self.flow_title(n, b, t, alpha, f);
        }
        if self.p.diff.is_some() {
            self.change_marks(n, b, t, alpha, unfaded, status);
        }
        if fade {
            self.faded -= 1;
        }
        self.ctx.set_alpha(1.0);
    }

    /// What a loop's, a branch's or an arm's keyword says it does; an untitled
    /// branch is what its first arm is (a `try` holding its handlers). None for other blocks.
    fn flow_role(&self, n: &Node) -> Option<FlowRole> {
        let of = |title: &str| flow_keyword(title).map(|k| k.2);
        match n.kind {
            Kind::Loop => Some(of(&n.title).unwrap_or(FlowRole::Loop)),
            Kind::Lane => Some(of(&n.title).unwrap_or(FlowRole::Branch)),
            Kind::Branch if n.title.is_empty() => Some(n.children.first().and_then(|&c| of(&self.m.node(c).title)).unwrap_or(FlowRole::Branch)),
            Kind::Branch => Some(of(&n.title).unwrap_or(FlowRole::Branch)),
            _ => None,
        }
    }

    /// A loop's or a branch's title on its band: its keyword in the band's
    /// ink (a loop's after a circling arrow), then the rest as code, its own
    /// keywords in the same ink; a loop's count at the right. The code is
    /// haloed in the band's color, so lines it is drawn over pass behind it.
    fn flow_title(&mut self, n: &Node, b: Rect, t: Xf, alpha: f64, flow: crate::theme::Flow) {
        let band = (FLOW_BAND * t.s).min(b.h);
        // Text grows with the zoom only so far: past that, more of it fits.
        let size = (12.0 * t.s).min(16.0);
        let shown = alpha * smoothstep(4.2, 7.0, size);
        if shown < 0.01 {
            return;
        }
        let pad = (10.0 * t.s).min(14.0);
        let (left, right) = (b.x + pad, b.x + b.w - pad);
        let mid = b.y + band / 2.0;
        let y = mid + size * 0.36;
        let split = flow_keyword(&n.title);
        self.ctx.set_alpha(shown);
        let mut end = right;
        if !n.badges.is_empty() {
            let badges = n.badges.join("  ");
            self.font(size * 0.92, Family::Sans, 700);
            let w = self.width(&badges);
            if right - w > left + 6.0 * size {
                self.ctx.set_fill(flow.ink);
                self.text(&badges, right - w, mid + size * 0.92 * 0.36, false);
                end = right - w - size * 0.8;
            }
        }
        let mut x = left;
        if let Some((keyword, _, role)) = split {
            if role == FlowRole::Loop {
                self.loop_mark(x + size * 0.36, mid, size * 0.36, flow.ink, shown);
                x += size * 1.05;
            }
            self.font(size, Family::Sans, 700);
            self.ctx.set_alpha(shown);
            self.ctx.set_fill(flow.ink);
            let k = self.fit(keyword, end - x);
            self.text(&k, x, y, false);
            x += self.width(&k) + size * 0.5;
        }
        let rest = split.map_or(n.title.as_str(), |s| s.1);
        if !rest.is_empty() {
            self.font(size * 0.95, Family::Mono, 400);
            let rest = self.fit(rest, end - x);
            self.code(&rest, x, y, flow.ink, shown, Some(flow.fill));
        }
    }

    /// A circling arrow around (cx, cy): the mark of a loop.
    fn loop_mark(&mut self, cx: f64, cy: f64, r: f64, color: Color, alpha: f64) {
        let (from, to) = (-PI * 0.35, PI * 1.25);
        let ctx = &mut *self.ctx;
        ctx.set_alpha(alpha);
        ctx.set_stroke(color);
        ctx.set_line_width(r * 0.36);
        ctx.begin_path();
        ctx.arc(cx, cy, r, from, to);
        ctx.stroke();
        // The head points the way the arc turns.
        let (px, py) = (cx + r * to.cos(), cy + r * to.sin());
        let (dx, dy) = (-to.sin(), to.cos());
        let (nx, ny) = (to.cos(), to.sin());
        let s = r * 0.62;
        ctx.set_fill(color);
        ctx.begin_path();
        ctx.move_to(px + dx * s, py + dy * s);
        ctx.line_to(px + nx * s * 0.8, py + ny * s * 0.8);
        ctx.line_to(px - nx * s * 0.8, py - ny * s * 0.8);
        ctx.close_path();
        ctx.fill();
    }

    /// Code at (x, y) in the current font, its words colored by what they
    /// are: the words joining its parts in `keyword`; over a `halo` of a color.
    fn code(&mut self, text: &str, x: f64, y: f64, keyword: Color, alpha: f64, halo: Option<Color>) {
        self.ctx.set_alpha(alpha);
        if let Some(halo) = halo {
            self.ctx.set_stroke(halo);
            self.ctx.set_line_width(3.0);
            self.ctx.set_round_join(true);
            self.text(text, x, y, true);
            self.ctx.set_round_join(false);
        }
        let mut x = x;
        for (part, tok) in code_tokens(text) {
            let color = match tok {
                Tok::Plain => self.theme.ink,
                Tok::Keyword => keyword,
                Tok::Literal => self.theme.literal,
                Tok::Str => self.theme.string,
            };
            if !part.trim().is_empty() {
                self.ctx.set_fill(color);
                self.text(part, x, y, false);
            }
            x += self.width(part);
        }
    }

    /// A caption in `px` pixels of sans and `color`, from `x` or centered on
    /// it: the keyword of a guard in it (`if ready`) in the color of a branch.
    fn caption(&mut self, text: &str, x: f64, y: f64, px: f64, color: Color, center: bool) {
        let keywords = guard_keywords(text);
        if keywords.is_empty() {
            self.font(px, Family::Sans, 400);
            self.ctx.set_fill(color);
            self.text(text, x, y, false);
            return;
        }
        let mut runs = vec![];
        let mut at = 0;
        for &(a, b) in &keywords {
            runs.push((&text[at..a], false));
            runs.push((&text[a..b], true));
            at = b;
        }
        runs.push((&text[at..], false));
        let mut widths = vec![];
        for &(run, keyword) in &runs {
            self.font(px, Family::Sans, if keyword { 600 } else { 400 });
            widths.push(self.width(run));
        }
        let mut x = if center { x - widths.iter().sum::<f64>() / 2.0 } else { x };
        self.ctx.set_align(Align::Left);
        for (&(run, keyword), w) in runs.iter().zip(widths) {
            self.font(px, Family::Sans, if keyword { 600 } else { 400 });
            self.ctx.set_fill(if keyword { self.theme.branch.ink } else { color });
            self.text(run, x, y, false);
            x += w;
        }
        if center {
            self.ctx.set_align(Align::Center);
        }
    }

    fn glyph(&mut self, n: &Node, b: Rect, t: Xf, alpha: f64) {
        let r = b.w.min(b.h) / 2.0;
        let (cx, cy) = (b.x + b.w / 2.0, b.y + b.h / 2.0);
        let status = self.status(n.id);
        let theme = self.theme;
        let ctx = &mut *self.ctx;
        ctx.set_alpha(alpha);
        ctx.begin_path();
        ctx.arc(cx, cy, r, 0.0, PI * 2.0);
        ctx.set_fill(theme.paper);
        ctx.fill();
        ctx.set_line_width(clamp(1.25 * t.s.sqrt(), 0.5, 1.4) + if status.is_some() { 0.5 } else { 0.0 });
        ctx.set_stroke(if self.p.selected == Some(n.id) {
            theme.accent
        } else if let Some(s) = status {
            theme.change(s).ink
        } else {
            theme.outline
        });
        ctx.stroke();
        if r < 2.5 {
            return;
        }
        if n.title == "+" {
            let a = r * 0.55;
            ctx.begin_path();
            ctx.move_to(cx - a, cy);
            ctx.line_to(cx + a, cy);
            ctx.move_to(cx, cy - a);
            ctx.line_to(cx, cy + a);
            ctx.stroke();
        } else {
            self.font(r * 1.15, Family::Sans, 600);
            self.ctx.set_fill(theme.ink);
            self.ctx.set_align(Align::Center);
            self.text(&n.title, cx, cy + r * 0.4, false);
            self.ctx.set_align(Align::Left);
        }
    }

    fn port(&mut self, n: &Node, b: Rect, t: Xf, alpha: f64, status: Option<Mark>) {
        if n.kind == Kind::Unused {
            self.stop(n, b, t, alpha);
            return;
        }
        if b.h == 0.0 || n.title.is_empty() {
            return;
        }
        let size = (11.5 * t.s).min(15.0);
        if size < 4.2 {
            return;
        }
        self.ctx.set_alpha(alpha * smoothstep(4.2, 7.0, size));
        self.font(size, Family::Mono, 400);
        // A new input or result is named in the color of what was added.
        self.ctx.set_fill(status.map_or(self.theme.muted, |s| self.theme.change(s).ink));
        self.ctx.set_align(Align::Center);
        let title = self.fit(&n.title, (b.w + 60.0 * t.s).max(40.0));
        self.text(&title, b.x + b.w / 2.0, b.y + b.h / 2.0 + size * 0.35, false);
        self.ctx.set_align(Align::Left);
        self.ctx.set_alpha(1.0);
    }

    /// Where a value nothing reads ends: a bar across the end of its line, and the word under it.
    fn stop(&mut self, n: &Node, b: Rect, t: Xf, alpha: f64) {
        let x = b.x + b.w / 2.0;
        let half = 6.0 * t.s;
        if half < 1.0 {
            return;
        }
        let ctx = &mut *self.ctx;
        ctx.set_alpha(alpha * 0.95);
        ctx.set_stroke(self.theme.faint_edge);
        ctx.set_line_width(clamp(1.3 * t.s.sqrt(), 0.45, 1.45));
        ctx.begin_path();
        ctx.move_to(x - half, b.y);
        ctx.line_to(x + half, b.y);
        ctx.stroke();
        let size = (10.5 * t.s).min(13.0);
        if size >= 4.2 {
            self.ctx.set_alpha(alpha * smoothstep(4.2, 7.0, size));
            self.font(size, Family::Sans, 400);
            self.ctx.set_fill(self.theme.faint);
            self.ctx.set_align(Align::Center);
            self.text(&n.title, x, b.y + 3.0 * t.s + size * 0.9, false);
            self.ctx.set_align(Align::Left);
        }
        self.ctx.set_alpha(1.0);
    }

    fn rows(&mut self, owner: &Node, t: Xf, alpha: f64) {
        // Rows read at most at the size names do, however far the table is
        // zoomed or however few rows give it a large scale beside its neighbors.
        let size = (11.5 * t.s).min(ROW_MAX);
        if size < 3.0 {
            return;
        }
        let f = size / 11.5;
        let mut y = t.y + 6.0 * f;
        let x = t.x + 10.0 * f;
        let text_alpha = alpha * smoothstep(3.5, 6.5, size);
        let cw = if owner.cw > 0.0 { owner.cw } else { 100.0 };
        for r in &owner.rows {
            let h = if r.heading { HEADING_H } else { ROW_H } * f;
            if y > self.p.height {
                break;
            }
            if y + h >= 0.0 {
                if size < 4.5 {
                    // Too small to read: a texture of lines that hints at the content.
                    self.ctx.set_alpha(alpha * 0.35);
                    self.ctx.set_fill(if r.heading { self.theme.heading_texture } else { self.theme.faint });
                    let chars = (r.key.chars().count() + r.value.chars().count() + 3) as f64;
                    self.ctx.fill_rect(x, y + h * 0.45, (chars * size * 0.6).min(cw * t.s - 20.0 * f), (size * 0.35).max(0.6));
                } else if r.heading {
                    self.ctx.set_alpha(text_alpha);
                    self.font(10.5 * f, Family::Sans, 700);
                    self.ctx.set_fill(self.theme.heading);
                    self.text(&r.key.to_uppercase(), x, y + h * 0.72, false);
                } else {
                    self.ctx.set_alpha(text_alpha);
                    self.font(size, Family::Mono, 400);
                    self.ctx.set_fill(self.theme.ink);
                    self.text(&r.key, x, y + h * 0.72, false);
                    if !r.value.is_empty() {
                        let kw = self.width(&r.key);
                        self.ctx.set_fill(self.theme.muted);
                        let cw = if owner.cw > 0.0 { owner.cw } else { 300.0 };
                        let v = self.fit(&format!(" = {}", r.value), cw * t.s - kw - 20.0 * f);
                        self.text(&v, x + kw, y + h * 0.72, false);
                    }
                }
            }
            y += h;
        }
        self.ctx.set_alpha(1.0);
    }

    // ----- edges -----------------------------------------------------------------------

    /// Draws an edge; `state` is true when lit, false when faded beside lit ones; `change` is
    /// true when a change being read added it, false when it left it untouched.
    #[allow(clippy::too_many_arguments)]
    fn edge(&mut self, owner: u32, index: usize, e: &Edge, t: Xf, alpha: f64, lw: f64, state: Option<bool>, container: i32, change: Option<bool>) {
        let pts = &e.points;
        if pts.len() < 4 {
            return;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        for c in pts.chunks_exact(2) {
            x0 = x0.min(c[0]);
            x1 = x1.max(c[0]);
            y0 = y0.min(c[1]);
            y1 = y1.max(c[1]);
        }
        if !intersects(Rect { x: t.x + x0 * t.s, y: t.y + y0 * t.s, w: (x1 - x0) * t.s + 1.0, h: (y1 - y0) * t.s + 1.0 }, self.p.width, self.p.height, 10.0) {
            return;
        }
        let lit = state == Some(true);
        let dim = state == Some(false);
        let screen: Vec<f64> = pts.chunks_exact(2).flat_map(|c| [t.x + c[0] * t.s, t.y + c[1] * t.s]).collect();
        // Edges can be pointed at once their diagram is legible.
        if alpha > 0.3 && t.s * 11.0 >= 6.0 && (x1 - x0 + y1 - y0) * t.s > 6.0 {
            self.edge_hits.push(EdgeHit { edge: (owner, index), container, pts: screen.clone() });
        }
        // Settings and sizes are faint; conditions are dotted: neither is the data itself.
        // Nor is a value nothing reads, which fades as its line ends.
        let theme = self.theme;
        let control = e.kind == EdgeKind::Control;
        let unread = self.unread(e);
        let mut color = if control {
            theme.context
        } else if e.kind == EdgeKind::Artifact {
            theme.artifact
        } else if e.kind == EdgeKind::Exec || unread {
            theme.faint_edge
        } else {
            theme.edge
        };
        let mut a = alpha * if control { 0.8 } else { 0.95 } * if dim { 0.35 } else { 1.0 };
        let mut width = if control { lw * 0.85 } else { lw };
        if lit {
            color = theme.accent;
            a = alpha.max(0.9);
            width = lw + 0.7;
        } else if change == Some(true) {
            color = theme.added.ink;
            a = a.max(0.9);
            width = lw + 0.6;
        } else if change == Some(false) {
            a *= UNTOUCHED;
        }
        let ctx = &mut *self.ctx;
        ctx.set_alpha(a);
        ctx.set_stroke(color);
        ctx.set_fill(color);
        ctx.set_line_width(width);
        ctx.set_round_join(true);
        match e.kind {
            EdgeKind::Exec => ctx.set_dash(&[5.0, 4.0]),
            EdgeKind::Artifact => ctx.set_dash(&[6.0, 3.0]),
            EdgeKind::Control => ctx.set_dash(&[1.5 * width.max(1.0), 2.5 * width.max(1.0)]),
            _ => ctx.set_dash(&[]),
        }
        let n = pts.len() / 2;
        let head = if e.open { 0.0 } else { clamp(7.0 * t.s.sqrt(), 3.0, 8.0) * if control { 0.8 } else { 1.0 } };
        let (ex, ey) = (screen[2 * n - 2], screen[2 * n - 1]);
        let (dx, dy) = (ex - screen[2 * n - 4], ey - screen[2 * n - 3]);
        let len = { let l = dx.hypot(dy); if l == 0.0 { 1.0 } else { l } };
        let (ux, uy) = (dx / len, dy / len);
        let r = clamp(7.0 * t.s, 0.0, 10.0);
        let mut radii = vec![0.0; n];
        for i in 1..n.saturating_sub(1) {
            let len_in = (screen[2 * i] - screen[2 * i - 2]).hypot(screen[2 * i + 1] - screen[2 * i - 1]);
            let len_out = (screen[2 * i + 2] - screen[2 * i]).hypot(screen[2 * i + 3] - screen[2 * i + 1]);
            radii[i] = r.min(len_in / 2.0).min(len_out / 2.0);
        }
        // Shorten the last segment so the line ends at the arrow's base.
        let mut line = screen;
        line[2 * n - 2] = ex - ux * head * 0.8;
        line[2 * n - 1] = ey - uy * head * 0.8;
        self.line(&line, &radii);
        let ctx = &mut *self.ctx;
        ctx.set_dash(&[]);
        if head > 0.0 {
            ctx.begin_path();
            ctx.move_to(ex, ey);
            ctx.line_to(ex - ux * head - uy * head * 0.42, ey - uy * head + ux * head * 0.42);
            ctx.line_to(ex - ux * head + uy * head * 0.42, ey - uy * head - ux * head * 0.42);
            ctx.close_path();
            ctx.fill();
        }
        ctx.set_round_join(false);
        ctx.set_alpha(1.0);
    }

    fn edge_label(&mut self, e: &Edge, t: Xf, alpha: f64, state: Option<bool>, placed: &mut Vec<Rect>, change: Option<bool>) {
        let Some([lx, ly]) = e.label_at else { return };
        if e.label.is_empty() || alpha < 0.03 {
            return;
        }
        let (x, y) = (t.x + lx * t.s, t.y + ly * t.s);
        if x < -200.0 || y < -20.0 || x > self.p.width + 20.0 || y > self.p.height + 20.0 {
            return;
        }
        let lit = state == Some(true);
        let size = (11.0 * t.s).min(13.5);
        self.font(size, Family::Mono, 400);
        // Labels never overprint each other; the one for a highlighted edge wins.
        let r = Rect { x: x - 1.0, y: y - size, w: self.width(&e.label) + 2.0, h: size + 2.0 };
        if !lit && placed.iter().any(|q| r.x < q.x + q.w && q.x < r.x + r.w && r.y < q.y + q.h && q.y < r.y + r.h) {
            return;
        }
        placed.push(r);
        let unread = self.unread(e);
        let theme = self.theme;
        self.ctx.set_alpha(if state == Some(false) {
            alpha * 0.4
        } else if change == Some(false) && !lit {
            alpha * UNTOUCHED
        } else {
            alpha
        });
        self.ctx.set_line_width(3.0);
        self.ctx.set_stroke(theme.label_halo);
        self.ctx.set_round_join(true);
        self.text(&e.label, x, y, true);
        // Values read in ink; conditions and values nothing reads stay muted like their lines.
        self.ctx.set_fill(if lit {
            theme.accent
        } else if change == Some(true) {
            theme.added.ink
        } else if e.kind == EdgeKind::Control || unread {
            theme.muted
        } else {
            theme.label_ink
        });
        self.text(&e.label, x, y, false);
        self.ctx.set_round_join(false);
        self.ctx.set_alpha(1.0);
    }

    /// Whether an edge carries values nothing reads to their stop.
    fn unread(&self, e: &Edge) -> bool {
        self.m.node(e.to).kind == Kind::Unused
    }

    /// A connection the change removed: a dashed red curve from the bottom of one block to the top of the other.
    fn removed_edge(&mut self, r: &GoneEdge, t: Xf, alpha: f64, lw: f64) {
        let a = box_of(t, self.m.node(r.from));
        let b = box_of(t, self.m.node(r.to));
        let (x0, y0, x1, y1) = (a.x + a.w / 2.0, a.y + a.h, b.x + b.w / 2.0, b.y);
        if !intersects(Rect { x: x0.min(x1), y: y0.min(y1), w: (x1 - x0).abs() + 1.0, h: (y1 - y0).abs() + 1.0 }, self.p.width, self.p.height, 20.0) {
            return;
        }
        let bend = (24.0 * t.s).max((y1 - y0).abs() / 2.0);
        let head = clamp(7.0 * t.s.sqrt(), 3.0, 8.0);
        let ink = self.theme.removed.ink;
        let ctx = &mut *self.ctx;
        ctx.set_alpha(alpha.max(0.85));
        ctx.set_stroke(ink);
        ctx.set_fill(ink);
        ctx.set_line_width(lw + 0.3);
        ctx.set_dash(&[5.0, 4.0]);
        // The curve in steps fine enough to look smooth, so it can be cut to the screen.
        let ye = y1 - head * 0.8;
        let k = clamp(((x1 - x0).abs() + (ye - y0).abs() + 2.0 * bend) / 8.0, 12.0, 512.0).ceil() as usize;
        let mut pts = vec![];
        for i in 0..=k {
            let u = i as f64 / k as f64;
            let v = 1.0 - u;
            pts.push((v * v * v + 3.0 * v * v * u) * x0 + (3.0 * v * u * u + u * u * u) * x1);
            pts.push(v * v * v * y0 + 3.0 * v * v * u * (y0 + bend) + 3.0 * v * u * u * (y1 - bend) + u * u * u * ye);
        }
        self.line(&pts, &vec![0.0; k + 1]);
        let ctx = &mut *self.ctx;
        ctx.set_dash(&[]);
        ctx.begin_path();
        ctx.move_to(x1, y1);
        ctx.line_to(x1 - head * 0.42, y1 - head);
        ctx.line_to(x1 + head * 0.42, y1 - head);
        ctx.close_path();
        ctx.fill();
        if !r.label.is_empty() && t.s * 11.0 >= 4.5 {
            let size = (11.0 * t.s).min(13.5);
            self.font(size, Family::Mono, 400);
            let (mx, my) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
            self.ctx.set_line_width(3.0);
            self.ctx.set_stroke(self.theme.label_halo);
            self.text(&r.label, mx + 4.0, my, true);
            self.text(&r.label, mx + 4.0, my, false);
        }
        self.ctx.set_alpha(1.0);
    }

    /// Strokes a line with rounded corners where it is on screen: a canvas dashes
    /// the whole of a line, and one many screens long costs a dash every few pixels.
    fn line(&mut self, pts: &[f64], radii: &[f64]) {
        let (w, h) = (self.p.width, self.p.height);
        for run in visible_runs(pts, radii, -CUT, -CUT, w + CUT, h + CUT) {
            let q = &run.pts;
            let ctx = &mut *self.ctx;
            ctx.begin_path();
            ctx.move_to(q[0], q[1]);
            let mut i = 2;
            while i + 2 < q.len() {
                ctx.arc_to(q[i], q[i + 1], q[i + 2], q[i + 3], run.radii[i / 2]);
                i += 2;
            }
            ctx.line_to(q[q.len() - 2], q[q.len() - 1]);
            ctx.set_dash_offset(run.at);
            ctx.stroke();
        }
        self.ctx.set_dash_offset(0.0);
    }

    /// Strokes a rounded rectangle's outline where it is on screen.
    fn outline(&mut self, b: Rect, radius: f64) {
        let (pts, radii) = rect_line(b, radius);
        self.line(&pts, &radii);
    }

    // ----- text ------------------------------------------------------------------------

    /// Sets the font to `px` pixels of `family`. Text is laid out only at the
    /// sizes of a ladder (`font_size`) and scaled to its own size as it is drawn.
    fn font(&mut self, px: f64, family: Family, weight: u16) {
        let size = font_size(px);
        self.font_key = (size.to_bits(), family, weight);
        self.font_scale = px / size;
        self.ctx.set_font(Font { px: size, family, weight });
    }

    /// The width of `text` in the current font.
    fn width(&mut self, text: &str) -> f64 {
        let key = (self.font_key.0, self.font_key.1, self.font_key.2, text.to_string());
        let w = match self.r.widths.get(&key) {
            Some(&w) => w,
            None => {
                let w = self.ctx.measure(text);
                if self.r.widths.len() > 8000 {
                    self.r.widths.clear();
                }
                self.r.widths.insert(key, w);
                w
            }
        };
        w * self.font_scale
    }

    /// Draws `text` at (x, y) in the current font: filled, or outlined when `stroke`.
    fn text(&mut self, text: &str, x: f64, y: f64, stroke: bool) {
        let k = self.font_scale;
        let ctx = &mut *self.ctx;
        ctx.save();
        ctx.scale(k);
        if stroke {
            let lw = ctx.line_width();
            ctx.set_line_width(lw / k);
            ctx.stroke_text(text, x / k, y / k);
        } else {
            ctx.fill_text(text, x / k, y / k);
        }
        ctx.restore();
    }

    /// A name broken over 1…MAX_LINES lines in `family` at `weight`, each as evenly as its words allow, with its widest line's width at 10 px.
    fn wrapped(&mut self, title: &str, family: Family, weight: u16) -> Vec<(Vec<String>, f64)> {
        let key = (family, weight, title.to_string());
        if let Some(w) = self.r.wraps.get(&key) {
            return w.clone();
        }
        self.font(10.0, family, weight);
        let parts = name_parts(title);
        let mut out = vec![];
        for k in 1..=MAX_LINES.min(parts.len()) {
            out.push(balance(&parts, k, &mut |t| self.width(t)));
        }
        if self.r.wraps.len() > 4000 {
            self.r.wraps.clear();
        }
        self.r.wraps.insert(key, out.clone());
        out
    }

    /// `parts` laid into at most `lines` lines of `max` pixels in the current font, each as full as it can be; the last takes what is left.
    fn fill(&mut self, parts: &[String], max: f64, lines: usize) -> Vec<String> {
        let mut out = vec![String::new()];
        for p in parts {
            let line = out.last().unwrap().clone();
            if !line.is_empty() && out.len() < lines && self.width(format!("{line}{p}").trim_end()) > max {
                out.push(p.clone());
            } else {
                *out.last_mut().unwrap() = line + p;
            }
        }
        out.into_iter().map(|l| l.trim().to_string()).collect()
    }

    /// Ellipsizes `text` to `max` pixels in the current font. Text sized to
    /// fill `max` fits, whatever the rounding in working out its size.
    fn fit(&mut self, text: &str, max: f64) -> String {
        if max <= 4.0 {
            return String::new();
        }
        let room = max * (1.0 + 1e-9);
        if self.width(text) <= room {
            return text.to_string();
        }
        let chars: Vec<char> = text.chars().collect();
        let (mut lo, mut hi) = (0, chars.len());
        while lo < hi {
            let mid = (lo + hi + 1) / 2;
            let cut: String = chars[..mid].iter().collect::<String>() + "…";
            if self.width(&cut) <= room {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        if lo > 0 {
            chars[..lo].iter().collect::<String>().trim_end().to_string() + "…"
        } else {
            String::new()
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_break_where_a_reader_would_break_them() {
        assert_eq!(name_parts("getUserName"), ["get", "User", "Name"]);
        assert_eq!(name_parts("prepare.py"), ["prepare.py"]);
        assert_eq!(name_parts("__init__"), ["__init__"]);
        assert_eq!(name_parts("load_data set"), ["load_", "data ", "set"]);
        let parts = name_parts("configure_optimizers");
        let (lines, _) = balance(&parts, 2, &mut |t| t.len() as f64);
        assert_eq!(lines, ["configure_", "optimizers"]);
    }

    #[test]
    fn font_sizes_climb_a_ladder_through_ten() {
        assert_eq!(font_size(10.0), 10.0);
        assert!((font_size(11.0) - 10.0 * 2f64.powf(0.25)).abs() < 1e-9);
        assert_eq!(clamp(5.0, 7.0, 6.0), 7.0);
    }

    #[test]
    fn control_flow_titles_split_at_their_keyword() {
        assert_eq!(flow_keyword("for block in blocks"), Some(("for", "block in blocks", FlowRole::Loop)));
        assert_eq!(flow_keyword("else if x > 0"), Some(("else if", "x > 0", FlowRole::Branch)));
        assert_eq!(flow_keyword("else"), Some(("else", "", FlowRole::Branch)));
        assert_eq!(flow_keyword("except ValueError"), Some(("except", "ValueError", FlowRole::Catch)));
        // A word that only starts like a keyword is not one; a Rust arm has none.
        assert_eq!(flow_keyword("format(x)"), None);
        assert_eq!(flow_keyword("Some(cmd)"), None);
    }

    #[test]
    fn code_is_colored_by_what_its_words_are() {
        let toks = code_tokens("targets is not None and k == 'a b' or 3");
        let of = |t: Tok| toks.iter().filter(|p| p.1 == t).map(|p| p.0).collect::<Vec<_>>();
        assert_eq!(of(Tok::Keyword), ["is", "not", "and", "or"]);
        assert_eq!(of(Tok::Literal), ["None", "3"]);
        assert_eq!(of(Tok::Str), ["'a b'"]);
        assert_eq!(toks.iter().map(|p| p.0).collect::<String>(), "targets is not None and k == 'a b' or 3");
        // An unclosed string, as an ellipsis leaves one, runs to the end.
        assert_eq!(code_tokens("x == \"ab…").last(), Some(&("\"ab…", Tok::Str)));
    }

    #[test]
    fn guards_in_captions_have_their_keyword_found() {
        let caption = "torch.nn.init · if module.bias is not None";
        let found = guard_keywords(caption);
        assert_eq!(found.iter().map(|&(a, b)| &caption[a..b]).collect::<Vec<_>>(), ["if"]);
        assert_eq!(guard_keywords("unless user"), [(0, 6)]);
        assert!(guard_keywords("iffy · gift").is_empty());
    }

    #[test]
    fn draws_the_whole_project_into_an_svg() {
        let files = [("model.py", "class Block:\n    def forward(self, x):\n        x = x + self.attn(x)\n        return x\n"), ("train.py", "from model import Block\nb = Block()\nb.forward(1)\n")];
        let inputs = files.iter().map(|(p, t)| blockviz_core::project::InFile { path: p.to_string(), text: Some(t.to_string()), bytes: t.len() as u64 }).collect();
        let m = Model::new(blockviz_core::analyze("t", inputs), files.iter().map(|(p, t)| (p.to_string(), t.to_string())).collect());
        let mut cam = crate::camera::Camera::new(&m);
        cam.fit_root(&m, 1000.0, 700.0);
        let xfs = cam.transforms(&m);
        let theme = Theme::light();
        let p = Paint { width: 1000.0, height: 700.0, hover: None, selected: None, path: &cam.path, xfs: &xfs, related: None, trace: None, hover_edge: None, diff: None };
        let mut pen = crate::svgpen::SvgPen::estimating();
        let drew = Renderer::default().draw(&mut pen, &m, &theme, &p);
        let svg = pen.svg(1000.0, 700.0);
        assert!(svg.contains(">model.py</text>") && svg.contains(">train.py</text>"), "{svg}");
        assert!(!drew.hits.is_empty());
    }
}
