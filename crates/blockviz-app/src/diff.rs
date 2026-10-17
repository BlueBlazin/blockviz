//! How the head of a pull request differs from its base, block by block,
//! ported from src/canvas/diff.ts (and the helpers that list changes, from
//! src/ui/ChangesPanel.tsx).

#![allow(dead_code)]

use crate::compare::{FileChange, FileStatus};
use crate::model::Model;
use crate::panel::kind_label;
use blockviz_core::ir::{EdgeKind, Kind, Node};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// What happened to a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Status {
    Added,
    Changed,
    Removed,
}

/// A change a reader steps through: a block added or changed (`id` in the
/// head), or removed (`id` in the base), shown at `at` in the head: the block
/// itself, or the block a removed one was in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change {
    pub status: Status,
    pub id: u32,
    pub at: u32,
}

/// A connection the base drew between two blocks the head still has, side by side, but no longer draws.
#[derive(Clone, Debug, PartialEq)]
pub struct RemovedEdge {
    pub from: u32,
    pub to: u32,
    pub kind: EdgeKind,
    pub label: String,
}

/// How many changes of each kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub added: usize,
    pub changed: usize,
    pub removed: usize,
}

/// Which of the two versions a block is in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Base,
    Head,
}

/// Blocks that only carry values across a boundary, or end one: never a change of their own.
fn is_port(k: Kind) -> bool {
    matches!(k, Kind::Input | Kind::Output | Kind::Unused)
}

/// How the head of a pull request differs from its base, block by block.
///
/// Blocks are matched by where they are: a file by its path (or the path it
/// was renamed from), a folder by its name, and anything in a file by its kind
/// and title among its siblings; siblings of the same title pair up by
/// identical code first, then in order. Only files the change touched have
/// added, changed or removed blocks: analysis of an untouched file can shift
/// with the code around it (a call now resolving), but that file did not change.
///
/// A block is changed when its code is (ignoring whitespace). What a reader
/// steps through are the innermost changes: an added or removed block (not
/// what is in it), and a changed block nothing in which changed.
pub struct SceneDiff {
    /// Head block → base block, and back.
    matches: HashMap<u32, u32>,
    back: HashMap<u32, u32>,
    /// The pairs in the order they were made.
    pairs: Vec<(u32, u32)>,
    statuses: HashMap<u32, Status>,
    /// Changes in each head block's subtree, its own included.
    subtree: HashMap<u32, usize>,
    units: HashSet<u32>,
    /// Removed blocks, by the head block they were in.
    removed_at: HashMap<u32, Vec<Change>>,
    /// Files the change touched, by their path in the head and in the base: indexes into `files`.
    head_changes: HashMap<String, usize>,
    base_changes: HashMap<String, usize>,
    /// Each block's code, whitespace collapsed, by version.
    texts: [RefCell<HashMap<u32, Option<Rc<str>>>>; 2],
    /// Where each change is in `changes`: by whether it is removed, and its block.
    order: HashMap<(bool, u32), usize>,
    pub changes: Vec<Change>,
    /// New connections, each by the block that draws it and its index in that block's edges.
    pub added: HashSet<(u32, usize)>,
    /// Removed connections, by the head block that would draw them.
    pub removed: HashMap<u32, Vec<RemovedEdge>>,
    pub files: Vec<FileChange>,
    pub base: Rc<Model>,
    pub head: Rc<Model>,
}

/// The path a changed file had in the base.
fn base_path(f: &FileChange) -> &str {
    match &f.previous {
        Some(p) if f.status == FileStatus::Renamed && !p.is_empty() => p,
        _ => &f.path,
    }
}

impl SceneDiff {
    pub fn new(base: Rc<Model>, head: Rc<Model>, files: Vec<FileChange>) -> Self {
        let mut head_changes = HashMap::new();
        let mut base_changes = HashMap::new();
        for (i, f) in files.iter().enumerate() {
            head_changes.insert(f.path.clone(), i);
            base_changes.insert(base_path(f).to_string(), i);
        }
        let mut d = SceneDiff {
            matches: HashMap::new(),
            back: HashMap::new(),
            pairs: vec![],
            statuses: HashMap::new(),
            subtree: HashMap::new(),
            units: HashSet::new(),
            removed_at: HashMap::new(),
            head_changes,
            base_changes,
            texts: Default::default(),
            order: HashMap::new(),
            changes: vec![],
            added: HashSet::new(),
            removed: HashMap::new(),
            files,
            base,
            head,
        };
        let (head_root, base_root) = (d.head.root(), d.base.root());
        d.pair_files();
        d.pair_tree(head_root, Some(base_root));
        d.pair_folders(head_root);
        d.pair_moved();
        d.pair_stops();
        d.mark(head_root);
        d.find_removed();
        d.count(head_root);
        d.list(head_root);
        d.compare_edges();
        d
    }

    /// What happened to a head block (none: nothing).
    pub fn status(&self, id: u32) -> Option<Status> {
        self.statuses.get(&id).copied()
    }

    /// The base block a head block was.
    pub fn base_of(&self, id: u32) -> Option<u32> {
        self.matches.get(&id).copied()
    }

    /// The head block a base block became.
    pub fn head_of(&self, id: u32) -> Option<u32> {
        self.back.get(&id).copied()
    }

    /// Changes inside a head block (removed blocks it held included), not counting its own.
    pub fn inside(&self, id: u32) -> usize {
        self.subtree.get(&id).copied().unwrap_or(0) - usize::from(self.units.contains(&id))
    }

    /// Changes in the code a block leads to: what a call runs, a reference's definition.
    pub fn through(&self, id: u32) -> usize {
        let n = self.head.node(id);
        match n.reference.or(n.link) {
            Some(target) if target != id => self.subtree.get(&target).copied().unwrap_or(0),
            _ => 0,
        }
    }

    /// Whether a head block changed or has changes inside.
    pub fn touched(&self, id: u32) -> bool {
        self.statuses.contains_key(&id) || self.subtree.get(&id).is_some_and(|&n| n > 0)
    }

    /// Removed blocks that were in a head block.
    pub fn removed_in(&self, id: u32) -> &[Change] {
        self.removed_at.get(&id).map_or(&[], Vec::as_slice)
    }

