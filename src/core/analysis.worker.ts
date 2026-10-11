/// <reference lib="webworker" />
import init, { analyze_project } from '../wasm/blockviz_core';

const ready = init();

self.onmessage = async (event: MessageEvent<{ name: string; files: { path: string; text?: string; bytes: number }[] }>) => {
  try {
    await ready;
    const start = performance.now();
    const result = JSON.parse(analyze_project(JSON.stringify(event.data)));
    self.postMessage({ ...result, elapsed: performance.now() - start });
  } catch (e) {
    self.postMessage({ ok: false, error: `Analysis failed: ${String(e)}` });
  }
};
