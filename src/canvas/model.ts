import type { Project, Scene, SceneNode, Span } from '../core/types';
import { childXf, parentXf, type Xf } from './geometry';

export interface SearchEntry { id: number; title: string; detail: string; path: string; haystack: string; rank: number }

/** Source shown for a block: `span` around it, read as part of `root`. */
export interface SourceView { root: SceneNode; span: Span }

/** Blocks whose own source is what a reader wants to see: definitions and files. */
const WHOLE = new Set(['repo', 'dir', 'file', 'notebook', 'doc', 'asset', 'class', 'function', 'section']);
/** A step is shown inside its definition when that is at most this long. */
const CONTEXT_LINES = 80;
/** …and otherwise with this many lines either side. */
const WINDOW = 8;

/** Read-only helpers over a laid-out scene. */
export class SceneModel {
  readonly nodes: SceneNode[];
  readonly root: number;
  private encoded = new Map<number, Uint8Array>();
  private texts: (string | undefined)[];
  private index: SearchEntry[] | null = null;
  private lineStarts = new Map<number, Uint32Array>();
  private userIndex: Map<number, number[]> | null = null;

  constructor(readonly scene: Scene, project: Project) {
    this.nodes = scene.nodes;
    this.root = scene.root;
    const byPath = new Map(project.files.map(f => [f.path, f.text]));
    const strip = (p: string) => p.replace(/^[^/]+\//, '');
    this.texts = scene.files.map(f => f.source ?? byPath.get(f.path) ?? byPath.get(strip(f.path)) ?? findSuffix(project, f.path));
  }

  node(id: number): SceneNode {
    return this.nodes[id];
  }

  /** The node whose interior is drawn inside `n`: itself, or the definition it references. */
  owner(n: SceneNode): SceneNode | null {
    if (n.children?.length || n.rows?.length) return n.inner || n.id === this.root ? n : null;
    if (n.ref !== undefined && n.inner) {
      const r = this.nodes[n.ref];
      if (r.children?.length || r.rows?.length) return r;
    }
    return null;
  }

  /** Transform of `n`'s interior, given the transform of the space `n` lives in. */
  interior(t: Xf, n: SceneNode, owner = this.owner(n)): Xf | null {
    if (!owner || !n.inner) return null;
    return childXf(t, n, owner.cw ?? 1, owner.ch ?? 1);
  }

  exterior(c: Xf, n: SceneNode): Xf {
    const o = this.owner(n)!;
    return parentXf(c, n, o.cw ?? 1, o.ch ?? 1);
  }

  /** Root-to-node chain through ownership (the node's definition site). */
  canonicalPath(id: number): number[] {
    const path: number[] = [];
    let cur: number | undefined = id;
    while (cur !== undefined) {
      path.unshift(cur);
      cur = this.nodes[cur].parent;
    }
    return path[0] === this.root ? path : [this.root, ...path];
  }

  fileOf(n: SceneNode): string | undefined {
    return n.span ? this.scene.files[n.span.file]?.path : undefined;
  }

  source(span: Span): string | null {
    const bytes = this.fileBytes(span.file);
    return bytes ? new TextDecoder().decode(bytes.subarray(span.start, span.end)) : null;
  }

  /** A file's text as UTF-8, the encoding spans and syntax runs index. */
  fileBytes(file: number): Uint8Array | null {
    const text = this.texts[file];
    if (text === undefined) return null;
    let bytes = this.encoded.get(file);
    if (!bytes) {
      bytes = new TextEncoder().encode(text);
      this.encoded.set(file, bytes);
    }
    return bytes;
  }

  fileText(file: number): string | undefined {
    return this.texts[file];
  }

  /**
   * Blocks that use `id`: calls of a function, constructions of a class,
   * components of its type, calls of a component. In scene order.
   */
  users(id: number): number[] {
    if (!this.userIndex) {
      const index = new Map<number, number[]>();
      const add = (target: number, user: number) => {
        const list = index.get(target);
        if (!list) index.set(target, [user]);
        else if (!list.includes(user)) list.push(user);
      };
      for (const n of this.nodes) {
        if (n.link !== undefined && n.link !== n.id) add(n.link, n.id);
        if (n.ref !== undefined && n.ref !== n.id) add(n.ref, n.id);
        for (const u of n.usedBy ?? []) add(n.id, u);
      }
      for (const list of index.values()) list.sort((a, b) => a - b);
      this.userIndex = index;
    }
    return this.userIndex.get(id) ?? [];
  }

  /** The definition a block is part of (`Block.forward`, a section, a file), for saying where it is. */
  where(id: number): { name: string; at: number } | null {
    let p = this.nodes[id].parent;
    while (p !== undefined && !WHOLE.has(this.nodes[p].kind)) p = this.nodes[p].parent;
    if (p === undefined) return null;
    const def = this.nodes[p];
    const up = def.parent !== undefined ? this.nodes[def.parent] : null;
    const name = def.kind === 'function' && up?.kind === 'class' ? `${up.title}.${def.title}` : def.title;
    return { name, at: p };
  }

  /**
   * The source to show for a block. Definitions and files show their own
   * code; a step is shown in the definition around it (or with a few lines
   * either side when that is long), so a single line is never read alone.
   */
  sourceView(n: SceneNode): SourceView | null {
    const span = n.span;
    if (!span) return null;
    if (WHOLE.has(n.kind)) return { root: n, span };
    let root = n;
    for (let p = n.parent; p !== undefined; p = this.nodes[p].parent) {
      const a = this.nodes[p];
      if (!a.span || a.span.file !== span.file || a.span.start > span.start || a.span.end < span.end) continue;
      if (!WHOLE.has(a.kind)) continue;
      root = a;
      if (a.span.endLine - a.span.line <= CONTEXT_LINES) return { root: a, span: a.span };
      break;
    }
    return { root, span: this.window(span, WINDOW) };
  }

  /** A span's lines plus `k` lines either side. */
  window(span: Span, k: number): Span {
    const starts = this.starts(span.file);
    if (!starts) return span;
    const lines = starts.length;
    const line = Math.max(1, span.line - k);
    const endLine = Math.min(lines, span.endLine + k);
    const bytes = this.fileBytes(span.file)!;
    return { file: span.file, start: starts[line - 1], end: endLine < lines ? starts[endLine] : bytes.length, line, endLine };
  }

  /** Byte offset of each line's start. */
  private starts(file: number): Uint32Array | null {
    let s = this.lineStarts.get(file);
    if (!s) {
      const bytes = this.fileBytes(file);
      if (!bytes) return null;
      const out = [0];
      for (let i = 0; i < bytes.length; i++) if (bytes[i] === 10 && i + 1 < bytes.length) out.push(i + 1);
      s = Uint32Array.from(out);
      this.lineStarts.set(file, s);
    }
    return s;
  }

  /**
   * The block drawn for a line of `root`'s source: the smallest block inside
   * it whose code covers the line, or `root` itself.
   */
  blockAt(root: SceneNode, line: number): SceneNode {
    const file = root.span?.file;
    let best = root;
    let size = Infinity;
    const visit = (n: SceneNode) => {
      for (const c of n.children ?? []) {
        const k = this.nodes[c];
        const sp = k.span;
        if (!sp || sp.file !== file || line < sp.line || line > sp.endLine) {
          // Sections and arms may have no span of their own; their steps do.
          if (!sp) visit(k);
          continue;
        }
        if (k.kind !== 'input' && k.kind !== 'output') {
          const lines = sp.endLine - sp.line;
          const extent = lines * 1e7 + (sp.end - sp.start);
          if (extent <= size) {
            best = k;
            size = extent;
          }
        }
        visit(k);
      }
    };
    visit(root);
    return best;
  }

  /**
   * An instance path to `target` that stays where the reader is: through the
   * deepest element of `near` whose interior holds `target` (the call it was
   * reached through), else the target's own definition site.
   */
  instancePath(target: number, near: number[] | null): number[] {
    const canonical = this.canonicalPath(target);
    if (!near) return canonical;
    for (let i = near.length - 1; i >= 0; i--) {
      const n = this.nodes[near[i]];
      const owner = this.owner(n) ?? n;
      const j = canonical.indexOf(owner.id);
      if (j >= 0) return [...near.slice(0, i + 1), ...canonical.slice(j + 1)];
    }
    return canonical;
  }

  search(): SearchEntry[] {
    if (this.index) return this.index;
    const rankOf = (n: SceneNode) => ({ file: 0, notebook: 0, class: 1, function: 2, section: 3, dir: 3, artifact: 4, table: 5, call: 6, symbol: 6, loop: 7, branch: 8, op: 9 } as Record<string, number>)[n.kind] ?? 8;
    this.index = this.nodes
      .filter(n => n.parent !== undefined && !['input', 'output', 'glyph', 'lane'].includes(n.kind) && n.title)
      .map(n => {
        const chain = this.canonicalPath(n.id).slice(1, -1).map(id => this.nodes[id].title).filter(Boolean);
        return {
          id: n.id,
          title: n.title,
          detail: n.subtitle ?? '',
          path: chain.join(' › '),
          haystack: `${n.title} ${n.subtitle ?? ''} ${chain.join(' ')}`.toLowerCase(),
          rank: rankOf(n),
        };
      });
    return this.index;
  }
}

function findSuffix(project: Project, path: string): string | undefined {
  return project.files.find(f => f.path.endsWith(`/${path}`))?.text;
}
