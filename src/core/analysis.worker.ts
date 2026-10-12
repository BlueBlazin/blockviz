/// <reference lib="webworker" />
import init, { analyze_project, highlight_source } from '../wasm/blockviz_core';
import type { AnalysisResult, InputFile } from './types';

export type Request =
  | { id: number; kind: 'analyze'; name: string; description?: string; files: InputFile[]; sharedTrunks?: boolean }
  | { id: number; kind: 'highlight'; path: string; text: string };

export type Response =
  | { id: number; kind: 'analyze'; result: AnalysisResult }
  | { id: number; kind: 'highlight'; runs: Uint32Array };

const ready = init();
const scope = self as unknown as DedicatedWorkerGlobalScope;

scope.onmessage = async (event: MessageEvent<Request>) => {
  const req = event.data;
  await ready.catch(() => undefined);
  if (req.kind === 'analyze') {
    let result: AnalysisResult;
    try {
      await ready;
      const start = performance.now();
      const input = new TextEncoder().encode(JSON.stringify({ name: req.name, description: req.description ?? '', files: req.files, shared_trunks: !!req.sharedTrunks }));
      result = { ...JSON.parse(new TextDecoder().decode(analyze_project(input))), elapsed: performance.now() - start };
    } catch (e) {
      result = { ok: false, error: `Analysis failed: ${String(e)}` };
    }
    scope.postMessage({ id: req.id, kind: 'analyze', result } satisfies Response);
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
