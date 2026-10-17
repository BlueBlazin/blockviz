//! Following one value through a definition (src/canvas/trace.ts).

use crate::render::{EdgeId, Trace};
use blockviz_core::ir::{EdgeKind, Kind, Node, Shape};
use std::collections::HashSet;

fn region(n: &Node) -> bool {
    n.shape == Shape::Region
}

/// The path of one value: every edge carrying it, from where it is made to
/// everything that reads it. A value keeps its name through steps that
/// re-bind it (`q = q.view(…).transpose(…)`), so the trace is every edge of
/// that name connected to the start through the steps they share, entering
/// and leaving branches and loops through their ports of that name. It stays
/// within the definition: calls are other code. An unnamed edge is traced
/// only to the other readers of the same result.
pub fn trace_value<'a>(node: &dyn Fn(u32) -> &'a Node, owner: u32, start: usize) -> Trace {
    // The definition's diagram and every region inside it.
    let mut top = node(owner);
    while region(top) {
        let Some(p) = top.parent else { break };
        top = node(p);
    }
    let mut scope = HashSet::new();
    fn collect<'a>(node: &dyn Fn(u32) -> &'a Node, n: &Node, scope: &mut HashSet<u32>) {
        scope.insert(n.id);
        for &c in &n.children {
            if region(node(c)) {
                collect(node, node(c), scope);
            }
        }
    }
    collect(node, top, &mut scope);

    let first = &node(owner).edges[start];
    let mut edges: HashSet<EdgeId> = HashSet::from([(owner, start)]);
    if first.kind != EdgeKind::Data {
        return Trace { scope, edges };
    }
    let label = first.label.clone();
    let carries = |k: EdgeKind, l: &str| k == EdgeKind::Data && l == label;
    if label.is_empty() {
        for (i, e) in node(owner).edges.iter().enumerate() {
            if carries(e.kind, &e.label) && e.from == first.from {
                edges.insert((owner, i));
            }
        }
        return Trace { scope, edges };
    }
    // (diagram, step) pairs the value reaches; edges of its name at a step carry it on.
    let mut seen = HashSet::new();
    let mut queue = vec![(owner, first.from), (owner, first.to)];
    while let Some((o, id)) = queue.pop() {
        if !seen.insert((o, id)) {
            continue;
        }
        let on = node(o);
        for (i, e) in on.edges.iter().enumerate() {
            if !carries(e.kind, &e.label) || (e.from != id && e.to != id) {
                continue;
            }
            edges.insert((o, i));
            queue.push((o, e.from));
            queue.push((o, e.to));
        }
        let n = node(id);
        // Into a branch or loop: its port of this name.
        if region(n) && n.id != o {
            for &c in &n.children {
                let k = node(c);
                if matches!(k.kind, Kind::Input | Kind::Output) && k.title == label {
                    queue.push((n.id, c));
                }
            }
        }
        // Out of one, from its port: the region's own edges outside.
        if matches!(n.kind, Kind::Input | Kind::Output) && region(on) {
            if let Some(p) = on.parent.filter(|p| scope.contains(p)) {
                queue.push((p, on.id));
            }
        }
    }
    Trace { scope, edges }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blockviz_core::ir::Builder;

    fn labels(b: &Builder, t: &Trace) -> Vec<String> {
        let mut out: Vec<(u32, usize, String)> = t.edges.iter().map(|&(o, i)| {
            let e = &b.get(o).edges[i];
            (o, i, format!("{}-{}", e.from, e.to))
        }).collect();
        out.sort();
        out.into_iter().map(|x| x.2).collect()
    }

    #[test]
    fn follows_a_name_through_re_binding_steps_and_keeps_other_names_out() {
        // x → c_attn → split ─q→ view(q) ─q→ att ─att→ softmax; ─k→ view(k) ─k→ att; ─q→ sdpa.
        let mut b = Builder::default();
        let f = b.add(None, Kind::Function, Shape::Box, "f");
        let ids: Vec<u32> = (0..9).map(|i| b.add(Some(f), Kind::Op, Shape::Box, format!("s{i}"))).collect();
        let edges = [(0, 1, ""), (1, 2, ""), (2, 3, "q"), (2, 4, "k"), (3, 5, "q"), (4, 5, "k"), (2, 6, "q"), (5, 7, "att"), (7, 8, "")];
        for (a, z, l) in edges {
            b.connect(ids[a], ids[z], EdgeKind::Data, l);
        }
        let node = |id: u32| b.get(id);
        let name = |i: usize| format!("{}-{}", ids[edges[i].0], ids[edges[i].1]);
        let q = labels(&b, &trace_value(&node, f, 4));
        assert_eq!(q, [name(2), name(4), name(6)]);
        assert_eq!(labels(&b, &trace_value(&node, f, 2)), q);
        assert_eq!(labels(&b, &trace_value(&node, f, 3)), [name(3), name(5)]);
        assert_eq!(labels(&b, &trace_value(&node, f, 0)), [name(0)]);
    }

    #[test]
    fn enters_and_leaves_branches_through_their_ports_of_the_same_name() {
        // f: 1 ─q→ branch 2 ─y→ 3; in the branch: port "q" ─q→ lane; lane ─y→ port "y";
        // in the lane: port "q" ─q→ step ─y→ port "y".
        let mut b = Builder::default();
        let f = b.add(None, Kind::Function, Shape::Box, "f");
        let one = b.add(Some(f), Kind::Op, Shape::Box, "one");
        let branch = b.add(Some(f), Kind::Branch, Shape::Region, "");
        let three = b.add(Some(f), Kind::Op, Shape::Box, "three");
        let bq = b.add(Some(branch), Kind::Input, Shape::Port, "q");
        let lane = b.add(Some(branch), Kind::Lane, Shape::Region, "if");
        let lq = b.add(Some(lane), Kind::Input, Shape::Port, "q");
        let step = b.add(Some(lane), Kind::Op, Shape::Box, "step");
        let ly = b.add(Some(lane), Kind::Output, Shape::Port, "y");
        let by = b.add(Some(branch), Kind::Output, Shape::Port, "y");
        b.connect(one, branch, EdgeKind::Data, "q");
        b.connect(branch, three, EdgeKind::Data, "y");
        b.connect(one, three, EdgeKind::Data, "k");
        b.connect(bq, lane, EdgeKind::Data, "q");
        b.connect(lane, by, EdgeKind::Data, "y");
        b.connect(lq, step, EdgeKind::Data, "q");
        b.connect(step, ly, EdgeKind::Data, "y");
        let node = |id: u32| b.get(id);
        let q = trace_value(&node, f, 0);
        assert_eq!(labels(&b, &q), [format!("{one}-{branch}"), format!("{bq}-{lane}"), format!("{lq}-{step}")]);
        let mut scope: Vec<u32> = q.scope.iter().copied().collect();
        scope.sort();
        assert_eq!(scope, [f, branch, lane]);
        // From inside out: y made in the lane leaves the branch and reaches three.
        let y = trace_value(&node, lane, 1);
        assert_eq!(labels(&b, &y), [format!("{branch}-{three}"), format!("{lane}-{by}"), format!("{step}-{ly}")]);
    }
}
