import type { Analyzer } from './analyzer';
import type { SceneModel } from '../canvas/model';

/** Syntax runs per file, computed by the core on first view and kept for the
 * most recently viewed files. */
export class Highlights {
  private pending = new Map<number, Promise<Uint32Array | null>>();
  private done = new Map<number, Uint32Array | null>();

  constructor(private analyzer: Analyzer, private model: SceneModel, private keep = 24) {}

  /** Already computed runs, without waiting. */
  peek(file: number): Uint32Array | null | undefined {
    return this.done.get(file);
  }

  runs(file: number): Promise<Uint32Array | null> {
    const known = this.pending.get(file);
    if (known) return known;
    const path = this.model.scene.files[file]?.path;
    const text = this.model.fileText(file);
    const p = path === undefined || text === undefined
      ? Promise.resolve(null)
      : this.analyzer.highlight(path, text).catch(() => null);
    this.pending.set(file, p);
    void p.then(r => {
      this.done.set(file, r);
      while (this.pending.size > this.keep) {
        const oldest = this.pending.keys().next().value!;
        this.pending.delete(oldest);
        this.done.delete(oldest);
      }
    });
    return p;
  }
}
