import { memo, useEffect, useLayoutEffect, useMemo, useRef, useSyncExternalStore, type CSSProperties, type MouseEvent } from 'react';
import type { Span } from '../core/types';
import type { SceneModel } from '../canvas/model';
import type { Highlights } from '../core/highlights';
import { codeLines, indentWidth, type CodeLine } from './code';
import { useSource, type Source } from './store';

/** Lines `[from, to]` of a file. */
export type LineRange = readonly [number, number];

export const within = (r: LineRange | null | undefined, no: number) => !!r && no >= r[0] && no <= r[1];

/** Subscribes to nothing, which never changes. */
const never = () => () => {};

/** No lines, ever: nothing is hovered. */
export const NO_LINES: Source<LineRange | null> = { get: () => null, subscribe: never };

/**
 * Source of a span with line numbers and syntax colors. Shows plain text
 * until the core has classified the file, which is usually a few ms. Long
 * lines wrap under their own indentation. `marked` lines (the selected
 * block's) are highlighted and scrolled into view, `hovered` ones (the block
 * under the pointer) tinted; clicking or pointing at a line reports it.
 */
export function CodeView({ model, highlights, span, marked, hovered = NO_LINES, onLine, onLineHover }: {
  model: SceneModel;
  highlights: Highlights;
  span: Span;
  marked?: LineRange | null;
  /** Read by each line, so that only the lines it tints or untints re-render. */
  hovered?: Source<LineRange | null>;
  onLine?: (line: number) => void;
  onLineHover?: (line: number | null) => void;
}) {
  const runs = useRuns(highlights, span.file);
  const root = useRef<HTMLPreElement>(null);
  const bytes = model.fileBytes(span.file);
  // By value: the same lines may come as a new span object on each render.
  const { lines, more } = useMemo(() => (bytes ? codeLines(bytes, runs ?? null, span) : { lines: [], more: 0 }), [bytes, runs, span.start, span.end, span.line]);
  const events = useLineEvents(onLine, onLineHover);

  // Bring the selected block's lines into view, with some code above them.
  useLayoutEffect(() => {
    const el = root.current?.querySelector<HTMLElement>('.ln.marked');
    const scroller = root.current?.closest<HTMLElement>('.insp-body');
    if (!el || !scroller) return;
    const top = el.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop;
    const view = scroller.clientHeight;
    if (top < scroller.scrollTop + 40 || top > scroller.scrollTop + view - 60) scroller.scrollTop = Math.max(0, top - view / 3);
  }, [span.file, span.start, span.end, marked?.[0], marked?.[1], lines.length]);

  if (!lines.length) return null;
  const digits = String(lines[lines.length - 1].no).length;
  return (
    <pre ref={root} className={onLine ? 'code interactive' : 'code'} style={{ '--digits': digits } as CSSProperties} {...events}>
      <code>
        {lines.map(l => <Line key={l.no} line={l} marked={within(marked, l.no)} hovered={hovered} />)}
      </code>
      {more > 0 && <div className="code-more">… {more.toLocaleString()} more lines</div>}
    </pre>
  );
}

/**
 * A file's syntax runs, asked for when it is first shown: undefined until
 * the core has classified it, then read with the file, so code is never
 * coloured by another file's runs.
 */
export function useRuns(highlights: Highlights | undefined, file: number | undefined): Uint32Array | null | undefined {
  useEffect(() => { if (file !== undefined) highlights?.request(file); }, [highlights, file]);
  return useSyncExternalStore(highlights?.subscribe ?? never, () => (file === undefined ? undefined : highlights?.peek(file)));
}

/**
 * Clicks and pointing on a code view's lines, handled once for the whole
 * view: each line names itself in `data-line`, so lines need no handlers of
 * their own. Pointing reports a line once, on entering it (null: off the
 * lines, or on one without a number to report).
 */
export function useLineEvents(onLine?: (line: number) => void, onLineHover?: (line: number | null) => void) {
  const last = useRef<number | null>(null);
  const point = (line: number | null) => {
    if (line === last.current) return;
    last.current = line;
    onLineHover?.(line);
  };
  return {
    onClick: (e: MouseEvent) => {
      const line = lineAt(e);
      if (line !== null && !window.getSelection()?.toString()) onLine?.(line);
    },
    onMouseOver: (e: MouseEvent) => point(lineAt(e)),
    onMouseLeave: () => point(null),
  };
}

function lineAt(e: MouseEvent): number | null {
  const el = (e.target as Element).closest<HTMLElement>('[data-line]');
  return el ? Number(el.dataset.line) : null;
}

const Line = memo(function Line({ line, marked, hovered }: { line: CodeLine; marked: boolean; hovered: Source<LineRange | null> }) {
  const tinted = useSource(hovered, r => within(r, line.no));
  // Wrapped parts of a line continue under its code, a little further in.
  const hang = `${indentWidth(line) + 2}ch`;
  return (
    <div className={`ln${marked ? ' marked' : ''}${tinted ? ' hovered' : ''}`} data-line={line.no}>
      <span className="no" aria-hidden="true">{line.no}</span>
      <span className="src" style={{ paddingLeft: hang, textIndent: `-${hang}` }}>{line.segs.map((s, i) => <span key={i} className={classOf(s.cls, s.dim)}>{s.text}</span>)}</span>
    </div>
  );
});

const classOf = (cls: number, dim: boolean) => (cls ? (dim ? `t${cls} faded` : `t${cls}`) : dim ? 'faded' : undefined);
