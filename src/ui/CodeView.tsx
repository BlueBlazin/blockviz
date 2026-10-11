import { useEffect, useMemo, useState, type CSSProperties } from 'react';
import type { Span } from '../core/types';
import type { SceneModel } from '../canvas/model';
import type { Highlights } from '../core/highlights';
import { codeLines } from './code';

/** Source of a span with line numbers and syntax colors. Shows plain text
 * until the core has classified the file, which is usually a few ms. Prose
 * (`wrap`) wraps; code scrolls sideways. */
export function CodeView({ model, highlights, span, wrap = false }: { model: SceneModel; highlights: Highlights; span: Span; wrap?: boolean }) {
  const [runs, setRuns] = useState<Uint32Array | null | undefined>(() => highlights.peek(span.file));
  useEffect(() => {
    let live = true;
    setRuns(highlights.peek(span.file));
    void highlights.runs(span.file).then(r => { if (live) setRuns(r); });
    return () => { live = false; };
  }, [highlights, span.file]);

  const bytes = model.fileBytes(span.file);
  const { lines, more } = useMemo(() => (bytes ? codeLines(bytes, runs ?? null, span) : { lines: [], more: 0 }), [bytes, runs, span]);
  if (!lines.length) return null;
  const digits = String(lines[lines.length - 1].no).length;
  return (
    <pre className={wrap ? 'code wrap' : 'code'} style={{ '--digits': digits } as CSSProperties}>
      <code>
        {lines.map(l => (
          <div className="ln" key={l.no}>
            <span className="no" aria-hidden="true">{l.no}</span>
            <span className="src">{l.segs.map((s, i) => <span key={i} className={classOf(s.cls, s.dim)}>{s.text}</span>)}</span>
          </div>
        ))}
      </code>
      {more > 0 && <div className="code-more">… {more.toLocaleString()} more lines</div>}
    </pre>
  );
}

const classOf = (cls: number, dim: boolean) => (cls ? (dim ? `t${cls} faded` : `t${cls}`) : dim ? 'faded' : undefined);
