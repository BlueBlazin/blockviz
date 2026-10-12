import { ArrowUpRight, Maximize2, X } from 'lucide-react';
import { useEffect, useMemo, useState, type CSSProperties } from 'react';
import type { SceneNode } from '../core/types';
import type { SceneModel } from '../canvas/model';
import type { Highlights } from '../core/highlights';
import { CodeView, type LineRange } from './CodeView';
import { KIND_LABEL, ResizeHandle, type PanelSize } from './panel';
import type { SceneDiff } from '../canvas/diff';
import { ChangeSteps, describeChange } from './ChangesPanel';
import { DiffView } from './DiffView';

export { clampPanel, DEFAULT_PANEL, type PanelSize } from './panel';

/** Reading a change (a pull request): how the code on screen differs from its base version. */
export interface InspectorChange {
  diff: SceneDiff;
  baseHighlights: Highlights;
  /** The change being read, as its index in `diff.changes` (-1: the block is not one). */
  index: number;
  onStep: (delta: number) => void;
  /** Goes to a change, by its index. */
  onChange: (index: number) => void;
}

function List({ label, values, mono = true }: { label: string; values?: string[]; mono?: boolean }) {
  if (!values?.length) return null;
  return (
    <div className="insp-field">
      <label>{label}</label>
      <div className="chips">{values.map((v, i) => <span key={i} className={mono ? 'chip mono' : 'chip'}>{v}</span>)}</div>
    </div>
  );
}

/** What a change did to a block, and where it is among the changes. */
function ChangeLine({ compare, node }: { compare: InspectorChange; node: SceneNode }) {
  const { diff } = compare;
  const status = diff.status(node.id);
  const inside = diff.inside(node.id);
  const through = inside ? 0 : diff.through(node.id);
  const plural = (n: number) => (n === 1 ? '' : 's');
  const says = status === 'added' ? 'New in this change'
    : inside ? `${inside} change${plural(inside)} inside`
    : through ? `Leads into code with ${through} change${plural(through)}`
    : status ? 'Its code changed' : 'Unchanged';
  const removed = diff.removedIn(node.id);
  return (
    <>
      <div className="change-status">
        {status && <span className={`change-badge ${status}`}>{status === 'added' ? 'Added' : 'Changed'}</span>}
        <span>{says}</span>
      </div>
      <ChangeSteps index={compare.index} total={diff.changes.length} onStep={compare.onStep} />
      {removed.length > 0 && (
        <div className="insp-field">
          <label>Removed from here <span className="count">{removed.length}</span></label>
          <div className="uses changes">
            {removed.map(c => {
              const d = describeChange(diff, c);
              return (
                <button key={c.id} onClick={() => compare.onChange(diff.indexOf(c.id, true))}>
                  <span className="use-name"><span className="change-mark removed">−</span>{d.name}<span className="dim"> · {d.kind.toLowerCase()}</span></span>
                  {d.line !== undefined && <span className="use-at">{d.line}</span>}
                </button>
              );
            })}
          </div>
        </div>
      )}
    </>
  );
}

/** What a use is, when it is not a call. */
const USE_KIND: Record<string, string> = { symbol: 'component', class: 'methods', file: 'imports' };

/** The places that use a block, each a link to it. */
function UsedBy({ model, id, onGo, onPeek }: { model: SceneModel; id: number; onGo: (id: number) => void; onPeek: (id: number | null) => void }) {
  const [all, setAll] = useState(false);
  const users = model.users(id);
  if (!users.length) return null;
  const shown = all ? users : users.slice(0, 8);
  return (
    <div className="insp-field">
      <label>Used by {users.length > 1 ? <span className="count">{users.length}</span> : null}</label>
      <div className="uses" onMouseLeave={() => onPeek(null)}>
        {shown.map(u => {
          const n = model.node(u);
          const where = model.where(u);
          const file = model.fileOf(n);
          const kind = USE_KIND[n.kind];
          return (
            <button key={u} onClick={() => onGo(u)} onMouseEnter={() => onPeek(u)} title={n.code ?? n.title}>
              <span className="use-name">{where?.name ?? n.title}{kind ? <span className="dim"> · {kind}{n.title !== model.node(id).title ? ` ${n.title}` : ''}</span> : null}</span>
              {file && n.span && <span className="use-at">{file.split('/').pop()}:{n.span.line}</span>}
            </button>
          );
        })}
      </div>
      {users.length > shown.length && <button className="more" onClick={() => setAll(true)}>Show all {users.length}</button>}
    </div>
  );
}

