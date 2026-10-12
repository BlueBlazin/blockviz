import { X } from 'lucide-react';
import { useEffect, type CSSProperties } from 'react';
import type { Edge } from '../core/types';
import type { SceneModel } from '../canvas/model';
import type { PanelSize } from './Inspector';

/** A step a value passes through, named as the diagram names it. */
function stepName(model: SceneModel, id: number): string {
  const n = model.node(id);
  if (n.kind === 'input') return `input ${n.title}`;
  if (n.kind === 'output') return n.title ? `result ${n.title}` : 'result';
  return n.title || n.kind;
}

/** The definition a diagram belongs to (a branch belongs to its function). */
function definition(model: SceneModel, id: number): number {
  let n = model.node(id);
  while (n.shape === 'region' && n.parent !== undefined) n = model.node(n.parent);
  return n.id;
}

/**
 * A value followed through its diagram: where it is made and every step that
 * reads it, in the order they are drawn. Each step is a link to it.
 */
export function ValuePanel({ model, owner, edges, start, size, onClose, onPick, onPeek }: {
  model: SceneModel;
  owner: number;
  edges: ReadonlySet<Edge>;
  start: Edge;
  size: PanelSize;
  onClose: () => void;
  onPick: (id: number) => void;
  onPeek: (id: number | null) => void;
}) {
  useEffect(() => () => onPeek(null), []);
  const list = [...edges];
  // Ports and branches only pass the value on; the steps are what read and make it.
  const step = (id: number) => { const n = model.node(id); return n.shape !== 'region' && !(n.kind === 'input' && n.parent !== undefined && model.node(n.parent).shape === 'region') && !(n.kind === 'output' && n.parent !== undefined && model.node(n.parent).shape === 'region'); };
  const targets = new Set(list.map(e => e.to));
  // Where it comes from: steps that send it but are not sent it under this name.
  const sources = [...new Set(list.map(e => e.from))].filter(id => !targets.has(id) && step(id));
  const order = (a: number, b: number) => (model.node(a).span?.start ?? 0) - (model.node(b).span?.start ?? 0) || model.node(a).y - model.node(b).y;
  const readers = [...targets].filter(step).sort(order);
  const row = (id: number) => (
    <button key={id} onClick={() => onPick(id)} onMouseEnter={() => onPeek(id)}>
      <span className="use-name">{stepName(model, id)}</span>
      {model.node(id).subtitle && <span className="use-at">{model.node(id).subtitle}</span>}
    </button>
  );
  return (
    <aside className="inspector" aria-label="Value details" style={{ '--panel-w': `${size.w}px`, ...(size.h ? { '--panel-h': `${size.h}px` } : {}) } as CSSProperties}>
      <div className="insp-body">
        <header>
          <span className="insp-kind">Value{start.kind === 'context' ? <> · <span>setting</span></> : null}</span>
          <button className="icon" aria-label="Close details" onClick={onClose}><X size={15} /></button>
        </header>
        <h2 className="mono">{start.label || 'unnamed value'}</h2>
        <p className="insp-doc">In {model.qualified(definition(model, owner))}. Its path is highlighted in the diagram.</p>
        <div className="insp-field">
          <label>Comes from</label>
          <div className="uses" onMouseLeave={() => onPeek(null)}>{sources.map(row)}</div>
        </div>
        <div className="insp-field">
          <label>Flows into <span className="count">{readers.length}</span></label>
          <div className="uses" onMouseLeave={() => onPeek(null)}>{readers.map(row)}</div>
        </div>
      </div>
    </aside>
  );
}
