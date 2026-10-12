import { useLayoutEffect, useRef, useState } from 'react';
import type { SceneModel } from '../canvas/model';
import { KIND_LABEL } from './Inspector';

/** What a block is and what double-clicking it does, shown beside the pointer
 * once it rests on a block: most names are too small to read at most zooms. */
export function Tooltip({ model, id, x, y }: { model: SceneModel; id: number; x: number; y: number }) {
  const box = useRef<HTMLDivElement>(null);
  const [at, setAt] = useState({ left: x + 14, top: y + 18 });
  useLayoutEffect(() => {
    const r = box.current?.getBoundingClientRect();
    if (!r) return;
    // Keep it on screen: flip to the pointer's other side near the edges.
    const left = x + 14 + r.width > window.innerWidth - 8 ? Math.max(8, x - 14 - r.width) : x + 14;
    const top = y + 18 + r.height > window.innerHeight - 8 ? Math.max(8, y - 12 - r.height) : y + 18;
    setAt({ left, top });
  }, [x, y, id]);
  const n = model.node(id);
  const meta = [KIND_LABEL[n.kind] ?? n.kind, n.subtitle, ...(n.badges ?? [])].filter(Boolean).join(' · ');
  const target = n.link ?? n.ref;
  const def = target !== undefined && target !== n.id ? model.node(target) : null;
  const hint = model.owner(n) ? 'Double-click to open' : def ? `Double-click to go to ${def.title}` : '';
  return (
    <div ref={box} className="tooltip" role="tooltip" style={at}>
      <div className={n.kind === 'op' ? 'tt-title mono' : 'tt-title'}>{n.title || KIND_LABEL[n.kind]}</div>
      {meta && <div className="tt-meta">{meta}</div>}
      {hint && <div className="tt-hint">{hint}</div>}
    </div>
  );
}
