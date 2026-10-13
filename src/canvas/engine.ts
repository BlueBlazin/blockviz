import { Camera, type CameraState, type Framing, type Plan } from './camera';
import { clamp, type Insets, type Xf } from './geometry';
import type { SceneDiff } from './diff';
import type { SceneModel } from './model';
import { Renderer, type EdgeHit, type Paint, type Trace } from './renderer';
import { svgOf } from './svg';
import type { Theme } from './theme';
import { distanceToPolyline } from './trace';

/** An edge, as its container and its index among the container's edges. */
export interface EdgeRef { owner: number; index: number }

export interface ViewState { path: number[] }

export interface EngineEvents {
  /** Where the reader is changed: the containers the view is in (what the breadcrumbs show). */
  onView(state: ViewState): void;
  /** The view moved. */
  onMove?(): void;
  onSelect(id: number | null, path: number[] | null): void;
  /** The block under the pointer changed (null: none, or the view is moving). */
  onHover?(id: number | null, x: number, y: number): void;
  /** A flight to somewhere else is about to start: a step in the reader's history. */
  onJump?(): void;
  /** An edge was clicked: follow its value. */
  onSelectEdge?(edge: EdgeRef): void;
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
  private hoverEdge: EdgeHit | null = null;
  private traced: Trace | null = null;
  /** A block pointed out from outside the canvas (a line of its source). */
  private peeked: number | null = null;
  private selected: number | null = null;
  /** How the code on screen differs from a base version (a pull request), when it is one. */
  private diff: SceneDiff | null = null;
  private flight: { start: number; duration: number; step: (t: number) => void } | null = null;
  private pointers = new Map<number, { x: number; y: number }>();
  /** A press that may still be a tap or click; `touch` for a finger or pen. */
  private press: { x: number; y: number; moved: boolean; touch: boolean } | null = null;
  /** The last tap of a finger, to tell a double tap. */
  private lastTap: { x: number; y: number; t: number } | null = null;
  /** When a double tap last opened a block (a browser may follow it with a dblclick). */
  private tapped = -Infinity;
  private lastEmit = 0;
  /** Where the reader was last reported to be. */
  private emitted: number[] | null = null;
  private disposed = false;
  /** Still showing the initial whole-project framing (nothing has moved the camera). */
  private framed = true;
  private observer: ResizeObserver;

  constructor(private canvas: HTMLCanvasElement, private model: SceneModel, private events: EngineEvents, theme: Theme, insets?: Insets) {
    this.ctx = canvas.getContext('2d', { alpha: false })!;
    this.renderer = new Renderer(this.ctx, model, theme);
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
    canvas.addEventListener('gesturestart', this.gesture);
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
    c.removeEventListener('gesturestart', this.gesture);
  }

  // ----- commands --------------------------------------------------------------------

  select(id: number | null) {
    this.selected = id;
    this.invalidate();
  }

  /** Draws in `theme`'s colours. */
  setTheme(theme: Theme) {
    this.renderer.theme = theme;
    this.invalidate();
  }

  /** Shows how the code differs from a base version (null: it is not a change). */
  setDiff(diff: SceneDiff | null) {
    this.diff = diff;
    this.invalidate();
  }

  /** Follows a value through its diagram (null: stop). */
  trace(t: Trace | null) {
    this.traced = t;
    this.invalidate();
  }

  /** Points out a block, as hovering it would. */
  peek(id: number | null) {
    if (id === this.peeked) return;
    this.peeked = id;
    this.invalidate();
  }

  private relatedCache: { id: number; set: ReadonlySet<number> } | null = null;

  /** The blocks that use `id`, outlined while it is in focus. */
  private related(id: number): ReadonlySet<number> {
    if (this.relatedCache?.id !== id) this.relatedCache = { id, set: new Set(this.model.users(id)) };
    return this.relatedCache.set;
  }

