//! Read-only helpers over a laid-out scene, ported from src/canvas/model.ts.

use crate::camera::{child_xf, parent_xf, Xf};
use blockviz_core::ir::{Kind, Node, Scene, Span};
use gpui::SharedString;
use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeSet, HashMap};

/// Blocks whose own source is what a reader wants to see: definitions and files.
fn whole(k: Kind) -> bool {
    matches!(k, Kind::Repo | Kind::Dir | Kind::File | Kind::Notebook | Kind::Doc | Kind::Asset | Kind::Class | Kind::Function | Kind::Section)
}

/// A step is shown inside its definition when that is at most this long…
const CONTEXT_LINES: u32 = 80;
/// …and otherwise with this many lines either side.
const WINDOW: u32 = 8;

/// Source shown for a block: `span` around it, read as part of `root`.
pub struct SourceView {
    pub root: u32,
    pub span: Span,
}

/// A block as search sees it.
pub struct SearchEntry {
    pub id: u32,
    lower: String,
    lower_detail: String,
    parent: u32,
    depth: usize,
    rank: u32,
}

pub struct Model {
    pub scene: Scene,
    /// Every block's title, ready to draw.
    pub titles: Vec<SharedString>,
    /// Each file's text, by its index in `scene.files`.
    texts: Vec<Option<String>>,
    line_starts: RefCell<HashMap<u32, Vec<usize>>>,
    users: OnceCell<HashMap<u32, Vec<u32>>>,
    search: OnceCell<(Vec<SearchEntry>, HashMap<u32, String>)>,
}

impl Model {
    /// `files` are the texts the scene was made from, by path.
    pub fn new(scene: Scene, files: HashMap<String, String>) -> Self {
        let titles = scene.nodes.iter().map(|n| SharedString::from(n.title.clone())).collect();
        let texts = scene
            .files
            .iter()
            .map(|f| {
                f.source.clone().or_else(|| files.get(&f.path).cloned()).or_else(|| {
                    // The core drops a folder wrapping everything from paths.
                    let suffix = format!("/{}", f.path);
                    files.iter().find(|(p, _)| p.ends_with(&suffix)).map(|(_, t)| t.clone())
                })
            })
            .collect();
        Model { scene, titles, texts, line_starts: Default::default(), users: OnceCell::new(), search: OnceCell::new() }
    }

    pub fn root(&self) -> u32 {
        self.scene.root
    }

    pub fn node(&self, id: u32) -> &Node {
        &self.scene.nodes[id as usize]
    }

    /// The node whose interior is drawn inside `n`: itself, or the definition it references.
    pub fn owner<'a>(&'a self, n: &'a Node) -> Option<&'a Node> {
        let content = |n: &Node| !n.children.is_empty() || !n.rows.is_empty();
        if content(n) {
            return (n.inner.is_some() || n.id == self.root()).then_some(n);
        }
        let r = self.node(n.reference?);
        (n.inner.is_some() && content(r)).then_some(r)
    }

    fn content_size(o: &Node) -> (f64, f64) {
        (if o.cw > 0.0 { o.cw } else { 1.0 }, if o.ch > 0.0 { o.ch } else { 1.0 })
    }

    /// Transform of `n`'s interior, given the transform of the space `n` lives in.
    pub fn interior(&self, t: Xf, n: &Node) -> Option<Xf> {
        let o = self.owner(n)?;
        n.inner?;
        let (cw, ch) = Self::content_size(o);
        Some(child_xf(t, n, cw, ch))
    }

    /// `interior` with the owner already known.
    pub fn interior_of(&self, t: Xf, n: &Node, owner: &Node) -> Option<Xf> {
        n.inner?;
        let (cw, ch) = Self::content_size(owner);
        Some(child_xf(t, n, cw, ch))
    }

    pub fn exterior(&self, c: Xf, n: &Node) -> Xf {
        let (cw, ch) = self.owner(n).map_or((1.0, 1.0), Self::content_size);
        parent_xf(c, n, cw, ch)
    }

    /// Root-to-node chain through ownership (the node's definition site).
    pub fn canonical_path(&self, id: u32) -> Vec<u32> {
        let mut path = vec![];
        let mut cur = Some(id);
        while let Some(c) = cur {
            path.push(c);
            cur = self.node(c).parent;
        }
        path.reverse();
        if path[0] != self.root() {
            path.insert(0, self.root());
        }
        path
    }

    pub fn file_of(&self, n: &Node) -> Option<&str> {
        n.span.as_ref().and_then(|s| self.scene.files.get(s.file as usize)).map(|f| f.path.as_str())
    }

