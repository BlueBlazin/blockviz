import type { Edge, SceneNode } from '../core/types';

/** Edges that carry values (not conditions, dependencies or executions). */
const VALUE_KINDS = new Set(['data']);

/** A value followed through a definition: the edges carrying it, and the
 * diagrams (the definition and the branches and loops inside it) it is traced in. */
export interface ValueTrace { scope: ReadonlySet<number>; edges: ReadonlySet<Edge> }

const isRegion = (n: SceneNode) => n.shape === 'region';

/**
 * The path of one value: every edge carrying it, from where it is made to
 * everything that reads it. A value keeps its name through steps that
 * re-bind it (`q = q.view(…).transpose(…)`), so the trace is every edge of
 * that name connected to the start through the steps they share, entering
 * and leaving branches and loops through their ports of that name. It stays
 * within the definition: calls are other code. An unnamed edge is traced
 * only to the other readers of the same result.
 */
export function traceValue(node: (id: number) => SceneNode, ownerId: number, start: Edge): ValueTrace {
  // The definition's diagram and every region inside it.
  let top = node(ownerId);
  while (isRegion(top) && top.parent !== undefined) top = node(top.parent);
  const scope = new Set<number>();
  const collect = (n: SceneNode) => {
    scope.add(n.id);
    for (const c of n.children ?? []) if (isRegion(node(c))) collect(node(c));
  };
  collect(top);

  const edges = new Set<Edge>([start]);
  if (!VALUE_KINDS.has(start.kind)) return { scope, edges };
  const label = start.label ?? '';
  const carries = (e: Edge) => VALUE_KINDS.has(e.kind) && (e.label ?? '') === label;
  if (!label) {
    for (const e of node(ownerId).edges ?? []) if (carries(e) && e.from === start.from) edges.add(e);
    return { scope, edges };
  }
  // (diagram, step) pairs the value reaches; edges of its name at a step carry it on.
  const seen = new Set<string>();
  const queue: [number, number][] = [[ownerId, start.from], [ownerId, start.to]];
  while (queue.length) {
    const [o, id] = queue.pop()!;
    const key = `${o}:${id}`;
    if (seen.has(key)) continue;
    seen.add(key);
    const owner = node(o);
    for (const e of owner.edges ?? []) {
      if (!carries(e) || (e.from !== id && e.to !== id)) continue;
      edges.add(e);
      queue.push([o, e.from], [o, e.to]);
    }
    const n = node(id);
    // Into a branch or loop: its port of this name.
    if (isRegion(n) && n.id !== o) {
      for (const c of n.children ?? []) {
        const k = node(c);
        if ((k.kind === 'input' || k.kind === 'output') && k.title === label) queue.push([n.id, c]);
      }
    }
    // Out of one, from its port: the region's own edges outside.
    if ((n.kind === 'input' || n.kind === 'output') && isRegion(owner) && owner.parent !== undefined && scope.has(owner.parent)) {
      queue.push([owner.parent, owner.id]);
    }
  }
  return { scope, edges };
}

/** Squared distance from a point to a polyline (screen coordinates, x0,y0,x1,y1…). */
export function distanceToPolyline(px: number, py: number, pts: number[]): number {
  let best = Infinity;
  for (let i = 0; i + 3 < pts.length; i += 2) {
    const ax = pts[i], ay = pts[i + 1], bx = pts[i + 2], by = pts[i + 3];
    const dx = bx - ax, dy = by - ay;
    const len = dx * dx + dy * dy;
    const t = len ? Math.max(0, Math.min(1, ((px - ax) * dx + (py - ay) * dy) / len)) : 0;
    const x = ax + t * dx - px, y = ay + t * dy - py;
    best = Math.min(best, x * x + y * y);
  }
  return best;
}
