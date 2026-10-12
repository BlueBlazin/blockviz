/// <reference lib="webworker" />
import init, { analyze_project, highlight_source } from '../wasm/blockviz_core';
import { scenes } from './cache';
import type { AnalysisResult, InputFile } from './types';

export type Request =
  | { id: number; kind: 'analyze'; name: string; description?: string; files: InputFile[]; sharedTrunks?: boolean }
  | { id: number; kind: 'highlight'; path: string; text: string };

export type Response =
  /** `reply`: an `AnalysisResult` without `elapsed`, as UTF-8 JSON. */
  | { id: number; kind: 'analyze'; reply: Uint8Array; elapsed: number }
  | { id: number; kind: 'highlight'; runs: Uint32Array };

const binary = fetch(new URL('../wasm/blockviz_core_bg.wasm', import.meta.url)).then(r => r.arrayBuffer());
const ready = init({ module_or_path: binary });
const scope = self as unknown as DedicatedWorkerGlobalScope;
const encode = (value: unknown) => new TextEncoder().encode(JSON.stringify(value));
const sha256 = async (data: BufferSource) => Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', data)), b => b.toString(16).padStart(2, '0')).join('');
const build = binary.then(sha256);

/** Analysis is a function of its input and of the analyzer: a scene made
 * before by this build from the same input is the answer, without analyzing. */
async function analyze(input: Uint8Array<ArrayBuffer>): Promise<Uint8Array> {
  // Without digests (a page not served securely) nothing is kept.
  const key = await Promise.all([build, sha256(input)]).then(d => d.join(' '), () => undefined);
  const kept = key && await scenes.get(key);
  if (kept) return kept;
  await ready;
  const reply = analyze_project(input);
  if (key) await scenes.put(key, reply);
  return reply;
}

scope.onmessage = async (event: MessageEvent<Request>) => {
  const req = event.data;
  if (req.kind === 'analyze') {
    const start = performance.now();
    let reply: Uint8Array;
    try {
      reply = await analyze(encode({ name: req.name, description: req.description ?? '', files: req.files, shared_trunks: !!req.sharedTrunks }));
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
