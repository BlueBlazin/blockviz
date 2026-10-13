//! Layered (Sugiyama-style) layout for one diagram, top to bottom.
//!
//! Pipeline: components → cycle breaking → layering → shared-trunk dummies →
//! barycentric ordering → x placement by weighted isotonic regression → port
//! assignment → orthogonal routing with per-channel track allocation.
//!
//! Edges that carry the same value from the same source share a trunk, so one
//! line splits into several consumers (like residual connections in an
//! architecture figure) instead of fanning out as parallel wires.

use crate::hash::{Map, Set};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pin {
    Free,
    Top,
    Bottom,
}

#[derive(Clone, Debug)]
pub struct LNode {
    pub w: f64,
    pub h: f64,
    pub pin: Pin,
    pub order: f64,
    /// Forces left-to-right order within a layer (branch lanes).
    pub rank: Option<u32>,
    /// All edges meet at the center (operator glyphs).
    pub converge: bool,
    /// Packing priority for disconnected pieces (lower first).
    pub pack: i32,
}

#[derive(Clone, Debug)]
pub struct LEdge {
    pub from: usize,
    pub to: usize,
    /// Edges from one source with the same group share a port and trunk.
    pub group: u64,
    /// Fixed attachment offsets measured from the node's left edge.
    pub from_x: Option<f64>,
    pub to_x: Option<f64>,
    pub weight: f64,
    /// Layout-only edges shape layering but are never routed.
    pub draw: bool,
    pub label: bool,
    /// Width of the label, when it sits beside the edge just above its reader.
    pub label_w: f64,
}

pub struct LOut {
    /// Top-left corner per node.
    pub pos: Vec<(f64, f64)>,
    pub routes: Vec<Vec<(f64, f64)>>,
    pub labels: Vec<Option<(f64, f64)>>,
    pub width: f64,
    pub height: f64,
}

const NODE_GAP: f64 = 30.;
const DUMMY_GAP: f64 = 14.;
const LAYER_GAP: f64 = 30.;
const TRACK_GAP: f64 = 8.;
const LABEL_H: f64 = 13.;
const COMPONENT_GAP: f64 = 56.;

/// Buffers for laying out diagrams, kept from one to the next: most take a
/// few microseconds, and allocating every buffer afresh cost as much again.
#[derive(Default)]
pub struct Scratch {
    // Components.
    uf: Vec<usize>,
    comp_of: Vec<usize>,
    comp_at: Vec<usize>,
    members: Lists,
    wires: Lists,
    order: Vec<usize>,
    local: Vec<usize>,
    // Orientation and layering.
    adj: Lists,
    reversed: Vec<bool>,
    state: Vec<u8>,
    roots: Vec<usize>,
    dfs: Vec<(usize, usize)>,
    succ: Lists,
    pred: Lists,
    indeg: Vec<usize>,
    topo: Vec<usize>,
    ready: Vec<usize>,
    layer: Vec<usize>,
    // Vertices and the segments between them.
    vw: Vec<f64>,
    vh: Vec<f64>,
    vlayer: Vec<usize>,
    vorder: Vec<f64>,
    vrank: Vec<Option<u32>>,
    segs: Vec<Seg>,
    groups: Vec<(usize, u64, bool)>,
    grouped: Vec<(usize, usize)>,
    by_group: Lists,
    links: Vec<usize>,
    chains: Vec<std::ops::Range<usize>>,
    vsucc: Lists,
    vpred: Lists,
    // Ordering and placement.
    layers: Lists,
    best: Vec<usize>,
    pos: Vec<f64>,
    tree: Vec<usize>,
    keyed: Vec<(usize, f64)>,
    offs: Vec<f64>,
    springs: Vec<(f64, f64, f64)>,
    x: Vec<f64>,
    blocks: Vec<(f64, f64, usize)>,
    // Ports and channels.
    end_label: Vec<Option<f64>>,
    out_x: Vec<f64>,
    in_x: Vec<f64>,
    ports: Vec<(f64, usize, usize)>,
    port_slot: Vec<usize>,
    port_x: Vec<f64>,
    port_label: Vec<bool>,
    perm: Vec<usize>,
    ins: Vec<usize>,
    free: Vec<usize>,
    steps: Vec<f64>,
    band_h: Vec<f64>,
    channel_h: Vec<f64>,
    channel_label: Vec<f64>,
    buses: Vec<(f64, f64, f64, usize)>,
    bus_at: Vec<usize>,
    runs: Vec<(f64, f64)>,
    band_y: Vec<f64>,
}

/// Lists for keys 0..n in one buffer: key k's list is
/// `items[start[k]..start[k + 1]]`, in the order its items came.
#[derive(Default)]
struct Lists {
    start: Vec<usize>,
    items: Vec<usize>,
}

impl Lists {
    /// Refills the lists with (key, item) pairs, counting each key's items first.
    fn fill(&mut self, n: usize, pairs: impl DoubleEndedIterator<Item = (usize, usize)> + Clone) {
        refill(&mut self.start, n + 1, 0);
        for (k, _) in pairs.clone() {
            self.start[k] += 1;
        }
        for k in 1..=n {
            self.start[k] += self.start[k - 1];
        }
        refill(&mut self.items, self.start[n], 0);
        // Placed from the back, so each key's end moves down to its start.
        for (k, v) in pairs.rev() {
            self.start[k] -= 1;
            self.items[self.start[k]] = v;
        }
    }
}

impl std::ops::Index<usize> for Lists {
    type Output = [usize];
    #[inline]
    fn index(&self, k: usize) -> &[usize] {
        &self.items[self.start[k]..self.start[k + 1]]
    }
}