    pub fn file_text(&self, file: u32) -> Option<&str> {
        self.texts.get(file as usize)?.as_deref()
    }

    /// Blocks that use `id`: calls of a function, constructions of a class,
    /// components of its type, calls of a component. In scene order.
    pub fn users(&self, id: u32) -> &[u32] {
        let index = self.users.get_or_init(|| {
            let mut index: HashMap<u32, BTreeSet<u32>> = HashMap::new();
            for n in &self.scene.nodes {
                for target in [n.link, n.reference].into_iter().flatten() {
                    if target != n.id {
                        index.entry(target).or_default().insert(n.id);
                    }
                }
                for &u in &n.used_by {
                    index.entry(n.id).or_default().insert(u);
                }
            }
            index.into_iter().map(|(k, v)| (k, v.into_iter().collect())).collect()
        });
        index.get(&id).map_or(&[], Vec::as_slice)
    }

    /// The definition a block is part of (`Block.forward`, a section, a file), for saying where it is.
    pub fn where_is(&self, id: u32) -> Option<(String, u32)> {
        let mut p = self.node(id).parent;
        while let Some(at) = p {
            if whole(self.node(at).kind) {
                return Some((self.qualified(at), at));
            }
            p = self.node(at).parent;
        }
        None
    }

    /// A definition's name as a reader would write it: `Block.forward` for a method.
    pub fn qualified(&self, id: u32) -> String {
        let def = self.node(id);
        match def.parent.map(|p| self.node(p)) {
            Some(up) if def.kind == Kind::Function && up.kind == Kind::Class => format!("{}.{}", up.title, def.title),
            _ => def.title.clone(),
        }
    }

    /// The source to show for a block. Definitions and files show their own
    /// code; a step is shown in the definition around it (or with a few lines
    /// either side when that is long), so a single line is never read alone.
    pub fn source_view(&self, n: &Node) -> Option<SourceView> {
        let span = n.span.clone()?;
        if whole(n.kind) {
            return Some(SourceView { root: n.id, span });
        }
        let mut root = n.id;
        let mut p = n.parent;
        while let Some(at) = p {
            let a = self.node(at);
            p = a.parent;
            let Some(sp) = &a.span else { continue };
            if sp.file != span.file || sp.start > span.start || sp.end < span.end || !whole(a.kind) {
                continue;
            }
            root = a.id;
            if sp.end_line - sp.line <= CONTEXT_LINES {
                return Some(SourceView { root, span: sp.clone() });
            }
            break;
        }
        Some(SourceView { root, span: self.window(&span, WINDOW) })
    }

    /// A span's lines plus `k` lines either side.
    pub fn window(&self, span: &Span, k: u32) -> Span {
        let Some(text) = self.file_text(span.file) else { return span.clone() };
        let mut cache = self.line_starts.borrow_mut();
        let starts = cache.entry(span.file).or_insert_with(|| {
            let bytes = text.as_bytes();
            std::iter::once(0).chain((0..bytes.len()).filter(|&i| bytes[i] == b'\n' && i + 1 < bytes.len()).map(|i| i + 1)).collect()
        });
        let lines = starts.len() as u32;
        let line = span.line.saturating_sub(k).max(1);
        let end_line = (span.end_line + k).min(lines);
        let end = if end_line < lines { starts[end_line as usize] } else { text.len() };
        Span { file: span.file, start: starts[line as usize - 1] as u32, end: end as u32, line, end_line }
    }

    /// The block drawn for a line of `root`'s source: the smallest block inside
    /// it whose code covers the line, or `root` itself.
    pub fn block_at(&self, root: u32, line: u32) -> u32 {
        let file = self.node(root).span.as_ref().map(|s| s.file);
        let mut best = (root, u64::MAX);
        fn visit(m: &Model, n: u32, file: Option<u32>, line: u32, best: &mut (u32, u64)) {
            for &c in &m.node(n).children {
                let k = m.node(c);
                let Some(sp) = &k.span else {
                    // Sections and arms may have no span of their own; their steps do.
                    visit(m, c, file, line, best);
                    continue;
                };
                if Some(sp.file) != file || line < sp.line || line > sp.end_line {
                    continue;
                }
                if !matches!(k.kind, Kind::Input | Kind::Output | Kind::Unused) {
                    let extent = (sp.end_line - sp.line) as u64 * 10_000_000 + (sp.end - sp.start) as u64;
                    if extent <= best.1 {
                        *best = (c, extent);
                    }
                }
                visit(m, c, file, line, best);
            }
        }
        visit(self, root, file, line, &mut best);
        best.0
    }

