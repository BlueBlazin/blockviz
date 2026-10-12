import { ChevronLeft, ChevronRight, ExternalLink, X } from 'lucide-react';
import { useEffect, useMemo, useRef, type CSSProperties } from 'react';
import type { Change, SceneDiff, Status } from '../canvas/diff';
import type { SceneModel } from '../canvas/model';
import type { Highlights } from '../core/highlights';
import type { CompareInfo, SceneNode } from '../core/types';
import { DiffView } from './DiffView';
import { TOUCH, useMedia } from './media';
import { KIND_LABEL, ResizeHandle, type PanelSize } from './panel';

/** A block's name as a list shows it: its title, or for an untitled branch what its first arm tests. */
export function blockName(model: SceneModel, n: SceneNode): string {
  if (n.title) return n.title;
  const arm = (n.children ?? []).map(c => model.node(c)).find(c => c.kind === 'lane' && c.title);
  return arm?.title ?? KIND_LABEL[n.kind] ?? n.kind;
}

/** The file a block's code is in. */
function fileOf(model: SceneModel, n: SceneNode): string | undefined {
  for (let c: SceneNode | undefined = n; c; c = c.parent === undefined ? undefined : model.node(c.parent)) if (c.span) return model.fileOf(c);
  return undefined;
}

const OUTER = new Set(['repo', 'dir', 'file', 'notebook', 'doc', 'asset', 'group']);

/** How a change is listed: its block's name, the definition it is in, its kind, and where its code is. */
export function describeChange(diff: SceneDiff, c: Change) {
  const model = c.status === 'removed' ? diff.base : diff.head;
  const n = model.node(c.id);
  // What it is in, unless that is just the repository.
  const w = OUTER.has(n.kind) ? null : model.where(c.id);
  const where = w && w.at !== model.root ? w : null;
  // A whole file starts at its first line: no line to give.
  return { name: blockName(model, n), context: where && where.name !== n.title ? where.name : null, kind: KIND_LABEL[n.kind] ?? n.kind, file: fileOf(model, n), line: OUTER.has(n.kind) ? undefined : n.span?.line };
}

const MARK: Record<Status, string> = { added: '+', changed: '~', removed: '−' };
const FILE_MARK: Record<string, string> = { added: 'A', modified: 'M', removed: 'D', renamed: 'R' };

/** Steps through the changes: which one this is, and buttons for the previous and next. */
export function ChangeSteps({ index, total, onStep }: { index: number; total: number; onStep: (delta: number) => void }) {
  if (!total) return null;
  return (
    <div className="change-steps">
      <span>{index >= 0 ? <>Change <strong>{index + 1}</strong> of {total}</> : <>{total} change{total === 1 ? '' : 's'} in all</>}</span>
      <button className="icon" aria-label="Previous change (P)" title="Previous change (P)" onClick={() => onStep(-1)}><ChevronLeft size={15} /></button>
      <button className="icon" aria-label="Next change (N)" title="Next change (N)" onClick={() => onStep(1)}><ChevronRight size={15} /></button>
    </div>
  );
}

/**
 * What a pull request (or comparison) changed, in the order the diagram
 * reads: grouped by file, each change a link to it, the one being read
 * marked. Files changed without changing any block (binaries, whitespace,
 * files not analyzed) are listed after.
 */
