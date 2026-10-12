import { describe, expect, it } from 'vitest';
import type { Project, Scene, SceneNode } from '../core/types';
import { SceneModel } from './model';
import { decodePath, encodePath } from './location';

function model(): SceneModel {
  const n = (id: number, title: string, extra: Partial<SceneNode> = {}): SceneNode =>
    ({ id, kind: 'function', shape: 'box', category: 'function', title, x: 0, y: 0, w: 10, h: 10, inner: { x: 0, y: 0, w: 10, h: 10 }, cw: 10, ch: 10, ...extra });
  const nodes: SceneNode[] = [
    n(0, 'repo', { kind: 'repo', children: [1] }),
    n(1, 'data/ a~b', { kind: 'file', parent: 0, children: [2, 3, 4] }),
    // An untitled branch holding a step.
    n(7, '', { kind: 'branch', shape: 'region', parent: 1, children: [8] }),
    n(8, 'y', { kind: 'op', parent: 7, inner: undefined }),
    n(2, 'view · transpose', { kind: 'op', parent: 1, inner: undefined }),
    n(3, 'view · transpose', { kind: 'op', parent: 1, inner: undefined }),
    // A call borrowing f's interior; f defines a step.
    n(4, 'f', { kind: 'call', parent: 1, ref: 5 }),
    n(5, 'f', { parent: 1, children: [6] }),
    n(6, 'x * 2', { kind: 'op', parent: 5, inner: undefined }),
  ];
  nodes[1].children = [2, 3, 4, 5, 7];
  nodes.sort((a, b) => a.id - b.id);
  const s: Scene = { version: 1, title: 'repo', root: 0, nodes, files: [], diagnostics: [], stats: { files: 0, pythonFiles: 0, lines: 0, nodes: nodes.length, maxDepth: 3 } };
  const p: Project = { name: 'repo', source: 'test', files: [] };
  return new SceneModel(s, p);
}

describe('locations', () => {
  const m = model();

  it('round-trips paths, telling same-named siblings apart', () => {
    for (const path of [[0], [0, 1], [0, 1, 2], [0, 1, 3], [0, 1, 4, 6], [0, 1, 5, 6], [0, 1, 7, 8]]) {
      expect(decodePath(m, encodePath(m, path))).toEqual(path);
    }
    expect(encodePath(m, [0, 1, 3])).toBe('data%2F%20a%7Eb/view%20%C2%B7%20transpose~2');
    expect(encodePath(m, [0, 1, 7, 8])).toBe('data%2F%20a%7Eb/~1/y');
  });

  it('resolves as far as the scene still has the path', () => {
    expect(decodePath(m, 'data%2F%20a%7Eb/gone/x')).toEqual([0, 1]);
    expect(decodePath(m, '%E0%A4%A')).toEqual([0]);
  });
});
