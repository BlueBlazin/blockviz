import { ChevronRight } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';

export interface Crumb { id: number; i: number; title: string }

/** Long paths keep their first and last steps; the middle folds into a menu. */
const KEEP_END = 3;
const FOLD_OVER = 5;

/** The containers the view is in, root first; each goes back out to it. */
export function Breadcrumbs({ crumbs, onGo }: { crumbs: Crumb[]; onGo: (c: Crumb) => void }) {
  const [open, setOpen] = useState(false);
  const menu = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (e: PointerEvent) => { if (!menu.current?.contains(e.target as Node)) setOpen(false); };
    window.addEventListener('pointerdown', close);
    return () => window.removeEventListener('pointerdown', close);
  }, [open]);
  useEffect(() => setOpen(false), [crumbs]);
  const fold = crumbs.length > FOLD_OVER;
  const hidden = fold ? crumbs.slice(1, crumbs.length - KEEP_END) : [];
  const shown = fold ? [crumbs[0], null, ...crumbs.slice(crumbs.length - KEEP_END)] : crumbs;
  return (
    <div className="crumbs">
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
