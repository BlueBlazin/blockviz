import { ChevronRight } from 'lucide-react';
import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { useWindowWidth } from './media';

export interface Crumb { id: number; i: number; title: string }

/** Long paths keep their first and last steps and fold the middle into a menu,
 * folding more (keeping fewer last steps) while the path is wider than its room;
 * at the last fold, names shrink with an ellipsis. */
const FOLDS = [{ over: 5, keep: 3 }, { over: 3, keep: 2 }, { over: 2, keep: 1 }];

/** The containers the view is in, root first; each goes back out to it. */
export function Breadcrumbs({ crumbs, onGo }: { crumbs: Crumb[]; onGo: (c: Crumb) => void }) {
  const [open, setOpen] = useState(false);
  const menu = useRef<HTMLSpanElement>(null);
  const box = useRef<HTMLDivElement>(null);
  const width = useWindowWidth();
  // Each new path or window width starts from the roomiest folding.
  const path = crumbs.map(c => c.id).join(' ');
  const [folded, setFolded] = useState({ level: 0, path, width });
  const level = folded.path === path && folded.width === width ? folded.level : 0;
  useLayoutEffect(() => {
    const el = box.current;
    if (el && el.scrollWidth > el.clientWidth + 1 && level < FOLDS.length - 1) setFolded({ level: level + 1, path, width });
  });
  useEffect(() => {
    if (!open) return;
    const close = (e: PointerEvent) => { if (!menu.current?.contains(e.target as Node)) setOpen(false); };
    window.addEventListener('pointerdown', close);
    return () => window.removeEventListener('pointerdown', close);
  }, [open]);
  useEffect(() => setOpen(false), [crumbs]);
  const { over, keep } = FOLDS[level];
  const fold = crumbs.length > over;
  const hidden = fold ? crumbs.slice(1, crumbs.length - keep) : [];
  const shown = fold ? [crumbs[0], null, ...crumbs.slice(crumbs.length - keep)] : crumbs;
  return (
    <div className={level === FOLDS.length - 1 ? 'crumbs squeeze' : 'crumbs'} ref={box}>
      {shown.map((c, k) => (
        <span key={c ? `${c.id}-${c.i}` : 'fold'} className="crumb">
          {k > 0 && <ChevronRight size={13} />}
          {c ? (
            <button onClick={() => onGo(c)}>{c.title}</button>
          ) : (
            <span className="crumb-fold" ref={menu}>
              <button aria-label={`${hidden.length} more`} aria-expanded={open} title={hidden.map(h => h.title).join(' › ')} onClick={() => setOpen(v => !v)}>…</button>
              {open && (
                <span className="crumb-menu" role="menu">
                  {hidden.map(h => <button key={`${h.id}-${h.i}`} role="menuitem" onClick={() => { setOpen(false); onGo(h); }}>{h.title}</button>)}
                </span>
              )}
            </span>
          )}
        </span>
      ))}
    </div>
  );
}
