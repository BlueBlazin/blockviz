import type { Edge, EdgeKind, FileChange, SceneNode } from '../core/types';
import type { SceneModel } from './model';

export type Status = 'added' | 'changed' | 'removed';

/**
 * A change a reader steps through: a block added or changed (`id` in the
 * head), or removed (`id` in the base), shown at `at` in the head: the block
 * itself, or the block a removed one was in.
 */
export interface Change { status: Status; id: number; at: number }

/** A connection the base drew between two blocks the head still has, side by side, but no longer draws. */
export interface RemovedEdge { from: number; to: number; kind: EdgeKind; label?: string }

const PORTS = new Set(['input', 'output']);

/**
 * How the head of a pull request differs from its base, block by block.
 *
 * Blocks are matched by where they are: a file by its path (or the path it
 * was renamed from), a folder by its name, and anything in a file by its kind
 * and title among its siblings; siblings of the same title pair up by
 * identical code first, then in order. Only files the change touched have
 * added, changed or removed blocks: analysis of an untouched file can shift
 * with the code around it (a call now resolving), but that file did not change.
 *
 * A block is changed when its code is (ignoring whitespace). What a reader
 * steps through are the innermost changes: an added or removed block (not
 * what is in it), and a changed block nothing in which changed.
 */
export class SceneDiff {
  /** Head block → base block, and back. */
  private match = new Map<number, number>();
  private back = new Map<number, number>();
  private statuses = new Map<number, Status>();
  /** Changes in each head block's subtree, its own included. */
  private subtree = new Map<number, number>();
  private units = new Set<number>();
  /** Removed blocks, by the head block they were in. */
  private removedAt = new Map<number, Change[]>();
  private headChanges = new Map<string, FileChange>();
  private baseChanges = new Map<string, FileChange>();
  private texts = new Map<SceneModel, Map<number, string | null>>();
  private order = new Map<string, number>();
  readonly changes: Change[] = [];
  readonly added = new Set<Edge>();
  readonly removed = new Map<number, RemovedEdge[]>();

  constructor(readonly base: SceneModel, readonly head: SceneModel, readonly files: FileChange[]) {
    for (const f of files) {
      this.headChanges.set(f.path, f);
      this.baseChanges.set(f.status === 'renamed' && f.previous ? f.previous : f.path, f);
    }
    this.pairFiles();
    this.pairTree(head.root, base.root);
    this.pairMoved();
    this.mark(head.root);
    this.findRemoved();
    this.count(head.root);
    this.list(head.root);
    this.compareEdges();
  }

  /** What happened to a head block (null: nothing). */
  status(id: number): Status | null {
    return this.statuses.get(id) ?? null;
  }

  /** The base block a head block was. */
  baseOf(id: number): number | undefined {
    return this.match.get(id);
  }

  /** The head block a base block became. */
  headOf(id: number): number | undefined {
    return this.back.get(id);
  }

  /** Changes inside a head block (removed blocks it held included), not counting its own. */
  inside(id: number): number {
    return (this.subtree.get(id) ?? 0) - (this.units.has(id) ? 1 : 0);
  }

  /** Changes in the code a block leads to: what a call runs, a reference's definition. */
  through(id: number): number {
    const n = this.head.node(id);
    const target = n.ref ?? n.link;
    return target === undefined || target === id ? 0 : this.subtree.get(target) ?? 0;
  }

  /** Whether a head block changed or has changes inside. */
  touched(id: number): boolean {
    return this.statuses.has(id) || (this.subtree.get(id) ?? 0) > 0;
  }

  /** Removed blocks that were in a head block. */
  removedIn(id: number): Change[] {
    return this.removedAt.get(id) ?? [];
  }

  /** Where a change is in the list (by its block), or -1. */
  indexOf(id: number, removed = false): number {
    return this.order.get(`${removed ? 'b' : 'h'}${id}`) ?? -1;
  }

  /** How many changes of each kind. */
  get summary(): Record<Status, number> {
    const out = { added: 0, changed: 0, removed: 0 };
    for (const c of this.changes) out[c.status]++;
    return out;
  }

  /** How a file changed, by its path in the head (or, `base`, in the base). */
  fileChange(path: string | undefined, base = false): FileChange | undefined {
    return path === undefined ? undefined : (base ? this.baseChanges : this.headChanges).get(path);
  }

