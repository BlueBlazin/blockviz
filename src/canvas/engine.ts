import { Camera } from './camera';
import { clamp, type Insets } from './geometry';
import type { SceneModel } from './model';
import { Renderer, type Hit } from './renderer';

export interface ViewState { path: number[]; zoom: number }

export interface EngineEvents {
  onView(state: ViewState): void;
  onSelect(id: number | null, path: number[] | null): void;
  /** The block under the pointer changed (null: none, or the view is moving). */
  onHover?(id: number | null, x: number, y: number): void;
}

const ease = (t: number) => (t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2);

/**
 * Owns the canvas: input → camera → frames. React never runs per frame; the
 * engine renders only when something changed and reports view changes at a
 * modest rate.
 */
export class Engine {
  private ctx: CanvasRenderingContext2D;
  private renderer: Renderer;
  readonly camera: Camera;
  private width = 1;
  private height = 1;
  private dpr = 1;
  private frame = 0;
  private hover: number | null = null;
  private selected: number | null = null;
  private flight: { start: number; duration: number; step: (t: number) => void } | null = null;
  private pointers = new Map<number, { x: number; y: number }>();
  private press: { x: number; y: number; moved: boolean } | null = null;
  private lastEmit = 0;
  private disposed = false;
  /** Still showing the initial whole-project framing (nothing has moved the camera). */
  private framed = true;
  private observer: ResizeObserver;

  constructor(private canvas: HTMLCanvasElement, private model: SceneModel, private events: EngineEvents, insets?: Insets) {
    this.ctx = canvas.getContext('2d', { alpha: false })!;
    this.renderer = new Renderer(this.ctx, model);
    this.camera = new Camera(model);
    if (insets) this.camera.insets = insets;
    this.observer = new ResizeObserver(() => this.resize());
    this.observer.observe(canvas);
    canvas.addEventListener('wheel', this.wheel, { passive: false });
    canvas.addEventListener('pointerdown', this.down);
    canvas.addEventListener('pointermove', this.move);
    canvas.addEventListener('pointerup', this.up);
    canvas.addEventListener('pointercancel', this.cancel);
    canvas.addEventListener('pointerleave', this.leave);
    canvas.addEventListener('dblclick', this.dblclick);
    this.resize();
    this.camera.fitRoot(this.width, this.height);
    // Paint synchronously so an opaque canvas never flashes black.
    cancelAnimationFrame(this.frame);
    this.frame = 0;
    this.draw(performance.now());
  }

  dispose() {
    this.disposed = true;
    cancelAnimationFrame(this.frame);
    this.observer.disconnect();
    const c = this.canvas;
    c.removeEventListener('wheel', this.wheel);
    c.removeEventListener('pointerdown', this.down);
    c.removeEventListener('pointermove', this.move);
    c.removeEventListener('pointerup', this.up);
    c.removeEventListener('pointercancel', this.cancel);
    c.removeEventListener('pointerleave', this.leave);
    c.removeEventListener('dblclick', this.dblclick);
  }

  // ----- commands --------------------------------------------------------------------

  select(id: number | null) {
    this.selected = id;
    this.invalidate();
  }

  /** Screen margins covered by overlays, kept clear when framing. */
  setInsets(insets: Insets) {
    this.camera.insets = insets;
  }

  /** Flies to frame the node at the end of an instance path. */
  flyTo(path: number[], pad?: number) {
    const leaf = !this.model.owner(this.model.node(path[path.length - 1]));
    const plan = this.camera.flight(path, this.width, this.height, pad ?? (leaf ? 0.9 : 0.08));
    this.animate(plan);
  }

  flyToNode(id: number) {
    this.flyTo(this.model.canonicalPath(id));
  }

  /** Back out to the container around the current view. */
  zoomOut() {
    const path = this.camera.path;
    if (path.length <= 1) {
      this.fit();
      return;
    }
    this.flyTo(path.slice(0, -1).length > 1 ? path.slice(0, -1) : [this.model.root], 0.08);
  }

  fit() {
    const path = this.camera.path;
    if (path.length <= 1) this.home();
    else this.flyTo(path, 0.04);
  }

  home() {
    const plan = this.camera.flight([this.model.root], this.width, this.height, 0.05);
    this.animate(plan);
  }

  zoomBy(factor: number) {
    this.flight = null;
    this.camera.zoom(factor, this.width / 2, this.height / 2);
    this.changed();
  }

  panBy(dx: number, dy: number) {
    this.flight = null;
    this.camera.pan(dx, dy);
    this.changed();
  }