    /// Where a change is in the list, by its block (in the base when `removed`).
    pub fn index_of(&self, id: u32, removed: bool) -> Option<usize> {
        self.order.get(&(removed, id)).copied()
    }

    /// How many changes of each kind.
    pub fn summary(&self) -> Summary {
        let mut out = Summary::default();
        for c in &self.changes {
            match c.status {
                Status::Added => out.added += 1,
                Status::Changed => out.changed += 1,
                Status::Removed => out.removed += 1,
            }
        }
        out
    }

    /// How a file changed, by its path in the head (or, `base`, in the base).
    pub fn file_change(&self, path: &str, base: bool) -> Option<&FileChange> {
        let index = if base { &self.base_changes } else { &self.head_changes };
        index.get(path).map(|&i| &self.files[i])
    }

    fn model(&self, side: Side) -> &Model {
        match side {
            Side::Base => &self.base,
            Side::Head => &self.head,
        }
    }

    // ----- matching ---------------------------------------------------------------------

    fn pair(&mut self, h: u32, b: u32) {
        self.matches.insert(h, b);
        self.back.insert(b, h);
        self.pairs.push((h, b));
    }

    /// Files pair by path wherever their folders are drawn (folders fold and regroup as files come and go).
    fn pair_files(&mut self) {
        let (base, head) = (self.base.clone(), self.head.clone());
        let base_files: HashMap<String, Vec<u32>> = tops(&base, |n| self.untouched(Side::Base, n)).into_iter().collect();
        let head_files = tops(&head, |n| self.untouched(Side::Head, n));
        for (path, ids) in head_files {
            let was = base_files.get(self.file_change(&path, false).map_or(path.as_str(), base_path)).map_or(&[][..], Vec::as_slice);
            // One block for the file in each: the same file, whatever it is called now.
            if let ([h], [b]) = (ids.as_slice(), was) {
                self.pair(*h, *b);
            } else {
                self.pair_among(&ids, was);
            }
        }
    }

    /// Pairs head blocks with base blocks of the same kind and title: identical code first, then in order.
    fn pair_among(&mut self, heads: &[u32], bases: &[u32]) {
        let (base, head) = (self.base.clone(), self.head.clone());
        let mut groups: HashMap<(Kind, &str), Vec<u32>> = HashMap::new();
        // Stops all look alike: each is paired through its block (`pair_stops`).
        for &b in bases {
            if !self.back.contains_key(&b) && base.node(b).kind != Kind::Unused {
                groups.entry(key_of(base.node(b))).or_default().push(b);
            }
        }
        let mut left = vec![];
        for &h in heads {
            if self.matches.contains_key(&h) {
                continue;
            }
            let key = key_of(head.node(h));
            // In an untouched file the two versions are the same code: order pairs them.
            let found = match groups.get(&key) {
                Some(same) if self.in_change(h) => self.text(Side::Head, h).and_then(|text| same.iter().position(|&b| self.text(Side::Base, b).as_deref() == Some(&*text))),
                _ => None,
            };
            match found {
                Some(i) => {
                    let b = groups.get_mut(&key).unwrap().remove(i);
                    self.pair(h, b);
                }
                None => left.push(h),
            }
        }
        for h in left {
            if let Some(same) = groups.get_mut(&key_of(head.node(h))).filter(|s| !s.is_empty()) {
                let b = same.remove(0);
                self.pair(h, b);
            }
        }
    }

    fn pair_tree(&mut self, h: u32, b: Option<u32>) {
        if let Some(b) = b.filter(|b| !self.matches.contains_key(&h) && !self.back.contains_key(b)) {
            self.pair(h, b);
        }
        let head = self.head.clone();
        let n = head.node(h);
        if self.untouched(Side::Head, n) {
            return;
        }
        if let Some(&mine) = self.matches.get(&h) {
            let base = self.base.clone();
            self.pair_among(&n.children, &base.node(mine).children);
        }
        for &c in &n.children {
            self.pair_tree(c, None);
        }
    }

    /// Whether a head block is in a file the change touched.
    fn in_change(&self, h: u32) -> bool {
        scope_file(&self.head, self.head.node(h)).is_some_and(|file| self.head_changes.contains_key(file))
    }

    /// Whether a block is code in a file the change did not touch: nothing in it can differ, so no walk goes in.
    fn untouched(&self, side: Side, n: &Node) -> bool {
        let changes = if side == Side::Head { &self.head_changes } else { &self.base_changes };
        self.model(side).file_of(n).is_some_and(|file| !changes.contains_key(file))
    }

    /// A folder renamed or regrouped (a package moved under `src/`) is the base
    /// folder most of what it holds came from. Inner folders first, so a moved
    /// tree pairs from the files up.
    fn pair_folders(&mut self, h: u32) {
        let (base, head) = (self.base.clone(), self.head.clone());
        let n = head.node(h);
        if self.untouched(Side::Head, n) {
            return;
        }
        for &c in &n.children {
            self.pair_folders(c);
        }
        if self.matches.contains_key(&h) || n.span.is_some() {
            return;
        }
        // Votes in the order first cast, so a tie goes to the first.
        let mut votes: Vec<(u32, usize)> = vec![];
        let mut matched = 0;
        for c in &n.children {
            let Some(&b) = self.matches.get(c) else { continue };
            matched += 1;
            let Some(p) = base.node(b).parent else { continue };
            if self.back.contains_key(&p) || base.node(p).kind != n.kind {
                continue;
            }
            match votes.iter_mut().find(|v| v.0 == p) {
                Some(v) => v.1 += 1,
                None => votes.push((p, 1)),
            }
        }
        let mut best = None;
        let mut most = 0;
        for &(p, v) in &votes {
            if v > most {
                (best, most) = (Some(p), v);
            }
        }
        if best.is_some() && most * 2 >= matched {
            self.pair_tree(h, best);
        }
    }

