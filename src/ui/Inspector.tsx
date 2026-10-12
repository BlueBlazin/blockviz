import { ArrowUpRight, Maximize2, X } from 'lucide-react';
import { useRef, type CSSProperties } from 'react';
import type { SceneNode } from '../core/types';
import type { SceneModel } from '../canvas/model';
import type { Highlights } from '../core/highlights';
import { CodeView } from './CodeView';

export const KIND_LABEL: Record<string, string> = {
  repo: 'Repository', dir: 'Folder', file: 'File', notebook: 'Notebook', doc: 'Document', asset: 'Asset',
  class: 'Class', function: 'Function', section: 'Section', loop: 'Loop', branch: 'Branch', lane: 'Branch arm',
  group: 'Group', call: 'Call', op: 'Step', glyph: 'Operator', table: 'Settings', artifact: 'File on disk',
  symbol: 'Definition', input: 'Input', output: 'Output',
};

function List({ label, values, mono = true }: { label: string; values?: string[]; mono?: boolean }) {
  if (!values?.length) return null;
  return (
    <div className="insp-field">
      <label>{label}</label>
      <div className="chips">{values.map((v, i) => <span key={i} className={mono ? 'chip mono' : 'chip'}>{v}</span>)}</div>
    </div>
  );
}

const PROSE = /\.(md|markdown|mdx|rst|txt|adoc)$/i;

export interface PanelSize { w: number; h: number }

const NARROW = '(max-width: 760px)';
export const DEFAULT_PANEL: PanelSize = { w: 380, h: 0 };

/** Keeps the panel inside the window, leaving room to see the diagram. */
export function clampPanel(size: PanelSize): PanelSize {
  const maxW = Math.max(300, window.innerWidth - 160);
  const maxH = Math.max(180, window.innerHeight - 140);
  return {
    w: Math.round(Math.min(Math.max(size.w || DEFAULT_PANEL.w, 300), maxW)),
    h: size.h ? Math.round(Math.min(Math.max(size.h, 160), maxH)) : 0,
  };
}

/** Drag (or arrow keys) on the panel's inner edge: the left edge beside the
 * diagram, or the top edge when the panel is a bottom sheet on narrow screens. */
function ResizeHandle({ size, onResize }: { size: PanelSize; onResize: (s: PanelSize) => void }) {
  const drag = useRef<{ x: number; y: number; start: PanelSize; narrow: boolean } | null>(null);
  const narrow = () => window.matchMedia(NARROW).matches;
  const sheetHeight = (s: PanelSize) => s.h || Math.round(window.innerHeight * 0.46);
  const end = () => {
    drag.current = null;
    document.body.classList.remove('resizing-x', 'resizing-y');
  };
  return (
    <div
      className="insp-resize"
      role="separator"
      aria-orientation={narrow() ? 'horizontal' : 'vertical'}
      aria-label="Resize details panel"
      aria-valuenow={narrow() ? sheetHeight(size) : size.w}
      tabIndex={0}
      title="Drag to resize · double-click to reset"
      onPointerDown={e => {
        if (e.button !== 0) return;
        e.preventDefault();
        e.currentTarget.setPointerCapture(e.pointerId);
        const n = narrow();
        drag.current = { x: e.clientX, y: e.clientY, start: { w: size.w, h: sheetHeight(size) }, narrow: n };
        document.body.classList.add(n ? 'resizing-y' : 'resizing-x');
      }}
      onPointerMove={e => {
        const d = drag.current;
        if (!d) return;
        onResize(clampPanel(d.narrow ? { w: d.start.w, h: d.start.h - (e.clientY - d.y) } : { w: d.start.w - (e.clientX - d.x), h: d.start.h }));
      }}
      onPointerUp={end}
      onLostPointerCapture={end}
      onDoubleClick={() => onResize(clampPanel(narrow() ? { ...size, h: 0 } : { ...size, w: DEFAULT_PANEL.w }))}
      onKeyDown={e => {
        const step = e.shiftKey ? 96 : 24;
        const n = narrow();
        const grow = n ? (e.key === 'ArrowUp' ? 1 : e.key === 'ArrowDown' ? -1 : 0) : (e.key === 'ArrowLeft' ? 1 : e.key === 'ArrowRight' ? -1 : 0);
        if (!grow) return;
        e.preventDefault();
        e.stopPropagation();
        onResize(clampPanel(n ? { w: size.w, h: sheetHeight(size) + grow * step } : { w: size.w + grow * step, h: size.h }));
      }}
    />
  );
}