export function ChangesPanel({ diff, info, size, current, onResize, onClose, onGo, onPeek }: {
  diff: SceneDiff;
  info: CompareInfo;
  size: PanelSize;
  /** The change being read, as its index in `diff.changes` (-1: none). */
  current: number;
  onResize: (size: PanelSize) => void;
  onClose: () => void;
  onGo: (index: number) => void;
  onPeek: (id: number | null) => void;
}) {
  useEffect(() => () => onPeek(null), []);
  const touch = useMedia(TOUCH);
  const list = useRef<HTMLDivElement>(null);
  const { groups, others } = useMemo(() => {
    const groups: { file: string; mark?: string; items: { c: Change; i: number; d: ReturnType<typeof describeChange> }[] }[] = [];
    diff.changes.forEach((c, i) => {
      const d = describeChange(diff, c);
      const file = d.file ?? '';
      let g = groups.find(x => x.file === file);
      if (!g) groups.push((g = { file, mark: FILE_MARK[diff.fileChange(file, c.status === 'removed')?.status ?? ''], items: [] }));
      g.items.push({ c, i, d });
    });
    const listed = new Set(groups.map(g => g.file));
    const others = diff.files.filter(f => !listed.has(f.path) && !(f.previous && listed.has(f.previous)));
    return { groups, others };
  }, [diff]);
  useEffect(() => {
    list.current?.querySelector<HTMLElement>('[aria-current="true"]')?.scrollIntoView({ block: 'nearest' });
  }, [current]);
  const s = diff.summary;
  return (
    <aside className="inspector" aria-label="Changes" style={{ '--panel-w': `${size.w}px`, ...(size.h ? { '--panel-h': `${size.h}px` } : {}) } as CSSProperties}>
      <ResizeHandle size={size} onResize={onResize} />
      <div className="insp-body" ref={list}>
        <div className="insp-head">
          <header>
            <span className="insp-kind">{info.kind === 'pr' ? `Pull request #${info.number}` : 'Comparison'}{info.state ? <> · <span>{info.state}</span></> : null}</span>
            <button className="icon" aria-label="Close changes" onClick={onClose}><X size={15} /></button>
          </header>
          <h2>{info.title}</h2>
        </div>
        <p className="change-refs">
          <code>{info.baseRef}</code> ← <code>{info.headRef}</code>
          <a href={info.url} target="_blank" rel="noreferrer">GitHub<ExternalLink size={11} /></a>
        </p>
        <div className="change-summary">
          <span className="added">{s.added} added</span>
          <span className="changed">{s.changed} changed</span>
          <span className="removed">{s.removed} removed</span>
        </div>
        {diff.changes.length > 0
          ? <p className="insp-doc">In the order the diagram reads. {touch ? 'Tap a change to go to it.' : <>Press <kbd>N</kbd> and <kbd>P</kbd> to step through them.</>}</p>
          : <p className="insp-doc">No block of the analyzed code changed{diff.files.length ? '; the files changed are listed below.' : '.'}</p>}
        {groups.map(g => (
          <div className="insp-field" key={g.file}>
            <label className="change-file">{g.mark && <span className={`file-mark ${g.mark}`}>{g.mark}</span>}{g.file || 'Repository'}</label>
            <div className="uses changes" onMouseLeave={() => onPeek(null)}>
              {g.items.map(({ c, i, d }) => (
                <button key={i} aria-current={i === current} onClick={() => onGo(i)} onMouseEnter={() => onPeek(c.at)}>
                  <span className="use-name"><span className={`change-mark ${c.status}`}>{MARK[c.status]}</span>{d.context && <span className="dim">{d.context} › </span>}{d.name}<span className="dim"> · {d.kind.toLowerCase()}</span></span>
                  {d.line !== undefined && <span className="use-at">{d.line}</span>}
                </button>
              ))}
            </div>
          </div>
        ))}
        {others.length > 0 && (
          <div className="insp-field">
            <label>Also changed <span className="count">{others.length}</span></label>
            <div className="other-files">{others.map(f => <div key={f.path}><span className={`file-mark ${FILE_MARK[f.status]}`}>{FILE_MARK[f.status]}</span>{f.previous ? `${f.previous} → ` : ''}{f.path}</div>)}</div>
          </div>
        )}
      </div>
    </aside>
  );
}

/** A block the change removed: what it was, where it was, and its code as it was. */
export function RemovedDetails({ diff, change, baseHighlights, size, index, onResize, onClose, onShow, onStep }: {
  diff: SceneDiff;
  change: Change;
  baseHighlights: Highlights;
  size: PanelSize;
  index: number;
  onResize: (size: PanelSize) => void;
  onClose: () => void;
  /** Shows the block it was in. */
  onShow: (id: number) => void;
  onStep: (delta: number) => void;
}) {
  const model = diff.base;
  const n = model.node(change.id);
  const d = describeChange(diff, change);
  const span = n.span;
  return (
    <aside className="inspector" aria-label="Removed block" style={{ '--panel-w': `${size.w}px`, ...(size.h ? { '--panel-h': `${size.h}px` } : {}) } as CSSProperties}>
      <ResizeHandle size={size} onResize={onResize} />
      <div className="insp-body">
        <div className="insp-head">
          <header>
            <span className="insp-kind">{d.kind}{n.subtitle ? <> · <span>{n.subtitle}</span></> : null}</span>
            <button className="icon" aria-label="Close details" onClick={onClose}><X size={15} /></button>
          </header>
          <h2 className={n.kind === 'op' ? 'mono' : undefined}>{d.name}</h2>
        </div>
        <div className="change-status"><span className="change-badge removed">Removed</span><span>from <button className="link" onClick={() => onShow(change.at)}>{diff.head.qualified(change.at)}</button></span></div>
        <ChangeSteps index={index} total={diff.changes.length} onStep={onStep} />
        {span && model.fileBytes(span.file) && (
          <div className="insp-source">
            <label>{d.file}<span className="dim"> · {span.line === span.endLine ? `line ${span.line}` : `lines ${span.line}–${span.endLine}`} before</span></label>
            <DiffView base={{ model, highlights: baseHighlights, span }} head={null} />
          </div>
        )}
      </div>
    </aside>
  );
}