    /// A stop is the base's stop under the block its own block was, if that had one.
    fn pair_stops(&mut self) {
        let (base, head) = (self.base.clone(), self.head.clone());
        let stop_of = |model: &Model, block: u32| -> Option<u32> {
            let p = model.node(block).parent?;
            model.node(p).edges.iter().find(|e| e.from == block && model.node(e.to).kind == Kind::Unused).map(|e| e.to)
        };
        for n in &head.scene.nodes {
            let Some(parent) = n.parent else { continue };
            if n.kind != Kind::Unused || self.matches.contains_key(&n.id) {
                continue;
            }
            let block = head.node(parent).edges.iter().find(|e| e.to == n.id).map(|e| e.from);
            let was = block.and_then(|b| self.matches.get(&b).copied());
            if let Some(stop) = was.and_then(|w| stop_of(&base, w)).filter(|s| !self.back.contains_key(s)) {
                self.pair(n.id, stop);
            }
        }
    }

    /// Code moved within a file, into or out of a branch, loop or other block
    /// (an `if` wrapped around steps): left over on both sides, the same kind,
    /// title and code. Paired, it is the same block in a new place, not one
    /// removed and another added.
    fn pair_moved(&mut self) {
        let (base, head) = (self.base.clone(), self.head.clone());
        /// Blocks below `root` not yet paired, ports aside, outside in.
        fn unpaired(model: &Model, root: u32, paired: &HashMap<u32, u32>, out: &mut Vec<u32>) {
            for &c in &model.node(root).children {
                if !paired.contains_key(&c) && !is_port(model.node(c).kind) {
                    out.push(c);
                }
                unpaired(model, c, paired, out);
            }
        }
        for (h, b) in self.pairs.clone() {
            let hn = head.node(h);
            let file = if hn.span.is_some() { head.file_of(hn) } else { None };
            if !file.is_some_and(|f| self.head_changes.contains_key(f)) || !is_top(&head, hn) {
                continue;
            }
            // Grouped by kind and title, so a block looks only among those it could be.
            let mut left: HashMap<(Kind, &str), Vec<u32>> = HashMap::new();
            let mut gone = vec![];
            unpaired(&base, b, &self.back, &mut gone);
            for x in gone {
                left.entry(key_of(base.node(x))).or_default().push(x);
            }
            if left.is_empty() {
                continue;
            }
            let mut new = vec![];
            unpaired(&head, h, &self.matches, &mut new);
            for c in new {
                if self.matches.contains_key(&c) {
                    continue;
                }
                let Some(text) = self.text(Side::Head, c) else { continue };
                let was = left.get(&key_of(head.node(c))).and_then(|xs| xs.iter().copied().find(|x| !self.back.contains_key(x) && self.text(Side::Base, *x).as_deref() == Some(&*text)));
                if was.is_some() {
                    self.pair_tree(c, was);
                }
            }
        }
    }

    // ----- statuses ---------------------------------------------------------------------

    /// A block's code, whitespace collapsed (none: it has none).
    fn text(&self, side: Side, id: u32) -> Option<Rc<str>> {
        let mut cache = self.texts[side as usize].borrow_mut();
        cache
            .entry(id)
            .or_insert_with(|| {
                let model = self.model(side);
                let src = model.node(id).span.as_ref().and_then(|s| model.source(s))?;
                Some(src.split_whitespace().collect::<Vec<_>>().join(" ").into())
            })
            .clone()
    }

    fn mark(&mut self, h: u32) {
        let head = self.head.clone();
        let n = head.node(h);
        if self.untouched(Side::Head, n) {
            return;
        }
        if let Some(s) = self.status_of(h) {
            self.statuses.insert(h, s);
        }
        for &c in &n.children {
            self.mark(c);
        }
    }

    fn status_of(&self, h: u32) -> Option<Status> {
        let head = &self.head;
        let n = head.node(h);
        // Above files: a new folder, or something no file is drawn in (a file on disk a program now writes).
        let Some(file) = scope_file(head, n) else {
            let new = |p: &str| self.file_change(p, false).is_some_and(|c| c.status == FileStatus::Added);
            return (!self.matches.contains_key(&h) && all_files(head, n, &new)).then_some(Status::Added);
        };
        let change = self.file_change(file, false)?;
        let Some(&b) = self.matches.get(&h).filter(|_| change.status != FileStatus::Added) else { return Some(Status::Added) };
        if n.span.is_some() && self.text(Side::Head, h) != self.text(Side::Base, b) {
            return Some(Status::Changed);
        }
        // A file moved without changing is still a change to the file.
        if change.status == FileStatus::Renamed && n.span.as_ref().is_some_and(|s| s.start == 0) && head.file_of(n) == Some(file) && is_top(head, n) {
            return Some(Status::Changed);
        }
        None
    }

    /// Whether a base block is one no head block became, in a file the change touched.
    fn gone(&self, b: u32) -> bool {
        if self.back.contains_key(&b) {
            return false;
        }
        let n = self.base.node(b);
        match scope_file(&self.base, n) {
            None => all_files(&self.base, n, &|p| self.file_change(p, true).is_some_and(|c| c.status == FileStatus::Removed)),
            Some(file) => self.file_change(file, true).is_some(),
        }
    }

    /// Base blocks no head block became, in files the change touched; listed by the head block they were in.
    fn find_removed(&mut self) {
        fn visit(d: &SceneDiff, b: u32, out: &mut HashMap<u32, Vec<Change>>) {
            let n = d.base.node(b);
            if d.untouched(Side::Base, n) {
                return;
            }
            if d.gone(b) && !is_port(n.kind) {
                // The outermost removed block stands for what was in it.
                let mut p = n.parent;
                while let Some(at) = p.filter(|at| !d.back.contains_key(at)) {
                    p = d.base.node(at).parent;
                }
                let at = p.map_or(d.head.root(), |p| d.back[&p]);
                out.entry(at).or_default().push(Change { status: Status::Removed, id: b, at });
                return;
            }
            for &c in &n.children {
                visit(d, c, out);
            }
        }
        let mut out = HashMap::new();
        visit(self, self.base.root(), &mut out);
        let base = &self.base;
        for list in out.values_mut() {
            list.sort_by(|a, b| {
                let (a, b) = (base.node(a.id), base.node(b.id));
                let start = |n: &Node| n.span.as_ref().map_or(0, |s| s.start);
                start(a).cmp(&start(b)).then(a.y.partial_cmp(&b.y).unwrap_or(std::cmp::Ordering::Equal))
            });
        }
        self.removed_at = out;
    }