  /**
   * The anchor path extended into the container that dominates the view
   * (contains the centre and fills a good part of the screen).
   */
  private focusPath(): number[] {
    const path = this.camera.path.slice();
    let t = this.camera.xf;
    const cx = this.width / 2;
    const cy = this.height / 2;
    const area = this.width * this.height;
    for (let guard = 0; guard < 64; guard++) {
      const owner = this.model.owner(this.model.node(path[path.length - 1]));
      if (!owner) break;
      let next: number | null = null;
      for (const c of owner.children ?? []) {
        const n = this.model.node(c);
        if (!this.model.owner(n)) continue;
        const b = { x: t.x + n.x * t.s, y: t.y + n.y * t.s, w: n.w * t.s, h: n.h * t.s };
        const visible = Math.max(0, Math.min(b.x + b.w, this.width) - Math.max(b.x, 0)) * Math.max(0, Math.min(b.y + b.h, this.height) - Math.max(b.y, 0));
        if (cx >= b.x && cx <= b.x + b.w && cy >= b.y && cy <= b.y + b.h && visible >= area * 0.3) {
          next = c;
          break;
        }
      }
      if (next === null) break;
      const inner = this.model.interior(t, this.model.node(next));
      if (!inner) break;
      t = inner;
      path.push(next);
    }
    return path;
  }

  /** Instance path (root first) of a hit. */
  private pathOf(hitIndex: number): number[] {
    const hits = this.renderer.hits;
    const out: number[] = [];
    for (let i = hitIndex; i >= 0; i = hits[i].parent) out.unshift(hits[i].id);
    return [this.model.root, ...out];
  }

  exportPng(name: string) {
    this.canvas.toBlob(blob => {
      if (!blob) return;
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = `${name}.png`;
      a.click();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
    });
  }

  // ----- frame loop ------------------------------------------------------------------

  private animate(plan: { duration: number; step: (t: number) => void }) {
    this.framed = false;
    if (matchMedia('(prefers-reduced-motion: reduce)').matches) {
      plan.step(1);
      this.changed();
      return;
    }
    this.flight = { start: performance.now(), duration: plan.duration, step: plan.step };
    this.invalidate();
  }

  private resize() {
    const r = this.canvas.getBoundingClientRect();
    const oldW = this.width;
    const oldH = this.height;
    this.width = Math.max(1, r.width);
    this.height = Math.max(1, r.height);
    this.dpr = Math.min(window.devicePixelRatio || 1, 2);
    this.canvas.width = Math.round(this.width * this.dpr);
    this.canvas.height = Math.round(this.height * this.dpr);
    // A canvas created before it had a size (a collapsed pane, a hidden frame)
    // framed the project in one pixel; until the view is moved, keep it framed.
    if (oldW <= 1 || oldH <= 1 || this.framed) {
      this.flight = null;
      this.camera.fitRoot(this.width, this.height);
    } else {
      this.camera.pan((this.width - oldW) / 2, (this.height - oldH) / 2);
      this.camera.reanchor(this.width, this.height);
    }
    this.invalidate();
  }

  private changed() {
    this.framed = false;
    this.camera.reanchor(this.width, this.height);
    this.invalidate();
  }

  private invalidate() {
    if (!this.frame && !this.disposed) this.frame = requestAnimationFrame(this.draw);
  }

  private draw = (now: number) => {
    this.frame = 0;
    if (this.flight) {
      const t = clamp((now - this.flight.start) / this.flight.duration, 0, 1);
      this.flight.step(ease(t));
      if (t >= 1) this.flight = null;
    }
    this.ctx.setTransform(this.dpr, 0, 0, this.dpr, 0, 0);
    this.renderer.draw({
      width: this.width,
      height: this.height,
      hover: this.hover,
      selected: this.selected,
      path: this.camera.path,
      xfs: this.camera.transforms(),
    });
    if (now - this.lastEmit > 120 || !this.flight) {
      this.lastEmit = now;
      this.events.onView({ path: this.focusPath(), zoom: this.camera.transforms()[0].s });
    }
    if (this.flight) this.invalidate();
  };

  // ----- input -----------------------------------------------------------------------

  private point(e: { clientX: number; clientY: number }) {
    const r = this.canvas.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  }