    /// An instance path to `target` that stays where the reader is: through the
    /// deepest element of `near` whose interior holds `target` (the call it was
    /// reached through), else the target's own definition site.
    pub fn instance_path(&self, target: u32, near: &[u32]) -> Vec<u32> {
        let canonical = self.canonical_path(target);
        for i in (0..near.len()).rev() {
            let n = self.node(near[i]);
            let owner = self.owner(n).unwrap_or(n);
            if let Some(j) = canonical.iter().position(|&c| c == owner.id) {
                return near[..=i].iter().chain(&canonical[j + 1..]).copied().collect();
            }
        }
        canonical
    }

    /// What a block is in, as search lists it: `model.py › Block`.
    pub fn trail(&self, id: u32) -> String {
        let path = self.canonical_path(id);
        path[1..path.len() - 1].iter().map(|&i| self.node(i).title.as_str()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join(" › ")
    }

    fn search_index(&self) -> &(Vec<SearchEntry>, HashMap<u32, String>) {
        self.search.get_or_init(|| {
            let rank = |n: &Node| match n.kind {
                Kind::File | Kind::Notebook => 0,
                Kind::Class => 1,
                Kind::Function => 2,
                Kind::Section | Kind::Dir => 3,
                Kind::Artifact => 4,
                Kind::Table => 5,
                Kind::Call | Kind::Symbol => 6,
                Kind::Loop => 7,
                Kind::Op => 9,
                _ => 8,
            };
            // Each block's path, made once from its parent's: the titles below the root, lowercased, and how many.
            let mut trails: HashMap<u32, (String, usize)> = HashMap::from([(self.root(), (String::new(), 0))]);
            fn trail<'a>(m: &Model, id: u32, trails: &'a mut HashMap<u32, (String, usize)>) -> (String, usize) {
                if let Some(t) = trails.get(&id) {
                    return t.clone();
                }
                let n = m.node(id);
                let up = n.parent.map_or((String::new(), 0), |p| trail(m, p, trails));
                let title = n.title.to_lowercase();
                let t = if title.is_empty() { up } else { (if up.0.is_empty() { title } else { format!("{} {title}", up.0) }, up.1 + 1) };
                trails.insert(id, t.clone());
                t
            }
            let entries = self
                .scene
                .nodes
                .iter()
                .filter(|n| n.parent.is_some() && !n.title.is_empty() && !matches!(n.kind, Kind::Input | Kind::Output | Kind::Unused | Kind::Glyph | Kind::Lane))
                .map(|n| {
                    let parent = n.parent.unwrap();
                    let depth = trail(self, parent, &mut trails).1;
                    SearchEntry { id: n.id, lower: n.title.to_lowercase(), lower_detail: n.subtitle.to_lowercase(), parent, depth, rank: rank(n) }
                })
                .collect();
            (entries, trails.into_iter().map(|(k, v)| (k, v.0)).collect())
        })
    }

