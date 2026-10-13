import { memo, useLayoutEffect, useMemo, useRef, useState, type CSSProperties } from 'react';
import type { Span } from '../core/types';
import type { SceneModel } from '../canvas/model';
import type { Highlights } from '../core/highlights';
import { codeLines, dedent, indentWidth, lineText, type CodeLine } from './code';
import { NO_LINES, useLineEvents, useRuns, within, type LineRange } from './CodeView';
import { diffLines, fold, type Gap, type LineOp } from './linediff';
import { useSource, type Source } from './store';

/** One version of a block's code: where it is, and how to color it. */
export interface Version { model: SceneModel; highlights: Highlights; span: Span }

/** Longer diffs fold unchanged stretches away. */
const FOLD_OVER = 40;

/** A version's lines, in the colors of `runs` (null: plain). */
const linesOf = (v: Version | null, runs: Uint32Array | null): CodeLine[] => {
  const bytes = v?.model.fileBytes(v.span.file);
  return v && bytes ? codeLines(bytes, runs, v.span, 4000, true).lines : [];
};

/** A version's lines as shown, in its syntax colors, and their text, which is what is compared. */
function useLines(v: Version | null): { lines: CodeLine[]; text: string[] } {
  const runs = useRuns(v?.highlights, v?.span.file);
  const lines = useMemo(() => linesOf(v, runs ?? null), [v?.model, v?.span.file, v?.span.start, v?.span.end, runs]);
  const text = useMemo(() => linesOf(v, null).map(lineText), [v?.model, v?.span.file, v?.span.start, v?.span.end]);
  return { lines, text };
}

/**
 * A block's code as it changed: lines removed from the base version and
 * added in the head, among the ones kept, with syntax colors from each
 * version. Without a base the code is all new; without a head, all removed.
 * Long stretches of unchanged lines fold away (click to unfold), never the
 * `marked` lines (the selected step's, scrolled into view). Clicking or
 * pointing at a line of the head reports its line number.
 */
export function DiffView({ base, head, marked, hovered = NO_LINES, onLine, onLineHover }: {
  base: Version | null;
  head: Version | null;
  marked?: LineRange | null;
  /** Read by each line, so that only the lines it tints or untints re-render. */
  hovered?: Source<LineRange | null>;
  onLine?: (line: number) => void;
  onLineHover?: (line: number | null) => void;
}) {
  const before = useLines(base);
  const after = useLines(head);
  const [unfolded, setUnfolded] = useState<ReadonlySet<string>>(new Set());
  const root = useRef<HTMLPreElement>(null);
  const events = useLineEvents(onLine, onLineHover);
  // Diffed by text alone: neither colors arriving nor the marked lines moving compute it again.
  const ops = useMemo(() => diffLines(before.text, after.text), [before.text, after.text]);
  const { oldLines, newLines } = useMemo(() => {
    // Both versions lose the same indentation, so moved code shows its new depth.
    const cut = dedent([...before.lines, ...after.lines]);
    return { oldLines: cut.slice(0, before.lines.length), newLines: cut.slice(before.lines.length) };
  }, [before.lines, after.lines]);
  const rows = useMemo(() => {
    const keep = (o: LineOp) => o.b >= 0 && within(marked, newLines[o.b].no);
    return ops.length > FOLD_OVER ? fold(ops, 3, keep) : ops;
  }, [ops, newLines, marked?.[0], marked?.[1]]);

  useLayoutEffect(() => {
    const el = root.current?.querySelector<HTMLElement>('.ln.marked') ?? root.current?.querySelector<HTMLElement>('.ln.add, .ln.del');
    const scroller = root.current?.closest<HTMLElement>('.insp-body');
    if (!el || !scroller) return;
    const top = el.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop;
    const view = scroller.clientHeight;
    if (top < scroller.scrollTop + 40 || top > scroller.scrollTop + view - 60) scroller.scrollTop = Math.max(0, top - view / 3);
  }, [head?.span.start, base?.span.start, marked?.[0], marked?.[1], rows.length]);

  if (!ops.length) return null;
  const shown: LineOp[] = [];
  const out: (LineOp | Gap)[] = [];
  for (const r of rows) {
    const key = `${r.a}:${r.b}`;
    if (r.kind === 'gap' && unfolded.has(key)) {
      const i = ops.findIndex(o => o.a === r.a && o.b === r.b);
      out.push(...ops.slice(i, i + r.count));
    } else out.push(r);
  }
  for (const r of out) if (r.kind !== 'gap') shown.push(r);
  const lastNo = Math.max(1, ...shown.map(o => (o.kind === 'del' ? oldLines[o.a].no : newLines[o.b].no)));
  return (
    <pre ref={root} className={`code diff${onLine ? ' interactive' : ''}`} style={{ '--digits': String(lastNo).length } as CSSProperties} {...events}>
      <code>
        {out.map(r => {
          if (r.kind === 'gap') {
            const key = `${r.a}:${r.b}`;
            return (
              <div key={`g${key}`} className="ln gap" onClick={() => setUnfolded(u => new Set([...u, key]))} title="Show these lines">
                <span className="no" aria-hidden="true">⋯</span>
                <span className="sign" />
                <span className="src">{r.count} unchanged lines</span>
              </div>
            );
          }
          const line = r.kind === 'del' ? oldLines[r.a] : newLines[r.b];
          return <Line key={`${r.kind}${r.a}:${r.b}`} line={line} kind={r.kind} marked={r.kind !== 'del' && within(marked, line.no)} hovered={hovered} />;
        })}
      </code>
    </pre>
  );
}

const SIGN = { same: ' ', add: '+', del: '−' };

const Line = memo(function Line({ line, kind, marked, hovered }: { line: CodeLine; kind: LineOp['kind']; marked: boolean; hovered: Source<LineRange | null> }) {
  // A removed line is not in the code on screen: it has no number to report, and is never the hovered block's.
  const head = kind !== 'del';
  const tinted = useSource(hovered, r => head && within(r, line.no));
  const hang = `${indentWidth(line) + 2}ch`;
  return (
    <div className={`ln ${kind}${marked ? ' marked' : ''}${tinted ? ' hovered' : ''}`} data-line={head ? line.no : undefined}>
      <span className="no" aria-hidden="true">{line.no}</span>
      <span className="sign" aria-label={kind === 'same' ? undefined : kind === 'add' ? 'added' : 'removed'}>{SIGN[kind]}</span>
      <span className="src" style={{ paddingLeft: hang, textIndent: `-${hang}` }}>{line.segs.map((s, i) => <span key={i} className={s.cls ? (s.dim ? `t${s.cls} faded` : `t${s.cls}`) : s.dim ? 'faded' : undefined}>{s.text}</span>)}</span>
    </div>
  );
});
