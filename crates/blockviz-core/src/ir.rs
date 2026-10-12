//! The scene IR: one tree of nested diagrams.
//!
//! Every node lives in the *interior* coordinate system of its parent. A node
//! with an interior (its own `children`, a table of `rows`, or a `ref` to
//! another node's interior) draws that interior inside `inner`, scaled to fit.
//! This uniform rule is what makes the canvas infinitely zoomable: the renderer
//! never needs to know what a node *is* to descend into it.
//!
//! Edges always connect siblings. Values crossing a container boundary pass
//! through `Input`/`Output` port nodes on that container's interior.

use serde::Serialize;

pub type Id = u32;

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Repo,
    Dir,
    File,
    Notebook,
    Doc,
    Asset,
    Class,
    Function,
    Section,
    Loop,
    Branch,
    Lane,
    Group,
    Call,
    Op,
    Glyph,
    Table,
    Artifact,
    Symbol,
    Input,
    Output,
}

/// How a node is drawn. `Box` nodes may have an interior that opens on zoom;
/// `Region` nodes are always open and drawn at the parent's scale.
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Shape {
    Box,
    Region,
    Glyph,
    Port,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum EdgeKind {
    /// A value flowing from producer to consumer.
    Data,
    /// A value that decides whether or which way a step runs (a condition).
    Control,
    /// A definition used by another definition or file.
    Uses,
    /// Code executed from another file (`exec(open(...).read())`).
    Exec,
    /// A file written by one program and read by another.
    Artifact,
}

#[derive(Serialize, Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    #[serde(serialize_with = "rounded")]
    pub x: f64,
    #[serde(serialize_with = "rounded")]
    pub y: f64,
    #[serde(serialize_with = "rounded")]
    pub w: f64,
    #[serde(serialize_with = "rounded")]
    pub h: f64,
}

/// Layout geometry is sent to two decimals, finer than any zoom shows (edge
/// points have one); a float's full precision would only lengthen the JSON.
/// Adding zero turns the -0 that rounding leaves of tiny negatives into 0.
fn rounded<S: serde::Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
    Number((v * 100.).round() / 100. + 0.).serialize(s)
}

fn numbers<S: serde::Serializer>(v: &[f64], s: S) -> Result<S::Ok, S::Error> {
    s.collect_seq(v.iter().map(|&x| Number(x)))
}

fn point<S: serde::Serializer>(v: &Option<[f64; 2]>, s: S) -> Result<S::Ok, S::Error> {
    v.map(|p| p.map(Number)).serialize(s)
}

/// A number in the JSON, written as an integer when it is one: `12`
/// parses to the same number as `12.0`, from fewer bytes, sooner.
struct Number(f64);

impl Serialize for Number {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let i = self.0 as i64;
        // Bit for bit, so that -0 stays a float.
        if (i as f64).to_bits() == self.0.to_bits() {
            s.serialize_i64(i)
        } else {
            s.serialize_f64(self.0)
        }
    }
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Span {
    pub file: u32,
    pub start: u32,
    pub end: u32,
    pub line: u32,
    pub end_line: u32,
}

#[derive(Serialize, Clone, Debug)]
pub struct Row {
    pub key: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub value: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// Section heading rows have no value.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub heading: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Edge {
    pub from: Id,
    pub to: Id,
    pub kind: EdgeKind,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// Polyline in the parent's interior coordinates: x0, y0, x1, y1, ...
    #[serde(serialize_with = "numbers")]
    pub points: Vec<f64>,
    /// Where the label is drawn (left-aligned baseline), when it is drawn.
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "point")]
    pub label_at: Option<[f64; 2]>,
    /// The line continues into a region instead of ending in an arrowhead.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub open: bool,
    #[serde(skip)]
    pub to_port: Option<Id>,
    #[serde(skip)]
    pub from_port: Option<Id>,
}