  // ----- matching ---------------------------------------------------------------------

  private pair(h: number, b: number) {
    this.match.set(h, b);
    this.back.set(b, h);
  }

  /** Files pair by path wherever their folders are drawn (folders fold and regroup as files come and go). */
  private pairFiles() {
    const baseFiles = new Map<string, number[]>();
    for (const [path, ids] of tops(this.base)) baseFiles.set(path, ids);
    for (const [path, ids] of tops(this.head)) {
      const change = this.headChanges.get(path);
      const was = baseFiles.get(change?.status === 'renamed' && change.previous ? change.previous : path) ?? [];
      // One block for the file in each: the same file, whatever it is called now.
      if (ids.length === 1 && was.length === 1) this.pair(ids[0], was[0]);
      else this.pairAmong(ids, was);
    }
  }

  /** Pairs head blocks with base blocks of the same kind and title: identical code first, then in order. */
  private pairAmong(heads: number[], bases: number[]) {
    const free = bases.filter(b => !this.back.has(b));
    const groups = new Map<string, number[]>();
    for (const b of free) {
      const k = keyOf(this.base.node(b));
      groups.set(k, [...(groups.get(k) ?? []), b]);
    }
    const left: number[] = [];
    for (const h of heads) {
      if (this.match.has(h)) continue;
      const same = groups.get(keyOf(this.head.node(h)));
      // In an untouched file the two versions are the same code: order pairs them.
      const text = same && this.inChange(h) ? this.text(this.head, h) : null;
      const i = same && text !== null ? same.findIndex(b => this.text(this.base, b) === text) : -1;
      if (i >= 0) this.pair(h, same!.splice(i, 1)[0]);
      else left.push(h);
    }
    for (const h of left) {
      const same = groups.get(keyOf(this.head.node(h)));
      if (same?.length) this.pair(h, same.shift()!);
    }
  }

  private pairTree(h: number, b: number | undefined) {
    if (!this.match.has(h) && b !== undefined && !this.back.has(b)) this.pair(h, b);
    const mine = this.match.get(h);
    const kids = this.head.node(h).children ?? [];
    if (mine !== undefined) this.pairAmong(kids, this.base.node(mine).children ?? []);
    for (const c of kids) this.pairTree(c, undefined);
  }

  /** Whether a head block is in a file the change touched. */
  private inChange(h: number): boolean {
    const file = scopeFile(this.head, this.head.node(h));
    return file !== undefined && this.headChanges.has(file);
  }

  /**
   * Code moved within a file, into or out of a branch, loop or other block
   * (an `if` wrapped around steps): left over on both sides, the same kind,
   * title and code. Paired, it is the same block in a new place, not one
   * removed and another added.
   */
  private pairMoved() {
    for (const [h, b] of [...this.match]) {
      const hn = this.head.node(h);
      const file = hn.span ? this.head.fileOf(hn) : undefined;
      if (file === undefined || !this.headChanges.has(file) || !isTop(this.head, hn)) continue;
      const unpaired = (model: SceneModel, root: number, paired: Map<number, number>): number[] => {
        const out: number[] = [];
        const visit = (id: number) => {
          for (const c of model.node(id).children ?? []) {
            if (!paired.has(c) && !PORTS.has(model.node(c).kind)) out.push(c);
            visit(c);
          }
        };
        visit(root);
        return out;
      };
      const left = unpaired(this.base, b, this.back);
      if (!left.length) continue;
      for (const c of unpaired(this.head, h, this.match)) {
        if (this.match.has(c)) continue;
        const text = this.text(this.head, c);
        if (text === null) continue;
        const key = keyOf(this.head.node(c));
        const i = left.findIndex(x => !this.back.has(x) && keyOf(this.base.node(x)) === key && this.text(this.base, x) === text);
        if (i < 0) continue;
        this.pairTree(c, left[i]);
      }
    }
  }

  // ----- statuses ---------------------------------------------------------------------

