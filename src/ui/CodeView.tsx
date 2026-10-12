import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties } from 'react';
import type { Span } from '../core/types';
import type { SceneModel } from '../canvas/model';
import type { Highlights } from '../core/highlights';
import { codeLines, indentWidth, type CodeLine } from './code';

/** Lines `[from, to]` of a file. */
export type LineRange = readonly [number, number];

/**
 * Source of a span with line numbers and syntax colors. Shows plain text
 * until the core has classified the file, which is usually a few ms. Long
 * lines wrap under their own indentation. `marked` lines (the selected
 * block's) are highlighted and scrolled into view, `hovered` ones (the block
 * under the pointer) tinted; clicking or pointing at a line reports it.
 */
export function CodeView({ model, highlights, span, marked, hovered, onLine, onLineHover }: {
  model: SceneModel;
  highlights: Highlights;
  span: Span;
  marked?: LineRange | null;
  hovered?: LineRange | null;
  onLine?: (line: number) => void;
  onLineHover?: (line: number | null) => void;
}) {
  const [runs, setRuns] = useState<Uint32Array | null | undefined>(() => highlights.peek(span.file));
  const root = useRef<HTMLPreElement>(null);
  useEffect(() => {
    let live = true;
    setRuns(highlights.peek(span.file));
    void highlights.runs(span.file).then(r => { if (live) setRuns(r); });
    return () => { live = false; };
  }, [highlights, span.file]);

  const bytes = model.fileBytes(span.file);
  const { lines, more } = useMemo(() => (bytes ? codeLines(bytes, runs ?? null, span) : { lines: [], more: 0 }), [bytes, runs, span]);

  // Bring the selected block's lines into view, with some code above them.
  useLayoutEffect(() => {
    const el = root.current?.querySelector<HTMLElement>('.ln.marked');
    const scroller = root.current?.closest<HTMLElement>('.insp-body');
    if (!el || !scroller) return;
    const top = el.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop;
    const view = scroller.clientHeight;
    if (top < scroller.scrollTop + 40 || top > scroller.scrollTop + view - 60) scroller.scrollTop = Math.max(0, top - view / 3);
  }, [span, marked?.[0], marked?.[1], lines.length]);

  if (!lines.length) return null;
  const digits = String(lines[lines.length - 1].no).length;
  const within = (r: LineRange | null | undefined, no: number) => !!r && no >= r[0] && no <= r[1];
  const interactive = !!onLine;
  return (
    <pre
      ref={root}
      className={interactive ? 'code interactive' : 'code'}
      style={{ '--digits': digits } as CSSProperties}
      onMouseLeave={() => onLineHover?.(null)}
    >
      <code>
        {lines.map(l => (
          <Line key={l.no} line={l} marked={within(marked, l.no)} hovered={within(hovered, l.no)} onLine={onLine} onLineHover={onLineHover} />
        ))}
      </code>
      {more > 0 && <div className="code-more">… {more.toLocaleString()} more lines</div>}
    </pre>
  );
}

const Line = memo(function Line({ line, marked, hovered, onLine, onLineHover }: {
  line: CodeLine;
  marked: boolean;
  hovered: boolean;
  onLine?: (line: number) => void;
  onLineHover?: (line: number | null) => void;
}) {
  // Wrapped parts of a line continue under its code, a little further in.
  const hang = `${indentWidth(line) + 2}ch`;
  return (
    <div
      className={`ln${marked ? ' marked' : ''}${hovered ? ' hovered' : ''}`}
      onClick={onLine && (() => { if (!window.getSelection()?.toString()) onLine(line.no); })}
      onMouseEnter={onLineHover && (() => onLineHover(line.no))}
    >
      <span className="no" aria-hidden="true">{line.no}</span>
      <span className="src" style={{ paddingLeft: hang, textIndent: `-${hang}` }}>{line.segs.map((s, i) => <span key={i} className={classOf(s.cls, s.dim)}>{s.text}</span>)}</span>
    </div>
  );
});

const classOf = (cls: number, dim: boolean) => (cls ? (dim ? `t${cls} faded` : `t${cls}`) : dim ? 'faded' : undefined);
