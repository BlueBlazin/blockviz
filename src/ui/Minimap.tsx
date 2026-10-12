import { useEffect, useMemo, useRef } from 'react';
import type { Engine } from '../canvas/engine';
import type { SceneModel } from '../canvas/model';
import { Renderer } from '../canvas/renderer';
import { theme } from '../canvas/theme';

const WIDTH = 188;
const MAX_HEIGHT = 150;

/**
 * The whole project in miniature with where the view is: the part of it on
 * screen, the top-level block the reader is in, and (when the view is too
 * deep to show as a rectangle) a marker on the spot. Clicking a block flies
 * there.
 */
export function Minimap({ model, engine, view }: { model: SceneModel; engine: Engine; view: unknown }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const root = model.node(model.root);
  const cw = root.cw ?? 1;
  const ch = root.ch ?? 1;
  const height = Math.min(MAX_HEIGHT, Math.max(60, (WIDTH * ch) / cw));
  // Root interior → minimap pixels.
  const fit = useMemo(() => {
    const s = Math.min((WIDTH - 12) / cw, (height - 12) / ch);
    return { x: (WIDTH - cw * s) / 2, y: (height - ch * s) / 2, s };
  }, [cw, ch, height]);

  // The miniature itself is drawn once per project.
  const base = useMemo(() => {
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const c = document.createElement('canvas');
    c.width = Math.round(WIDTH * dpr);
    c.height = Math.round(height * dpr);
    const ctx = c.getContext('2d', { alpha: false })!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    new Renderer(ctx, model).draw({ width: WIDTH, height, hover: null, selected: null, path: [model.root], xfs: [fit] });
    return c;
  }, [model, fit, height]);

  useEffect(() => {
    const c = canvas.current;
    if (!c) return;
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    c.width = Math.round(WIDTH * dpr);
    c.height = Math.round(height * dpr);
    const ctx = c.getContext('2d')!;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.drawImage(base, 0, 0);
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.fillStyle = 'rgba(255,255,255,0.35)';
    ctx.fillRect(0, 0, WIDTH, height);
    const { xf, width, height: h, path } = engine.overview();
    const to = (u: number, v: number) => ({ x: fit.x + u * fit.s, y: fit.y + v * fit.s });
    // The top-level block the view is in.
    const top = path[1] !== undefined ? model.node(path[1]) : null;
    if (top) {
      const a = to(top.x, top.y);
      ctx.strokeStyle = theme.accent;
      ctx.lineWidth = 1.5;
      ctx.strokeRect(a.x, a.y, top.w * fit.s, top.h * fit.s);
    }
    // What is on screen, in root coordinates.
    const a = to(-xf.x / xf.s, -xf.y / xf.s);
    const b = to((width - xf.x) / xf.s, (h - xf.y) / xf.s);
    const rw = b.x - a.x;
    const rh = b.y - a.y;
    ctx.fillStyle = 'rgba(37, 99, 235, 0.12)';
    ctx.strokeStyle = theme.accent;
    ctx.lineWidth = 1.2;
    if (rw >= 8 && rh >= 6) {
      ctx.fillRect(a.x, a.y, rw, rh);
      ctx.strokeRect(a.x, a.y, rw, rh);
    } else {
      // Too deep to draw as a rectangle: a ring on the spot.
      const cx = a.x + rw / 2;
      const cy = a.y + rh / 2;
      ctx.beginPath();
      ctx.arc(cx, cy, 5, 0, Math.PI * 2);
      ctx.fill();
      ctx.stroke();
      ctx.beginPath();
      ctx.arc(cx, cy, 1.5, 0, Math.PI * 2);
      ctx.fillStyle = theme.accent;
      ctx.fill();
    }
  }, [base, engine, fit, height, model, view]);

  return (
    <canvas
      ref={canvas}
      className="minimap"
      style={{ width: WIDTH, height }}
      aria-label="Overview of the whole project; click a block to go there"
      onClick={e => {
        const r = e.currentTarget.getBoundingClientRect();
        const u = (e.clientX - r.left - fit.x) / fit.s;
        const v = (e.clientY - r.top - fit.y) / fit.s;
        const hit = (root.children ?? []).map(id => model.node(id)).find(n => u >= n.x && u <= n.x + n.w && v >= n.y && v <= n.y + n.h);
        if (hit) engine.flyTo([model.root, hit.id], 'fit');
        else engine.home();
      }}
    />
  );
}