  /** A block's code, whitespace collapsed (null: it has none). */
  private text(model: SceneModel, id: number): string | null {
    let cache = this.texts.get(model);
    if (!cache) this.texts.set(model, (cache = new Map()));
    if (!cache.has(id)) {
      const span = model.node(id).span;
      const src = span ? model.source(span) : null;
      cache.set(id, src === null ? null : src.replace(/\s+/g, ' ').trim());
    }
    return cache.get(id)!;
  }

  private mark(h: number) {
    const s = this.statusOf(h);
    if (s) this.statuses.set(h, s);
    for (const c of this.head.node(h).children ?? []) this.mark(c);
  }

  private statusOf(h: number): Status | null {
    const n = this.head.node(h);
    const file = scopeFile(this.head, n);
    // Above files: a new folder, or something no file is drawn in (a file on disk a program now writes).
    if (file === undefined) return !this.match.has(h) && allFiles(this.head, n, p => this.headChanges.get(p)?.status === 'added') ? 'added' : null;
    const change = this.headChanges.get(file);
    if (!change) return null;
    if (change.status === 'added' || !this.match.has(h)) return 'added';
    const b = this.match.get(h)!;
    if (n.span && this.text(this.head, h) !== this.text(this.base, b)) return 'changed';
    // A file moved without changing is still a change to the file.
    if (change.status === 'renamed' && n.span?.start === 0 && this.head.fileOf(n) === file && isTop(this.head, n)) return 'changed';
    return null;
  }

  /** Base blocks no head block became, in files the change touched; listed by the head block they were in. */
  private findRemoved() {
    const gone = (b: number): boolean => {
      if (this.back.has(b)) return false;
      const n = this.base.node(b);
      const file = scopeFile(this.base, n);
      if (file === undefined) return allFiles(this.base, n, p => this.baseChanges.get(p)?.status === 'removed');
      return !!this.baseChanges.get(file);
    };
    const visit = (b: number) => {
      const n = this.base.node(b);
      if (gone(b) && !PORTS.has(n.kind)) {
        // The outermost removed block stands for what was in it.
        let p = n.parent;
        while (p !== undefined && !this.back.has(p)) p = this.base.node(p).parent;
        const at = p === undefined ? this.head.root : this.back.get(p)!;
        this.removedAt.set(at, [...(this.removedAt.get(at) ?? []), { status: 'removed', id: b, at }]);
        return;
      }
      for (const c of n.children ?? []) visit(c);
    };
    visit(this.base.root);
    for (const list of this.removedAt.values()) list.sort((a, b) => (this.base.node(a.id).span?.start ?? 0) - (this.base.node(b.id).span?.start ?? 0) || this.base.node(a.id).y - this.base.node(b.id).y);
  }

  private count(h: number): number {
    const n = this.head.node(h);
    const s = this.statuses.get(h);
    let total = this.removedIn(h).length;
    // An added block is one change, whatever is in it.
    if (s === 'added') total = 0;
    else for (const c of n.children ?? []) total += this.count(c);
    if (s && !PORTS.has(n.kind) && total === 0) {
      this.units.add(h);
      total = 1;
    }
    this.subtree.set(h, total);
    return total;
  }

  /** Changes in diagram order: top to bottom, left to right, outside in. */
  private list(h: number) {
    if (this.units.has(h)) this.add({ status: this.statuses.get(h)!, id: h, at: h });
    if (this.statuses.get(h) !== 'added') {
      const kids = [...(this.head.node(h).children ?? [])].sort((a, b) => this.head.node(a).y - this.head.node(b).y || this.head.node(a).x - this.head.node(b).x);
      for (const c of kids) if ((this.subtree.get(c) ?? 0) > 0) this.list(c);
    }
    for (const r of this.removedIn(h)) this.add(r);
  }

  private add(c: Change) {
    this.order.set(`${c.status === 'removed' ? 'b' : 'h'}${c.id}`, this.changes.length);
    this.changes.push(c);
  }

  // ----- connections ------------------------------------------------------------------

