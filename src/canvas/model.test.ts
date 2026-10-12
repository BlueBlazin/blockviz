import { describe, expect, it } from 'vitest';
import type { Project, Scene, SceneNode, Span } from '../core/types';
import { SceneModel } from './model';

// A file holding a class whose method calls a helper defined after it.
const src = [
  'class A:',            // 1
  '    def f(self, x):', // 2
  '        y = g(x)',    // 3
  '        z = y + 1',   // 4
  '        return z',    // 5
  'def g(x):',           // 6
  '    return x * 2',    // 7
].join('\n') + '\n';

function spanOf(line: number, endLine = line): Span {
  const lines = src.split('\n');
  const start = lines.slice(0, line - 1).reduce((a, l) => a + l.length + 1, 0);
  const end = lines.slice(0, endLine).reduce((a, l) => a + l.length + 1, 0) - 1;
  return { file: 0, start, end, line, endLine };
}

function scene(): SceneModel {
  const n = (id: number, kind: SceneNode['kind'], title: string, extra: Partial<SceneNode> = {}): SceneNode =>
    ({ id, kind, shape: 'box', category: 'function', title, x: 0, y: 0, w: 10, h: 10, ...extra });
  const nodes: SceneNode[] = [
    n(0, 'repo', 'repo', { children: [1], inner: { x: 0, y: 0, w: 10, h: 10 }, cw: 10, ch: 10 }),
    n(1, 'file', 'a.py', { parent: 0, children: [2, 6], span: spanOf(1, 7), inner: { x: 0, y: 0, w: 10, h: 10 }, cw: 10, ch: 10 }),
    n(2, 'class', 'A', { parent: 1, children: [3], span: spanOf(1, 5), inner: { x: 0, y: 0, w: 10, h: 10 }, cw: 10, ch: 10 }),
    n(3, 'function', 'f', { parent: 2, children: [4, 5, 9], span: spanOf(2, 5), inner: { x: 0, y: 0, w: 10, h: 10 }, cw: 10, ch: 10 }),
    // The call borrows g's interior.
    n(4, 'call', 'g', { parent: 3, span: spanOf(3), ref: 6, link: 6, inner: { x: 0, y: 0, w: 10, h: 10 } }),
    n(5, 'op', 'z', { parent: 3, span: spanOf(4) }),
    n(6, 'function', 'g', { parent: 1, children: [7], span: spanOf(6, 7), inner: { x: 0, y: 0, w: 10, h: 10 }, cw: 10, ch: 10 }),
    n(7, 'op', 'x * 2', { parent: 6, span: spanOf(7) }),
    n(8, 'input', 'x', { parent: 6 }),
    n(9, 'output', 'z', { parent: 3, span: spanOf(5) }),
  ];
  const s: Scene = { version: 1, title: 'repo', root: 0, nodes, files: [{ path: 'a.py', language: 'python', bytes: src.length, lines: 7, analyzed: true }], diagnostics: [], stats: { files: 1, pythonFiles: 1, lines: 7, nodes: nodes.length, maxDepth: 4 } };
  const p: Project = { name: 'repo', source: 'test', files: [{ path: 'a.py', text: src, bytes: src.length }] };
  return new SceneModel(s, p);
}

describe('source views', () => {
  const m = scene();

  it('shows a definition whole', () => {
    expect(m.sourceView(m.node(3))).toEqual({ root: m.node(3), span: spanOf(2, 5) });
  });

  it('shows a step inside the definition around it', () => {
    const v = m.sourceView(m.node(5))!;
    expect(v.root.id).toBe(3);
    expect(v.span).toEqual(spanOf(2, 5));
  });

  it('windows lines around a span', () => {
    const w = m.window(spanOf(4), 1);
    expect([w.line, w.endLine]).toEqual([3, 5]);
    expect(src.slice(w.start, w.end)).toBe('        y = g(x)\n        z = y + 1\n        return z\n');
    expect(m.window(spanOf(7), 3).endLine).toBe(7);
  });

  it('finds the block drawn for a line', () => {
    expect(m.blockAt(m.node(3), 4).id).toBe(5);
    expect(m.blockAt(m.node(3), 3).id).toBe(4);
    // The def line itself is the function.
    expect(m.blockAt(m.node(3), 2).id).toBe(3);
  });

  it('keeps instance paths where the reader is', () => {
    // Inside f, through the call to g: g's step is reached through the call.
    expect(m.instancePath(7, [0, 1, 2, 3, 4])).toEqual([0, 1, 2, 3, 4, 7]);
    // Elsewhere: g's own definition site.
    expect(m.instancePath(7, [0, 1, 2])).toEqual([0, 1, 6, 7]);
    expect(m.instancePath(7, null)).toEqual([0, 1, 6, 7]);
  });
});
