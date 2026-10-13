import { describe, expect, it } from 'vitest';
import { balance, fontSize, nameParts } from './renderer';

describe('names', () => {
  it('break between words, never before an extension or inside separators', () => {
    expect(nameParts('modeling_flash_attention_utils.py')).toEqual(['modeling_', 'flash_', 'attention_', 'utils.py']);
    expect(nameParts('External packages')).toEqual(['External ', 'packages']);
    expect(nameParts('__init__.py')).toEqual(['__init__.py']);
    expect(nameParts('getUserName')).toEqual(['get', 'User', 'Name']);
    expect(nameParts('HTTPServer')).toEqual(['HTTPServer']);
    expect(nameParts('models/llama')).toEqual(['models/', 'llama']);
  });

  it('go over lines as evenly as their words allow', () => {
    const parts = nameParts('convert_slow_tokenizers_checkpoints_to_fast.py');
    expect(balance(parts, 1, t => t.length).lines).toEqual(['convert_slow_tokenizers_checkpoints_to_fast.py']);
    expect(balance(parts, 2, t => t.length)).toEqual({ lines: ['convert_slow_tokenizers_', 'checkpoints_to_fast.py'], width: 24 });
    expect(balance(parts, 3, t => t.length)).toEqual({ lines: ['convert_slow_', 'tokenizers_', 'checkpoints_to_fast.py'], width: 22 });
    expect(balance(['External ', 'packages'], 2, t => t.length).lines).toEqual(['External', 'packages']);
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