impl std::ops::IndexMut<usize> for Lists {
    #[inline]
    fn index_mut(&mut self, k: usize) -> &mut [usize] {
        &mut self.items[self.start[k]..self.start[k + 1]]
    }
}

/// Empties a kept buffer and fills it with `n` copies of `v`.
fn refill<T: Clone>(buf: &mut Vec<T>, n: usize, v: T) {
    buf.clear();
    buf.resize(n, v);
}

/// `arrive`: labels sit where edges arrive (shared trunks) rather than where they leave.
pub fn layout(nodes: &[LNode], edges: &[LEdge], arrive: bool, scratch: &mut Scratch) -> LOut {
    let n = nodes.len();
    let mut out = LOut { pos: vec![(0., 0.); n], routes: vec![vec![]; edges.len()], labels: vec![None; edges.len()], width: 0., height: 0. };
    if n == 0 {
        return out;
    }
    // Weakly connected components; everything touching a pinned port is one piece
    // so inputs and outputs span the whole diagram.
    let uf = &mut scratch.uf;
    uf.clear();
    uf.extend(0..n);
    fn find(uf: &mut [usize], mut x: usize) -> usize {
        while uf[x] != x {
            uf[x] = uf[uf[x]];
            x = uf[x];
        }
        x
    }
    let union = |uf: &mut [usize], a: usize, b: usize| {
        let (ra, rb) = (find(uf, a), find(uf, b));
        if ra != rb {
            uf[ra] = rb;
        }
    };
    for e in edges {
        union(uf, e.from, e.to);
    }
    let pinned: Vec<usize> = (0..n).filter(|&i| nodes[i].pin != Pin::Free).collect();
    for w in pinned.windows(2) {
        union(uf, w[0], w[1]);
    }
    // Each component's nodes and edges, numbered in order of their first node.
    let (comp_of, comp_at) = (&mut scratch.comp_of, &mut scratch.comp_at);
    refill(comp_at, n, usize::MAX);
    refill(comp_of, n, 0);
    let mut count = 0;
    for i in 0..n {
        let r = find(uf, i);
        if comp_at[r] == usize::MAX {
            comp_at[r] = count;
            count += 1;
        }
        comp_of[i] = comp_at[r];
    }
    scratch.members.fill(count, (0..n).map(|i| (comp_of[i], i)));
    scratch.wires.fill(count, edges.iter().enumerate().map(|(ei, e)| (comp_of[e.from], ei)));
    let members = &scratch.members;
    let key = |c: usize| {
        let c = &members[c];
        let pinned = c.iter().any(|&i| nodes[i].pin != Pin::Free);
        let pack = c.iter().map(|&i| nodes[i].pack).min().unwrap_or(0);
        let order = c.iter().map(|&i| nodes[i].order).fold(f64::INFINITY, f64::min);
        (!pinned, pack, order)
    };
    let order = &mut scratch.order;
    order.clear();
    order.extend(0..count);
    order.sort_by(|&a, &b| key(a).partial_cmp(&key(b)).unwrap_or(std::cmp::Ordering::Equal));

    let mut pieces = Vec::with_capacity(count);
    let (mut sub_nodes, mut sub_edges) = (vec![], vec![]);
    refill(&mut scratch.local, n, 0);
    for k in 0..count {
        let c = scratch.order[k];
        // A diagram in one piece is laid out as it is.
        let piece = if count == 1 {
            layered(nodes, edges, arrive, scratch)
        } else {
            let (comp, local) = (&scratch.members[c], &mut scratch.local);
            for (i, &g) in comp.iter().enumerate() {
                local[g] = i;
            }
            sub_nodes.clear();
            sub_nodes.extend(comp.iter().map(|&g| nodes[g].clone()));
            sub_edges.clear();
            sub_edges.extend(scratch.wires[c].iter().map(|&ei| LEdge { from: local[edges[ei].from], to: local[edges[ei].to], ..edges[ei].clone() }));
            layered(&sub_nodes, &sub_edges, arrive, scratch)
        };
        pieces.push((c, piece));
    }

    // Shelf-pack pieces in reading order, aiming for a landscape aspect.
    let area: f64 = pieces.iter().map(|(_, p)| (p.width + COMPONENT_GAP) * (p.height + COMPONENT_GAP)).sum();
    let widest = pieces.iter().map(|(_, p)| p.width).fold(0., f64::max);
    let row_width = widest.max((area * 1.7).sqrt());
    let (mut x, mut y, mut row_h) = (0f64, 0f64, 0f64);
    for (c, mut piece) in pieces {
        if x > 0. && x + piece.width > row_width {
            x = 0.;
            y += row_h + COMPONENT_GAP;
            row_h = 0.;
        }
        for (i, &g) in scratch.members[c].iter().enumerate() {
            out.pos[g] = (piece.pos[i].0 + x, piece.pos[i].1 + y);
        }
        for (i, &ei) in scratch.wires[c].iter().enumerate() {
            let mut route = std::mem::take(&mut piece.routes[i]);
            for p in &mut route {
                *p = (p.0 + x, p.1 + y);
            }
            out.routes[ei] = route;
            out.labels[ei] = piece.labels[i].map(|(lx, ly)| (lx + x, ly + y));
        }
        out.width = out.width.max(x + piece.width);
        out.height = out.height.max(y + piece.height);
        row_h = row_h.max(piece.height);
        x += piece.width + COMPONENT_GAP;
    }
    out
}