    /// Search results for `query`, best first: blocks named by it (exactly,
    /// by its start, anywhere in the name); then blocks named by its last word
    /// inside blocks named by the others, so `Block.forward`, `block forward`,
    /// `Db::get` and `api.py get` find the definition; then blocks whose
    /// caption or path mentions it. Definitions come before steps.
    pub fn find(&self, query: &str) -> Vec<u32> {
        const LISTED: usize = 60;
        let (entries, paths) = self.search_index();
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return entries.iter().filter(|e| e.rank <= 2).take(40).map(|e| e.id).collect();
        }
        let words: Vec<&str> = q.split(|c: char| c.is_whitespace() || ".:/\\›#,()".contains(c)).filter(|w| !w.is_empty()).collect();
        let Some(&last) = words.last() else { return vec![] };
        let in_path = |w: &str| paths.iter().filter(|(_, p)| p.contains(w)).map(|(&id, _)| id).collect::<std::collections::HashSet<u32>>();
        let others: Vec<(&str, std::collections::HashSet<u32>)> = words[..words.len() - 1].iter().map(|&w| (w, in_path(w))).collect();
        let mentioned = in_path(&q);
        let spaced = q.contains(' ');
        let mentions = |e: &SearchEntry| {
            mentioned.contains(&e.parent)
                || if spaced {
                    let p = &paths[&e.parent];
                    let cut = p.char_indices().nth(q.chars().count().saturating_sub(1)).map_or(p.len(), |(i, _)| i);
                    format!("{} {} {}", e.lower, e.lower_detail, &p[..cut]).contains(&q)
                } else {
                    e.lower_detail.contains(&q)
                }
        };
        let starts_word = |t: &str, w: &str| t.starts_with(w) || t.split(|c: char| c.is_whitespace() || "_-.".contains(c)).any(|p| p.starts_with(w));
        let mut best: Vec<(u64, u32)> = vec![];
        for e in entries {
            let t = &e.lower;
            let tier = match t.find(&q) {
                Some(0) if t.len() == q.len() => 0,
                Some(0) => 1,
                Some(_) => 2,
                None if !others.is_empty() && others.iter().all(|(w, at)| t.contains(w) || at.contains(&e.parent)) && starts_word(t, last) => {
                    if t == last {
                        3
                    } else if t.starts_with(last) {
                        4
                    } else {
                        5
                    }
                }
                None if mentions(e) => 6,
                None => continue,
            };
            let score = tier * 1000 + e.rank as u64 * 10 + e.depth.min(9) as u64;
            if best.len() == LISTED && score >= best[LISTED - 1].0 {
                continue;
            }
            // Of equal scores the first found stays first.
            let at = best.partition_point(|b| b.0 <= score);
            best.insert(at, (score, e.id));
            best.truncate(LISTED);
        }
        best.into_iter().map(|b| b.1).collect()
    }

    /// The code a span covers (none when its file's text is not known).
    pub fn source(&self, span: &Span) -> Option<&str> {
        self.file_text(span.file)?.get(span.start as usize..span.end as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(files: &[(&str, &str)]) -> Model {
        let inputs = files.iter().map(|(p, t)| blockviz_core::project::InFile { path: p.to_string(), text: Some(t.to_string()), bytes: t.len() as u64 }).collect();
        let scene = blockviz_core::analyze("t", inputs);
        Model::new(scene, files.iter().map(|(p, t)| (p.to_string(), t.to_string())).collect())
    }

    fn find_titled(m: &Model, title: &str) -> u32 {
        m.scene.nodes.iter().find(|n| n.title == title).unwrap_or_else(|| panic!("no {title}")).id
    }

    const MODEL_PY: &str = "class Block:\n    def __init__(self):\n        self.attn = Attention()\n\n    def forward(self, x):\n        x = x + self.attn(x)\n        return x\n\n\nclass Attention:\n    def __call__(self, x):\n        return x\n";

    #[test]
    fn shows_a_definition_whole_and_a_step_inside_the_definition_around_it() {
        let m = model(&[("model.py", MODEL_PY), ("api.py", "def get():\n    return 1\n")]);
        let forward = find_titled(&m, "forward");
        let v = m.source_view(m.node(forward)).unwrap();
        assert_eq!(v.root, forward);
        let step = m.node(forward).children.iter().copied().find(|&c| m.node(c).span.is_some() && !matches!(m.node(c).kind, Kind::Input | Kind::Output)).unwrap();
        let v = m.source_view(m.node(step)).unwrap();
        assert_eq!(v.root, forward);
        assert_eq!((v.span.line, v.span.end_line), (5, 7));
    }

    #[test]
    fn finds_the_block_drawn_for_a_line() {
        let m = model(&[("model.py", MODEL_PY), ("api.py", "def get():\n    return 1\n")]);
        let forward = find_titled(&m, "forward");
        let at = m.block_at(forward, 6);
        assert_ne!(at, forward);
        assert_eq!(m.node(at).span.as_ref().unwrap().line, 6);
        // A line with no step of its own is the definition.
        assert_eq!(m.block_at(forward, 5), forward);
    }

    #[test]
    fn finds_a_method_by_its_qualified_name_and_definitions_first() {
        let m = model(&[("model.py", MODEL_PY), ("api.py", "def get():\n    return 1\n")]);
        let found = m.find("Block.forward");
        assert_eq!(m.node(found[0]).title, "forward");
        assert_eq!(m.trail(found[0]), "model.py › Block");
        let found = m.find("api.py get");
        assert_eq!(m.node(found[0]).title, "get");
        assert_eq!(m.node(m.find("block")[0]).title, "Block");
    }

    #[test]
    fn says_where_a_step_is_defined() {
        let m = model(&[("model.py", MODEL_PY), ("api.py", "def get():\n    return 1\n")]);
        let forward = find_titled(&m, "forward");
        let step = m.node(forward).children[0];
        assert_eq!(m.where_is(step).unwrap().0, "Block.forward");
        // The attention class is used by the component that holds it.
        let attention = find_titled(&m, "Attention");
        assert!(!m.users(attention).is_empty());
    }
}