    fn count(&mut self, h: u32) -> usize {
        let head = self.head.clone();
        let n = head.node(h);
        if self.untouched(Side::Head, n) {
            return 0;
        }
        let s = self.statuses.get(&h).copied();
        let mut total = self.removed_in(h).len();
        // An added block is one change, whatever is in it.
        if s == Some(Status::Added) {
            total = 0;
        } else {
            for &c in &n.children {
                total += self.count(c);
            }
        }
        if s.is_some() && !is_port(n.kind) && total == 0 {
            self.units.insert(h);
            total = 1;
        }
        self.subtree.insert(h, total);
        total
    }

    /// Changes in diagram order: top to bottom, left to right, outside in.
    fn list(&mut self, h: u32) {
        if self.units.contains(&h) {
            self.add(Change { status: self.statuses[&h], id: h, at: h });
        }
        if self.statuses.get(&h) != Some(&Status::Added) {
            let head = self.head.clone();
            let mut kids = head.node(h).children.clone();
            kids.sort_by(|&a, &b| {
                let (a, b) = (head.node(a), head.node(b));
                a.y.partial_cmp(&b.y).unwrap_or(std::cmp::Ordering::Equal).then(a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal))
            });
            for c in kids {
                if self.subtree.get(&c).is_some_and(|&n| n > 0) {
                    self.list(c);
                }
            }
        }
        for r in self.removed_in(h).to_vec() {
            self.add(r);
        }
    }

    fn add(&mut self, c: Change) {
        self.order.insert((c.status == Status::Removed, c.id), self.changes.len());
        self.changes.push(c);
    }

    // ----- connections ------------------------------------------------------------------

    /// New and removed connections between blocks, in diagrams the change
    /// touched (and the repository's own, of files and folders). Dependencies
    /// are drawn transitively reduced, so one still implied through others is
    /// neither new nor removed.
    fn compare_edges(&mut self) {
        let (base, head) = (self.base.clone(), self.head.clone());
        for hn in &head.scene.nodes {
            let Some(&b) = self.matches.get(&hn.id) else { continue };
            let bn = base.node(b);
            if hn.edges.is_empty() && bn.edges.is_empty() {
                continue;
            }
            if scope_file(&head, hn).is_some_and(|file| self.file_change(file, false).is_none()) {
                continue;
            }
            let was: HashSet<(u32, u32, EdgeKind)> = bn.edges.iter().filter_map(|e| Some((*self.back.get(&e.from)?, *self.back.get(&e.to)?, e.kind))).collect();
            let now: HashSet<(u32, u32, EdgeKind)> = hn.edges.iter().map(|e| (e.from, e.to, e.kind)).collect();
            for (i, e) in hn.edges.iter().enumerate() {
                if was.contains(&(e.from, e.to, e.kind)) {
                    continue;
                }
                if let (EdgeKind::Uses, Some(&f), Some(&t)) = (e.kind, self.matches.get(&e.from), self.matches.get(&e.to)) {
                    if reaches(bn, f, t) {
                        continue;
                    }
                }
                self.added.insert((hn.id, i));
            }
            let mut gone: Vec<RemovedEdge> = vec![];
            for e in &bn.edges {
                // A removed block takes its connections with it; one drawn elsewhere now is not here to connect.
                let (Some(&f), Some(&t)) = (self.back.get(&e.from), self.back.get(&e.to)) else { continue };
                if f == t || head.node(f).parent != Some(hn.id) || head.node(t).parent != Some(hn.id) {
                    continue;
                }
                if now.contains(&(f, t, e.kind)) || (e.kind == EdgeKind::Uses && reaches(hn, f, t)) {
                    continue;
                }
                if gone.iter().any(|g| g.from == f && g.to == t && g.kind == e.kind) {
                    continue;
                }
                gone.push(RemovedEdge { from: f, to: t, kind: e.kind, label: e.label.clone() });
            }
            if !gone.is_empty() {
                self.removed.insert(hn.id, gone);
            }
        }
    }
}

/// Siblings that may be the same block across versions.
fn key_of(n: &Node) -> (Kind, &str) {
    (n.kind, n.title.as_str())
}

/// The path of the file a block is in (the nearest block with code), or none above files.
fn scope_file<'a>(model: &'a Model, n: &'a Node) -> Option<&'a str> {
    let mut c = Some(n);
    while let Some(at) = c {
        if at.span.is_some() {
            return model.file_of(at);
        }
        c = at.parent.map(|p| model.node(p));
    }
    None
}

/// Whether a block is the outermost one drawn for its file.
fn is_top(model: &Model, n: &Node) -> bool {
    let file = n.span.as_ref().map(|s| s.file);
    let mut p = n.parent;
    while let Some(at) = p {
        if model.node(at).span.as_ref().map(|s| s.file) == file {
            return false;
        }
        p = model.node(at).parent;
    }
    true
}

/// The outermost blocks drawn for each file, by path in the order first
/// drawn (not looking inside blocks to `skip`).
fn tops(model: &Model, skip: impl Fn(&Node) -> bool) -> Vec<(String, Vec<u32>)> {
    fn visit(model: &Model, id: u32, seen: &mut Vec<u32>, skip: &dyn Fn(&Node) -> bool, out: &mut Vec<(String, Vec<u32>)>) {
        let n = model.node(id);
        let fresh = n.span.as_ref().map(|s| s.file).filter(|f| !seen.contains(f));
        if let Some(file) = fresh {
            if let Some(path) = model.file_of(n) {
                match out.iter_mut().find(|(p, _)| p == path) {
                    Some((_, ids)) => ids.push(id),
                    None => out.push((path.to_string(), vec![id])),
                }
            }
            seen.push(file);
        }
        if !skip(n) {
            for &c in &n.children {
                visit(model, c, seen, skip, out);
            }
        }
        if fresh.is_some() {
            seen.pop();
        }
    }
    let mut out = vec![];
    visit(model, model.root(), &mut vec![], &skip, &mut out);
    out
}

/// Whether every file a folder (or group) holds is `which`; true of a block that holds no files.
fn all_files(model: &Model, n: &Node, which: &dyn Fn(&str) -> bool) -> bool {
    fn visit(model: &Model, c: &Node, which: &dyn Fn(&str) -> bool, any: &mut bool, all: &mut bool) {
        if c.span.is_some() {
            if let Some(path) = model.file_of(c) {
                *any = true;
                if !which(path) {
                    *all = false;
                }
            }
            return;
        }
        for &k in &c.children {
            visit(model, model.node(k), which, any, all);
        }
    }
    let (mut any, mut all) = (false, true);
    visit(model, n, which, &mut any, &mut all);
    !any || all
}

