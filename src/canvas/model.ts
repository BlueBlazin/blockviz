import type { Project, Scene, SceneNode, Span } from '../core/types';
import { childXf, parentXf, type Xf } from './geometry';

export interface SearchEntry { id: number; title: string; detail: string; path: string; haystack: string; rank: number }

/** Read-only helpers over a laid-out scene. */
export class SceneModel {
  readonly nodes: SceneNode[];
  readonly root: number;
  private encoded = new Map<number, Uint8Array>();
  private texts: (string | undefined)[];
  private index: SearchEntry[] | null = null;

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
    const text = this.texts[span.file];
    if (text === undefined) return null;
    let bytes = this.encoded.get(span.file);
    if (!bytes) {
      bytes = new TextEncoder().encode(text);
      this.encoded.set(span.file, bytes);
    }
    return new TextDecoder().decode(bytes.slice(span.start, span.end));
  }

  /** Line text around a span's start, for context. */
  fileText(file: number): string | undefined {
    return this.texts[file];
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
