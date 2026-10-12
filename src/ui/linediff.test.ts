import { describe, expect, it } from 'vitest';
import { diffLines, fold, type Gap, type LineOp } from './linediff';

/** A diff as text: ` x`, `+x`, `-x`, and `… n` for folded lines. */
const show = (a: string[], b: string[], ops: (LineOp | Gap)[]) => ops.map(o => (o.kind === 'gap' ? `… ${o.count}` : o.kind === 'same' ? ` ${b[o.b]}` : o.kind === 'add' ? `+${b[o.b]}` : `-${a[o.a]}`));

describe('line diff', () => {
  it('finds the shortest edit, deletions before additions', () => {
    const a = ['def f(x):', '    y = g(x)', '    return y'];
    const b = ['def f(x):', '    y = g(x, strict=True)', '    z = h(y)', '    return z'];
    expect(show(a, b, diffLines(a, b))).toEqual([' def f(x):', '-    y = g(x)', '-    return y', '+    y = g(x, strict=True)', '+    z = h(y)', '+    return z']);
  });

  it('keeps lines that moved around edits', () => {
    const a = ['a', 'b', 'c', 'd', 'e'];
    const b = ['a', 'x', 'c', 'd', 'y', 'e'];
    expect(show(a, b, diffLines(a, b))).toEqual([' a', '-b', '+x', ' c', ' d', '+y', ' e']);
  });

  it('handles empty sides', () => {
    expect(show([], ['a'], diffLines([], ['a']))).toEqual(['+a']);
    expect(show(['a'], [], diffLines(['a'], []))).toEqual(['-a']);
    expect(diffLines([], [])).toEqual([]);
  });

  it('folds long unchanged runs, keeping context and the lines asked for', () => {
    const a = Array.from({ length: 20 }, (_, i) => `l${i}`);
    const b = a.map((l, i) => (i === 10 ? 'changed' : l));
    expect(show(a, b, fold(diffLines(a, b), 2))).toEqual(['… 8', ' l8', ' l9', '-l10', '+changed', ' l11', ' l12', '… 7']);
    const kept = fold(diffLines(a, b), 2, o => o.b === 1);
    expect(show(a, b, kept).slice(0, 5)).toEqual([' l0', ' l1', ' l2', ' l3', '… 4']);
  });
});