/// Layout-only information; never serialized.
#[derive(Clone, Debug, Default)]
pub struct Hints {
    /// Source order, used to break ties so diagrams read like the code.
    pub order: u32,
    /// Interior is drawn at the parent's scale (regions) instead of fitted.
    pub inline: bool,
    /// Ports that are pure pass-throughs on an inline region's border.
    pub hidden: bool,
    /// Ordering constraints that shape the layout but are not drawn.
    pub after: Vec<Id>,
    /// Interior children are pinned left-to-right in this order (branch lanes).
    pub lanes: bool,
    /// Packing priority for disconnected pieces (lower first).
    pub pack: i32,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    pub id: Id,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<Id>,
    pub kind: Kind,
    pub shape: Shape,
    pub category: &'static str,
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub subtitle: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub doc: String,
    /// Full source text of a fused operation, for the inspector. Left empty
    /// when it is exactly the source at `span`, which the page reads there.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub code: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub badges: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<Span>,
    /// This node's interior is borrowed from another node (a call shows its callee).
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<Id>,
    /// Where "go to definition" leads (a class for a constructor call or component).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<Id>,
    /// Blocks that use this one without linking to it: the calls of a component.
    #[serde(rename = "usedBy", skip_serializing_if = "Vec::is_empty")]
    pub used_by: Vec<Id>,
    #[serde(serialize_with = "rounded")]
    pub x: f64,
    #[serde(serialize_with = "rounded")]
    pub y: f64,
    #[serde(serialize_with = "rounded")]
    pub w: f64,
    #[serde(serialize_with = "rounded")]
    pub h: f64,
    /// Box-local rectangle the interior is fitted into.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inner: Option<Rect>,
    /// Size of the interior this node owns (children or rows).
    #[serde(skip_serializing_if = "is_zero", serialize_with = "rounded")]
    pub cw: f64,
    #[serde(skip_serializing_if = "is_zero", serialize_with = "rounded")]
    pub ch: f64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Id>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<Edge>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<Row>,
    #[serde(skip)]
    pub hints: Hints,
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

impl Node {
    pub fn has_interior(&self) -> bool {
        !self.children.is_empty() || !self.rows.is_empty()
    }
    pub fn is_port(&self) -> bool {
        matches!(self.kind, Kind::Input | Kind::Output)
    }
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FileMeta {
    pub path: String,
    pub language: String,
    pub bytes: u64,
    pub lines: u32,
    /// Whether the file's text was analyzed (false for binaries and skipped files).
    pub analyzed: bool,
    /// Text the spans refer to when it differs from the file (notebooks → Python).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub files: u32,
    pub python_files: u32,
    pub lines: u32,
    pub nodes: u32,
    pub max_depth: u32,
}

/// Nodes by id, held in chunks of a fixed size so that the list grows
/// without moving: a vector doubles by copying every node, holding both
/// copies for a moment, and can leave half of itself unused.
#[derive(Default)]
pub struct Nodes {
    chunks: Vec<Vec<Node>>,
}

/// Nodes per chunk: a power of two, so an id splits with a shift and a mask.
const CHUNK: usize = 1 << 12;

impl Nodes {
    pub fn push(&mut self, node: Node) {
        if self.chunks.last().is_none_or(|c| c.len() == CHUNK) {
            self.chunks.push(Vec::with_capacity(CHUNK));
        }
        self.chunks.last_mut().unwrap().push(node);
    }

    pub fn len(&self) -> usize {
        self.chunks.last().map_or(0, |c| (self.chunks.len() - 1) * CHUNK + c.len())
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    pub fn iter(&self) -> std::iter::Flatten<std::slice::Iter<'_, Vec<Node>>> {
        self.chunks.iter().flatten()
    }

    pub fn iter_mut(&mut self) -> std::iter::Flatten<std::slice::IterMut<'_, Vec<Node>>> {
        self.chunks.iter_mut().flatten()
    }
}

impl std::ops::Index<usize> for Nodes {
    type Output = Node;
    fn index(&self, i: usize) -> &Node {
        &self.chunks[i / CHUNK][i % CHUNK]
    }
}

impl std::ops::IndexMut<usize> for Nodes {
    fn index_mut(&mut self, i: usize) -> &mut Node {
        &mut self.chunks[i / CHUNK][i % CHUNK]
    }
}

impl<'a> IntoIterator for &'a Nodes {
    type Item = &'a Node;
    type IntoIter = std::iter::Flatten<std::slice::Iter<'a, Vec<Node>>>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a> IntoIterator for &'a mut Nodes {
    type Item = &'a mut Node;
    type IntoIter = std::iter::Flatten<std::slice::IterMut<'a, Vec<Node>>>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

impl Serialize for Nodes {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(self)
    }
}

impl std::fmt::Debug for Nodes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self).finish()
    }
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Scene {
    pub version: u32,
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub root: Id,
    pub nodes: Nodes,
    pub files: Vec<FileMeta>,
    pub diagnostics: Vec<String>,
    pub stats: Stats,
}

/// Arena used while building a scene.
#[derive(Default)]
pub struct Builder {
    pub nodes: Nodes,
}

