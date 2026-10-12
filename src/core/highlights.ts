import type { Analyzer } from './analyzer';
import type { SceneModel } from '../canvas/model';

/** Syntax runs per file, computed by the core on first view and kept for the
 * most recently viewed files. Views read them with `peek`, and `subscribe`
 * to hear when more arrive. */
export class Highlights {
  /** Every file asked for and kept, least recently viewed first. */
  private pending = new Map<number, Promise<Uint32Array | null>>();
  private done = new Map<number, Uint32Array | null>();
  private listeners = new Set<() => void>();

  constructor(private analyzer: Analyzer, private model: SceneModel, private keep = 24) {}

  /** Already computed runs, without waiting. */
  peek(file: number): Uint32Array | null | undefined {
    return this.done.get(file);
  }

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };

  /** Asks the core for a file's runs, unless it has been asked already; either way the file is now the most recently viewed. */
  request(file: number) {
    const known = this.pending.get(file);
    if (known) {
      this.pending.delete(file);
      this.pending.set(file, known);
      return;
    }
    const path = this.model.scene.files[file]?.path;
    const text = this.model.fileText(file);
    const p = path === undefined || text === undefined ? Promise.resolve(null) : this.analyzer.highlight(path, text);
    this.pending.set(file, p);
    while (this.pending.size > this.keep) {
      const oldest = this.pending.keys().next().value!;
      this.pending.delete(oldest);
      this.done.delete(oldest);
    }
    p.then(
      r => {
        if (this.pending.get(file) !== p) return;
        this.done.set(file, r);
        for (const l of this.listeners) l();
      },
      // Not an answer about the file (the worker was replaced for a new analysis): it is asked again when next shown.
      () => { if (this.pending.get(file) === p) this.pending.delete(file); },
    );
  }
}
