//! Layered (Sugiyama-style) layout for one diagram, top to bottom.
//!
//! Pipeline: components → cycle breaking → layering → shared-trunk dummies →
//! barycentric ordering → x placement by weighted isotonic regression → port
//! assignment → orthogonal routing with per-channel track allocation.
//!
//! Edges that carry the same value from the same source share a trunk, so one
//! line splits into several consumers (like residual connections in an
//! architecture figure) instead of fanning out as parallel wires.

use std::collections::HashMap;

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
    /// All edges meet at the centre (operator glyphs).
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

/// `arrive`: labels sit where edges arrive (shared trunks) rather than where they leave.
pub fn layout(nodes: &[LNode], edges: &[LEdge], arrive: bool) -> LOut {
    let n = nodes.len();
    let mut out = LOut { pos: vec![(0., 0.); n], routes: vec![vec![]; edges.len()], labels: vec![None; edges.len()], width: 0., height: 0. };
    if n == 0 {
        return out;
    }
    // Weakly connected components; everything touching a pinned port is one piece
    // so inputs and outputs span the whole diagram.
    let mut uf: Vec<usize> = (0..n).collect();
    fn find(uf: &mut Vec<usize>, mut x: usize) -> usize {
        while uf[x] != x {
            uf[x] = uf[uf[x]];
            x = uf[x];
        }
        x
    }
    let union = |uf: &mut Vec<usize>, a: usize, b: usize| {
        let (ra, rb) = (find(uf, a), find(uf, b));
        if ra != rb {
            uf[ra] = rb;
        }
    };
    for e in edges {
        union(&mut uf, e.from, e.to);
    }
    let pinned: Vec<usize> = (0..n).filter(|&i| nodes[i].pin != Pin::Free).collect();
    for w in pinned.windows(2) {
        union(&mut uf, w[0], w[1]);
    }
    // Each component's nodes and edges, bucketed in one pass over each.
    let mut comp_of = vec![usize::MAX; n];
    let mut comps: Vec<(Vec<usize>, Vec<usize>)> = vec![];
    for i in 0..n {
        let r = find(&mut uf, i);
        if comp_of[r] == usize::MAX {
            comp_of[r] = comps.len();
            comps.push((vec![], vec![]));
        }
        comps[comp_of[r]].0.push(i);
    }
    for (ei, e) in edges.iter().enumerate() {
        let r = find(&mut uf, e.from);
        comps[comp_of[r]].1.push(ei);
    }
    let key = |c: &Vec<usize>| {
        let pinned = c.iter().any(|&i| nodes[i].pin != Pin::Free);
        let pack = c.iter().map(|&i| nodes[i].pack).min().unwrap_or(0);
        let order = c.iter().map(|&i| nodes[i].order).fold(f64::INFINITY, f64::min);
        (!pinned, pack, order)
    };
    comps.sort_by(|a, b| key(&a.0).partial_cmp(&key(&b.0)).unwrap_or(std::cmp::Ordering::Equal));

    let mut pieces = vec![];
    let mut local = vec![0; n];
    for (comp, edge_ids) in comps {
        for (i, &g) in comp.iter().enumerate() {
            local[g] = i;
        }
        let sub_nodes: Vec<LNode> = comp.iter().map(|&g| nodes[g].clone()).collect();
        let sub_edges: Vec<LEdge> = edge_ids.iter().map(|&ei| LEdge { from: local[edges[ei].from], to: local[edges[ei].to], ..edges[ei].clone() }).collect();
        let piece = layered(&sub_nodes, &sub_edges, arrive);
        pieces.push((comp, edge_ids, piece));
    }

    // Shelf-pack pieces in reading order, aiming for a landscape aspect.
    let area: f64 = pieces.iter().map(|(_, _, p)| (p.width + COMPONENT_GAP) * (p.height + COMPONENT_GAP)).sum();
    let widest = pieces.iter().map(|(_, _, p)| p.width).fold(0., f64::max);
    let row_width = widest.max((area * 1.7).sqrt());
    let (mut x, mut y, mut row_h) = (0f64, 0f64, 0f64);
    for (comp, edge_ids, piece) in pieces {
        if x > 0. && x + piece.width > row_width {
            x = 0.;
            y += row_h + COMPONENT_GAP;
            row_h = 0.;
        }
        for (i, &g) in comp.iter().enumerate() {
            out.pos[g] = (piece.pos[i].0 + x, piece.pos[i].1 + y);
        }
        for (i, &ei) in edge_ids.iter().enumerate() {
            out.routes[ei] = piece.routes[i].iter().map(|&(px, py)| (px + x, py + y)).collect();
            out.labels[ei] = piece.labels[i].map(|(lx, ly)| (lx + x, ly + y));
        }
        out.width = out.width.max(x + piece.width);
        out.height = out.height.max(y + piece.height);
        row_h = row_h.max(piece.height);
        x += piece.width + COMPONENT_GAP;
    }
    out
}

