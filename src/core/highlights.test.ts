import { describe, expect, it } from 'vitest';
import type { Analyzer } from './analyzer';
import type { SceneModel } from '../canvas/model';
import { Highlights } from './highlights';

/** A core whose answers the test gives, one call at a time. */
function core() {
  const calls: { path: string; resolve: (r: Uint32Array) => void; reject: (e: Error) => void }[] = [];
  const analyzer = { highlight: (path: string) => new Promise<Uint32Array>((resolve, reject) => calls.push({ path, resolve, reject })) } as unknown as Analyzer;
  const model = { scene: { files: [{ path: 'a.py' }, { path: 'b.py' }, { path: 'c.py' }] }, fileText: () => 'x = 1\n' } as unknown as SceneModel;
  return { calls, analyzer, model };
}

const settle = () => new Promise(r => setTimeout(r, 0));

describe('highlights', () => {
  it('tells subscribers when runs arrive', async () => {
    const { calls, analyzer, model } = core();
    const h = new Highlights(analyzer, model);
    let heard = 0;
    h.subscribe(() => heard++);
    h.request(0);
    h.request(0);
    expect(calls.length).toBe(1);
    expect(h.peek(0)).toBeUndefined();
    calls[0].resolve(Uint32Array.of(6, 0));
    await settle();
    expect(h.peek(0)).toEqual(Uint32Array.of(6, 0));
    expect(heard).toBe(1);
  });

  it('asks again after a failure, rather than keeping the file uncolored', async () => {
    const { calls, analyzer, model } = core();
    const h = new Highlights(analyzer, model);
    h.request(0);
    calls[0].reject(new Error('canceled'));
    await settle();
    expect(h.peek(0)).toBeUndefined();
    h.request(0);
    expect(calls.length).toBe(2);
    calls[1].resolve(Uint32Array.of(6, 0));
    await settle();
    expect(h.peek(0)).toEqual(Uint32Array.of(6, 0));
  });

  it('forgets the least recently viewed file', async () => {
    const { calls, analyzer, model } = core();
    const h = new Highlights(analyzer, model, 2);
    h.request(0);
    h.request(1);
    for (const c of calls) c.resolve(Uint32Array.of(6, 0));
    await settle();
    // a.py is viewed again, so b.py goes when c.py comes.
    h.request(0);
    h.request(2);
    expect(h.peek(0)).toBeDefined();
    expect(h.peek(1)).toBeUndefined();
    expect(calls.map(c => c.path)).toEqual(['a.py', 'b.py', 'c.py']);
  });
});
