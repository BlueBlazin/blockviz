import { describe, expect, it } from 'vitest';
import { diffLines, fold, type Gap, type LineOp } from './linediff';

/** A diff as text: ` x`, `+x`, `-x`, and `… n` for folded lines. */
const show = (a: string[], b: string[], ops: (LineOp | Gap)[]) => ops.map(o => (o.kind === 'gap' ? `… ${o.count}` : o.kind === 'same' ? ` ${b[o.b]}` : o.kind === 'add' ? `+${b[o.b]}` : `-${a[o.a]}`));

/** Edits beyond which two texts are shown as replaced whole rather than diffed. */
const MAX_EDITS = 4000;

/** diffLines as first written, keeping every step's reaches on all diagonals: the diff it finds is the one to find. */
function reference(a: string[], b: string[]): LineOp[] {
  let pre = 0;
  while (pre < a.length && pre < b.length && a[pre] === b[pre]) pre++;
  let post = 0;
  while (post < a.length - pre && post < b.length - pre && a[a.length - 1 - post] === b[b.length - 1 - post]) post++;
  const head: LineOp[] = Array.from({ length: pre }, (_, i) => ({ kind: 'same', a: i, b: i }));
  const tail: LineOp[] = Array.from({ length: post }, (_, i) => ({ kind: 'same', a: a.length - post + i, b: b.length - post + i }));
  return [...head, ...middle(a.slice(pre, a.length - post), b.slice(pre, b.length - post), pre), ...tail];
}

function middle(a: string[], b: string[], offset: number): LineOp[] {
  const n = a.length;
  const m = b.length;
  const replaced = (): LineOp[] => [
    ...a.map((_, i) => ({ kind: 'del' as const, a: offset + i, b: -1 })),
    ...b.map((_, i) => ({ kind: 'add' as const, a: -1, b: offset + i })),
  ];
  if (!n || !m) return replaced();
  const max = n + m;
  const v = new Int32Array(2 * max + 2);
  const trace: Int32Array[] = [];
  let found = -1;
  for (let d = 0; d <= Math.min(max, MAX_EDITS); d++) {
    trace.push(v.slice());
    for (let k = -d; k <= d; k += 2) {
      let x = k === -d || (k !== d && v[max + k - 1] < v[max + k + 1]) ? v[max + k + 1] : v[max + k - 1] + 1;
      let y = x - k;
      while (x < n && y < m && a[x] === b[y]) {
        x++;
        y++;
      }
      v[max + k] = x;
      if (x >= n && y >= m) {
        found = d;
        break;
      }
    }
    if (found >= 0) break;
  }
  if (found < 0) return replaced();
  // Walk back through the furthest reaches to recover the edits.
  const ops: LineOp[] = [];
  let x = n;
  let y = m;
  for (let d = found; d > 0; d--) {
    const pv = trace[d];
    const k = x - y;
    const down = k === -d || (k !== d && pv[max + k - 1] < pv[max + k + 1]);
    const pk = down ? k + 1 : k - 1;
    const px = pv[max + pk];
    const py = px - pk;
    while (x > px && y > py) ops.push({ kind: 'same', a: offset + --x, b: offset + --y });
    if (down) ops.push({ kind: 'add', a: -1, b: offset + --y });
    else ops.push({ kind: 'del', a: offset + --x, b: -1 });
  }
  while (x > 0 && y > 0) ops.push({ kind: 'same', a: offset + --x, b: offset + --y });
  ops.reverse();
  return groupEdits(ops);
}

/** Within each run of edits, deletions before additions. */
function groupEdits(ops: LineOp[]): LineOp[] {
  const out: LineOp[] = [];
  let i = 0;
  while (i < ops.length) {
    if (ops[i].kind === 'same') {
      out.push(ops[i++]);
      continue;
    }
    const run: LineOp[] = [];
    while (i < ops.length && ops[i].kind !== 'same') run.push(ops[i++]);
    out.push(...run.filter(o => o.kind === 'del'), ...run.filter(o => o.kind === 'add'));
  }
  return out;
}

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

  it('finds the diff it always found, for any texts', () => {
    // Seeded, so a failure repeats: texts of a few kinds of line (many equally short diffs), and texts edited.
    let seed = 1;
    const random = (n: number) => Math.floor(((seed = (seed * 16807) % 2147483647) / 2147483647) * n);
    const text = (n: number, kinds: number) => Array.from({ length: n }, () => `l${random(kinds)}`);
    const edit = (a: string[]) => a.flatMap(l => [[], [l], [l], [l, 'new'], ['new']][random(5)]);
    for (let i = 0; i < 2000; i++) {
      const a = text(random(40), 1 + random(6));
      const b = i % 2 ? text(random(40), 1 + random(6)) : edit(a);
      expect(diffLines(a, b)).toEqual(reference(a, b));
    }
  });

  it('diffs texts up to 4,000 edits apart, and shows ones further apart as replaced', () => {
    const lines = (prefix: string) => Array.from({ length: 1000 }, (_, i) => `${prefix}${i}`);
    const a = [...lines('a'), 'kept', ...lines('b')];
    const b = [...lines('c'), 'kept', ...lines('d')];
    const kept = (ops: LineOp[]) => ops.filter(o => o.kind === 'same').length;
    expect(kept(diffLines(a, b))).toBe(1);
    expect(kept(diffLines([...a, 'more'], b))).toBe(0);
  });

  it('folds long unchanged runs, keeping context and the lines asked for', () => {
    const a = Array.from({ length: 20 }, (_, i) => `l${i}`);
    const b = a.map((l, i) => (i === 10 ? 'changed' : l));
    expect(show(a, b, fold(diffLines(a, b), 2))).toEqual(['… 8', ' l8', ' l9', '-l10', '+changed', ' l11', ' l12', '… 7']);
    const kept = fold(diffLines(a, b), 2, o => o.b === 1);
    expect(show(a, b, kept).slice(0, 5)).toEqual([' l0', ' l1', ' l2', ' l3', '… 4']);
  });
});