  /**
   * New and removed connections between blocks, in diagrams the change
   * touched (and the repository's own, of files and folders). Dependencies
   * are drawn transitively reduced, so one still implied through others is
   * neither new nor removed.
   */
  private compareEdges() {
    for (const hn of this.head.nodes) {
      if (!hn.edges?.length && !this.base.node(this.match.get(hn.id) ?? -1)?.edges?.length) continue;
      const b = this.match.get(hn.id);
      if (b === undefined) continue;
      const file = scopeFile(this.head, hn);
      if (file !== undefined && !this.headChanges.get(file)) continue;
      const bn = this.base.node(b);
      const key = (from: number, to: number, kind: string) => `${from}>${to}>${kind}`;
      const was = new Set<string>();
      for (const e of bn.edges ?? []) {
        const f = this.back.get(e.from);
        const t = this.back.get(e.to);
        if (f !== undefined && t !== undefined) was.add(key(f, t, e.kind));
      }
      const now = new Set((hn.edges ?? []).map(e => key(e.from, e.to, e.kind)));
      for (const e of hn.edges ?? []) {
        if (was.has(key(e.from, e.to, e.kind))) continue;
        const f = this.match.get(e.from);
        const t = this.match.get(e.to);
        if (e.kind === 'uses' && f !== undefined && t !== undefined && reaches(bn, f, t)) continue;
        this.added.add(e);
      }
      const gone: RemovedEdge[] = [];
      for (const e of bn.edges ?? []) {
        const f = this.back.get(e.from);
        const t = this.back.get(e.to);
        // A removed block takes its connections with it; one drawn elsewhere now is not here to connect.
        if (f === undefined || t === undefined || f === t || this.head.node(f).parent !== hn.id || this.head.node(t).parent !== hn.id) continue;
        if (now.has(key(f, t, e.kind)) || (e.kind === 'uses' && reaches(hn, f, t))) continue;
        if (gone.some(g => g.from === f && g.to === t && g.kind === e.kind)) continue;
        gone.push({ from: f, to: t, kind: e.kind, label: e.label });
      }
      if (gone.length) this.removed.set(hn.id, gone);
    }
  }
}

/** Siblings that may be the same block across versions. */
const keyOf = (n: SceneNode) => `${n.kind}\0${n.title}`;

/** The path of the file a block is in (the nearest block with code), or undefined above files. */
function scopeFile(model: SceneModel, n: SceneNode): string | undefined {
  for (let c: SceneNode | undefined = n; c; c = c.parent === undefined ? undefined : model.node(c.parent)) {
    if (c.span) return model.fileOf(c);
  }
  return undefined;
}

/** Whether a block is the outermost one drawn for its file. */
function isTop(model: SceneModel, n: SceneNode): boolean {
  const file = n.span?.file;
  for (let p = n.parent; p !== undefined; p = model.node(p).parent) if (model.node(p).span?.file === file) return false;
  return true;
}

/** The outermost blocks drawn for each file, by path. */
function tops(model: SceneModel): Map<string, number[]> {
  const out = new Map<string, number[]>();
  const visit = (id: number, seen: ReadonlySet<number>) => {
    const n = model.node(id);
    let inner = seen;
    if (n.span && !seen.has(n.span.file)) {
      const path = model.fileOf(n);
      if (path !== undefined) out.set(path, [...(out.get(path) ?? []), id]);
      inner = new Set([...seen, n.span.file]);
    }
    for (const c of n.children ?? []) visit(c, inner);
  };
  visit(model.root, new Set());
  return out;
}

/** Whether every file a folder (or group) holds is `which`; true of a block that holds no files. */
function allFiles(model: SceneModel, n: SceneNode, which: (path: string) => boolean): boolean {
  let any = false;
  let all = true;
  const visit = (c: SceneNode) => {
    if (c.span) {
      const path = model.fileOf(c);
      if (path !== undefined) {
        any = true;
        if (!which(path)) all = false;
      }
      return;
    }
    for (const k of c.children ?? []) visit(model.node(k));
  };
  visit(n);
  return !any || all;
}

/** Whether `to` is reached from `from` along a diagram's dependencies. */
function reaches(owner: SceneNode, from: number, to: number): boolean {
  const next = new Map<number, number[]>();
  for (const e of owner.edges ?? []) if (e.kind === 'uses') next.set(e.from, [...(next.get(e.from) ?? []), e.to]);
  const seen = new Set([from]);
  const stack = [from];
  while (stack.length) {
    for (const t of next.get(stack.pop()!) ?? []) {
      if (t === to) return true;
      if (!seen.has(t)) {
        seen.add(t);
        stack.push(t);
      }
    }
  }
  return false;
}
