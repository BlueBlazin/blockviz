import { ArrowUpRight, Maximize2, X } from 'lucide-react';
import type { SceneNode } from '../core/types';
import type { SceneModel } from '../canvas/model';

const KIND_LABEL: Record<string, string> = {
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

export function Inspector({ model, node, onClose, onZoom, onGo }: {
  model: SceneModel;
  node: SceneNode;
  onClose: () => void;
  onZoom: () => void;
  onGo: (id: number) => void;
}) {
  const owner = model.owner(node);
  const target = node.link ?? node.ref;
  const def = target !== undefined && target !== node.id ? model.node(target) : null;
  const span = node.span;
  const file = model.fileOf(node);
  const code = span ? model.source(span) : null;
  const shownCode = code && code.length < 6000 ? code : code ? `${code.slice(0, 6000)}\n…` : node.code;
  const kind = KIND_LABEL[node.kind] ?? node.kind;
  const effects = node.effects?.filter(e => !e.startsWith('__'));
  return (
    <aside className="inspector" aria-label="Block details">
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
      {shownCode && (
        <div className="insp-source">
          <label>{file ? <>{file}{span ? <span className="dim"> · {span.line === span.endLine ? `line ${span.line}` : `lines ${span.line}–${span.endLine}`}</span> : null}</> : 'Code'}</label>
          <pre>{dedent(shownCode)}</pre>
        </div>
      )}
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
