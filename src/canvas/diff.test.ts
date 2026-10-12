import { describe, expect, it } from 'vitest';
import type { EdgeKind, FileChange, Kind, Project, Scene, SceneNode } from '../core/types';
import { SceneDiff } from './diff';
import { SceneModel } from './model';

/** A block of a hand-built diagram: `code` is its text in its file (found after its previous sibling's). */
interface Spec { kind: Kind; title: string; file?: string; code?: string; children?: Spec[]; edges?: [string, string, EdgeKind?][]; ref?: string }

/** A diagram of `files` drawn as `root`; titles of blocks are unique enough to name them in tests. */
function model(files: Record<string, string>, root: Spec): SceneModel {
  const paths = Object.keys(files);
  const nodes: SceneNode[] = [];
  const refs: [SceneNode, string][] = [];
  const build = (spec: Spec, parent: SceneNode | undefined, file: string | undefined, from: number, index: number): SceneNode => {
    const n: SceneNode = { id: nodes.length, kind: spec.kind, shape: spec.kind === 'loop' || spec.kind === 'branch' ? 'region' : 'box', category: 'function', title: spec.title, x: 0, y: index * 10, w: 10, h: 10, parent: parent?.id };
    nodes.push(n);
    const f = spec.file ?? file;
    if (spec.file) n.span = spanOf(files, paths, spec.file, 0, files[spec.file].length);
    else if (f && spec.code !== undefined) {
      const start = files[f].indexOf(spec.code, from);
      if (start < 0) throw new Error(`no ${spec.code} in ${f}`);
      n.span = spanOf(files, paths, f, start, start + spec.code.length);
    }
    if (spec.ref) refs.push([n, spec.ref]);
    let cursor = n.span?.start ?? from;
    const kids = (spec.children ?? []).map((c, i) => {
      const k = build(c, n, f, cursor, i);
      if (k.span && k.span.file === n.span?.file) cursor = k.span.end;
      return k;
    });
    if (kids.length) {
      n.children = kids.map(k => k.id);
      n.inner = { x: 0, y: 0, w: 10, h: 10 };
      n.cw = 10;
      n.ch = 10;
    }
    if (spec.edges) n.edges = spec.edges.map(([a, b, kind = 'data']) => ({ from: kids.find(k => k.title === a)!.id, to: kids.find(k => k.title === b)!.id, kind, points: [] }));
    return n;
  };
  build(root, undefined, undefined, 0, 0);
  for (const [n, title] of refs) n.ref = n.link = nodes.find(m => m.title === title && m.kind === 'function')!.id;
  const scene: Scene = { version: 1, title: 'repo', root: 0, nodes, files: paths.map(path => ({ path, language: 'python', bytes: files[path].length, lines: 1, analyzed: true })), diagnostics: [], stats: { files: paths.length, pythonFiles: paths.length, lines: 1, nodes: nodes.length, maxDepth: 4 } };
  const project: Project = { name: 'repo', source: 'test', files: paths.map(path => ({ path, text: files[path], bytes: files[path].length })) };
  return new SceneModel(scene, project);
}

function spanOf(files: Record<string, string>, paths: string[], path: string, start: number, end: number) {
  const text = files[path];
  const line = text.slice(0, start).split('\n').length;
  return { file: paths.indexOf(path), start, end, line, endLine: line + text.slice(start, end).split('\n').length - 1 };
}

const fn = (title: string, code: string, children: Spec[] = [], edges: Spec['edges'] = []): Spec => ({ kind: 'function', title, code, children, edges });
const step = (title: string, code: string, ref?: string): Spec => ({ kind: 'call', title, code, ref });

/** The head block titled `title` (the first, or the one in `parent`). */
const id = (m: SceneModel, title: string, parent?: string) => m.nodes.find(n => n.title === title && (!parent || m.node(n.parent!)?.title === parent))!.id;
const changes = (d: SceneDiff) => d.changes.map(c => `${c.status} ${(c.status === 'removed' ? d.base : d.head).node(c.id).title}`);