  /** Brings a block into view unless it is already on screen and legible. */
  reveal(path: number[]) {
    const id = path[path.length - 1];
    const i = this.camera.insets;
    const seen = this.renderer.hits.some(h => h.id === id && h.clear
      && h.box.x >= i.left && h.box.y >= i.top && h.box.x + h.box.w <= this.width - i.right && h.box.y + h.box.h <= this.height - i.bottom);
    if (!seen) this.flyTo(path);
  }

  /** Screen margins covered by overlays, kept clear when framing. */
  setInsets(insets: Insets) {
    this.camera.insets = insets;
  }

  /** Flies to the node at the end of an instance path: `open` to read it
   * (the default), `fit` to see all of it. */
  flyTo(path: number[], how: Framing = 'open', pad = 0.08, record = true) {
    this.animate(this.camera.flight(path, this.width, this.height, how, pad), record);
  }

  /** Where the camera is, to come back to with `restore`. */
  state(): CameraState {
    return this.camera.state();
  }

  /** Flies back to a saved camera state (history navigation: not itself a step). */
  restore(state: CameraState) {
    if (!state.path.every(id => this.model.nodes[id])) return;
    this.animate(this.camera.flightTo(state, this.width, this.height), false);
  }

  /** The containers the view is in, root first (what the breadcrumbs show). */
  focus(): number[] {
    return this.focusPath();
  }

  flyToNode(id: number) {
    this.flyTo(this.model.canonicalPath(id));
  }

  /** Frames all of the block the breadcrumbs end at. */
  fit() {
    const path = this.focusPath();
    if (path.length <= 1) this.home();
    else this.flyTo(path, 'fit', 0.04);
  }