  /** The innermost block under a point that the reader can make out; a block
   * too small to see is passed over for the one around it. */
  private hitAt(x: number, y: number): number {
    const hits = this.renderer.hits;
    let fallback = -1;
    for (let i = hits.length - 1; i >= 0; i--) {
      const b = hits[i].box;
      if (x >= b.x && x <= b.x + b.w && y >= b.y && y <= b.y + b.h) {
        if (hits[i].clear) return i;
        if (fallback < 0) fallback = i;
      }
    }
    return fallback;
  }

  private wheel = (e: WheelEvent) => {
    e.preventDefault();
    this.flight = null;
    this.unhover();
    const p = this.point(e);
    const lineMode = e.deltaMode === 1;
    const dy = lineMode ? e.deltaY * 16 : e.deltaY;
    const dx = lineMode ? e.deltaX * 16 : e.deltaX;
    // Pinch (ctrl+wheel) and classic mouse wheels zoom; two-finger trackpad scroll pans.
    const mouseWheel = lineMode || (dx === 0 && Math.abs(dy) >= 50 && Number.isInteger(dy));
    if (e.ctrlKey || e.metaKey || mouseWheel) {
      const k = e.ctrlKey ? 0.011 : 0.0022;
      this.camera.zoom(Math.exp(-clamp(dy, -160, 160) * k), p.x, p.y);
    } else {
      this.camera.pan(-dx, -dy);
    }
    this.changed();
  };

  private down = (e: PointerEvent) => {
    if (e.button !== 0 && e.button !== 1) return;
    this.flight = null;
    this.unhover();
    this.canvas.setPointerCapture(e.pointerId);
    const p = this.point(e);
    this.pointers.set(e.pointerId, p);
    this.press = { ...p, moved: false };
  };

  private move = (e: PointerEvent) => {
    const p = this.point(e);
    const prev = this.pointers.get(e.pointerId);
    if (!prev) {
      const i = this.hitAt(p.x, p.y);
      const id = i >= 0 ? this.renderer.hits[i].id : null;
      if (id !== this.hover) {
        this.hover = id;
        this.canvas.style.cursor = id === null ? 'grab' : 'pointer';
        this.events.onHover?.(id, e.clientX, e.clientY);
        this.invalidate();
      }
      return;
    }
    if (this.pointers.size === 2) {
      const other = [...this.pointers.entries()].find(([k]) => k !== e.pointerId)![1];
      const before = Math.hypot(prev.x - other.x, prev.y - other.y);
      const after = Math.hypot(p.x - other.x, p.y - other.y);
      if (before > 0) this.camera.zoom(after / before, (p.x + other.x) / 2, (p.y + other.y) / 2);
      this.camera.pan((p.x - prev.x) / 2, (p.y - prev.y) / 2);
    } else {
      this.camera.pan(p.x - prev.x, p.y - prev.y);
      this.canvas.style.cursor = 'grabbing';
    }
    this.pointers.set(e.pointerId, p);
    if (this.press && Math.hypot(p.x - this.press.x, p.y - this.press.y) > 4) this.press.moved = true;
    this.changed();
  };

  private up = (e: PointerEvent) => {
    const p = this.point(e);
    if (this.press && !this.press.moved && this.pointers.size === 1) {
      const i = this.hitAt(p.x, p.y);
      this.selected = i >= 0 ? this.renderer.hits[i].id : null;
      this.events.onSelect(this.selected, i >= 0 ? this.pathOf(i) : null);
      this.invalidate();
    }
    this.pointers.delete(e.pointerId);
    this.press = null;
    this.canvas.style.cursor = this.hover === null ? 'grab' : 'pointer';
  };

  private cancel = (e: PointerEvent) => {
    this.pointers.delete(e.pointerId);
    this.press = null;
  };

  private leave = () => {
    this.unhover();
  };

  /** Forgets the hovered block while the view moves; the next pointer move finds it again. */
  private unhover() {
    if (this.hover === null) return;
    this.hover = null;
    this.events.onHover?.(null, 0, 0);
    this.invalidate();
  }

  private dblclick = (e: MouseEvent) => {
    const p = this.point(e);
    const i = this.hitAt(p.x, p.y);
    if (i >= 0) {
      const n = this.model.node(this.renderer.hits[i].id);
      // A block with nothing inside but a definition elsewhere goes there.
      if (!this.model.owner(n) && n.link !== undefined && n.link !== n.id) {
        this.selected = n.link;
        this.events.onSelect(n.link, this.model.canonicalPath(n.link));
        this.flyToNode(n.link);
      } else {
        this.flyTo(this.pathOf(i));
      }
    } else {
      this.flight = null;
      this.camera.zoom(2, p.x, p.y);
      this.changed();
    }
  };
}