impl Builder {
    pub fn add(&mut self, parent: Option<Id>, kind: Kind, shape: Shape, title: impl Into<String>) -> Id {
        let id = self.nodes.len() as Id;
        let order = parent.map(|p| self.nodes[p as usize].children.len() as u32).unwrap_or(0);
        self.nodes.push(Node {
            id,
            parent,
            kind,
            shape,
            category: "neutral",
            title: title.into(),
            subtitle: String::new(),
            doc: String::new(),
            code: String::new(),
            badges: vec![],
            effects: vec![],
            inputs: vec![],
            outputs: vec![],
            span: None,
            reference: None,
            link: None,
            used_by: vec![],
            x: 0.,
            y: 0.,
            w: 0.,
            h: 0.,
            inner: None,
            cw: 0.,
            ch: 0.,
            children: vec![],
            edges: vec![],
            rows: vec![],
            hints: Hints { order, ..Hints::default() },
        });
        if let Some(p) = parent {
            self.nodes[p as usize].children.push(id);
        }
        id
    }

    pub fn get(&self, id: Id) -> &Node {
        &self.nodes[id as usize]
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn get_mut(&mut self, id: Id) -> &mut Node {
        &mut self.nodes[id as usize]
    }

    /// Adds (or merges into) an edge between two siblings. Labels are merged
    /// as a comma-separated set so parallel values bundle into one line.
    pub fn connect(&mut self, from: Id, to: Id, kind: EdgeKind, label: &str) {
        self.connect_port(from, to, kind, label, None)
    }

    pub fn connect_port(&mut self, from: Id, to: Id, kind: EdgeKind, label: &str, to_port: Option<Id>) {
        if from == to {
            return;
        }
        let parent = self.nodes[from as usize].parent;
        debug_assert_eq!(parent, self.nodes[to as usize].parent, "edges connect siblings");
        let Some(parent) = parent else { return };
        let edges = &mut self.nodes[parent as usize].edges;
        // A stronger kind wins when two relations share an edge.
        if let Some(e) = edges.iter_mut().find(|e| e.from == from && e.to == to && e.to_port == to_port) {
            if kind < e.kind {
                e.kind = kind;
            }
            // Names joined once each: `pets` and `users, pets` make `pets, users`.
            for part in label.split(", ").filter(|p| !p.is_empty()) {
                if !e.label.split(", ").any(|l| l == part) {
                    if !e.label.is_empty() {
                        e.label.push_str(", ");
                    }
                    e.label.push_str(part);
                }
            }
            return;
        }
        edges.push(Edge {
            from,
            to,
            kind,
            label: label.to_string(),
            points: vec![],
            label_at: None,
            open: false,
            to_port,
            from_port: None,
        });
    }

    /// Detaches a node from its parent (used when a speculative node turns out to be noise).
    pub fn remove(&mut self, id: Id) {
        if let Some(p) = self.nodes[id as usize].parent {
            let parent = &mut self.nodes[p as usize];
            parent.children.retain(|c| *c != id);
            parent.edges.retain(|e| e.from != id && e.to != id);
        }
        self.nodes[id as usize].parent = None;
    }

    pub fn depth(&self, mut id: Id) -> u32 {
        let mut d = 0;
        while let Some(p) = self.nodes[id as usize].parent {
            d += 1;
            id = p;
        }
        d
    }

    /// The nodes reachable from `root`, renumbered in order.
    pub fn finish(self, root: Id) -> (Nodes, Id) {
        let mut keep = vec![false; self.nodes.len()];
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            if keep[id as usize] {
                continue;
            }
            keep[id as usize] = true;
            let n = &self.nodes[id as usize];
            stack.extend(n.children.iter().copied());
            if let Some(r) = n.reference {
                stack.push(r);
            }
        }
        // A kept node's new id is the number of kept nodes before it.
        let mut map = vec![0; keep.len()];
        let mut next = 0;
        for (i, &k) in keep.iter().enumerate() {
            map[i] = next;
            next += k as Id;
        }
        let new = |id: Id| keep[id as usize].then(|| map[id as usize]);
        let remap = |ids: &mut Vec<Id>| {
            ids.retain(|&id| keep[id as usize]);
            for id in ids {
                *id = map[*id as usize];
            }
        };
        // Chunk by chunk, each freed for the next once its nodes have moved.
        let mut nodes = Nodes::default();
        for chunk in self.nodes.chunks {
            for mut n in chunk.into_iter().filter(|n| keep[n.id as usize]) {
                n.id = map[n.id as usize];
                n.parent = n.parent.and_then(new);
                n.reference = n.reference.and_then(new);
                n.link = n.link.and_then(new);
                remap(&mut n.used_by);
                remap(&mut n.children);
                n.edges.retain(|e| keep[e.from as usize] && keep[e.to as usize]);
                for e in &mut n.edges {
                    e.from = map[e.from as usize];
                    e.to = map[e.to as usize];
                }
                nodes.push(n);
            }
        }
        (nodes, map[root as usize])
    }
}