/// Counts pairs (i, j) with a_i < a_j and b_i > b_j, i.e. edge crossings
/// between two layers, in O(E log E) with a Fenwick tree over b ranks.
/// `bs` and `tree` are scratch space, reused across calls.
fn inversions(pairs: &mut [(f64, f64)], bs: &mut Vec<f64>, tree: &mut Vec<usize>) -> usize {
    if pairs.len() < 2 {
        return 0;
    }
    pairs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    bs.clear();
    bs.extend(pairs.iter().map(|p| p.1));
    bs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    bs.dedup();
    let rank = |b: f64| bs.partition_point(|&v| v < b) + 1;
    tree.clear();
    tree.resize(bs.len() + 1, 0);
    let mut total = 0;
    let mut seen = 0;
    let mut i = 0;
    while i < pairs.len() {
        // Pairs sharing a source position never cross each other.
        let mut j = i;
        while j < pairs.len() && pairs[j].0 == pairs[i].0 {
            let r = rank(pairs[j].1);
            let mut k = r;
            let mut le = 0;
            while k > 0 {
                le += tree[k];
                k &= k - 1;
            }
            total += seen - le;
            j += 1;
        }
        for p in &pairs[i..j] {
            let mut k = rank(p.1);
            while k < tree.len() {
                tree[k] += 1;
                k += k & k.wrapping_neg();
            }
            seen += 1;
        }
        i = j;
    }
    total
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
    port: u64,
}

