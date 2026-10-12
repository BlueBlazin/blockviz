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
use std::collections::HashMap;

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
    /// Configuration or ambient state (settings, globals); drawn lightly.
    Context,
    /// A definition used by another definition or file.
    Uses,
    /// Code executed from another file (`exec(open(...).read())`).
    Exec,
    /// A file written by one program and read by another.
    Artifact,
}

#[derive(Serialize, Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
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
    pub points: Vec<f64>,
    /// Where the label is drawn (left-aligned baseline), when it is drawn.
    #[serde(skip_serializing_if = "Option::is_none")]
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
    /// Full source text of a fused operation, for the inspector.
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
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// Box-local rectangle the interior is fitted into.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inner: Option<Rect>,
    /// Size of the interior this node owns (children or rows).
    #[serde(skip_serializing_if = "is_zero")]
    pub cw: f64,
    #[serde(skip_serializing_if = "is_zero")]
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

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Scene {
    pub version: u32,
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub root: Id,
    pub nodes: Vec<Node>,
    pub files: Vec<FileMeta>,
    pub diagnostics: Vec<String>,
    pub stats: Stats,
}

/// Arena used while building a scene.
#[derive(Default)]
pub struct Builder {
    pub nodes: Vec<Node>,
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

    /// Compacts the arena to nodes reachable from `root`, renumbering ids.
    pub fn finish(self, root: Id) -> (Vec<Node>, Id) {
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
        let mut map: HashMap<Id, Id> = HashMap::new();
        for (i, k) in keep.iter().enumerate() {
            if *k {
                let next = map.len() as Id;
                map.insert(i as Id, next);
            }
        }
        let mut out = Vec::with_capacity(map.len());
        for (i, mut n) in self.nodes.into_iter().enumerate() {
            if !keep[i] {
                continue;
            }
            n.id = map[&n.id];
            n.parent = n.parent.and_then(|p| map.get(&p).copied());
            n.reference = n.reference.and_then(|r| map.get(&r).copied());
            n.link = n.link.and_then(|r| map.get(&r).copied());
            n.used_by = n.used_by.iter().filter_map(|u| map.get(u).copied()).collect();
            n.children = n.children.iter().filter_map(|c| map.get(c).copied()).collect();
            n.edges.retain(|e| map.contains_key(&e.from) && map.contains_key(&e.to));
            for e in &mut n.edges {
                e.from = map[&e.from];
                e.to = map[&e.to];
            }
            out.push(n);
        }
        (out, map[&root])
    }
}
