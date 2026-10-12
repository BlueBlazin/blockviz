/// <reference lib="webworker" />
import init, { analyze_project, highlight_source } from '../wasm/blockviz_core';
import type { AnalysisResult, InputFile } from './types';

export type Request =
  | { id: number; kind: 'analyze'; name: string; description?: string; files: InputFile[]; sharedTrunks?: boolean }
  | { id: number; kind: 'highlight'; path: string; text: string };

export type Response =
  /** `reply`: an `AnalysisResult` without `elapsed`, as UTF-8 JSON. */
  | { id: number; kind: 'analyze'; reply: Uint8Array; elapsed: number }
  | { id: number; kind: 'highlight'; runs: Uint32Array };

const ready = init();
const scope = self as unknown as DedicatedWorkerGlobalScope;
const encode = (value: unknown) => new TextEncoder().encode(JSON.stringify(value));

scope.onmessage = async (event: MessageEvent<Request>) => {
  const req = event.data;
  await ready.catch(() => undefined);
  if (req.kind === 'analyze') {
    const start = performance.now();
    let reply: Uint8Array;
    try {
      await ready;
      reply = analyze_project(encode({ name: req.name, description: req.description ?? '', files: req.files, shared_trunks: !!req.sharedTrunks }));
    } catch (e) {
      reply = encode({ ok: false, error: `Analysis failed: ${String(e)}` } satisfies AnalysisResult);
    }
    // Moved, not copied: the page parses the scene once, from the bytes.
    scope.postMessage({ id: req.id, kind: 'analyze', reply, elapsed: performance.now() - start } satisfies Response, [reply.buffer]);
  } else {
    let runs: Uint32Array;
    try {
      await ready;
      runs = highlight_source(req.path, req.text);
    } catch {
      // Unhighlighted is always a valid answer.
      runs = new Uint32Array([new TextEncoder().encode(req.text).length, 0]);
    }
    scope.postMessage({ id: req.id, kind: 'highlight', runs } satisfies Response, [runs.buffer]);
  }
};
