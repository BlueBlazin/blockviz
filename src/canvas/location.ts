import type { SceneModel } from './model';

/**
 * Places in a scene as text that survives re-analysis: an instance path as
 * the titles along it (`model.py/CausalSelfAttention/forward`), with `~k`
 * for the k-th of several siblings sharing a title. A block without a title
 * (an if/else) is always `~k`, so no step of the path is empty.
 */
export function encodePath(model: SceneModel, path: number[]): string {
  const out: string[] = [];
  for (let i = 1; i < path.length; i++) {
    const parent = model.node(path[i - 1]);
    const owner = model.owner(parent) ?? parent;
    const title = model.node(path[i]).title;
    let k = 0;
    for (const c of owner.children ?? []) {
      if (model.node(c).title === title) k++;
      if (c === path[i]) break;
    }
    out.push(encodeURIComponent(title).replace(/~/g, '%7E') + (k > 1 || !title ? `~${k}` : ''));
  }
  return out.join('/');
}

/** The instance path a location names, as far as it still resolves. */
export function decodePath(model: SceneModel, text: string): number[] {
  const path = [model.root];
  for (const seg of text.split('/').filter(Boolean)) {
    const m = seg.match(/^(.*?)(?:~(\d+))?$/)!;
    let title: string;
    try { title = decodeURIComponent(m[1]); } catch { break; }
    const k = Number(m[2] ?? 1);
    const parent = model.node(path[path.length - 1]);
    const owner = model.owner(parent) ?? parent;
    const next = (owner.children ?? []).filter(c => model.node(c).title === title)[k - 1];
    if (next === undefined) break;
    path.push(next);
  }
  return path;
}
