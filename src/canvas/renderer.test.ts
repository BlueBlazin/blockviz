import { describe, expect, it } from 'vitest';
import { fontSize, splitTitle } from './renderer';

describe('titles', () => {
  it('split at the space nearest the middle', () => {
    expect(splitTitle('External packages')).toEqual(['External', 'packages']);
    expect(splitTitle('N_layer, n_head and n_embd are determined')).toEqual(['N_layer, n_head and', 'n_embd are determined']);
    expect(splitTitle('single')).toEqual(['single', '']);
  });
});

describe('font sizes', () => {
  it('lay text out at a few sizes, each within an eighth of an octave of the size asked for', () => {
    expect(fontSize(10)).toBe(10);
    expect(fontSize(5)).toBe(5);
    const sizes = new Set<number>();
    for (let px = 5; px < 10; px *= 1.01) {
      const at = fontSize(px);
      expect(Math.abs(Math.log2(at / px))).toBeLessThanOrEqual(1 / 8 + 1e-9);
      expect(fontSize(at)).toBe(at);
      sizes.add(at);
    }
    // An octave of zoom lays a name out at four sizes (and the next octave's first).
    expect(sizes.size).toBe(5);
  });
});
