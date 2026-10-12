import { describe, expect, it } from 'vitest';
import type { Edge, SceneNode } from '../core/types';
import { distanceToPolyline, traceValue as trace } from './trace';

const e = (from: number, to: number, label = '', kind: Edge['kind'] = 'data'): Edge => ({ from, to, kind, label, points: [] });

// x → c_attn → split ─q→ view(q) ─q→ att ─att→ softmax
//                    ─k→ view(k) ─k→ att
//                    ─q→ sdpa (the other branch)
const edges = [
  e(0, 1), e(1, 2), e(2, 3, 'q'), e(2, 4, 'k'), e(3, 5, 'q'), e(4, 5, 'k'), e(2, 6, 'q'), e(5, 7, 'att'), e(7, 8),
];
const owner = { id: 99, kind: 'function', shape: 'box', edges } as unknown as SceneNode;
const flat = (id: number) => (id === 99 ? owner : ({ id, kind: 'op', shape: 'box' } as SceneNode));
const traceValue = (o: SceneNode, start: Edge) => trace(flat, o.id, start).edges;
const labels = (s: ReadonlySet<Edge>, all = edges) => all.filter(x => s.has(x)).map(x => `${x.from}-${x.to}`);

describe('tracing a value', () => {
  it('follows a name from where it is made through the steps that re-bind it', () => {
    expect(labels(traceValue(owner, edges[4]))).toEqual(['2-3', '3-5', '2-6']);
    expect(labels(traceValue(owner, edges[2]))).toEqual(['2-3', '3-5', '2-6']);
  });

  it('keeps other names out', () => {
    expect(labels(traceValue(owner, edges[3]))).toEqual(['2-4', '4-5']);
  });

  it('traces an unnamed edge only to the other readers of the same result', () => {
    expect(labels(traceValue(owner, edges[0]))).toEqual(['0-1']);
  });

  it('enters and leaves branches through their ports of the same name', () => {
    // f: 1 ─q→ branch 2 ─y→ 3. Inside the branch: port 4 "q" ─q→ lane 5; inside
    // the lane: port 6 "q" ─q→ step 7 ─y→ port 8 "y"; lane ─y→ branch port 9 "y".
    const outer = [e(1, 2, 'q'), e(2, 3, 'y'), e(1, 3, 'k')];
    const inBranch = [e(4, 5, 'q'), e(5, 9, 'y')];
    const inLane = [e(6, 7, 'q'), e(7, 8, 'y')];
    const nodes: Record<number, SceneNode> = {
      0: { id: 0, kind: 'function', shape: 'box', edges: outer, children: [1, 2, 3] },
      1: { id: 1, kind: 'op', shape: 'box', parent: 0 },
      2: { id: 2, kind: 'branch', shape: 'region', parent: 0, children: [4, 5, 9], edges: inBranch },
      3: { id: 3, kind: 'op', shape: 'box', parent: 0 },
      4: { id: 4, kind: 'input', shape: 'port', title: 'q', parent: 2 },
      5: { id: 5, kind: 'lane', shape: 'region', parent: 2, children: [6, 7, 8], edges: inLane },
      6: { id: 6, kind: 'input', shape: 'port', title: 'q', parent: 5 },
      7: { id: 7, kind: 'op', shape: 'box', parent: 5 },
      8: { id: 8, kind: 'output', shape: 'port', title: 'y', parent: 5 },
      9: { id: 9, kind: 'output', shape: 'port', title: 'y', parent: 2 },
    } as unknown as Record<number, SceneNode>;
    const all = [...outer, ...inBranch, ...inLane];
    const q = trace(id => nodes[id], 0, outer[0]);
    expect(labels(q.edges, all)).toEqual(['1-2', '4-5', '6-7']);
    expect([...q.scope].sort()).toEqual([0, 2, 5]);
    // From inside out: y made in the lane leaves the branch and reaches 3.
    expect(labels(trace(id => nodes[id], 5, inLane[1]).edges, all)).toEqual(['2-3', '5-9', '7-8']);
  });

  it('measures distance to a polyline', () => {
    expect(distanceToPolyline(5, 3, [0, 0, 10, 0])).toBe(9);
    expect(distanceToPolyline(12, 0, [0, 0, 10, 0, 10, 10])).toBe(4);
  });
});
