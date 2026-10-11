import type { AnalysisResult, Project } from './types';
import type { Request, Response } from './analysis.worker';

type WithoutId<T> = T extends unknown ? Omit<T, 'id'> : never;

/** Owns the WebAssembly core's worker. Analysis is latest-request-wins:
 * terminating the worker is the only way to cancel synchronous WebAssembly,
 * so a new analysis (or `cancel`) replaces the worker and fails whatever was
 * still pending on it. Between analyses a fresh worker serves highlighting. */
export class Analyzer {
  private worker: Worker | null = null;
  private pending = new Map<number, { resolve: (r: Response) => void; reject: (e: Error) => void }>();
  private nextId = 1;

  analyze(project: Project, timeoutMs = 60_000): Promise<AnalysisResult> {
    this.cancel();
    let timer: ReturnType<typeof setTimeout> | undefined;
    const timeout = new Promise<never>((_, reject) => {
      timer = setTimeout(() => {
        this.cancel(new Error(`Analysis took longer than ${timeoutMs / 1000}s. Try a smaller folder.`));
        reject(new Error(`Analysis took longer than ${timeoutMs / 1000}s. Try a smaller folder.`));
      }, timeoutMs);
    });
    const work = this.call({ kind: 'analyze', name: project.name, description: project.description, files: project.files }).then(r => {
      // WebAssembly memory never shrinks, and analysis can grow it to
      // gigabytes: drop that worker and keep a fresh one for highlighting.
      this.cancel();
      this.spawn();
      return (r as Extract<Response, { kind: 'analyze' }>).result;
    });
    return Promise.race([work, timeout]).finally(() => clearTimeout(timer));
  }

  /** Syntax classes for a file: `[len, class, ...]` runs over its UTF-8 bytes. */
  async highlight(path: string, text: string): Promise<Uint32Array> {
    const r = await this.call({ kind: 'highlight', path, text });
    return (r as Extract<Response, { kind: 'highlight' }>).runs;
  }

  cancel(reason = new Error('cancelled')) {
    this.worker?.terminate();
    this.worker = null;
    for (const p of this.pending.values()) p.reject(reason);
    this.pending.clear();
  }

  private call(body: WithoutId<Request>): Promise<Response> {
    const worker = this.worker ?? this.spawn();
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      worker.postMessage({ ...body, id } as Request);
    });
  }

  private spawn(): Worker {
    const worker = new Worker(new URL('./analysis.worker.ts', import.meta.url), { type: 'module' });
    worker.onmessage = (e: MessageEvent<Response>) => {
      const p = this.pending.get(e.data.id);
      this.pending.delete(e.data.id);
      p?.resolve(e.data);
    };
    worker.onerror = () => {
      if (this.worker === worker) this.cancel(new Error('The WebAssembly analyzer failed to start. Reload and try again.'));
    };
    this.worker = worker;
    return worker;
  }
}
