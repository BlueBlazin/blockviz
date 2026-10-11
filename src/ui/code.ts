/** Turns a span of a file plus its syntax runs into display lines. */

export interface Seg { text: string; cls: number; dim: boolean }
export interface CodeLine { no: number; segs: Seg[] }

const NL = 10;
const decoder = new TextDecoder();

/**
 * The whole lines a span touches, as classed segments. Text on those lines
 * outside the span is marked `dim`, so a call is shown in its statement.
 * Common indentation is removed. `runs` is `[len, class, ...]` over the
 * file's UTF-8 bytes; without it everything is plain.
 */
export function codeLines(
  bytes: Uint8Array,
  runs: Uint32Array | null,
  span: { start: number; end: number; line: number },
  maxLines = 2000,
): { lines: CodeLine[]; more: number } {
  const start = Math.min(span.start, bytes.length);
  let end = Math.min(Math.max(span.end, start), bytes.length);
  while (end > start && (bytes[end - 1] === NL || bytes[end - 1] === 13)) end--;
  const from = start === 0 ? 0 : bytes.lastIndexOf(NL, start - 1) + 1;
  const nl = bytes.indexOf(NL, end);
  const to = nl < 0 ? bytes.length : nl;

  const lines: CodeLine[] = [{ no: span.line, segs: [] }];
  let full = false;
  // Splits [a, b) at newlines into the current and following lines.
  const emit = (a: number, b: number, cls: number) => {
    let s = a;
    while (s < b && !full) {
      let e = s;
      while (e < b && bytes[e] !== NL) e++;
      if (e > s) {
        const text = decoder.decode(bytes.subarray(s, e));
        lines[lines.length - 1].segs.push({ text: text.endsWith('\r') ? text.slice(0, -1) : text, cls, dim: e <= start || s >= end });
      }
      if (e < b) {
        if (lines.length >= maxLines) full = true;
        else lines.push({ no: lines[lines.length - 1].no + 1, segs: [] });
        s = e + 1;
      } else s = e;
    }
  };
  // Also splits at the span's ends, where dimming starts and stops.
  const cuts = [start, end].filter(c => c > from && c < to);
  const piece = (a: number, b: number, cls: number) => {
    let s = a;
    for (const c of cuts) if (c > s && c < b) { emit(s, c, cls); s = c; }
    emit(s, b, cls);
  };
  if (!runs || runs.length < 2) piece(from, to, 0);
  else {
    let at = 0;
    for (let i = 0; i + 1 < runs.length && at < to; i += 2) {
      const next = at + runs[i];
      if (next > from) piece(Math.max(at, from), Math.min(next, to), runs[i + 1]);
      at = next;
    }
    if (at < to) piece(Math.max(at, from), to, 0);
  }
  const more = full ? countLines(bytes, from, to) - lines.length : 0;
  return { lines: dedent(lines), more };
}

function countLines(bytes: Uint8Array, from: number, to: number): number {
  let n = 1;
  for (let i = from; i < to; i++) if (bytes[i] === NL) n++;
  return n;
}

const leading = (s: string) => s.match(/^[ \t]*/)![0].length;

/** Removes the indentation every non-blank line shares. */
export function dedent(lines: CodeLine[]): CodeLine[] {
  const indentOf = (l: CodeLine) => {
    let n = 0;
    for (const s of l.segs) {
      const ws = leading(s.text);
      n += ws;
      if (ws < s.text.length) return n;
    }
    return null;
  };
  const indents = lines.map(indentOf).filter((n): n is number => n !== null);
  const cut = indents.length ? Math.min(...indents) : 0;
  if (!cut) return lines;
  return lines.map(l => {
    let left = cut;
    const segs: Seg[] = [];
    for (const s of l.segs) {
      if (left === 0) {
        segs.push(s);
        continue;
      }
      const take = Math.min(left, leading(s.text));
      if (take === s.text.length) {
        left -= take;
        continue;
      }
      segs.push(take ? { ...s, text: s.text.slice(take) } : s);
      left = 0;
    }
    return { no: l.no, segs };
  });
}