/// Whether `to` is reached from `from` along a diagram's dependencies.
fn reaches(owner: &Node, from: u32, to: u32) -> bool {
    let mut next: HashMap<u32, Vec<u32>> = HashMap::new();
    for e in owner.edges.iter().filter(|e| e.kind == EdgeKind::Uses) {
        next.entry(e.from).or_default().push(e.to);
    }
    let mut seen = HashSet::from([from]);
    let mut stack = vec![from];
    while let Some(at) = stack.pop() {
        for &t in next.get(&at).map_or(&[][..], Vec::as_slice) {
            if t == to {
                return true;
            }
            if seen.insert(t) {
                stack.push(t);
            }
        }
    }
    false
}

// ----- listing ----------------------------------------------------------------------

/// A block's name as a list shows it: its title, or for an untitled branch what its first arm tests.
pub fn block_name(model: &Model, n: &Node) -> String {
    if !n.title.is_empty() {
        return n.title.clone();
    }
    let arm = n.children.iter().map(|&c| model.node(c)).find(|c| c.kind == Kind::Lane && !c.title.is_empty());
    arm.map_or_else(|| kind_label(n.kind).to_string(), |a| a.title.clone())
}

/// Blocks that are where code is, not code: no definition around them to name, no line to give.
fn is_outer(k: Kind) -> bool {
    matches!(k, Kind::Repo | Kind::Dir | Kind::File | Kind::Notebook | Kind::Doc | Kind::Asset | Kind::Group)
}

/// How a change is listed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeInfo {
    /// Its block's name.
    pub name: String,
    /// The definition it is in, when that is not just the repository and not named the same.
    pub context: Option<String>,
    /// What kind of block it is.
    pub kind: &'static str,
    /// The file its code is in.
    pub file: Option<String>,
    /// The line its code starts on (none for a whole file or folder).
    pub line: Option<u32>,
}