/// Puts each horizontal run `(start, end)`, in order, one track above the
/// highest earlier run it overlaps. Overlaps are found in a max segment tree
/// over the gaps between the runs' ends: O(n log n), where comparing every
/// pair was quadratic in channels of thousands of runs.
fn stack(runs: &[(f64, f64)]) -> Vec<usize> {
    let mut ends: Vec<f64> = runs.iter().flat_map(|&(a, b)| [a, b]).collect();
    ends.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    ends.dedup();
    let rank = |v: f64| ends.partition_point(|&e| e < v);
    let span = (0, ends.len());
    let mut tree = Tree { all: vec![0; 4 * ends.len()], any: vec![0; 4 * ends.len()] };
    runs.iter()
        .map(|&(a, b)| {
            let gaps = (rank(a), rank(b));
            let t = tree.highest(1, span, gaps);
            tree.raise(1, span, gaps, t + 1);
            t
        })
        .collect()
}

/// A segment tree whose values are only ever raised. Per node: the highest
/// value raised over all of its span, and over any part of it.
struct Tree {
    all: Vec<usize>,
    any: Vec<usize>,
}

impl Tree {
    fn highest(&self, k: usize, (lo, hi): (usize, usize), (l, r): (usize, usize)) -> usize {
        if r <= lo || hi <= l {
            0
        } else if l <= lo && hi <= r {
            self.any[k]
        } else {
            let mid = (lo + hi) / 2;
            self.all[k].max(self.highest(2 * k, (lo, mid), (l, r))).max(self.highest(2 * k + 1, (mid, hi), (l, r)))
        }
    }

    fn raise(&mut self, k: usize, (lo, hi): (usize, usize), (l, r): (usize, usize), v: usize) {
        if r <= lo || hi <= l {
            return;
        }
        self.any[k] = self.any[k].max(v);
        if l <= lo && hi <= r {
            self.all[k] = self.all[k].max(v);
        } else {
            let mid = (lo + hi) / 2;
            self.raise(2 * k, (lo, mid), (l, r), v);
            self.raise(2 * k + 1, (mid, hi), (l, r), v);
        }
    }
}

struct Seg {
    a: usize,
    b: usize,
    weight: f64,
    from_x: Option<f64>,
    to_x: Option<f64>,
    /// The port it leaves by, one per (vertex, group).
    port: usize,
}