/// Layered layout of one connected piece.
fn layered(nodes: &[LNode], edges: &[LEdge], arrive: bool) -> LOut {
    let n = nodes.len();
    let mut out = LOut { pos: vec![(0., 0.); n], routes: vec![vec![]; edges.len()], labels: vec![None; edges.len()], width: 0., height: 0. };
    if n == 1 {
        out.width = nodes[0].w;
        out.height = nodes[0].h;
        return out;
    }

    // 1. Orient edges: break cycles with a DFS in source order; pins win.
    let mut adj: Vec<Vec<usize>> = vec![vec![]; n];
    for (ei, e) in edges.iter().enumerate() {
        if e.from != e.to {
            adj[e.from].push(ei);
        }
    }
    let mut reversed = vec![false; edges.len()];
    let mut state = vec![0u8; n];
    let mut roots: Vec<usize> = (0..n).collect();
    roots.sort_by(|&a, &b| {
        let pa = nodes[a].pin != Pin::Top;
        let pb = nodes[b].pin != Pin::Top;
        (pa, nodes[a].order).partial_cmp(&(pb, nodes[b].order)).unwrap_or(std::cmp::Ordering::Equal)
    });
    for &r in &roots {
        if state[r] != 0 {
            continue;
        }
        let mut stack = vec![(r, 0usize)];
        state[r] = 1;
        while let Some(&mut (v, ref mut i)) = stack.last_mut() {
            if *i < adj[v].len() {
                let ei = adj[v][*i];
                *i += 1;
                let w = edges[ei].to;
                match state[w] {
                    0 => {
                        state[w] = 1;
                        stack.push((w, 0));
                    }
                    1 => reversed[ei] = true,
                    _ => {}
                }
            } else {
                state[v] = 2;
                stack.pop();
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
    let oriented = |ei: usize| -> (usize, usize) {
        let e = &edges[ei];
        if reversed[ei] {
            (e.to, e.from)
        } else {
            (e.from, e.to)
        }
    };

    // 2. Layering: longest path, then pull sources and fan-out nodes toward consumers.
    let mut succ: Vec<Vec<usize>> = vec![vec![]; n];
    let mut pred: Vec<Vec<usize>> = vec![vec![]; n];
    for ei in 0..edges.len() {
        let (a, b) = oriented(ei);
        if a != b {
            succ[a].push(b);
            pred[b].push(a);
        }
    }
    let mut indeg: Vec<usize> = pred.iter().map(|p| p.len()).collect();
    let mut topo = Vec::with_capacity(n);
    let mut ready: Vec<usize> = (0..n).filter(|&v| indeg[v] == 0).collect();
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
        let seen: std::collections::HashSet<usize> = topo.iter().copied().collect();
        topo.extend((0..n).filter(|v| !seen.contains(v)));
    }
    let has_top = nodes.iter().any(|v| v.pin == Pin::Top);
    let mut layer = vec![0usize; n];
    for &v in &topo {
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
    let mut vw: Vec<f64> = nodes.iter().map(|v| v.w).collect();
    let mut vh: Vec<f64> = nodes.iter().map(|v| v.h).collect();
    let mut vlayer = layer.clone();
    let mut vorder: Vec<f64> = nodes.iter().map(|v| v.order).collect();
    let mut vrank: Vec<Option<u32>> = nodes.iter().map(|v| v.rank).collect();
    let mut segs: Vec<Seg> = vec![];
    let mut seg_index: HashMap<(usize, usize, u64), usize> = HashMap::new();
    let mut chains: Vec<Vec<usize>> = vec![vec![]; edges.len()];
    // Edges by (source, group, reversed), in order of first appearance.
    let mut groups: Vec<((usize, u64, bool), Vec<usize>)> = vec![];
    let mut group_at: HashMap<(usize, u64, bool), usize> = HashMap::with_capacity(edges.len());
    for (ei, e) in edges.iter().enumerate() {
        if !e.draw || e.from == e.to {
            continue;
        }
        let (a, _) = oriented(ei);
        let k = (a, e.group, reversed[ei]);
        let g = *group_at.entry(k).or_insert_with(|| {
            groups.push((k, vec![]));
            groups.len() - 1
        });
        groups[g].1.push(ei);
    }
    let mut add_seg = |segs: &mut Vec<Seg>, a: usize, b: usize, port: u64, weight: f64, from_x: Option<f64>, to_x: Option<f64>| -> usize {
        let key = (a, b, if to_x.is_some() { to_x.unwrap().to_bits() } else { u64::MAX });
        if let Some(&s) = seg_index.get(&key) {
            segs[s].weight = segs[s].weight.max(weight);
            return s;
        }
        segs.push(Seg { a, b, weight, from_x, to_x, port });
        seg_index.insert(key, segs.len() - 1);
        segs.len() - 1
    };
    for (k, eids) in &groups {
        let (a, group, rev) = *k;
        let deepest = eids.iter().map(|&ei| layer[oriented(ei).1]).max().unwrap_or(layer[a]);
        let mut trunk = vec![];
        // Dummy for each intermediate layer between source and the deepest consumer.
        for l in layer[a] + 1..deepest {
            let _ = l;
            vw.push(0.);
            vh.push(0.);
            vlayer.push(layer[a] + 1 + trunk.len());
            vorder.push(nodes[a].order + 0.5);
            vrank.push(None);
            trunk.push(vw.len() - 1);
        }
        for &ei in eids {
            let e = &edges[ei];
            let (_, b) = oriented(ei);
            let (from_x, to_x) = if rev { (None, None) } else { (e.from_x, e.to_x) };
            let mut prev = a;
            let mut chain = vec![];
            let span = layer[b].saturating_sub(layer[a]);
            for step in 0..span.saturating_sub(1) {
                let d = trunk[step];
                let s = add_seg(&mut segs, prev, d, group, e.weight, if prev == a { from_x } else { None }, None);
                chain.push(s);
                prev = d;
            }
            let s = add_seg(&mut segs, prev, b, group, e.weight, if prev == a { from_x } else { None }, to_x);
            chain.push(s);
            chains[ei] = chain;
        }
    }
    let nv = vw.len();
    let mut vsucc: Vec<Vec<usize>> = vec![vec![]; nv];
    let mut vpred: Vec<Vec<usize>> = vec![vec![]; nv];
    for (si, s) in segs.iter().enumerate() {
        vsucc[s.a].push(si);
        vpred[s.b].push(si);
    }

    // 4. Ordering by barycentre sweeps, keeping the best crossing count.
    let mut layers: Vec<Vec<usize>> = vec![vec![]; nlayers];
    for v in 0..nv {
        layers[vlayer[v]].push(v);
    }
    for l in &mut layers {
        l.sort_by(|&a, &b| (vrank[a].unwrap_or(u32::MAX), vorder[a]).partial_cmp(&(vrank[b].unwrap_or(u32::MAX), vorder[b])).unwrap_or(std::cmp::Ordering::Equal));
    }
    let mut pos = vec![0f64; nv];
    let refresh = |layers: &Vec<Vec<usize>>, pos: &mut Vec<f64>| {
        for l in layers {
            for (i, &v) in l.iter().enumerate() {
                pos[v] = i as f64;
            }
        }
    };
    refresh(&layers, &mut pos);
    // Scratch for the sweeps, allocated once per diagram.
    let (mut pairs, mut bs, mut tree) = (vec![], vec![], vec![]);
    let mut crossings = |layers: &Vec<Vec<usize>>, pos: &Vec<f64>| -> usize {
        let mut total = 0;
        for l in 0..layers.len().saturating_sub(1) {
            pairs.clear();
            for &v in &layers[l] {
                for &si in &vsucc[v] {
                    pairs.push((pos[v], pos[segs[si].b]));
                }
            }
            total += inversions(&mut pairs, &mut bs, &mut tree);
        }
        total
    };
    let mut keyed: Vec<(usize, f64)> = vec![];
    let mut best = layers.clone();
    let mut best_cross = crossings(&layers, &pos);
    for iter in 0..16 {
        let down = iter % 2 == 0;
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
            for (i, &(v, _)) in keyed.iter().enumerate() {
                layers[l][i] = v;
                pos[v] = i as f64;
            }
        }
        let c = crossings(&layers, &pos);
        if c < best_cross {
            best_cross = c;
            best.clone_from(&layers);
        }
        if best_cross == 0 {
            break;
        }
    }
    layers = best;
    refresh(&layers, &mut pos);

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
    let seg_w = |s: &Seg| -> f64 {
        s.weight
            * match (is_dummy(s.a), is_dummy(s.b)) {
                (true, true) => 8.,
                (false, false) => 1.,
                _ => 2.,
            }
    };
    // Offset of an attachment point from the node centre, when fixed.
    let out_off = |s: &Seg| s.from_x.map(|fx| fx - vw[s.a] / 2.).unwrap_or(0.);
    let in_off = |s: &Seg| s.to_x.map(|tx| tx - vw[s.b] / 2.).unwrap_or(0.);
    let mut x = vec![0f64; nv];
    for l in &layers {
        let mut cursor = 0.;
        for (i, &v) in l.iter().enumerate() {
            if i > 0 {
                cursor += gap(l[i - 1], v);
            }
            x[v] = cursor;
        }
    }
    // Where a vertex would sit to straighten its segments: (target centre, weight).
    let desired = |v: usize, x: &[f64], mode: usize| -> (f64, f64) {
        let mut sum = 0.;
        let mut wsum = 0.;
        if mode != 1 {
            for &si in &vpred[v] {
                let s = &segs[si];
                let w = seg_w(s);
                sum += w * (x[s.a] + out_off(s) - in_off(s));
                wsum += w;
            }
        }
        if mode != 0 {
            for &si in &vsucc[v] {
                let s = &segs[si];
                let w = seg_w(s);
                sum += w * (x[s.b] + in_off(s) - out_off(s));
                wsum += w;
            }
        }
        if wsum == 0. {
            (x[v], 0.05)
        } else {
            (sum / wsum, wsum)
        }
    };
    // Per layer, each vertex's offset from the first at minimum spacing and
    // the pooled blocks (weight, weighted sum, count); reused by every sweep.
    let mut offs: Vec<f64> = vec![];
    let mut blocks: Vec<(f64, f64, usize)> = vec![];
    for iter in 0..12 {
        let mode = if iter == 11 { 2 } else { iter % 2 };
        for k in 0..nlayers {
            let l = &layers[if mode == 1 { nlayers - 1 - k } else { k }];
            offs.clear();
            blocks.clear();
            for (i, &v) in l.iter().enumerate() {
                offs.push(if i == 0 { 0. } else { offs[i - 1] + gap(l[i - 1], v) });
                let (d, w) = desired(v, &x, mode);
                let w = w.max(1e-3);
                blocks.push((w, w * (d - offs[i]), 1));
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
            for &(w, s, c) in &blocks {
                for _ in 0..c {
                    x[l[i]] = s / w + offs[i];
                    i += 1;
                }
            }
        }
    }
    let min_left = (0..nv).map(|v| x[v] - vw[v] / 2.).fold(f64::INFINITY, f64::min);
    for v in x.iter_mut() {
        *v -= min_left;
    }

    // Label widths of the segments that end labelled edges (labelled where they arrive).
    let mut end_label: HashMap<usize, f64> = HashMap::new();
    for (ei, e) in edges.iter().enumerate() {
        if arrive && e.draw && e.label && !reversed[ei] {
            if let Some(&si) = chains[ei].last() {
                let w = end_label.entry(si).or_insert(0.);
                *w = w.max(e.label_w);
            }
        }
    }

    // 6. Ports: one outgoing port per (node, group), one incoming port per segment.
    let left = |v: usize, x: &Vec<f64>| x[v] - vw[v] / 2.;
    let mut out_x = vec![0f64; segs.len()];
    let mut in_x = vec![0f64; segs.len()];
    let spread = |count: usize, width: f64| -> f64 {
        if count <= 1 {
            0.
        } else {
            ((width * 0.72) / (count - 1) as f64).min(26.)
        }
    };
    // Where each (vertex, group) is among its vertex's ports.
    let mut port_at: HashMap<(usize, u64), usize> = HashMap::with_capacity(segs.len());
    for v in 0..nv {
        if is_dummy(v) || nodes[v].converge {
            for &si in &vsucc[v] {
                out_x[si] = x[v];
            }
            continue;
        }
        let mut ports: Vec<(u64, Vec<usize>)> = vec![];
        for &si in &vsucc[v] {
            let p = segs[si].port;
            let k = *port_at.entry((v, p)).or_insert_with(|| {
                ports.push((p, vec![]));
                ports.len() - 1
            });
            ports[k].1.push(si);
        }
        let target_x = |si: usize| -> f64 {
            let s = &segs[si];
            s.to_x.map(|tx| left(s.b, &x) + tx).unwrap_or(x[s.b])
        };
        ports.sort_by(|a, b| {
            let ma = a.1.iter().map(|&s| target_x(s)).sum::<f64>() / a.1.len() as f64;
            let mb = b.1.iter().map(|&s| target_x(s)).sum::<f64>() / b.1.len() as f64;
            ma.partial_cmp(&mb).unwrap_or(std::cmp::Ordering::Equal)
        });
        let step = spread(ports.len(), vw[v]);
        let start = x[v] - step * (ports.len() as f64 - 1.) / 2.;
        let single = ports.len() == 1;
        for (i, (_, sids)) in ports.iter().enumerate() {
            let fixed = segs[sids[0]].from_x.map(|fx| left(v, &x) + fx);
            let mut px = fixed.unwrap_or(start + step * i as f64);
            if fixed.is_none() && single && sids.len() == 1 {
                let t = target_x(sids[0]);
                if t > left(v, &x) + 8. && t < left(v, &x) + vw[v] - 8. {
                    px = t;
                }
            }
            for &si in sids {
                out_x[si] = px;
            }
        }
    }
    for v in 0..nv {
        let mut ins: Vec<usize> = vpred[v].clone();
        if is_dummy(v) || nodes[v].converge {
            for si in ins {
                in_x[si] = x[v];
            }
            continue;
        }
        ins.sort_by(|&a, &b| out_x[a].partial_cmp(&out_x[b]).unwrap_or(std::cmp::Ordering::Equal));
        let free: Vec<usize> = ins.iter().copied().filter(|&si| segs[si].to_x.is_none()).collect();
        let base = spread(free.len(), vw[v]);
        // With labels where edges arrive, arrows enter far enough apart for
        // the label beside each to fit, as far as the box is wide.
        let steps: Vec<f64> = free.windows(2).map(|w| base.max(end_label.get(&w[0]).map_or(0., |lw| lw + 12.))).collect();
        let total: f64 = steps.iter().sum();
        let fit = if total > vw[v] * 0.92 && total > 0. { (vw[v] * 0.92 / total).min(1.) } else { 1. };
        let mut px = x[v] - total * fit / 2.;
        for (i, &si) in free.iter().enumerate() {
            if i > 0 {
                px += steps[i - 1] * fit;
            }
            let at = if arrive { px } else { x[v] - base * (free.len() as f64 - 1.) / 2. + base * i as f64 };
            in_x[si] = if free.len() == 1 && out_x[si] > left(v, &x) + 8. && out_x[si] < left(v, &x) + vw[v] - 8. { out_x[si] } else { at };
        }
        for &si in &ins {
            if let Some(tx) = segs[si].to_x {
                in_x[si] = left(v, &x) + tx;
            }
        }
    }

    // 7. Channels: horizontal runs between layers get tracks; buses from one
    // port share a track so a value visibly splits.
    let band_h: Vec<f64> = (0..nlayers).map(|l| layers[l].iter().map(|&v| vh[v]).fold(0., f64::max)).collect();
    // Labels sit under the port an edge leaves, or above the reader it reaches.
    let labeled_port: std::collections::HashSet<(usize, u64)> = edges
        .iter()
        .enumerate()
        .filter(|(ei, e)| !arrive && e.draw && e.label && !reversed[*ei])
        .map(|(ei, e)| (oriented(ei).0, e.group))
        .collect();
    let labeled_end: std::collections::HashSet<usize> = edges
        .iter()
        .enumerate()
        .filter(|(ei, e)| arrive && e.draw && e.label && !reversed[*ei] && !chains[*ei].is_empty())
        .map(|(ei, _)| *chains[ei].last().unwrap())
        .collect();
    let mut track_of = vec![0usize; segs.len()];
    let mut channel_h = vec![LAYER_GAP; nlayers];
    let mut channel_label = vec![0f64; nlayers];
    // Where each bus is in its layer's list, by its key.
    let mut bus_at: HashMap<(usize, u64), usize> = HashMap::with_capacity(segs.len());
    for l in 0..nlayers.saturating_sub(1) {
        // bus key → (min, max, start x, segments)
        let mut buses: Vec<((usize, u64), f64, f64, f64, Vec<usize>)> = vec![];
        for &v in &layers[l] {
            for &si in &vsucc[v] {
                let (x1, x2) = (out_x[si], in_x[si]);
                if (x1 - x2).abs() < 0.5 {
                    continue;
                }
                let key = (v, if is_dummy(v) { 0 } else { segs[si].port });
                let i = *bus_at.entry(key).or_insert_with(|| {
                    buses.push((key, x1.min(x2), x1.max(x2), x1, vec![]));
                    buses.len() - 1
                });
                let b = &mut buses[i];
                b.1 = b.1.min(x1.min(x2));
                b.2 = b.2.max(x1.max(x2));
                b.4.push(si);
            }
        }
        // Rightward buses whose start is further right sit higher; leftward mirror.
        buses.sort_by(|a, b| {
            let dir = |bus: &(_, f64, f64, f64, _)| if (bus.2 - bus.3).abs() < 0.5 { 1 } else if (bus.1 - bus.3).abs() < 0.5 { 0 } else { 2 };
            let ka = (dir(a), if dir(a) == 0 { -a.3 } else { a.3 });
            let kb = (dir(b), if dir(b) == 0 { -b.3 } else { b.3 });
            ka.partial_cmp(&kb).unwrap_or(std::cmp::Ordering::Equal)
        });
        // Buses closer than 6 px count as overlapping.
        let tracks = stack(&buses.iter().map(|b| (b.1, b.2 + 6.)).collect::<Vec<_>>());
        for (b, &t) in buses.iter().zip(&tracks) {
            for &si in &b.4 {
                track_of[si] = t;
            }
        }
        let used = tracks.iter().map(|t| t + 1).max().unwrap_or(0);
        // Room for labels: above the tracks for those leaving this layer, below
        // them for those arriving at the next.
        let has_label = layers[l].iter().any(|&v| vsucc[v].iter().any(|&si| labeled_end.contains(&si) || (!is_dummy(v) && labeled_port.contains(&(v, segs[si].port)))));
        channel_label[l] = if has_label { LABEL_H } else { 0. };
        channel_h[l] = (LAYER_GAP + channel_label[l] * 0.6).max(channel_label[l] + (used as f64 + 1.) * TRACK_GAP + 6.);
    }
    let mut band_y = vec![0f64; nlayers];
    for l in 1..nlayers {
        band_y[l] = band_y[l - 1] + band_h[l - 1] + channel_h[l - 1];
    }
    // Lanes hang from the top of their band; everything else is centred.
    let top = |v: usize| band_y[vlayer[v]] + if vrank[v].is_some() { 0. } else { (band_h[vlayer[v]] - vh[v]) / 2. };
    for v in 0..n {
        out.pos[v] = (x[v] - vw[v] / 2., top(v));
    }
    out.width = (0..nv).map(|v| x[v] + vw[v] / 2.).fold(0., f64::max);
    out.height = band_y[nlayers - 1] + band_h[nlayers - 1];

    // 8. Routes.
    let track_y = |si: usize| {
        let l = vlayer[segs[si].a];
        band_y[l] + band_h[l] + if arrive { 0. } else { channel_label[l] } + (track_of[si] as f64 + 1.) * TRACK_GAP + 2.
    };
    for (ei, chain) in chains.iter().enumerate() {
        if chain.is_empty() {
            continue;
        }
        let first = &segs[chain[0]];
        let mut pts: Vec<(f64, f64)> = vec![(out_x[chain[0]], top(first.a) + vh[first.a])];
        for &si in chain {
            let s = &segs[si];
            let (x1, x2) = (out_x[si], in_x[si]);
            if (x1 - x2).abs() >= 0.5 {
                let ty = track_y(si);
                pts.push((x1, ty));
                pts.push((x2, ty));
            }
            if !is_dummy(s.b) {
                pts.push((x2, top(s.b)));
            }
        }
        // Drop collinear points.
        let mut clean: Vec<(f64, f64)> = vec![];
        for p in pts {
            if let Some(&last) = clean.last() {
                if (last.0 - p.0).abs() < 0.01 && (last.1 - p.1).abs() < 0.01 {
                    continue;
                }
            }
            if clean.len() >= 2 {
                let a = clean[clean.len() - 2];
                let b = clean[clean.len() - 1];
                let collinear = ((a.0 - b.0).abs() < 0.01 && (b.0 - p.0).abs() < 0.01) || ((a.1 - b.1).abs() < 0.01 && (b.1 - p.1).abs() < 0.01);
                if collinear {
                    clean.pop();
                }
            }
            clean.push(p);
        }
        if reversed[ei] {
            clean.reverse();
        }
        out.routes[ei] = clean;
    }
    // Each branch says what it carries just above the block it reaches; or
    // one label per port, just under the block it leaves.
    let mut labeled: std::collections::HashSet<(usize, u64)> = std::collections::HashSet::new();
    for (ei, e) in edges.iter().enumerate() {
        if !e.draw || !e.label || reversed[ei] || chains[ei].is_empty() {
            continue;
        }
        if arrive {
            let si = *chains[ei].last().unwrap();
            out.labels[ei] = Some((in_x[si] + 4., top(segs[si].b) - 4.));
        } else {
            let (a, _) = oriented(ei);
            if labeled.insert((a, e.group)) {
                let si = chains[ei][0];
                out.labels[ei] = Some((out_x[si] + 4., top(a) + vh[a] + 10.5));
            }
        }
    }
    out
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
        let out = layout(&nodes, &[edge(0, 1, 1), edge(1, 2, 2)], false);
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
        let out = layout(&nodes, &[edge(0, 1, 7), edge(0, 2, 7), edge(1, 2, 3)], false);
        assert!(out.pos[2].1 > out.pos[1].1);
        // Both edges from x leave from the same point.
        assert_eq!(out.routes[0][0], out.routes[1][0]);
    }
    #[test]
    fn siblings_do_not_overlap_and_cycles_terminate() {
        let nodes: Vec<LNode> = (0..6).map(|i| node(90., i as f64)).collect();
        let edges = vec![edge(0, 1, 0), edge(1, 2, 0), edge(2, 0, 0), edge(0, 3, 0), edge(0, 4, 0), edge(0, 5, 0)];
        let out = layout(&nodes, &edges, false);
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
        let out = layout(&nodes, &[], false);
        assert!(out.width < 9. * 130., "packed into rows, not one line");
    }
}