/// How a change is listed: its block's name, the definition it is in, its kind, and where its code is.
pub fn describe_change(diff: &SceneDiff, c: &Change) -> ChangeInfo {
    let model = if c.status == Status::Removed { &diff.base } else { &diff.head };
    let n = model.node(c.id);
    let outer = is_outer(n.kind);
    // What it is in, unless that is just the repository.
    let w = if outer { None } else { model.where_is(c.id) };
    let context = w.filter(|(name, at)| *at != model.root() && *name != n.title).map(|(name, _)| name);
    ChangeInfo {
        name: block_name(model, n),
        context,
        kind: kind_label(n.kind),
        file: scope_file(model, n).map(str::to_string),
        // A whole file starts at its first line: no line to give.
        line: if outer { None } else { n.span.as_ref().map(|s| s.line) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blockviz_core::ir::{Builder, Edge, FileMeta, Rect, Scene, Shape, Span, Stats};

    /// A block of a hand-built diagram: `code` is its text in its file (found after its previous sibling's).
    struct Spec {
        kind: Kind,
        title: String,
        file: Option<&'static str>,
        code: Option<String>,
        children: Vec<Spec>,
        edges: Vec<(String, String, EdgeKind)>,
        reference: Option<String>,
    }

    fn spec(kind: Kind, title: &str) -> Spec {
        Spec { kind, title: title.to_string(), file: None, code: None, children: vec![], edges: vec![], reference: None }
    }

    impl Spec {
        fn file(mut self, f: &'static str) -> Self {
            self.file = Some(f);
            self
        }
        fn code(mut self, c: &str) -> Self {
            self.code = Some(c.to_string());
            self
        }
        fn kids(mut self, k: Vec<Spec>) -> Self {
            self.children = k;
            self
        }
        fn edges(mut self, e: &[(&str, &str, EdgeKind)]) -> Self {
            self.edges = e.iter().map(|(a, b, k)| (a.to_string(), b.to_string(), *k)).collect();
            self
        }
    }

    fn func(title: &str, code: &str, children: Vec<Spec>, edges: &[(&str, &str, EdgeKind)]) -> Spec {
        spec(Kind::Function, title).code(code).kids(children).edges(edges)
    }

    fn step(title: &str, code: &str) -> Spec {
        spec(Kind::Call, title).code(code)
    }

    fn file(path: &'static str) -> Spec {
        spec(Kind::File, path.rsplit('/').next().unwrap()).file(path)
    }

    use EdgeKind::{Data, Uses};

    type Files<'a> = [(&'static str, &'a str)];

    fn span_of(files: &Files, path: &str, start: usize, end: usize) -> Span {
        let index = files.iter().position(|f| f.0 == path).unwrap();
        let text = files[index].1;
        let line = text[..start].matches('\n').count() as u32 + 1;
        Span { file: index as u32, start: start as u32, end: end as u32, line, end_line: line + text[start..end].matches('\n').count() as u32 }
    }

    /// A diagram of `files` drawn as `root`; titles of blocks are unique enough to name them in tests.
    fn scene(files: &Files, root: Spec) -> Scene {
        fn build(b: &mut Builder, files: &Files, spec: &Spec, parent: Option<u32>, file: Option<&'static str>, from: usize, refs: &mut Vec<(u32, String)>) -> u32 {
            let shape = if matches!(spec.kind, Kind::Loop | Kind::Branch) { Shape::Region } else { Shape::Box };
            let id = b.add(parent, spec.kind, shape, spec.title.clone());
            let text = |f: &str| files.iter().find(|x| x.0 == f).unwrap().1;
            let f = spec.file.or(file);
            let span = if let Some(sf) = spec.file {
                Some(span_of(files, sf, 0, text(sf).len()))
            } else if let (Some(f), Some(code)) = (f, &spec.code) {
                let start = text(f)[from..].find(code.as_str()).unwrap_or_else(|| panic!("no {code} in {f}")) + from;
                Some(span_of(files, f, start, start + code.len()))
            } else {
                None
            };
            let n = b.get_mut(id);
            (n.w, n.h) = (10.0, 10.0);
            n.span = span.clone();
            if let Some(r) = &spec.reference {
                refs.push((id, r.clone()));
            }
            let mut cursor = span.as_ref().map_or(from, |s| s.start as usize);
            let mut kids = vec![];
            for (i, c) in spec.children.iter().enumerate() {
                let k = build(b, files, c, Some(id), f, cursor, refs);
                b.get_mut(k).y = i as f64 * 10.0;
                if let Some(ks) = &b.get(k).span {
                    if Some(ks.file) == span.as_ref().map(|s| s.file) {
                        cursor = ks.end as usize;
                    }
                }
                kids.push(k);
            }
            if !kids.is_empty() {
                let n = b.get_mut(id);
                n.inner = Some(Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 });
                (n.cw, n.ch) = (10.0, 10.0);
            }
            let named = |b: &Builder, t: &str| kids.iter().copied().find(|&k| b.get(k).title == t).unwrap();
            let edges: Vec<Edge> = spec
                .edges
                .iter()
                .map(|(from, to, kind)| Edge { from: named(b, from), to: named(b, to), kind: *kind, label: String::new(), points: vec![], label_at: None, open: false, to_port: None, from_port: None })
                .collect();
            b.get_mut(id).edges = edges;
            id
        }
        let mut b = Builder::default();
        let mut refs = vec![];
        build(&mut b, files, &root, None, None, 0, &mut refs);
        for (id, title) in refs {
            let target = b.nodes.iter().find(|m| m.title == title && m.kind == Kind::Function).unwrap().id;
            let n = b.get_mut(id);
            (n.reference, n.link) = (Some(target), Some(target));
        }
        let meta = files.iter().map(|(path, text)| FileMeta { path: path.to_string(), language: "python".into(), bytes: text.len() as u64, lines: 1, analyzed: true, source: None }).collect();
        Scene { version: 1, title: "repo".into(), description: String::new(), root: 0, nodes: b.nodes, files: meta, diagnostics: vec![], stats: Stats::default() }
    }

    fn wrap(files: &Files, scene: Scene) -> Rc<Model> {
        Rc::new(Model::new(scene, files.iter().map(|(p, t)| (p.to_string(), t.to_string())).collect()))
    }

    fn model(files: &Files, root: Spec) -> Rc<Model> {
        wrap(files, scene(files, root))
    }

    fn modified(path: &str) -> FileChange {
        FileChange { path: path.into(), status: FileStatus::Modified, previous: None }
    }

    fn renamed(path: &str, previous: &str) -> FileChange {
        FileChange { path: path.into(), status: FileStatus::Renamed, previous: Some(previous.into()) }
    }

    fn added(path: &str) -> FileChange {
        FileChange { path: path.into(), status: FileStatus::Added, previous: None }
    }

    /// The block titled `title` (the first, or the one in `parent`).
    fn id_in(nodes: &blockviz_core::ir::Nodes, title: &str, parent: Option<&str>) -> u32 {
        nodes
            .iter()
            .find(|n| n.title == title && parent.is_none_or(|p| n.parent.is_some_and(|q| nodes[q as usize].title == p)))
            .unwrap_or_else(|| panic!("no {title}"))
            .id
    }

    fn id(m: &Model, title: &str) -> u32 {
        id_in(&m.scene.nodes, title, None)
    }

    fn changes(d: &SceneDiff) -> Vec<String> {
        d.changes
            .iter()
            .map(|c| {
                let (status, m) = match c.status {
                    Status::Added => ("added", &d.head),
                    Status::Changed => ("changed", &d.head),
                    Status::Removed => ("removed", &d.base),
                };
                format!("{status} {}", m.node(c.id).title)
            })
            .collect()
    }

    /// A connection as `from→to`, by the titles of the blocks it joins.
    fn titled(m: &Model, from: u32, to: u32) -> String {
        format!("{}→{}", m.node(from).title, m.node(to).title)
    }

    fn added_edges(d: &SceneDiff) -> Vec<String> {
        let mut out: Vec<String> = d
            .added
            .iter()
            .map(|&(owner, i)| {
                let e = &d.head.node(owner).edges[i];
                titled(&d.head, e.from, e.to)
            })
            .collect();
        out.sort();
        out
    }

    const BASE_MODEL: &str = "def load(path):\n    data = read(path)\n    return parse(data)\n\ndef save(x):\n    write(x)\n\ndef old():\n    pass\n";
    const HEAD_MODEL: &str = "def load(path):\n    data = read(path)\n    return parse(data, strict=True)\n\ndef save(x):\n    write(x)\n\ndef fresh():\n    a = make()\n    b = use(a)\n";
    const UTIL: &str = "def helper():\n    pass\n";
    const BASE_FILES: &Files = &[("model.py", BASE_MODEL), ("util.py", UTIL)];
    const HEAD_FILES: &Files = &[("model.py", HEAD_MODEL), ("util.py", UTIL)];

    fn load_base() -> Spec {
        func("load", "def load(path):\n    data = read(path)\n    return parse(data)", vec![step("read", "read(path)"), step("parse", "parse(data)")], &[("read", "parse", Data)])
    }

    fn load_head() -> Spec {
        func("load", "def load(path):\n    data = read(path)\n    return parse(data, strict=True)", vec![step("read", "read(path)"), step("parse", "parse(data, strict=True)")], &[("read", "parse", Data)])
    }

    fn tree(defs: Vec<Spec>, util: Vec<Spec>) -> Spec {
        spec(Kind::Repo, "repo").kids(vec![file("model.py").kids(defs), file("util.py").kids(util)])
    }

    /// The pull request most tests read: `load` changed, `old` removed, `fresh` added.
    fn fixture() -> SceneDiff {
        let helper = || func("helper", "def helper():\n    pass", vec![], &[]);
        let save = || func("save", "def save(x):\n    write(x)", vec![step("write", "write(x)")], &[]);
        let base = model(BASE_FILES, tree(vec![load_base(), save(), func("old", "def old():\n    pass", vec![], &[])], vec![helper()]));
        let fresh = func("fresh", "def fresh():\n    a = make()\n    b = use(a)", vec![step("make", "make()"), step("use", "use(a)")], &[("make", "use", Data)]);
        // The analysis of an untouched file may shift (a step now resolving): not a change.
        let helper_head = func("helper", "def helper():\n    pass", vec![step("pass", "pass")], &[]);
        let head = model(HEAD_FILES, tree(vec![load_head(), save(), fresh], vec![helper_head]));
        SceneDiff::new(base, head, vec![modified("model.py")])
    }

    #[test]
    fn marks_what_changed_down_to_the_innermost_change() {
        let d = fixture();
        let h = &d.head;
        assert_eq!(d.status(id(h, "parse")), Some(Status::Changed));
        assert_eq!(d.status(id(h, "load")), Some(Status::Changed));
        assert_eq!(d.status(id(h, "model.py")), Some(Status::Changed));
        assert_eq!(d.status(id(h, "read")), None);
        assert_eq!(d.status(id(h, "save")), None);
        // Something new in an untouched file is not a change.
        assert_eq!(d.status(id(h, "pass")), None);
        assert_eq!(d.status(id(h, "util.py")), None);
    }

    #[test]
    fn counts_an_added_block_once_and_lists_removed_blocks_where_they_were() {
        let d = fixture();
        let (h, b) = (&d.head, &d.base);
        assert_eq!(d.status(id(h, "fresh")), Some(Status::Added));
        assert_eq!(d.status(id(h, "make")), Some(Status::Added));
        assert_eq!(changes(&d), ["changed parse", "added fresh", "removed old"]);
        assert_eq!(d.changes[2].at, id(h, "model.py"));
        assert_eq!(d.inside(id(h, "model.py")), 3);
        assert_eq!(d.inside(id(h, "load")), 1);
        assert_eq!(d.inside(id(h, "repo")), 3);
        assert!(!d.touched(id(h, "util.py")));
        assert_eq!(d.summary(), Summary { added: 1, changed: 1, removed: 1 });
        assert_eq!(d.index_of(id(h, "fresh"), false), Some(1));
        assert_eq!(d.index_of(id(b, "old"), true), Some(2));
    }

    #[test]
    fn matches_files_by_the_path_they_were_renamed_from() {
        let moved: &Files = &[("core.py", HEAD_MODEL), ("util.py", UTIL)];
        let h = model(moved, spec(Kind::Repo, "repo").kids(vec![file("core.py").kids(vec![load_head()]), file("util.py")]));
        let b = model(BASE_FILES, spec(Kind::Repo, "repo").kids(vec![file("model.py").kids(vec![load_base()]), file("util.py")]));
        let r = SceneDiff::new(b.clone(), h.clone(), vec![renamed("core.py", "model.py")]);
        assert_eq!(r.status(id(&h, "load")), Some(Status::Changed));
        assert_eq!(r.base_of(id(&h, "core.py")), Some(id(&b, "model.py")));
        assert_eq!(changes(&r), ["changed parse"]);
    }

    #[test]
    fn counts_a_new_folder_once() {
        let more: &Files = &[("model.py", BASE_MODEL), ("util.py", UTIL), ("pkg/a.py", "x = 1\n"), ("pkg/b.py", "y = 2\n")];
        let repo = |extra: Vec<Spec>| spec(Kind::Repo, "repo").kids(vec![file("model.py"), file("util.py")].into_iter().chain(extra).collect());
        let h = model(more, repo(vec![spec(Kind::Dir, "pkg/").kids(vec![file("pkg/a.py"), file("pkg/b.py")])]));
        let b = model(BASE_FILES, repo(vec![]));
        let r = SceneDiff::new(b, h.clone(), vec![added("pkg/a.py"), added("pkg/b.py")]);
        assert_eq!(changes(&r), ["added pkg/"]);
        assert_eq!(r.status(id(&h, "a.py")), Some(Status::Added));
    }

    #[test]
    fn pairs_a_folder_moved_under_src_with_the_one_it_was() {
        let bf: &Files = &[("pkg/a.py", "A"), ("pkg/b.py", "B"), ("tests/t.py", "T")];
        let hf: &Files = &[("src/pkg/a.py", "A2"), ("src/pkg/b.py", "B"), ("tests/t.py", "T")];
        let repo = |files: &Files, pkg: &str| {
            let inner = files.iter().filter(|f| !f.0.starts_with("tests")).map(|f| file(f.0)).collect();
            spec(Kind::Repo, "repo")
                .kids(vec![spec(Kind::Dir, pkg).kids(inner), spec(Kind::Dir, "tests/").kids(vec![file("tests/t.py")])])
                .edges(&[("tests/", pkg, Uses)])
        };
        let b = model(bf, repo(bf, "pkg/"));
        let h = model(hf, repo(hf, "src/pkg/"));
        let r = SceneDiff::new(b.clone(), h.clone(), vec![renamed("src/pkg/a.py", "pkg/a.py"), renamed("src/pkg/b.py", "pkg/b.py")]);
        assert_eq!(r.base_of(id(&h, "src/pkg/")), Some(id(&b, "pkg/")));
        assert_eq!(r.status(id(&h, "src/pkg/")), None);
        // tests/ depended on the package before too.
        assert!(r.added.is_empty());
        assert_eq!(changes(&r), ["changed a.py", "changed b.py"]);
    }

    #[test]
    fn pairs_blocks_of_the_same_title_by_their_code_first() {
        let bf: &Files = &[("m.py", "def f():\n    view(a)\n    view(b)\n")];
        let hf: &Files = &[("m.py", "def f():\n    view(z)\n    view(a)\n    view(b)\n")];
        let shape = |files: &Files, steps: &[&str]| spec(Kind::Repo, "repo").kids(vec![file("m.py").kids(vec![func("f", files[0].1.trim_end(), steps.iter().map(|s| step("view", s)).collect(), &[])])]);
        let b = model(bf, shape(bf, &["view(a)", "view(b)"]));
        let h = model(hf, shape(hf, &["view(z)", "view(a)", "view(b)"]));
        let r = SceneDiff::new(b, h.clone(), vec![modified("m.py")]);
        // The new call is the first; the others are the ones that were there.
        let views: Vec<_> = h.scene.nodes.iter().filter(|n| n.title == "view").map(|n| r.status(n.id)).collect();
        assert_eq!(views, [Some(Status::Added), None, None]);
    }

    #[test]
    fn follows_code_moved_into_a_new_branch() {
        let bf: &Files = &[("m.py", "def f(x):\n    y = scale(x)\n    return pick(y)\n")];
        let hf: &Files = &[("m.py", "def f(x):\n    if x:\n        y = scale(x)\n        return pick(y)\n    return x\n")];
        let b = model(bf, spec(Kind::Repo, "repo").kids(vec![file("m.py").kids(vec![func("f", bf[0].1.trim_end(), vec![step("scale", "scale(x)"), step("pick", "pick(y)")], &[])])]));
        let branch = spec(Kind::Branch, "").code("if x:\n        y = scale(x)\n        return pick(y)").kids(vec![step("scale", "scale(x)"), step("pick", "pick(y)")]);
        let h = model(hf, spec(Kind::Repo, "repo").kids(vec![file("m.py").kids(vec![func("f", hf[0].1.trim_end(), vec![branch], &[])])]));
        let r = SceneDiff::new(b.clone(), h.clone(), vec![modified("m.py")]);
        // One change: the new branch. The steps in it are the ones that were there.
        assert_eq!(changes(&r), ["added "]);
        assert_eq!(r.base_of(id(&h, "scale")), Some(id(&b, "scale")));
        assert_eq!(r.status(id(&h, "scale")), None);
    }

    #[test]
    fn finds_new_and_removed_connections_not_ones_still_implied() {
        let bf: &Files = &[("a.py", "A"), ("b.py", "B"), ("c.py", "C")];
        let hf: &Files = &[("a.py", "A2"), ("b.py", "B2"), ("c.py", "C")];
        let repo = |files: &Files, edges: &[(&str, &str, EdgeKind)]| spec(Kind::Repo, "repo").kids(files.iter().map(|f| file(f.0)).collect()).edges(edges);
        // a → c was drawn; now a → b → c draws it through b (still a dependency), and b → a is gone.
        // b → c is drawn now, but b reached c before too (through a): not new.
        let b = model(bf, repo(bf, &[("a.py", "c.py", Uses), ("b.py", "a.py", Uses)]));
        let h = model(hf, repo(hf, &[("a.py", "b.py", Uses), ("b.py", "c.py", Uses)]));
        let r = SceneDiff::new(b, h.clone(), vec![modified("a.py"), modified("b.py")]);
        assert_eq!(added_edges(&r), ["a.py→b.py"]);
        let removed: Vec<String> = r.removed.get(&h.root()).map_or(vec![], |gone| gone.iter().map(|e| titled(&h, e.from, e.to)).collect());
        assert_eq!(removed, ["b.py→a.py"]);
        // Inside a changed function: the new step's arrow is new.
        assert!(added_edges(&fixture()).is_empty());
    }

    #[test]
    fn pairs_a_stop_with_the_stop_of_the_block_it_hangs_under() {
        // A value of `make` is now unread too; that of `take` was before. Only
        // the first stop's line is new, though the new stop comes first.
        let files: &Files = &[("f.py", "def f():\n    a = make()\n    b = take()\n")];
        let shape = |stops: usize| {
            let mut steps = vec![step("make", "make()"), step("take", "take()")];
            steps.extend((0..stops).map(|_| spec(Kind::Unused, "unused")));
            spec(Kind::Repo, "repo").kids(vec![file("f.py").kids(vec![func("f", "def f():\n    a = make()\n    b = take()", steps, &[])])])
        };
        let wire = |s: &mut Scene, lines: &[(&str, usize)]| {
            let f = id_in(&s.nodes, "f", None);
            let stops: Vec<u32> = s.nodes[f as usize].children.iter().copied().filter(|&c| s.nodes[c as usize].kind == Kind::Unused).collect();
            let edges = lines
                .iter()
                .map(|&(from, i)| Edge { from: id_in(&s.nodes, from, None), to: stops[i], kind: Data, label: from.into(), points: vec![], label_at: None, open: false, to_port: None, from_port: None })
                .collect();
            s.nodes[f as usize].edges = edges;
        };
        let (mut bs, mut hs) = (scene(files, shape(1)), scene(files, shape(2)));
        wire(&mut bs, &[("take", 0)]);
        wire(&mut hs, &[("make", 0), ("take", 1)]);
        let r = SceneDiff::new(wrap(files, bs), wrap(files, hs), vec![modified("f.py")]);
        let from: Vec<&str> = r.added.iter().map(|&(owner, i)| r.head.node(r.head.node(owner).edges[i].from).title.as_str()).collect();
        assert_eq!(from, ["make"]);
        assert!(r.removed.is_empty());
    }

    #[test]
    fn says_when_a_call_leads_into_changed_code() {
        let bf: &Files = &[("m.py", "def g():\n    old()\n\ndef f():\n    g()\n")];
        let hf: &Files = &[("m.py", "def g():\n    new()\n\ndef f():\n    g()\n")];
        let shape = |inner: &str| {
            let mut call = step("g", "g()");
            call.reference = Some("g".into());
            spec(Kind::Repo, "repo").kids(vec![file("m.py").kids(vec![func("g", &format!("def g():\n    {inner}()"), vec![step(inner, &format!("{inner}()"))], &[]), func("f", "def f():\n    g()", vec![call], &[])])])
        };
        let r = SceneDiff::new(model(bf, shape("old")), model(hf, shape("new")), vec![modified("m.py")]);
        let call = id_in(&r.head.scene.nodes, "g", Some("f"));
        assert_eq!(r.through(call), 2);
        assert_eq!(r.status(call), None);
        assert_eq!(changes(&r), ["added new", "removed old"]);
    }

    #[test]
    fn describes_a_change_by_its_name_definition_and_line() {
        let d = fixture();
        let parse = describe_change(&d, &d.changes[0]);
        assert_eq!(parse, ChangeInfo { name: "parse".into(), context: Some("load".into()), kind: "Call", file: Some("model.py".into()), line: Some(3) });
        // A definition in a file is in no definition; a removed block is read in the base.
        let old = describe_change(&d, &d.changes[2]);
        assert_eq!(old, ChangeInfo { name: "old".into(), context: Some("model.py".into()), kind: "Function", file: Some("model.py".into()), line: Some(8) });
        // A file gives no line, and is in nothing but the repository.
        let file = describe_change(&d, &Change { status: Status::Changed, id: id(&d.head, "model.py"), at: 0 });
        assert_eq!((file.context, file.line, file.kind), (None, None, "File"));
    }
}