describe('scene diff', () => {
  const baseFiles = {
    'model.py': 'def load(path):\n    data = read(path)\n    return parse(data)\n\ndef save(x):\n    write(x)\n\ndef old():\n    pass\n',
    'util.py': 'def helper():\n    pass\n',
  };
  const headFiles = {
    'model.py': 'def load(path):\n    data = read(path)\n    return parse(data, strict=True)\n\ndef save(x):\n    write(x)\n\ndef fresh():\n    a = make()\n    b = use(a)\n',
    'util.py': 'def helper():\n    pass\n',
  };
  const tree = (files: Record<string, string>, defs: Spec[], util: Spec[] = [fn('helper', 'def helper():\n    pass')]): Spec => ({
    kind: 'repo', title: 'repo', children: [
      { kind: 'file', title: 'model.py', file: 'model.py', children: defs },
      { kind: 'file', title: 'util.py', file: 'util.py', children: util },
    ],
  });
  const loadBase = fn('load', 'def load(path):\n    data = read(path)\n    return parse(data)', [step('read', 'read(path)'), step('parse', 'parse(data)')], [['read', 'parse']]);
  const loadHead = fn('load', 'def load(path):\n    data = read(path)\n    return parse(data, strict=True)', [step('read', 'read(path)'), step('parse', 'parse(data, strict=True)')], [['read', 'parse']]);
  const base = model(baseFiles, tree(baseFiles, [loadBase, fn('save', 'def save(x):\n    write(x)', [step('write', 'write(x)')]), fn('old', 'def old():\n    pass')]));
  const head = model(headFiles, tree(headFiles, [loadHead, fn('save', 'def save(x):\n    write(x)', [step('write', 'write(x)')]), fn('fresh', 'def fresh():\n    a = make()\n    b = use(a)', [step('make', 'make()'), step('use', 'use(a)')], [['make', 'use']])],
    // The analysis of an untouched file may shift (a step now resolving): not a change.
    [fn('helper', 'def helper():\n    pass', [step('pass', 'pass')])]));
  const files: FileChange[] = [{ path: 'model.py', status: 'modified' }];
  const d = new SceneDiff(base, head, files);

  it('marks what changed, down to the innermost change', () => {
    expect(d.status(id(head, 'parse'))).toBe('changed');
    expect(d.status(id(head, 'load'))).toBe('changed');
    expect(d.status(id(head, 'model.py'))).toBe('changed');
    expect(d.status(id(head, 'read'))).toBeNull();
    expect(d.status(id(head, 'save'))).toBeNull();
    // Something new in an untouched file is not a change.
    expect(d.status(id(head, 'pass'))).toBeNull();
    expect(d.status(id(head, 'util.py'))).toBeNull();
  });

  it('counts an added block once, and lists removed blocks where they were', () => {
    expect(d.status(id(head, 'fresh'))).toBe('added');
    expect(d.status(id(head, 'make'))).toBe('added');
    expect(changes(d)).toEqual(['changed parse', 'added fresh', 'removed old']);
    expect(d.changes[2].at).toBe(id(head, 'model.py'));
    expect(d.inside(id(head, 'model.py'))).toBe(3);
    expect(d.inside(id(head, 'load'))).toBe(1);
    expect(d.inside(id(head, 'repo'))).toBe(3);
    expect(d.touched(id(head, 'util.py'))).toBe(false);
    expect(d.summary).toEqual({ added: 1, changed: 1, removed: 1 });
    expect(d.indexOf(id(head, 'fresh'))).toBe(1);
    expect(d.indexOf(id(base, 'old'), true)).toBe(2);
  });

  it('matches files by the path they were renamed from', () => {
    const moved = { 'core.py': headFiles['model.py'], 'util.py': headFiles['util.py'] };
    const h = model(moved, { kind: 'repo', title: 'repo', children: [{ kind: 'file', title: 'core.py', file: 'core.py', children: [loadHead] }, { kind: 'file', title: 'util.py', file: 'util.py' }] });
    const b = model(baseFiles, { kind: 'repo', title: 'repo', children: [{ kind: 'file', title: 'model.py', file: 'model.py', children: [loadBase] }, { kind: 'file', title: 'util.py', file: 'util.py' }] });
    const r = new SceneDiff(b, h, [{ path: 'core.py', status: 'renamed', previous: 'model.py' }]);
    expect(r.status(id(h, 'load'))).toBe('changed');
    expect(r.baseOf(id(h, 'core.py'))).toBe(id(b, 'model.py'));
    expect(changes(r)).toEqual(['changed parse']);
  });

  it('counts a new folder once', () => {
    const more = { ...baseFiles, 'pkg/a.py': 'x = 1\n', 'pkg/b.py': 'y = 2\n' };
    const files = (f: Record<string, string>, extra: Spec[]): Spec => ({ kind: 'repo', title: 'repo', children: [{ kind: 'file', title: 'model.py', file: 'model.py' }, { kind: 'file', title: 'util.py', file: 'util.py' }, ...extra] });
    const h = model(more, files(more, [{ kind: 'dir', title: 'pkg/', children: [{ kind: 'file', title: 'a.py', file: 'pkg/a.py' }, { kind: 'file', title: 'b.py', file: 'pkg/b.py' }] }]));
    const b = model(baseFiles, files(baseFiles, []));
    const r = new SceneDiff(b, h, [{ path: 'pkg/a.py', status: 'added' }, { path: 'pkg/b.py', status: 'added' }]);
    expect(changes(r)).toEqual(['added pkg/']);
    expect(r.status(id(h, 'a.py'))).toBe('added');
  });

  it('pairs blocks of the same title by their code first', () => {
    const bf = { 'm.py': 'def f():\n    view(a)\n    view(b)\n' };
    const hf = { 'm.py': 'def f():\n    view(z)\n    view(a)\n    view(b)\n' };
    const shape = (files: Record<string, string>, steps: string[]): Spec => ({ kind: 'repo', title: 'repo', children: [{ kind: 'file', title: 'm.py', file: 'm.py', children: [fn('f', files['m.py'].trimEnd(), steps.map(s => step('view', s)))] }] });
    const b = model(bf, shape(bf, ['view(a)', 'view(b)']));
    const h = model(hf, shape(hf, ['view(z)', 'view(a)', 'view(b)']));
    const r = new SceneDiff(b, h, [{ path: 'm.py', status: 'modified' }]);
    // The new call is the first; the others are the ones that were there.
    const views = h.nodes.filter(n => n.title === 'view').map(n => r.status(n.id));
    expect(views).toEqual(['added', null, null]);
  });

  it('follows code moved into a new branch', () => {
    const bf = { 'm.py': 'def f(x):\n    y = scale(x)\n    return pick(y)\n' };
    const hf = { 'm.py': 'def f(x):\n    if x:\n        y = scale(x)\n        return pick(y)\n    return x\n' };
    const b = model(bf, { kind: 'repo', title: 'repo', children: [{ kind: 'file', title: 'm.py', file: 'm.py', children: [fn('f', bf['m.py'].trimEnd(), [step('scale', 'scale(x)'), step('pick', 'pick(y)')])] }] });
    const branch: Spec = { kind: 'branch', title: '', code: 'if x:\n        y = scale(x)\n        return pick(y)', children: [step('scale', 'scale(x)'), step('pick', 'pick(y)')] };
    const h = model(hf, { kind: 'repo', title: 'repo', children: [{ kind: 'file', title: 'm.py', file: 'm.py', children: [fn('f', hf['m.py'].trimEnd(), [branch])] }] });
    const r = new SceneDiff(b, h, [{ path: 'm.py', status: 'modified' }]);
    // One change: the new branch. The steps in it are the ones that were there.
    expect(changes(r)).toEqual(['added ']);
    expect(r.baseOf(id(h, 'scale'))).toBe(id(b, 'scale'));
    expect(r.status(id(h, 'scale'))).toBeNull();
  });

  it('finds new and removed connections, not ones still implied', () => {
    const bf = { 'a.py': 'A', 'b.py': 'B', 'c.py': 'C' };
    const hf = { 'a.py': 'A2', 'b.py': 'B2', 'c.py': 'C' };
    const repo = (files: Record<string, string>, edges: Spec['edges']): Spec => ({ kind: 'repo', title: 'repo', children: Object.keys(files).map(f => ({ kind: 'file' as Kind, title: f, file: f })), edges });
    // a → c was drawn; now a → b → c draws it through b (still a dependency), and b → a is gone.
    // b → c is drawn now, but b reached c before too (through a): not new.
    const b = model(bf, repo(bf, [['a.py', 'c.py', 'uses'], ['b.py', 'a.py', 'uses']]));
    const h = model(hf, repo(hf, [['a.py', 'b.py', 'uses'], ['b.py', 'c.py', 'uses']]));
    const r = new SceneDiff(b, h, [{ path: 'a.py', status: 'modified' }, { path: 'b.py', status: 'modified' }]);
    const titled = (e: { from: number; to: number }) => `${h.node(e.from).title}→${h.node(e.to).title}`;
    expect([...r.added].map(titled)).toEqual(['a.py→b.py']);
    expect((r.removed.get(h.root) ?? []).map(titled)).toEqual(['b.py→a.py']);
    // Inside a changed function: the new step's arrow is new.
    expect([...d.added].map(e => `${head.node(e.from).title}→${head.node(e.to).title}`)).toEqual([]);
  });

  it('says when a call leads into changed code', () => {
    const bf = { 'm.py': 'def g():\n    old()\n\ndef f():\n    g()\n' };
    const hf = { 'm.py': 'def g():\n    new()\n\ndef f():\n    g()\n' };
    const shape = (files: Record<string, string>, inner: string): Spec => ({ kind: 'repo', title: 'repo', children: [{ kind: 'file', title: 'm.py', file: 'm.py', children: [fn('g', `def g():\n    ${inner}()`, [step(inner, `${inner}()`)]), fn('f', 'def f():\n    g()', [step('g', 'g()', 'g')])] }] });
    const r = new SceneDiff(model(bf, shape(bf, 'old')), model(hf, shape(hf, 'new')), [{ path: 'm.py', status: 'modified' }]);
    const h = r.head;
    expect(r.through(id(h, 'g', 'f'))).toBe(2);
    expect(r.status(id(h, 'g', 'f'))).toBeNull();
    expect(changes(r)).toEqual(['added new', 'removed old']);
  });
});
