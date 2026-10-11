import { describe, expect, it } from 'vitest';
import { codeLines } from './code';

const enc = (s: string) => new TextEncoder().encode(s);
const text = (r: ReturnType<typeof codeLines>) => r.lines.map(l => l.segs.map(s => s.text).join(''));

describe('code lines', () => {
  const src = 'class A:\n    def f(self):\n        x = self.attn(x)\n        return x\n';
  const bytes = enc(src);

  it('shows whole lines, dedented, numbered from the span', () => {
    const start = src.indexOf('def');
    const r = codeLines(bytes, null, { start, end: src.length, line: 2 });
    expect(text(r)).toEqual(['def f(self):', '    x = self.attn(x)', '    return x']);
    expect(r.lines.map(l => l.no)).toEqual([2, 3, 4]);
  });

  it('dims the rest of the statement around a call', () => {
    const start = src.indexOf('self.attn');
    const end = start + 'self.attn(x)'.length;
    const r = codeLines(bytes, null, { start, end, line: 3 });
    expect(text(r)).toEqual(['x = self.attn(x)']);
    expect(r.lines[0].segs.map(s => [s.text, s.dim])).toEqual([['x = ', true], ['self.attn(x)', false]]);
  });

  it('applies syntax runs and splits them at line ends', () => {
    // "a = 'x'\nb\n": plain "a = ", string "'x'", plain "\nb\n".
    const s = "a = 'x'\nb\n";
    const runs = new Uint32Array([4, 0, 3, 2, 3, 0]);
    const r = codeLines(enc(s), runs, { start: 0, end: s.length, line: 1 });
    expect(r.lines[0].segs).toEqual([{ text: 'a = ', cls: 0, dim: false }, { text: "'x'", cls: 2, dim: false }]);
    expect(text(r)).toEqual(["a = 'x'", 'b']);
  });

  it('keeps multi-byte characters whole', () => {
    const s = 'π = "日本"\n';
    const b = enc(s);
    const r = codeLines(b, new Uint32Array([5, 0, 8, 2, 1, 0]), { start: 0, end: b.length, line: 1 });
    expect(r.lines[0].segs.map(x => x.text)).toEqual(['π = ', '"日本"']);
  });

  it('caps very long spans', () => {
    const s = Array.from({ length: 50 }, (_, i) => `l${i}`).join('\n');
    const r = codeLines(enc(s), null, { start: 0, end: s.length, line: 1 }, 10);
    expect(r.lines.length).toBe(10);
    expect(r.more).toBe(40);
  });
});
