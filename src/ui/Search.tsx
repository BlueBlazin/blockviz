import { Search as SearchIcon, X } from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import { findEntries, type SceneModel } from '../canvas/model';
import { TOUCH, useMedia } from './media';

export function Search({ model, onPick, onClose }: { model: SceneModel; onPick: (id: number) => void; onClose: () => void }) {
  const [query, setQuery] = useState('');
  const [active, setActive] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  const touch = useMedia(TOUCH);
  const list = useRef<HTMLDivElement>(null);
  useEffect(() => input.current?.focus(), []);
  const results = useMemo(() => findEntries(model.search(), query), [query, model]);
  useEffect(() => setActive(0), [query]);
  useEffect(() => {
    list.current?.querySelector<HTMLElement>('[aria-selected="true"]')?.scrollIntoView({ block: 'nearest' });
  }, [active]);
  const pick = (i: number) => {
    const r = results[i];
    if (r) onPick(r.id);
  };
  return (
    <div className="scrim" onMouseDown={e => { if (e.target === e.currentTarget) onClose(); }}>
      <div className="palette" role="dialog" aria-modal="true" aria-label="Search">
        <label className="palette-input">
          <SearchIcon size={16} />
          <input
            ref={input}
            value={query}
            placeholder="Jump to a file, class, function or block…"
            aria-label="Search"
            onChange={e => setQuery(e.target.value)}
            onKeyDown={e => {
              if (e.key === 'ArrowDown') { e.preventDefault(); setActive(a => Math.min(a + 1, results.length - 1)); }
              if (e.key === 'ArrowUp') { e.preventDefault(); setActive(a => Math.max(a - 1, 0)); }
              if (e.key === 'Enter') pick(active);
              if (e.key === 'Escape') onClose();
            }}
          />
          {touch ? <button type="button" className="icon" aria-label="Close search" onClick={onClose}><X size={16} /></button> : <kbd>esc</kbd>}
        </label>
        <div className="palette-results" ref={list} role="listbox">
          {results.length === 0 && <div className="palette-empty">No matches</div>}
          {results.map((r, i) => (
            <button key={r.id} role="option" aria-selected={i === active} onMouseEnter={() => setActive(i)} onClick={() => pick(i)}>
              <span className="pr-title">{r.title}{r.detail && <span className="pr-detail">{r.detail}</span>}</span>
              <span className="pr-path">{r.path}</span>
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