export function Inspector({ model, highlights, node, size, onResize, onClose, onZoom, onGo }: {
  model: SceneModel;
  highlights: Highlights;
  node: SceneNode;
  size: PanelSize;
  onResize: (size: PanelSize) => void;
  onClose: () => void;
  onZoom: () => void;
  onGo: (id: number) => void;
}) {
  const owner = model.owner(node);
  const target = node.link ?? node.ref;
  const def = target !== undefined && target !== node.id ? model.node(target) : null;
  const span = node.span;
  const file = model.fileOf(node);
  const hasSource = !!span && !!model.fileBytes(span.file);
  const kind = KIND_LABEL[node.kind] ?? node.kind;
  const effects = node.effects?.filter(e => !e.startsWith('__'));
  return (
    <aside
      className="inspector"
      aria-label="Block details"
      style={{ '--panel-w': `${size.w}px`, ...(size.h ? { '--panel-h': `${size.h}px` } : {}) } as CSSProperties}
    >
      <ResizeHandle size={size} onResize={onResize} />
      <div className="insp-body">
        <header>
          <span className="insp-kind">{kind}{node.subtitle ? <> · <span>{node.subtitle}</span></> : null}</span>
          <button className="icon" aria-label="Close details" onClick={onClose}><X size={15} /></button>
        </header>
        <h2 className={node.kind === 'op' ? 'mono' : undefined}>{node.title || kind}</h2>
        {node.doc && <p className="insp-doc">{node.doc}</p>}
        <div className="insp-actions">
          {owner && <button onClick={onZoom}><Maximize2 size={13} />Zoom in</button>}
          {def && <button onClick={() => onGo(def.id)}><ArrowUpRight size={13} />Go to {def.kind === 'class' ? 'class' : 'definition'}<span className="dim">{def.title}</span></button>}
        </div>
        <List label="Inputs" values={node.inputs} />
        <List label="Outputs" values={node.outputs} />
        <List label="Tags" values={node.badges} mono={false} />
        <List label="Side effects" values={effects} mono={false} />
        {node.rows && node.rows.length > 0 && (
          <div className="insp-field">
            <label>{node.rows.length} entries</label>
            <div className="rows">{node.rows.slice(0, 80).map((r, i) => r.heading ? <div key={i} className="row-heading">{r.key}</div> : <div key={i} className="row"><code>{r.key}</code>{r.value ? <span> = {r.value}</span> : null}</div>)}</div>
          </div>
        )}
        {(hasSource || node.code) && (
          <div className="insp-source">
            <label>{file ? <>{file}{span ? <span className="dim"> · {span.line === span.endLine ? `line ${span.line}` : `lines ${span.line}–${span.endLine}`}</span> : null}</> : 'Code'}</label>
            {hasSource ? <CodeView model={model} highlights={highlights} span={span!} wrap={PROSE.test(file ?? '')} /> : <pre className="code plain">{dedent(node.code!)}</pre>}
          </div>
        )}
      </div>
    </aside>
  );
}

/** Removes common indentation so nested code reads naturally. */
function dedent(text: string): string {
  const lines = text.split('\n');
  if (lines.length < 2) return text;
  const rest = lines.slice(1).filter(l => l.trim());
  const indent = Math.min(...rest.map(l => l.match(/^\s*/)![0].length));
  return [lines[0], ...lines.slice(1).map(l => l.slice(Math.min(indent, l.match(/^\s*/)![0].length)))].join('\n');
}
