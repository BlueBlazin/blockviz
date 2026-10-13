import { describe, expect, it } from 'vitest';
import { DARK, LIGHT } from './theme';

const rgb = (hex: string) => [1, 3, 5].map(i => parseInt(hex.slice(i, i + 2), 16));
const hue = (hex: string) => {
  const [r, g, b] = rgb(hex);
  return Math.atan2(Math.sqrt(3) * (g - b), 2 * r - g - b);
};
const light = (hex: string) => Math.max(...rgb(hex)) + Math.min(...rgb(hex));

describe('the dark theme', () => {
  it('draws a white block as the page, and every role in its own hue, above the page', () => {
    expect(DARK.swatches.op.fill).toBe(DARK.background);
    for (const [role, s] of Object.entries(LIGHT.swatches)) {
      const d = DARK.swatches[role];
      if (light(s.fill) < 2 * 255 - 1) expect(light(d.fill)).toBeGreaterThan(light(DARK.background));
      // Interiors stay nearer the page than fills, as they stay nearer white.
      expect(light(d.tint)).toBeLessThanOrEqual(light(d.fill));
      const [r, g, b] = rgb(s.fill);
      if (Math.max(r, g, b) - Math.min(r, g, b) > 20) expect(Math.abs(hue(d.fill) - hue(s.fill))).toBeLessThan(0.08);
    }
  });
});
