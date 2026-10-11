import type { AnalysisResult, Project } from './types';

/** Latest-request-wins ownership of the analysis worker. Terminating a
 * superseded worker is the only way to cancel synchronous WebAssembly. */
export class Analyzer {
  private worker: Worker | null = null;
  private reject: ((reason: Error) => void) | null = null;

  analyze(project: Project, timeoutMs = 60_000): Promise<AnalysisResult> {
    this.cancel();
    return new Promise((resolve, reject) => {
      this.reject = reject;
      const worker = new Worker(new URL('./analysis.worker.ts', import.meta.url), { type: 'module' });
      this.worker = worker;
      const timer = setTimeout(() => {
        done();
        reject(new Error(`Analysis took longer than ${timeoutMs / 1000}s. Try a smaller folder.`));
      }, timeoutMs);
      const done = () => {
        clearTimeout(timer);
        worker.terminate();
        if (this.worker === worker) {
          this.worker = null;
          this.reject = null;
        }
      };
      worker.onmessage = e => {
        done();
        resolve(e.data as AnalysisResult);
      };
      worker.onerror = () => {
        done();
        reject(new Error('The WebAssembly analyzer failed to start. Reload and try again.'));
      };
      worker.postMessage({ name: project.name, files: project.files });
    });
  }

  cancel() {
    this.worker?.terminate();
    this.worker = null;
    this.reject?.(new Error('cancelled'));
    this.reject = null;
  }
}