/// Layered layout of one connected piece.
fn layered(nodes: &[LNode], edges: &[LEdge], arrive: bool, scratch: &mut Scratch) -> LOut {
    let n = nodes.len();
    let mut out = LOut { pos: vec![(0., 0.); n], routes: vec![vec![]; edges.len()], labels: vec![None; edges.len()], width: 0., height: 0. };
    if n == 1 {
        out.width = nodes[0].w;
        out.height = nodes[0].h;
        return out;
    }
    let Scratch {
        adj, reversed, state, roots, dfs, succ, pred, indeg, topo, ready, layer,
        vw, vh, vlayer, vorder, vrank, segs, groups, grouped, by_group, links, chains, vsucc, vpred,
        layers, best, pos, tree, keyed, offs, springs, x, blocks,
        end_label, out_x, in_x, ports, port_slot, port_x, port_label, perm, ins, free, steps,
        band_h, channel_h, channel_label, buses, bus_at, runs, band_y, ..
    } = scratch;

    // 1. Orient edges: break cycles with a DFS in source order; pins win.
    adj.fill(n, edges.iter().enumerate().filter(|(_, e)| e.from != e.to).map(|(ei, e)| (e.from, ei)));
    refill(reversed, edges.len(), false);
    refill(state, n, 0u8);
    roots.clear();
    roots.extend(0..n);
    roots.sort_by(|&a, &b| {
        let pa = nodes[a].pin != Pin::Top;
        let pb = nodes[b].pin != Pin::Top;
        (pa, nodes[a].order).partial_cmp(&(pb, nodes[b].order)).unwrap_or(std::cmp::Ordering::Equal)
    });
    for &r in roots.iter() {
        if state[r] != 0 {
            continue;
        }
        dfs.clear();
        dfs.push((r, 0usize));
        state[r] = 1;
        while let Some(&mut (v, ref mut i)) = dfs.last_mut() {
            if *i < adj[v].len() {
                let ei = adj[v][*i];
                *i += 1;
                let w = edges[ei].to;
                match state[w] {
                    0 => {
                        state[w] = 1;
                        dfs.push((w, 0));
                    }
                    1 => reversed[ei] = true,
                    _ => {}
                }
            } else {
                state[v] = 2;
                dfs.pop();
            }
        }
    }
    for (ei, e) in edges.iter().enumerate() {
        if nodes[e.to].pin == Pin::Top || nodes[e.from].pin == Pin::Bottom {
            reversed[ei] = true;
        }
        if nodes[e.from].pin == Pin::Top || nodes[e.to].pin == Pin::Bottom {
            reversed[ei] = false;
        }
    }
    let reversed = &*reversed;
    let oriented = |ei: usize| -> (usize, usize) {
        let e = &edges[ei];
        if reversed[ei] {
            (e.to, e.from)
        } else {
            (e.from, e.to)
        }
    };

    // 2. Layering: longest path, then pull sources and fan-out nodes toward consumers.
    let arcs = (0..edges.len()).map(oriented).filter(|&(a, b)| a != b);
    succ.fill(n, arcs.clone());
    pred.fill(n, arcs.map(|(a, b)| (b, a)));
    indeg.clear();
    indeg.extend((0..n).map(|v| pred[v].len()));
    topo.clear();
    ready.clear();
    ready.extend((0..n).filter(|&v| indeg[v] == 0));
    ready.sort_by(|a, b| nodes[*b].order.partial_cmp(&nodes[*a].order).unwrap_or(std::cmp::Ordering::Equal));
    while let Some(v) = ready.pop() {
        topo.push(v);
        for &w in &succ[v] {
            indeg[w] -= 1;
            if indeg[w] == 0 {
                ready.push(w);
            }
        }
    }
    // Any leftovers (should not happen after orientation) keep source order.
    if topo.len() < n {
        let seen: Set<usize> = topo.iter().copied().collect();
        topo.extend((0..n).filter(|v| !seen.contains(v)));
    }
    let has_top = nodes.iter().any(|v| v.pin == Pin::Top);
    refill(layer, n, 0);
    for &v in topo.iter() {
        let base = if has_top && nodes[v].pin == Pin::Free { 1 } else { 0 };
        layer[v] = pred[v].iter().map(|&p| layer[p] + 1).max().unwrap_or(0).max(base);
        if nodes[v].pin == Pin::Top {
            layer[v] = 0;
        }
    }
    for &v in topo.iter().rev() {
        if nodes[v].pin != Pin::Free || succ[v].is_empty() {
            continue;
        }
        if succ[v].len() >= pred[v].len() {
            let below = succ[v].iter().map(|&s| layer[s]).min().unwrap_or(layer[v] + 1);
            if below > layer[v] + 1 {
                layer[v] = below - 1;
            }
        }
    }
    let last = (0..n).filter(|&v| nodes[v].pin != Pin::Bottom).map(|v| layer[v]).max().unwrap_or(0);
    for v in 0..n {
        if nodes[v].pin == Pin::Bottom {
            layer[v] = last + 1;
        }
    }
    // Sinks hug their producers rather than sinking to the bottom row.
    let nlayers = layer.iter().copied().max().unwrap_or(0) + 1;

    // 3. Vertices = real nodes + trunk dummies. Edges sharing (source, group) share a trunk.
    vw.clear();
    vw.extend(nodes.iter().map(|v| v.w));
    vh.clear();
    vh.extend(nodes.iter().map(|v| v.h));
    vlayer.clone_from(layer);
    vorder.clear();
    vorder.extend(nodes.iter().map(|v| v.order));
    vrank.clear();
    vrank.extend(nodes.iter().map(|v| v.rank));
    segs.clear();
    let mut seg_index: Map<(usize, usize, u64), usize> = Map::with_capacity_and_hasher(edges.len(), Default::default());
    // Edges by (source, group, reversed), in order of first appearance.
    groups.clear();
    grouped.clear();
    let mut group_at: Map<(usize, u64, bool), usize> = Map::with_capacity_and_hasher(edges.len(), Default::default());
    for (ei, e) in edges.iter().enumerate() {
        if !e.draw || e.from == e.to {
            continue;
        }
        let (a, _) = oriented(ei);
        let k = (a, e.group, reversed[ei]);
        let g = *group_at.entry(k).or_insert_with(|| {
            groups.push(k);
            groups.len() - 1
        });
        grouped.push((g, ei));
    }
    by_group.fill(groups.len(), grouped.iter().copied());
    // Ports by (vertex, group), numbered as segments first leave by them.
    let mut port_at: Map<(usize, u64), usize> = Map::with_capacity_and_hasher(edges.len(), Default::default());
    let mut add_seg = |segs: &mut Vec<Seg>, a: usize, b: usize, group: u64, weight: f64, from_x: Option<f64>, to_x: Option<f64>| -> usize {
        let key = (a, b, if to_x.is_some() { to_x.unwrap().to_bits() } else { u64::MAX });
        if let Some(&s) = seg_index.get(&key) {
            segs[s].weight = segs[s].weight.max(weight);
            return s;
        }
        let next = port_at.len();
        let port = *port_at.entry((a, group)).or_insert(next);
        segs.push(Seg { a, b, weight, from_x, to_x, port });
        seg_index.insert(key, segs.len() - 1);
        segs.len() - 1
    };
    // Each edge's segments, source to reader, are links[chains[edge]].
    links.clear();
    refill(chains, edges.len(), 0..0);
    for (g, &(a, group, rev)) in groups.iter().enumerate() {
        let eids = &by_group[g];
        let deepest = eids.iter().map(|&ei| layer[oriented(ei).1]).max().unwrap_or(layer[a]);
        // Dummy for each intermediate layer between source and the deepest consumer.
        let trunk = vw.len();
        for l in layer[a] + 1..deepest {
            vw.push(0.);
            vh.push(0.);
            vlayer.push(l);
            vorder.push(nodes[a].order + 0.5);
            vrank.push(None);
        }
        for &ei in eids {
            let e = &edges[ei];
            let (_, b) = oriented(ei);
            let (from_x, to_x) = if rev { (None, None) } else { (e.from_x, e.to_x) };
            let mut prev = a;
            let start = links.len();
            let span = layer[b].saturating_sub(layer[a]);
            for step in 0..span.saturating_sub(1) {
                let d = trunk + step;
                let s = add_seg(segs, prev, d, group, e.weight, if prev == a { from_x } else { None }, None);
                links.push(s);
                prev = d;
            }
            let s = add_seg(segs, prev, b, group, e.weight, if prev == a { from_x } else { None }, to_x);
            links.push(s);
            chains[ei] = start..links.len();
        }
    }
    let chain_of = |ei: usize| &links[chains[ei].clone()];
    let nv = vw.len();
    vsucc.fill(nv, segs.iter().enumerate().map(|(si, s)| (s.a, si)));
    vpred.fill(nv, segs.iter().enumerate().map(|(si, s)| (s.b, si)));

    // 4. Ordering by barycenter sweeps, keeping the best crossing count.
    layers.fill(nlayers, (0..nv).map(|v| (vlayer[v], v)));
    for l in 0..nlayers {
        layers[l].sort_by(|&a, &b| (vrank[a].unwrap_or(u32::MAX), vorder[a]).partial_cmp(&(vrank[b].unwrap_or(u32::MAX), vorder[b])).unwrap_or(std::cmp::Ordering::Equal));
    }
    refill(pos, nv, 0.);
    let refresh = |layers: &Lists, pos: &mut Vec<f64>| {
        for l in 0..nlayers {
            for (i, &v) in layers[l].iter().enumerate() {
                pos[v] = i as f64;
            }
        }
    };
    refresh(layers, pos);
    // Crossings between each layer and the next: pairs of segments whose ends
    // are in opposite orders. Left to right, a Fenwick tree over positions
    // below counts the earlier segments that end right of each one; segments
    // from one vertex never cross. A segment ends in the next layer or, between
    // pinned ports, in its own.
    let mut crossings = |layers: &Lists, pos: &Vec<f64>| -> usize {
        let mut total = 0;
        for l in 0..nlayers - 1 {
            refill(tree, layers[l].len().max(layers[l + 1].len()) + 1, 0);
            let mut seen = 0;
            for &v in &layers[l] {
                for &si in &vsucc[v] {
                    let mut k = pos[segs[si].b] as usize + 1;
                    let mut le = 0;
                    while k > 0 {
                        le += tree[k];
                        k &= k - 1;
                    }
                    total += seen - le;
                }
                for &si in &vsucc[v] {
                    let mut k = pos[segs[si].b] as usize + 1;
                    while k < tree.len() {
                        tree[k] += 1;
                        k += k & k.wrapping_neg();
                    }
                    seen += 1;
                }
            }
        }
        total
    };
    best.clone_from(&layers.items);
    let mut best_cross = crossings(layers, pos);
    // Sweeps in a row that moved nothing: after one each way, none would.
    let mut still = 0;
    for iter in 0..16 {
        if best_cross == 0 || still == 2 {
            break;
        }
        let down = iter % 2 == 0;
        let mut moved = false;
        for k in 1..nlayers {
            let l = if down { k } else { nlayers - 1 - k };
            keyed.clear();
            keyed.extend(layers[l].iter().map(|&v| {
                let nbrs = if down { &vpred[v] } else { &vsucc[v] };
                let at = |si: usize| pos[if down { segs[si].a } else { segs[si].b }];
                let bary = if nbrs.is_empty() { pos[v] } else { nbrs.iter().map(|&si| at(si)).sum::<f64>() / nbrs.len() as f64 };
                (v, bary)
            }));
            keyed.sort_by(|a, b| {
                (vrank[a.0].unwrap_or(u32::MAX), a.1, vorder[a.0])
                    .partial_cmp(&(vrank[b.0].unwrap_or(u32::MAX), b.1, vorder[b.0]))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let row = &mut layers[l];
            for (i, &(v, _)) in keyed.iter().enumerate() {
                moved |= row[i] != v;
                row[i] = v;
                pos[v] = i as f64;
            }
        }
        if !moved {
            still += 1;
            continue;
        }
        still = 0;
        let c = crossings(layers, pos);
        if c < best_cross {
            best_cross = c;
            best.clone_from(&layers.items);
        }
    }
    std::mem::swap(&mut layers.items, best);
    let layers = &*layers;
    refresh(layers, pos);

    // 5. Horizontal placement: alternating sweeps, each solved exactly per layer
    // as weighted isotonic regression under minimum-separation constraints.
    let is_dummy = |v: usize| v >= n;
    let gap = |a: usize, b: usize| -> f64 {
        let g = match (is_dummy(a), is_dummy(b)) {
            (false, false) => NODE_GAP,
            (true, true) => DUMMY_GAP * 0.7,
            _ => DUMMY_GAP,
        };
        (vw[a] + vw[b]) / 2. + g
    };
    // Each vertex's offset from the first in its layer at minimum spacing,
    // which is also where placement starts.
    refill(offs, nv, 0.);
    for l in 0..nlayers {
        let l = &layers[l];
        let mut cursor = 0.;
        for (i, &v) in l.iter().enumerate() {
            if i > 0 {
                cursor += gap(l[i - 1], v);
            }
            offs[v] = cursor;
        }
    }
    x.clone_from(offs);
    // Each segment as a spring that straightens it: its stiffness, and where
    // it attaches above and below, from the centers of its vertices.
    springs.clear();
    springs.extend(segs.iter().map(|s| {
        let k = match (is_dummy(s.a), is_dummy(s.b)) {
            (true, true) => 8.,
            (false, false) => 1.,
            _ => 2.,
        };
        let off = |at: Option<f64>, v: usize| at.map(|p| p - vw[v] / 2.).unwrap_or(0.);
        (s.weight * k, off(s.from_x, s.a), off(s.to_x, s.b))
    }));
    // Where a vertex would sit to straighten its segments: (target center, weight).
    let desired = |v: usize, x: &[f64], mode: usize| -> (f64, f64) {
        let mut sum = 0.;
        let mut wsum = 0.;
        if mode != 1 {
            for &si in &vpred[v] {
                let (k, up, down) = springs[si];
                sum += k * (x[segs[si].a] + up - down);
                wsum += k;
            }
        }
        if mode != 0 {
            for &si in &vsucc[v] {
                let (k, up, down) = springs[si];
                sum += k * (x[segs[si].b] + down - up);
                wsum += k;
            }
        }
        if wsum == 0. {
            (x[v], 0.05)
        } else {
            (sum / wsum, wsum)
        }
    };
    // As in ordering, once a sweep each way moves nothing, only the last one can.
    let mut still = 0;
    for iter in 0..12 {
        let mode = if iter == 11 { 2 } else { iter % 2 };
        if mode != 2 && still == 2 {
            continue;
        }
        let mut moved = false;
        for k in 0..nlayers {
            let l = &layers[if mode == 1 { nlayers - 1 - k } else { k }];
            // The pooled blocks (weight, weighted sum, count).
            blocks.clear();
            for &v in l.iter() {
                let (d, w) = desired(v, x, mode);
                let w = w.max(1e-3);
                blocks.push((w, w * (d - offs[v]), 1));
                while blocks.len() >= 2 {
                    let (w2, s2, c2) = blocks[blocks.len() - 1];
                    let (w1, s1, c1) = blocks[blocks.len() - 2];
                    if s1 / w1 > s2 / w2 {
                        blocks.pop();
                        let last = blocks.len() - 1;
                        blocks[last] = (w1 + w2, s1 + s2, c1 + c2);
                    } else {
                        break;
                    }
                }
            }
            // Every vertex is read before any moves, so a sweep sees the layer as it was.
            let mut i = 0;
            for &(w, s, c) in blocks.iter() {
                for _ in 0..c {
                    let nx = s / w + offs[l[i]];
                    moved |= nx.to_bits() != x[l[i]].to_bits();
                    x[l[i]] = nx;
                    i += 1;
                }
            }
        }
        still = if moved { 0 } else { still + 1 };
    }
    let min_left = (0..nv).map(|v| x[v] - vw[v] / 2.).fold(f64::INFINITY, f64::min);
    for v in x.iter_mut() {
        *v -= min_left;
    }
    let x = &*x;

    // Label widths of the segments that end labeled edges (labeled where they arrive).
    refill(end_label, segs.len(), None);
    for (ei, e) in edges.iter().enumerate() {
        if arrive && e.draw && e.label && !reversed[ei] {
            if let Some(&si) = chain_of(ei).last() {
                let w = end_label[si].get_or_insert(0.);
                *w = w.max(e.label_w);
            }
        }
    }

    // 6. Ports: one outgoing port per (node, group), one incoming port per segment.
    let left = |v: usize| x[v] - vw[v] / 2.;
    refill(out_x, segs.len(), 0.);
    refill(in_x, segs.len(), 0.);
    let spread = |count: usize, width: f64| -> f64 {
        if count <= 1 {
            0.
        } else {
            ((width * 0.72) / (count - 1) as f64).min(26.)
        }
    };
    let target_x = |si: usize| -> f64 {
        let s = &segs[si];
        s.to_x.map(|tx| left(s.b) + tx).unwrap_or(x[s.b])
    };
    // Where each port is among its vertex's ports.
    refill(port_slot, port_at.len(), usize::MAX);
    for v in 0..nv {
        if is_dummy(v) || nodes[v].converge {
            for &si in &vsucc[v] {
                out_x[si] = x[v];
            }
            continue;
        }
        // Per port: the sum of its targets, its segments and its first segment.
        ports.clear();
        for &si in &vsucc[v] {
            let p = segs[si].port;
            if port_slot[p] == usize::MAX {
                port_slot[p] = ports.len();
                ports.push((0., 0, si));
            }
            let k = port_slot[p];
            ports[k].0 += target_x(si);
            ports[k].1 += 1;
        }
        // Ports left to right by where they lead.
        perm.clear();
        perm.extend(0..ports.len());
        perm.sort_by(|&a, &b| (ports[a].0 / ports[a].1 as f64).partial_cmp(&(ports[b].0 / ports[b].1 as f64)).unwrap_or(std::cmp::Ordering::Equal));
        let step = spread(ports.len(), vw[v]);
        let start = x[v] - step * (ports.len() as f64 - 1.) / 2.;
        let single = ports.len() == 1;
        refill(port_x, ports.len(), 0.);
        for (i, &k) in perm.iter().enumerate() {
            let (_, count, first) = ports[k];
            let fixed = segs[first].from_x.map(|fx| left(v) + fx);
            let mut px = fixed.unwrap_or(start + step * i as f64);
            if fixed.is_none() && single && count == 1 {
                let t = target_x(first);
                if t > left(v) + 8. && t < left(v) + vw[v] - 8. {
                    px = t;
                }
            }
            port_x[k] = px;
        }
        for &si in &vsucc[v] {
            out_x[si] = port_x[port_slot[segs[si].port]];
        }
    }
    for v in 0..nv {
        if is_dummy(v) || nodes[v].converge {
            for &si in &vpred[v] {
                in_x[si] = x[v];
            }
            continue;
        }
        ins.clear();
        ins.extend_from_slice(&vpred[v]);
        ins.sort_by(|&a, &b| out_x[a].partial_cmp(&out_x[b]).unwrap_or(std::cmp::Ordering::Equal));
        free.clear();
        free.extend(ins.iter().copied().filter(|&si| segs[si].to_x.is_none()));
        let base = spread(free.len(), vw[v]);
        // With labels where edges arrive, arrows enter far enough apart for
        // the label beside each to fit, as far as the box is wide.
        steps.clear();
        steps.extend(free.windows(2).map(|w| base.max(end_label[w[0]].map_or(0., |lw| lw + 12.))));
        let total: f64 = steps.iter().sum();
        let fit = if total > vw[v] * 0.92 && total > 0. { (vw[v] * 0.92 / total).min(1.) } else { 1. };
        let mut px = x[v] - total * fit / 2.;
        for (i, &si) in free.iter().enumerate() {
            if i > 0 {
                px += steps[i - 1] * fit;
            }
            let at = if arrive { px } else { x[v] - base * (free.len() as f64 - 1.) / 2. + base * i as f64 };
            in_x[si] = if free.len() == 1 && out_x[si] > left(v) + 8. && out_x[si] < left(v) + vw[v] - 8. { out_x[si] } else { at };
        }
        for &si in ins.iter() {
            if let Some(tx) = segs[si].to_x {
                in_x[si] = left(v) + tx;
            }
        }
    }

    // 7. Channels: horizontal runs between layers get tracks; buses from one
    // port share a track so a value visibly splits.
    band_h.clear();
    band_h.extend((0..nlayers).map(|l| layers[l].iter().map(|&v| vh[v]).fold(0., f64::max)));
    // Labels sit under the port an edge leaves, or above the reader it reaches.
    refill(port_label, port_at.len(), false);
    for (ei, e) in edges.iter().enumerate() {
        if !arrive && e.draw && e.label && !reversed[ei] {
            if let Some(&p) = port_at.get(&(oriented(ei).0, e.group)) {
                port_label[p] = true;
            }
        }
    }
    refill(channel_h, nlayers, LAYER_GAP);
    refill(channel_label, nlayers, 0.);
    // Buses, each (min, max, start x, track), and the bus that the
    // horizontal runs leaving each port share.
    buses.clear();
    refill(bus_at, port_at.len(), usize::MAX);
    for l in 0..nlayers - 1 {
        let first = buses.len();
        for &v in &layers[l] {
            for &si in &vsucc[v] {
                let (x1, x2) = (out_x[si], in_x[si]);
                if (x1 - x2).abs() < 0.5 {
                    continue;
                }
                let p = segs[si].port;
                if bus_at[p] == usize::MAX {
                    bus_at[p] = buses.len();
                    buses.push((x1.min(x2), x1.max(x2), x1, 0));
                }
                let b = &mut buses[bus_at[p]];
                b.0 = b.0.min(x1.min(x2));
                b.1 = b.1.max(x1.max(x2));
            }
        }
        // Rightward buses whose start is further right sit higher; leftward mirror.
        let dir = |b: &(f64, f64, f64, usize)| if (b.1 - b.2).abs() < 0.5 { 1 } else if (b.0 - b.2).abs() < 0.5 { 0 } else { 2 };
        let key = |b: &(f64, f64, f64, usize)| (dir(b), if dir(b) == 0 { -b.2 } else { b.2 });
        perm.clear();
        perm.extend(first..buses.len());
        perm.sort_by(|&a, &b| key(&buses[a]).partial_cmp(&key(&buses[b])).unwrap_or(std::cmp::Ordering::Equal));
        // Buses closer than 6 px count as overlapping.
        runs.clear();
        runs.extend(perm.iter().map(|&b| (buses[b].0, buses[b].1 + 6.)));
        let tracks = stack(runs);
        for (&b, &t) in perm.iter().zip(&tracks) {
            buses[b].3 = t;
        }
        let used = tracks.iter().map(|t| t + 1).max().unwrap_or(0);
        // Room for labels: above the tracks for those leaving this layer, below
        // them for those arriving at the next.
        let has_label = layers[l].iter().any(|&v| vsucc[v].iter().any(|&si| end_label[si].is_some() || port_label[segs[si].port]));
        channel_label[l] = if has_label { LABEL_H } else { 0. };
        channel_h[l] = (LAYER_GAP + channel_label[l] * 0.6).max(channel_label[l] + (used as f64 + 1.) * TRACK_GAP + 6.);
    }
    refill(band_y, nlayers, 0.);
    for l in 1..nlayers {
        band_y[l] = band_y[l - 1] + band_h[l - 1] + channel_h[l - 1];
    }
    // Lanes hang from the top of their band; everything else is centered.
    let top = |v: usize| band_y[vlayer[v]] + if vrank[v].is_some() { 0. } else { (band_h[vlayer[v]] - vh[v]) / 2. };
    for v in 0..n {
        out.pos[v] = (x[v] - vw[v] / 2., top(v));
    }
    out.width = (0..nv).map(|v| x[v] + vw[v] / 2.).fold(0., f64::max);
    out.height = band_y[nlayers - 1] + band_h[nlayers - 1];

    // 8. Routes.
    // Segments in no bus run within the last layer, on its first track.
    let track = |si: usize| buses.get(bus_at[segs[si].port]).map_or(0, |b| b.3);
    let track_y = |si: usize| {
        let l = vlayer[segs[si].a];
        band_y[l] + band_h[l] + if arrive { 0. } else { channel_label[l] } + (track(si) as f64 + 1.) * TRACK_GAP + 2.
    };
    for ei in 0..edges.len() {
        let chain = chain_of(ei);
        if chain.is_empty() {
            continue;
        }
        let first = &segs[chain[0]];
        let mut route = Vec::with_capacity(1 + 3 * chain.len());
        turn(&mut route, (out_x[chain[0]], top(first.a) + vh[first.a]));
        for &si in chain {
            let s = &segs[si];
            let (x1, x2) = (out_x[si], in_x[si]);
            if (x1 - x2).abs() >= 0.5 {
                let ty = track_y(si);
                turn(&mut route, (x1, ty));
                turn(&mut route, (x2, ty));
            }
            if !is_dummy(s.b) {
                turn(&mut route, (x2, top(s.b)));
            }
        }
        if reversed[ei] {
            route.reverse();
        }
        out.routes[ei] = route;
    }
    // Each branch says what it carries just above the block it reaches; or
    // one label per port, on the first edge of its group, just under the
    // block it leaves.
    if arrive {
        for (ei, e) in edges.iter().enumerate() {
            let chain = chain_of(ei);
            if e.draw && e.label && !reversed[ei] && !chain.is_empty() {
                let si = chain[chain.len() - 1];
                out.labels[ei] = Some((in_x[si] + 4., top(segs[si].b) - 4.));
            }
        }
    } else {
        for (g, &(a, _, rev)) in groups.iter().enumerate() {
            if let Some(&ei) = by_group[g].iter().find(|&&ei| !rev && edges[ei].label) {
                let si = chain_of(ei)[0];
                out.labels[ei] = Some((out_x[si] + 4., top(a) + vh[a] + 10.5));
            }
        }
    }
    out
}

/// Adds a point to a route, dropping repeats and the middle of straight runs.
fn turn(route: &mut Vec<(f64, f64)>, p: (f64, f64)) {
    if let Some(&last) = route.last() {
        if (last.0 - p.0).abs() < 0.01 && (last.1 - p.1).abs() < 0.01 {
            return;
        }
    }
    if route.len() >= 2 {
        let a = route[route.len() - 2];
        let b = route[route.len() - 1];
        let collinear = ((a.0 - b.0).abs() < 0.01 && (b.0 - p.0).abs() < 0.01) || ((a.1 - b.1).abs() < 0.01 && (b.1 - p.1).abs() < 0.01);
        if collinear {
            route.pop();
        }
    }
    route.push(p);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn node(w: f64, order: f64) -> LNode {
        LNode { w, h: 40., pin: Pin::Free, order, rank: None, converge: false, pack: 0 }
    }
    fn edge(a: usize, b: usize, group: u64) -> LEdge {
        LEdge { from: a, to: b, group, from_x: None, to_x: None, weight: 1., draw: true, label: true, label_w: 0. }
    }
    #[test]
    fn chain_is_vertical_and_non_overlapping() {
        let nodes = vec![node(100., 0.), node(120., 1.), node(80., 2.)];
        let out = layout(&nodes, &[edge(0, 1, 1), edge(1, 2, 2)], false, &mut Scratch::default());
        assert!(out.pos[0].1 < out.pos[1].1 && out.pos[1].1 < out.pos[2].1);
        let cx: Vec<f64> = (0..3).map(|i| out.pos[i].0 + nodes[i].w / 2.).collect();
        assert!((cx[0] - cx[1]).abs() < 1. && (cx[1] - cx[2]).abs() < 1.);
        assert_eq!(out.routes[0].len(), 2, "straight edge has two points");
    }
    #[test]
    fn residual_shares_a_trunk() {
        // x -> f -> add, x -> add (skip connection)
        let mut nodes = vec![node(60., 0.), node(100., 1.), node(22., 2.)];
        nodes[0].pin = Pin::Top;
        let out = layout(&nodes, &[edge(0, 1, 7), edge(0, 2, 7), edge(1, 2, 3)], false, &mut Scratch::default());
        assert!(out.pos[2].1 > out.pos[1].1);
        // Both edges from x leave from the same point.
        assert_eq!(out.routes[0][0], out.routes[1][0]);
    }
    #[test]
    fn siblings_do_not_overlap_and_cycles_terminate() {
        let nodes: Vec<LNode> = (0..6).map(|i| node(90., i as f64)).collect();
        let edges = vec![edge(0, 1, 0), edge(1, 2, 0), edge(2, 0, 0), edge(0, 3, 0), edge(0, 4, 0), edge(0, 5, 0)];
        let out = layout(&nodes, &edges, false, &mut Scratch::default());
        for i in 0..6 {
            for j in i + 1..6 {
                let (a, b) = (out.pos[i], out.pos[j]);
                let overlap = a.0 < b.0 + 90. && b.0 < a.0 + 90. && a.1 < b.1 + 40. && b.1 < a.1 + 40.;
                assert!(!overlap, "{i} and {j} overlap");
            }
        }
        assert!(out.routes.iter().all(|r| r.len() >= 2));
    }
    #[test]
    fn tracks_stack_as_if_every_pair_were_compared() {
        let mut seed = 7u64;
        let mut next = |max: f64| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 33) as f64 / (1u64 << 31) as f64 * max).round()
        };
        for n in [0, 1, 2, 5, 40, 300] {
            // Whole numbers, so runs often share ends or just touch.
            let runs: Vec<(f64, f64)> = (0..n)
                .map(|_| {
                    let a = next(400.);
                    (a, a + next(80.) + 6.)
                })
                .collect();
            let mut pairwise: Vec<usize> = vec![];
            for i in 0..n {
                let t = (0..i).filter(|&j| runs[j].0 < runs[i].1 && runs[i].0 < runs[j].1).map(|j| pairwise[j] + 1).max().unwrap_or(0);
                pairwise.push(t);
            }
            assert_eq!(stack(&runs), pairwise, "{n} runs");
        }
    }
    #[test]
    fn disconnected_pieces_are_packed() {
        let nodes: Vec<LNode> = (0..9).map(|i| node(100., i as f64)).collect();
        let out = layout(&nodes, &[], false, &mut Scratch::default());
        assert!(out.width < 9. * 130., "packed into rows, not one line");
    }
}
