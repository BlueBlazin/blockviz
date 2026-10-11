import { describe, expect, it } from 'vitest';
import type { SceneNode } from '../core/types';
import { childXf, frame, parentXf, smoothPath, viewOf, xfOf, zoomAt } from './geometry';

const box = (over: Partial<SceneNode> = {}): SceneNode => ({
  id: 1, kind: 'function', shape: 'box', category: 'function', title: 'f',
  x: 40, y: 60, w: 200, h: 150, inner: { x: 8, y: 30, w: 184, h: 112 }, ...over,
});

describe('placement', () => {
  it('child and parent transforms are inverses', () => {
    const t = { x: 13, y: -7, s: 0.8 };
    const n = box();
    const c = childXf(t, n, 600, 300);
    const back = parentXf(c, n, 600, 300);
    expect(back.x).toBeCloseTo(t.x, 9);
    expect(back.y).toBeCloseTo(t.y, 9);
    expect(back.s).toBeCloseTo(t.s, 12);
  });

  it('fits content inside the inner rect, centred', () => {
    const n = box();
    const c = childXf({ x: 0, y: 0, s: 1 }, n, 368, 112);
    // Width-limited: 184 / 368 = 0.5, content height 56 centred in 112.
    expect(c.s).toBeCloseTo(0.5);
    expect(c.x).toBeCloseTo(48);
    expect(c.y).toBeCloseTo(60 + 30 + 28);
  });

  it('stays precise many levels deep', () => {
    let t = { x: 0, y: 0, s: 1 };
    const n = box();
    for (let i = 0; i < 30; i++) t = childXf(t, n, 600, 300);
    for (let i = 0; i < 30; i++) t = parentXf(t, n, 600, 300);
    expect(t.s).toBeCloseTo(1, 6);
    expect(Math.abs(t.x)).toBeLessThan(1e-3);
  });
});

describe('camera', () => {
  it('zooming keeps the point under the cursor fixed', () => {
    const t = { x: 100, y: 50, s: 2 };
    const z = zoomAt(t, 3, 400, 300);
    const u = (400 - t.x) / t.s;
    expect(z.x + u * z.s).toBeCloseTo(400);
  });

  it('views round-trip through transforms', () => {
    const t = { x: -120, y: 33, s: 1.7 };
    const back = xfOf(viewOf(t, 1200, 800), 1200, 800);
    expect(back.x).toBeCloseTo(t.x);
    expect(back.y).toBeCloseTo(t.y);
    expect(back.s).toBeCloseTo(t.s);
  });

  it('frames a box with padding and the screen aspect', () => {
    const v = frame({ x: 0, y: 0, w: 100, h: 100 }, 1600, 800, 0.1);
    expect(v.cx).toBe(50);
    expect(v.w).toBeCloseTo(200 * 1.2);
  });

  it('smooth paths start and end at the given views', () => {
    const a = { cx: 0, cy: 0, w: 1000 };
    const b = { cx: 5000, cy: -300, w: 20 };
    const p = smoothPath(a, b);
    const s = p.at(0);
    const e = p.at(1);
    expect(s.cx).toBeCloseTo(a.cx);
    expect(s.w).toBeCloseTo(a.w);
    expect(e.cx).toBeCloseTo(b.cx, 3);
    expect(e.w).toBeCloseTo(b.w, 3);
    // Zooms out on the way when travelling far.
    expect(Math.max(...[0.25, 0.5, 0.75].map(t => p.at(t).w))).toBeGreaterThan(a.w);
  });

  it('pure zooms interpolate scale', () => {
    const p = smoothPath({ cx: 1, cy: 1, w: 100 }, { cx: 1, cy: 1, w: 1 });
    expect(p.at(1).w).toBeCloseTo(1);
    expect(p.at(0.5).w).toBeCloseTo(10, 0);
  });
});