  home() {
    const plan = this.camera.flight([this.model.root], this.width, this.height, 'fit', 0.05);
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

  /** Where the reader is, as of when the view last moved. */
  private focused: { path: number[]; xf: Xf; focus: number[] } | null = null;

  /** Where the reader is, worked out when the view moves: a panel opening
   * or closing over a view that stays put does not move the reader. */
  private focusPath(): number[] {
    const { path, xf } = this.camera;
    const f = this.focused;
    if (f && f.xf.x === xf.x && f.xf.y === xf.y && f.xf.s === xf.s && f.path.length === path.length && f.path.every((id, i) => id === path[i])) return f.focus;
    const focus = this.findFocus();
    this.focused = { path: path.slice(), xf: { ...xf }, focus };
    return focus;
  }

  /**
   * The anchor path extended into the container that dominates the view:
   * it contains the centre of what the overlays leave visible, and fills a
   * good part of it, or all of it in one direction (as fitting it does).
   */
  private findFocus(): number[] {
    const path = this.camera.path.slice();
    let t = this.camera.xf;
    const i = this.camera.insets;
    const x0 = i.left;
    const y0 = i.top;
    const x1 = Math.max(this.width - i.right, x0 + this.width * 0.3);
    const y1 = Math.max(this.height - i.bottom, y0 + this.height * 0.3);
    const cx = (x0 + x1) / 2;
    const cy = (y0 + y1) / 2;
    const area = (x1 - x0) * (y1 - y0);
    for (let guard = 0; guard < 64; guard++) {
      const owner = this.model.owner(this.model.node(path[path.length - 1]));
      if (!owner) break;
      let next: number | null = null;
      for (const c of owner.children ?? []) {
        const n = this.model.node(c);
        if (!this.model.owner(n)) continue;
        const b = { x: t.x + n.x * t.s, y: t.y + n.y * t.s, w: n.w * t.s, h: n.h * t.s };
        const w = Math.max(0, Math.min(b.x + b.w, x1) - Math.max(b.x, x0));
        const h = Math.max(0, Math.min(b.y + b.h, y1) - Math.max(b.y, y0));
        const fills = w * h >= area * 0.3 || (w * h >= area * 0.15 && (w >= (x1 - x0) * 0.85 || h >= (y1 - y0) * 0.85));
        if (cx >= b.x && cx <= b.x + b.w && cy >= b.y && cy <= b.y + b.h && fills) {
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

  /** The view as a PNG image. */
  png(): Promise<Blob | null> {
    return new Promise(done => this.canvas.toBlob(done));
  }

  /** The view as SVG, drawn as it is on screen. */
  svg(): string {
    return svgOf(this.width, this.height, pen => new Renderer(pen, this.model, this.renderer.theme).draw(this.paint()));
  }

  // ----- frame loop ------------------------------------------------------------------

  private animate(plan: Plan, record = true) {
    // A flight that goes nowhere (H where the view already is) is not a step back.
    if (record && plan.length > 1e-3) this.events.onJump?.();
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

  /** What is drawn: the view, and what is in focus, followed or changed in it. */
  private paint(): Paint {
    const focus = this.hover ?? this.peeked ?? this.selected;
    return {
      width: this.width,
      height: this.height,
      hover: this.hover ?? this.peeked,
      related: focus === null ? undefined : this.related(focus),
      trace: this.traced,
      hoverEdge: this.hoverEdge?.edge ?? null,
      selected: this.selected,
      path: this.camera.path,
      xfs: this.camera.transforms(),
      diff: this.diff,
    };
  }

  private draw = (now: number) => {
    this.frame = 0;
    if (this.flight) {
      const t = clamp((now - this.flight.start) / this.flight.duration, 0, 1);
      this.flight.step(ease(t));
      if (t >= 1) this.flight = null;
    }
    this.ctx.setTransform(this.dpr, 0, 0, this.dpr, 0, 0);
    this.renderer.draw(this.paint());
    if (now - this.lastEmit > 120 || !this.flight) {
      this.lastEmit = now;
      // Reported only when it changes: not on every frame of a pan or zoom.
      const path = this.focusPath();
      const was = this.emitted;
      if (!was || was.length !== path.length || path.some((id, i) => id !== was[i])) {
        this.emitted = path;
        this.events.onView({ path });
      }
      this.events.onMove?.();
    }
    if (this.flight) this.invalidate();
  };

  // ----- input -----------------------------------------------------------------------

  private point(e: { clientX: number; clientY: number }) {
    const r = this.canvas.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  }

  /** The innermost block under a point that the reader can make out; a block
   * too small to see is passed over for the one around it, unless `any`
   * (double-clicking a speck zooms into it). */
  private hitAt(x: number, y: number, any = false): number {
    const hits = this.renderer.hits;
    let fallback = -1;
    for (let i = hits.length - 1; i >= 0; i--) {
      const b = hits[i].box;
      if (x >= b.x && x <= b.x + b.w && y >= b.y && y <= b.y + b.h) {
        if (any) return this.shown(i);
        if (hits[i].clear) return i;
        if (fallback < 0) fallback = i;
      }
    }
    return fallback;
  }

  /** Hit `i`, or the closed block it is drawn inside: a block in the thumbnail
   * of a block whose name is still on its face is part of that block's picture. */
  private shown(i: number): number {
    const hits = this.renderer.hits;
    let out = i;
    for (let j = hits[i].parent; j >= 0; j = hits[j].parent) if (hits[j].face) out = j;
    return out;
  }

  private wheel = (e: WheelEvent) => {
    e.preventDefault();
    this.flight = null;
    this.unhover();
    const p = this.point(e);
    const dy = e.deltaMode === 1 ? e.deltaY * 16 : e.deltaY;
    // Scrolling zooms; a pinch arrives as ctrl+wheel in finer steps. Dragging pans.
    const k = e.ctrlKey ? 0.011 : 0.0022;
    this.camera.zoom(Math.exp(-clamp(dy, -160, 160) * k), p.x, p.y);
    this.changed();
  };

  private down = (e: PointerEvent) => {
    if (e.button !== 0 && e.button !== 1) return;
    this.flight = null;
    this.unhover();
    this.canvas.setPointerCapture(e.pointerId);
    const p = this.point(e);
    this.pointers.set(e.pointerId, p);
    this.press = { ...p, moved: false, touch: e.pointerType !== 'mouse' };
  };

  private move = (e: PointerEvent) => {
    const p = this.point(e);
    const prev = this.pointers.get(e.pointerId);
    if (!prev) {
      const i = this.hitAt(p.x, p.y);
      const edge = this.edgeAt(p.x, p.y, i);
      const id = edge ? null : i >= 0 ? this.renderer.hits[i].id : null;
      if (edge?.edge !== this.hoverEdge?.edge) {
        this.hoverEdge = edge;
        this.invalidate();
      }
      if (id !== this.hover) {
        this.hover = id;
        this.events.onHover?.(id, e.clientX, e.clientY);
        this.invalidate();
      }
      this.canvas.style.cursor = id === null && !edge ? 'grab' : 'pointer';
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
    // A finger wobbles more than a mouse while tapping.
    if (this.press && Math.hypot(p.x - this.press.x, p.y - this.press.y) > (this.press.touch ? 10 : 4)) this.press.moved = true;
    this.changed();
  };

  private up = (e: PointerEvent) => {
    const p = this.point(e);
    if (this.press && !this.press.moved && this.pointers.size === 1) {
      const i = this.hitAt(p.x, p.y);
      const edge = this.edgeAt(p.x, p.y, i, this.press.touch ? 12 : 5);
      if (this.press.touch && this.doubleTap(p)) {
        // A double tap opens, as a double click does.
        this.tapped = performance.now();
        this.open(p);
      } else if (edge && this.events.onSelectEdge) {
        this.events.onSelectEdge(this.refOf(edge));
      } else {
        this.selected = i >= 0 ? this.renderer.hits[i].id : null;
        this.events.onSelect(this.selected, i >= 0 ? this.pathOf(i) : null);
      }
      this.invalidate();
    }
    this.pointers.delete(e.pointerId);
    this.press = null;
    this.canvas.style.cursor = this.hover === null ? 'grab' : 'pointer';
  };

  /** Whether a tap at `p` is the second of a double tap. */
  private doubleTap(p: { x: number; y: number }): boolean {
    const now = performance.now();
    const last = this.lastTap;
    const double = !!last && now - last.t < 350 && Math.hypot(p.x - last.x, p.y - last.y) < 30;
    this.lastTap = double ? null : { ...p, t: now };
    return double;
  }

  private cancel = (e: PointerEvent) => {
    this.pointers.delete(e.pointerId);
    this.press = null;
  };

  private leave = () => {
    this.unhover();
  };

  /** Forgets the hovered block while the view moves; the next pointer move finds it again. */
  private unhover() {
    if (this.hoverEdge) {
      this.hoverEdge = null;
      this.invalidate();
    }
    if (this.hover === null) return;
    this.hover = null;
    this.events.onHover?.(null, 0, 0);
    this.invalidate();
  }

  /** The edge within `radius` pixels of a point, drawn in the interior of hit `container`
   * (the block under the point, so a block drawn over an edge wins). */
  private edgeAt(x: number, y: number, container: number, radius = 5): EdgeHit | null {
    let best: EdgeHit | null = null;
    let d = radius * radius;
    for (const h of this.renderer.edgeHits) {
      if (h.container !== container) continue;
      const k = distanceToPolyline(x, y, h.pts);
      if (k <= d) { d = k; best = h; }
    }
    return best;
  }

  private refOf(h: EdgeHit): EdgeRef {
    return { owner: h.owner, index: (this.model.node(h.owner).edges ?? []).indexOf(h.edge) };
  }

  private dblclick = (e: MouseEvent) => {
    if (performance.now() - this.tapped < 600) return;
    this.open(this.point(e));
  };

  /** Safari: a pinch on the diagram zooms the diagram, not the page. */
  private gesture = (e: Event) => e.preventDefault();

  /** Opens the block at a point (a double click or tap): zooms into it, or goes to its definition. */
  private open(p: { x: number; y: number }) {
    const i = this.hitAt(p.x, p.y, true);
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
  }
}
