/** One line of a diff: kept, added (only in the new text) or deleted (only in the old). `a` and
 * `b` are its indexes in the old and new lines (-1 where it is not there). */
export interface LineOp { kind: 'same' | 'add' | 'del'; a: number; b: number }

/** Unchanged lines left out of a diff, between two shown ones. */
export interface Gap { kind: 'gap'; a: number; b: number; count: number }

/** Edits beyond which two texts are shown as replaced whole rather than diffed. */
const MAX_EDITS = 4000;

/**
 * The shortest edit from `a` to `b`, line by line (Myers' algorithm, after
 * the lines they share at either end). Deletions come before additions where
 * both are possible, as diffs usually read.
 */
export function diffLines(a: string[], b: string[]): LineOp[] {
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
  // Texts too far apart are often plainly so (a file reindented): no need to diff them to find out.
  if (!n || !m || fewestEdits(a, b) > MAX_EDITS) return replaced();
  const max = n + m;
  const v = new Int32Array(2 * max + 2);
  const trace: Int32Array[] = [];
  let found = -1;
  for (let d = 0; d <= Math.min(max, MAX_EDITS); d++) {
    // The reaches step d starts from; walking back reads them on diagonals -d..d only.
    trace.push(v.slice(max - d, max + d + 1));
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
    const down = k === -d || (k !== d && pv[d + k - 1] < pv[d + k + 1]);
    const pk = down ? k + 1 : k - 1;
    const px = pv[d + pk];
    const py = px - pk;
    while (x > px && y > py) ops.push({ kind: 'same', a: offset + --x, b: offset + --y });
    if (down) ops.push({ kind: 'add', a: -1, b: offset + --y });
    else ops.push({ kind: 'del', a: offset + --x, b: -1 });
  }
  while (x > 0 && y > 0) ops.push({ kind: 'same', a: offset + --x, b: offset + --y });
  ops.reverse();
  return groupEdits(ops);
}

/** Edits any diff from `a` to `b` takes at least: one for each copy of a line the other has fewer of. */
function fewestEdits(a: string[], b: string[]): number {
  const extra = new Map<string, number>();
  for (const l of a) extra.set(l, (extra.get(l) ?? 0) + 1);
  for (const l of b) extra.set(l, (extra.get(l) ?? 0) - 1);
  let edits = 0;
  for (const e of extra.values()) edits += Math.abs(e);
  return edits;
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

/**
 * A diff with long runs of unchanged lines folded to `context` lines either
 * side of a change. Lines `keep` says to show (the selected block's) are
 * never folded away.
 */
export function fold(ops: LineOp[], context = 3, keep: (op: LineOp) => boolean = () => false): (LineOp | Gap)[] {
  const show = ops.map(o => o.kind !== 'same' || keep(o));
  const near = show.slice();
  show.forEach((s, i) => {
    if (!s) return;
    for (let j = Math.max(0, i - context); j <= Math.min(ops.length - 1, i + context); j++) near[j] = true;
  });
  const out: (LineOp | Gap)[] = [];
  for (let i = 0; i < ops.length;) {
    if (near[i]) {
      out.push(ops[i++]);
      continue;
    }
    let j = i;
    while (j < ops.length && !near[j]) j++;
    // Folding one line saves nothing.
    if (j - i === 1) out.push(ops[i]);
    else out.push({ kind: 'gap', a: ops[i].a, b: ops[i].b, count: j - i });
    i = j;
  }
  return out;
}