export function Inspector({ model, highlights, node, hover, size, compare, onResize, onClose, onZoom, onGo, onPick, onPeek }: {
  model: SceneModel;
  highlights: Highlights;
  node: SceneNode;
  /** When reading a change: its block's status, the change steps, and code shown as it changed. */
  compare?: InspectorChange;
  /** The block under the pointer in the diagram: its lines are tinted. */
  hover: number | null;
  size: PanelSize;
  onResize: (size: PanelSize) => void;
  onClose: () => void;
  onZoom: () => void;
  onGo: (id: number) => void;
  /** A line of the source was clicked: select the block drawn for it. */
  onPick: (id: number) => void;
  /** The pointer is on a line: point out its block in the diagram. */
  onPeek: (id: number | null) => void;
}) {
  const owner = model.owner(node);
  const target = node.link ?? node.ref;
  const def = target !== undefined && target !== node.id ? model.node(target) : null;
  const span = node.span;
  const file = model.fileOf(node);
  // A step is read in the definition around it, its own lines marked.
  const view = useMemo(() => model.sourceView(node), [model, node]);
  // A block pointed out from this panel stops being pointed out with it.
  useEffect(() => () => onPeek(null), [node.id]);
  const hasSource = !!view && !!model.fileBytes(view.span.file);
  const marked: LineRange | null = view && span && view.root !== node ? [span.line, span.endLine] : null;
  const h = hover !== null ? model.node(hover) : null;
  const hovered: LineRange | null = view && h?.span && h.id !== view.root.id && h.span.file === view.span.file
    && h.span.start >= view.span.start && h.span.end <= view.span.end ? [h.span.line, h.span.endLine] : null;
  const where = view && view.root !== node && view.root.kind !== 'file' ? view.root.title : null;
  // In a change, code is shown as it changed: the whole definition around the block, folded, its lines kept.
  const diff = compare?.diff;
  const changed = !!diff && !!view && (diff.status(view.root.id) !== null || diff.inside(view.root.id) > 0);
  const baseRoot = changed ? diff!.baseOf(view!.root.id) : undefined;
  const baseSpan = baseRoot !== undefined ? diff!.base.node(baseRoot).span : undefined;
  const rootSpan = view?.root.span ?? view?.span;
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
        <div className="insp-head">
          <header>
            <span className="insp-kind">{kind}{node.subtitle ? <> · <span>{node.subtitle}</span></> : null}</span>
            <button className="icon" aria-label="Close details" onClick={onClose}><X size={15} /></button>
          </header>
          <h2 className={node.kind === 'op' ? 'mono' : undefined}>{node.title || kind}</h2>
        </div>
        {node.doc && <p className="insp-doc">{node.doc}</p>}
        {compare && <ChangeLine compare={compare} node={node} />}
        <div className="insp-actions">
          {owner && <button onClick={onZoom}><Maximize2 size={13} />Zoom in</button>}
          {def && <button onClick={() => onGo(def.id)}><ArrowUpRight size={13} />Go to {def.kind === 'class' ? 'class' : 'definition'}<span className="dim">{def.title}</span></button>}
        </div>
        <UsedBy key={node.id} model={model} id={node.id} onGo={onGo} onPeek={onPeek} />
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
            <label>{file ? <>{file}{span ? <span className="dim">{where ? ` · in ${where}` : ''} · {span.line === span.endLine ? `line ${span.line}` : `lines ${span.line}–${span.endLine}`}</span> : null}</> : 'Code'}</label>
            {hasSource && changed && rootSpan
              ? <DiffView
                  head={{ model, highlights, span: rootSpan }}
                  base={baseSpan && diff!.base.fileBytes(baseSpan.file) ? { model: diff!.base, highlights: compare!.baseHighlights, span: baseSpan } : null}
                  marked={marked}
                  hovered={hovered}
                  onLine={line => onPick(model.blockAt(view!.root, line).id)}
                  onLineHover={line => {
                    const b = line === null ? null : model.blockAt(view!.root, line);
                    onPeek(b && b !== view!.root ? b.id : null);
                  }}
                />
              : hasSource
              ? <CodeView
                  model={model}
                  highlights={highlights}
                  span={view!.span}
                  marked={marked}
                  hovered={hovered}
                  onLine={line => onPick(model.blockAt(view!.root, line).id)}
                  onLineHover={line => {
                    const b = line === null ? null : model.blockAt(view!.root, line);
                    onPeek(b && b !== view!.root ? b.id : null);
                  }}
                />
              : <pre className="code plain">{dedent(node.code!)}</pre>}
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
