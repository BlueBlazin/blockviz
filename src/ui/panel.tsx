import { useRef } from 'react';
import { NARROW } from './media';

/** What each kind of block is called. */
export const KIND_LABEL: Record<string, string> = {
  repo: 'Repository', dir: 'Folder', file: 'File', notebook: 'Notebook', doc: 'Document', asset: 'Asset',
  class: 'Class', function: 'Function', section: 'Section', loop: 'Loop', branch: 'Branch', lane: 'Branch arm',
  group: 'Group', call: 'Call', op: 'Step', glyph: 'Operator', table: 'Settings', artifact: 'File on disk',
  symbol: 'Definition', input: 'Input', output: 'Output',
};

export interface PanelSize { w: number; h: number }

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
export function ResizeHandle({ size, onResize }: { size: PanelSize; onResize: (s: PanelSize) => void }) {
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

