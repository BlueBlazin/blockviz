import type { Edge, SceneNode } from '../core/types';
import { boxOf, clamp, intersects, smoothstep, type Box, type Xf } from './geometry';
import type { SceneModel } from './model';
import { swatch, theme } from './theme';

/** A block drawn this frame. `clear` when the reader can see what it is (its
 * name is legible, or the box is large), so clicks never land on a block
 * too small to make out. */
export interface Hit { id: number; box: Box; parent: number; clear: boolean }

/** An edge drawn this frame: its container (`owner`), the hit of the box
 * whose interior it is in (-1 for the root), and its route on screen. */
export interface EdgeHit { edge: Edge; owner: number; container: number; pts: number[] }

/** A value being followed: the diagrams it is traced in and the edges carrying it. */
export interface Trace { scope: ReadonlySet<number>; edges: ReadonlySet<Edge> }

export interface Paint {
  width: number;
  height: number;
  hover: number | null;
  selected: number | null;
  /** Anchor path (root first) and the interior transform of each element. */
  path: number[];
  xfs: Xf[];
  /** Blocks that use the one in focus (calls of a component or function), outlined. */
  related?: ReadonlySet<number>;
  /** A value followed through its diagram: its edges lit, the rest faded. */
  trace?: Trace | null;
  /** The edge under the pointer. */
  hoverEdge?: Edge | null;
}

const ROW_H = 17;
const HEADING_H = 22;
/** Interior scale at which a box starts / finishes opening. */
const OPEN_FROM = 0.028;
const OPEN_TO = 0.075;

const CODE_KINDS = new Set(['op', 'loop', 'branch']);
/** Boxes whose names are navigation landmarks and stay readable longest. */
const NAMED_KINDS = new Set(['file', 'notebook', 'dir', 'class', 'function', 'section', 'artifact', 'doc', 'table']);

/**
 * Immediate-mode renderer for nested diagrams. Every frame walks only what is
 * visible: containers are culled against the viewport, interiors are entered
 * only when they are large enough to show something, and text fades in by its
 * on-screen size. Hit regions are recorded as a side effect of drawing.
 */
export class Renderer {
  hits: Hit[] = [];
  edgeHits: EdgeHit[] = [];
  private widths = new Map<string, number>();
  private edgeBounds = new WeakMap<Edge, [number, number, number, number]>();
  private quietEdges = new WeakMap<SceneNode, Set<Edge>>();

  constructor(private ctx: CanvasRenderingContext2D, private model: SceneModel) {}

  draw(p: Paint) {
    const ctx = this.ctx;
    ctx.fillStyle = theme.background;
    ctx.fillRect(0, 0, p.width, p.height);
    this.hits = [];
    this.edgeHits = [];
    const root = this.model.node(this.model.root);
    this.interior(root, p.xfs[0], -1, 0, p, [root.id], 1);
  }

  // ----- containers ----------------------------------------------------------------

  private interior(owner: SceneNode, t: Xf, parentHit: number, pathPos: number, p: Paint, chain: number[], alpha: number) {
    if (owner.rows?.length) {
      this.rows(owner, t, p, alpha);
      return;
    }
    const kids = owner.children ?? [];
    const edgeAlpha = alpha * smoothstep(0.035, 0.12, t.s);
    const lw = clamp(1.3 * Math.sqrt(t.s), 0.45, 1.45);
    const focus = p.hover ?? p.selected;
    const focused = focus !== null && kids.includes(focus);
    // A value followed here outshines the block in focus.
    const traced = p.trace?.scope.has(owner.id) ? p.trace.edges : null;
    const lit = (e: Edge): boolean | null => {
      if (e === p.hoverEdge) return true;
      if (traced) return traced.has(e);
      return focused ? e.from === focus || e.to === focus : null;
    };
    if (edgeAlpha > 0.02) {
      const quiet = this.quiet(owner);
      for (const e of owner.edges ?? []) {
        // Settings that fan out everywhere are drawn only for the block in focus.
        if (quiet.has(e) && !lit(e)) continue;
        this.edge(e, t, p, edgeAlpha, lw, lit(e), owner.id, parentHit);
      }
    }
    for (const id of kids) {
      const n = this.model.node(id);
      const b = boxOf(t, n);
      if (!intersects(b, p.width, p.height, 2) || (b.w < 1 && b.h < 1)) continue;
      const nextPos = pathPos >= 0 && p.path[pathPos + 1] === id ? pathPos + 1 : -1;
      this.node(n, b, t, parentHit, nextPos, p, chain, alpha);
    }
    if (edgeAlpha > 0.02 && t.s * 11 >= 4.5) {
      const placed: Box[] = [];
      const quiet = this.quiet(owner);
      // Labels of lit edges are placed first, so they are the ones shown.
      const order = [...(owner.edges ?? [])].sort((a, b) => Number(!!lit(b)) - Number(!!lit(a)));
      for (const e of order) {
        if (quiet.has(e) && !lit(e)) continue;
        this.edgeLabel(e, t, p, edgeAlpha * smoothstep(4.5, 7, t.s * 11), lit(e), placed);
      }
    }
  }

  /** Context edges from sources that feed three or more blocks. */
  private quiet(owner: SceneNode): Set<Edge> {
    let q = this.quietEdges.get(owner);
    if (!q) {
      const fan = new Map<number, number>();
      for (const e of owner.edges ?? []) if (e.kind === 'context') fan.set(e.from, (fan.get(e.from) ?? 0) + 1);
      q = new Set((owner.edges ?? []).filter(e => e.kind === 'context' && (fan.get(e.from) ?? 0) >= 3));
      this.quietEdges.set(owner, q);
    }
    return q;
  }

  private node(n: SceneNode, b: Box, t: Xf, parentHit: number, pathPos: number, p: Paint, chain: number[], alpha: number) {
    switch (n.shape) {
      case 'port':
        this.port(n, b, t, alpha);
        return;
      case 'glyph':
        this.glyph(n, b, t, alpha, p);
        this.hit(n, b, parentHit, Math.min(b.w, b.h) >= 12);
        return;
      case 'region':
        this.region(n, b, t, parentHit, pathPos, p, chain, alpha);
        return;
      default:
        this.box(n, b, t, parentHit, pathPos, p, chain, alpha);
    }
  }

  private hit(n: SceneNode, b: Box, parent: number, clear = false): number {
    if (b.w < 5 || b.h < 4) return parent;
    this.hits.push({ id: n.id, box: b, parent, clear: clear || (b.w >= 160 && b.h >= 100) });
    return this.hits.length - 1;
  }

  /** Marks the hit just recorded for `n` as one the reader can make out. */
  private seen(at: number, n: SceneNode, size: number, alpha: number) {
    const h = this.hits[at];
    if (h?.id === n.id && size >= 7 && alpha >= 0.5) h.clear = true;
  }

  private box(n: SceneNode, b: Box, t: Xf, parentHit: number, pathPos: number, p: Paint, chain: number[], alpha: number) {
    const ctx = this.ctx;
    const owner = this.model.owner(n);
    const recursive = !!owner && owner !== n && chain.includes(owner.id);
    const inner = owner && !recursive ? (pathPos >= 0 ? p.xfs[pathPos] : this.model.interior(t, n, owner)) : null;
    const open = inner ? smoothstep(OPEN_FROM, OPEN_TO, inner.s) * smoothstep(40, 90, b.w) : 0;
    const sw = swatch(n.category);
    const radius = clamp(6 * t.s, 1.2, 10);
    const selected = p.selected === n.id;
    const hovered = p.hover === n.id;

    ctx.globalAlpha = alpha;
    ctx.beginPath();
    ctx.roundRect(b.x, b.y, b.w, b.h, radius);
    ctx.fillStyle = open > 0 ? mix(sw.fill, sw.tint, open) : sw.fill;
    ctx.fill();
    if (open > 0.01) {
      // Header band keeps the box's identity once it is open.
      const hh = Math.min(30 * t.s, b.h);
      ctx.save();
      ctx.clip();
      ctx.globalAlpha = alpha * open;
      ctx.fillStyle = sw.fill;
      ctx.fillRect(b.x, b.y, b.w, hh);
      ctx.restore();
      ctx.globalAlpha = alpha;
    }
    const lw = clamp(1.25 * Math.sqrt(t.s), 0.5, 1.4);
    ctx.lineWidth = selected ? 2.2 : hovered ? lw + 0.6 : lw;
    ctx.strokeStyle = selected ? theme.accent : n.category === 'op' ? '#7d838c' : theme.outline;
    ctx.stroke();
    if (selected) {
      ctx.beginPath();
      ctx.roundRect(b.x - 4, b.y - 4, b.w + 8, b.h + 8, radius + 4);
      ctx.strokeStyle = theme.accentSoft;
      ctx.lineWidth = 6;
      ctx.stroke();
    } else if (p.related?.has(n.id)) {
      // A use of the block in focus: outlined so the reader sees where it is used.
      ctx.beginPath();
      ctx.roundRect(b.x - 3, b.y - 3, b.w + 6, b.h + 6, radius + 3);
      ctx.strokeStyle = theme.accent;
      ctx.lineWidth = 1.6;
      ctx.setLineDash([5, 3]);
      ctx.stroke();
      ctx.setLineDash([]);
    }
    const me = this.hit(n, b, parentHit);

    // While the header is too small to read, the name stays centred on a
    // pill over the faint interior; it moves to the header once legible.
    const legible = smoothstep(9, 10.5, 13 * t.s);
    const faceAlpha = Math.max(1 - open, open * (1 - legible));
    if (open > 0.01 && inner && owner) {
      this.seen(me, n, Math.min(13 * t.s, 19), alpha * open * legible * smoothstep(4.2, 7, Math.min(13 * t.s, 19)));
      this.header(n, b, t, alpha * open * legible);
      const shown = owner.rows?.length ? alpha * open * legible : alpha * open;
      this.interior(owner, inner, me, pathPos, p, owner === n ? chain : [...chain, owner.id], shown);
    }
    if (faceAlpha > 0.01) {
      const [size, shown] = this.face(n, b, t, alpha * faceAlpha, !!owner && !recursive, open > 0.2);
      this.seen(me, n, size, shown);
    }
    this.effectsMark(n, b, t, alpha);
    ctx.globalAlpha = 1;
  }

  /** Draws a closed box's name; returns the title's size and opacity. */
  private face(n: SceneNode, b: Box, t: Xf, alpha: number, deep: boolean, overlay: boolean): [number, number] {
    if (alpha < 0.02) return [0, 0];
    const ctx = this.ctx;
    const code = n.kind === 'op' || n.kind === 'table';
    // Names of boxes big enough to hold them stay readable at any zoom.
    const natural = (code ? 12 : 13) * t.s;
    const named = NAMED_KINDS.has(n.kind);
    const prominent = b.w >= (named ? 44 : 70) && b.h >= 18 && t.s >= (named ? 0.08 : 0.14);
    const fontAt = (px: number) => (code ? `${px}px ${theme.mono}` : `${n.kind === 'call' || n.kind === 'symbol' ? 500 : 600} ${px}px ${theme.sans}`);
    // Readable minimum, but never larger than what fits the box.
    const fits = (b.w - 14 * Math.min(t.s, 1)) / Math.max(this.measure(fontAt(10), n.title) / 10, 1);
    const size = prominent ? clamp(natural, Math.min(11.5, b.h * 0.55, Math.max(fits, natural)), 18) : natural;
    if (size < 4.2) return [0, 0];
    const meta = [n.subtitle, ...(n.badges ?? [])].filter(Boolean).join(' · ');
    const textAlpha = alpha * smoothstep(4.2, 7, size);
    // The caption under a name is always smaller than the name.
    const ms = Math.min(size * 0.82, Math.max(10.5 * t.s, 10));
    const twoLines = !!meta && b.h >= size + ms + 10;
    const cx = b.x + b.w / 2;
    const cy = b.y + b.h / 2;
    const maxW = b.w - 14 * Math.min(t.s, 1);
    ctx.font = code ? `${size}px ${theme.mono}` : `${n.kind === 'call' || n.kind === 'symbol' ? 500 : 600} ${size}px ${theme.sans}`;
    const title = this.fit(n.title, maxW);
    if (overlay && title) {
      // A soft backing keeps the name legible over the interior preview.
      const w = this.measure(ctx.font, title) + size * 1.1;
      const h = (twoLines ? size + ms + 6 : size) + size * 0.7;
      ctx.globalAlpha = alpha * 0.92;
      ctx.fillStyle = '#ffffff';
      ctx.beginPath();
      ctx.roundRect(cx - w / 2, cy - h / 2, w, h, h / 2.6);
      ctx.fill();
    }
    ctx.globalAlpha = textAlpha;
    ctx.textAlign = 'center';
    ctx.fillStyle = theme.ink;
    const titleY = twoLines ? cy - ms * 0.15 : cy + size * 0.35;
    ctx.fillText(title, cx, titleY);
    if (twoLines) {
      ctx.font = `${ms}px ${theme.sans}`;
      ctx.fillStyle = theme.muted;
      ctx.fillText(this.fit(meta, maxW), cx, titleY + ms + 3);
    }
    if (deep && !overlay && b.w > 46 && size > 6) {
      // Something lives inside: a small corner mark invites zooming.
      const s = clamp(4 * t.s, 2, 5);
      ctx.globalAlpha = textAlpha * 0.6;
      ctx.strokeStyle = theme.muted;
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(b.x + b.w - 3 * s, b.y + s);
      ctx.lineTo(b.x + b.w - s, b.y + s);
      ctx.lineTo(b.x + b.w - s, b.y + 3 * s);
      ctx.stroke();
    }
    ctx.textAlign = 'left';
    return [title ? size : 0, textAlpha];
  }

  private header(n: SceneNode, b: Box, t: Xf, alpha: number) {
    const size = Math.min(13 * t.s, 19);
    if (size < 4.2 || alpha < 0.02) return;
    const ctx = this.ctx;
    ctx.globalAlpha = alpha * smoothstep(4.2, 7, size);
    const band = 30 * t.s;
    const x = b.x + Math.min(9 * t.s, 14);
    const y = b.y + Math.min(band / 2 + size * 0.36, band - 4);
    const maxW = b.w - 18 * t.s;
    ctx.font = `600 ${size}px ${theme.sans}`;
    ctx.fillStyle = theme.ink;
    const title = this.fit(n.title, maxW);
    ctx.fillText(title, x, y);
    const used = this.measure(ctx.font, title) + 8 * t.s;
    const meta = [n.subtitle, ...(n.badges ?? [])].filter(Boolean).join(' · ');
    if (meta && used < maxW - 20 * t.s) {
      ctx.font = `${size * 0.82}px ${theme.sans}`;
      ctx.fillStyle = theme.muted;
      ctx.fillText(this.fit(meta, maxW - used), x + used, y);
    }
  }

  private effectsMark(n: SceneNode, b: Box, t: Xf, alpha: number) {
    const fx = n.effects ?? [];
    if (!fx.length || b.w < 30) return;
    const loud = fx.some(e => /write|network|exec|mutat|distributed|process|raises|reads|filesystem|state/.test(e));
    if (!loud) return;
    const ctx = this.ctx;
    const r = clamp(3 * t.s, 1.5, 4);
    ctx.globalAlpha = alpha * 0.9;
    ctx.fillStyle = theme.effect;
    ctx.beginPath();
    ctx.arc(b.x + b.w - r * 2.2, b.y + b.h - r * 2.2, r, 0, Math.PI * 2);
    ctx.fill();
  }

  private region(n: SceneNode, b: Box, t: Xf, parentHit: number, pathPos: number, p: Paint, chain: number[], alpha: number) {
    const ctx = this.ctx;
    const lane = n.kind === 'lane';
    const sw = swatch(lane ? 'lane' : n.kind === 'table' ? 'config' : 'region');
    const radius = clamp(9 * t.s, 1.5, 14);
    ctx.globalAlpha = alpha;
    ctx.beginPath();
    ctx.roundRect(b.x, b.y, b.w, b.h, radius);
    ctx.fillStyle = n.kind === 'table' ? swatch('config').tint : sw.fill;
    ctx.fill();
    ctx.lineWidth = p.selected === n.id ? 2 : clamp(1.1 * Math.sqrt(t.s), 0.45, 1.2);
    ctx.strokeStyle = p.selected === n.id ? theme.accent : lane ? theme.laneOutline : theme.regionOutline;
    if (lane) ctx.setLineDash([4 * t.s, 3 * t.s]);
    ctx.stroke();
    ctx.setLineDash([]);
    const me = this.hit(n, b, parentHit);
    const size = 11.5 * t.s;
    if (n.title) this.seen(me, n, size, alpha * smoothstep(4.2, 7, size));
    if (n.title && size >= 4.2) {
      ctx.globalAlpha = alpha * smoothstep(4.2, 7, size);
      const code = CODE_KINDS.has(n.kind);
      ctx.font = code ? `${size}px ${theme.mono}` : `600 ${size}px ${theme.sans}`;
      ctx.fillStyle = theme.muted;
      const badges = (n.badges ?? []).join('  ');
      let reserve = 0;
      if (badges) {
        ctx.font = `700 ${12 * t.s}px ${theme.sans}`;
        reserve = this.measure(ctx.font, badges) + 18 * t.s;
        ctx.fillStyle = theme.ink;
        ctx.textAlign = 'right';
        ctx.fillText(badges, b.x + b.w - 10 * t.s, b.y + 17 * t.s);
        ctx.textAlign = 'left';
        ctx.font = code ? `${size}px ${theme.mono}` : `600 ${size}px ${theme.sans}`;
        ctx.fillStyle = theme.muted;
      }
      ctx.fillText(this.fit(n.title, b.w - 20 * t.s - reserve), b.x + 10 * t.s, b.y + 17 * t.s);
    }
    const inner = pathPos >= 0 ? p.xfs[pathPos] : this.model.interior(t, n, n);
    if (inner) this.interior(n, inner, me, pathPos, p, chain, alpha);
    ctx.globalAlpha = 1;
  }

  private glyph(n: SceneNode, b: Box, t: Xf, alpha: number, p: Paint) {
    const ctx = this.ctx;
    const r = Math.min(b.w, b.h) / 2;
    const cx = b.x + b.w / 2;
    const cy = b.y + b.h / 2;
    ctx.globalAlpha = alpha;
    ctx.beginPath();
    ctx.arc(cx, cy, r, 0, Math.PI * 2);
    ctx.fillStyle = '#ffffff';
    ctx.fill();
    ctx.lineWidth = clamp(1.25 * Math.sqrt(t.s), 0.5, 1.4);
    ctx.strokeStyle = p.selected === n.id ? theme.accent : theme.outline;
    ctx.stroke();
    if (r < 2.5) return;
    if (n.title === '+') {
      const a = r * 0.55;
      ctx.beginPath();
      ctx.moveTo(cx - a, cy);
      ctx.lineTo(cx + a, cy);
      ctx.moveTo(cx, cy - a);
      ctx.lineTo(cx, cy + a);
      ctx.stroke();
    } else {
      ctx.font = `600 ${r * 1.15}px ${theme.sans}`;
      ctx.fillStyle = theme.ink;
      ctx.textAlign = 'center';
      ctx.fillText(n.title, cx, cy + r * 0.4);
      ctx.textAlign = 'left';
    }
  }

  private port(n: SceneNode, b: Box, t: Xf, alpha: number) {
    if (b.h === 0 || !n.title) return;
    const size = Math.min(11.5 * t.s, 15);
    if (size < 4.2) return;
    const ctx = this.ctx;
    const context = n.category === 'context';
    ctx.globalAlpha = alpha * smoothstep(4.2, 7, size);
    ctx.font = `${context ? 'italic ' : ''}${size}px ${theme.mono}`;
    ctx.fillStyle = context ? theme.faint : theme.muted;
    ctx.textAlign = 'center';
    ctx.fillText(this.fit(n.title, Math.max(b.w + 60 * t.s, 40)), b.x + b.w / 2, b.y + b.h / 2 + size * 0.35);
    ctx.textAlign = 'left';
    ctx.globalAlpha = 1;
  }

  private rows(owner: SceneNode, t: Xf, p: Paint, alpha: number) {
    const ctx = this.ctx;
    const size = 11.5 * t.s;
    if (size < 3) return;
    let y = t.y + 6 * t.s;
    if (size > 40) return;
    const x = t.x + 10 * t.s;
    const textAlpha = alpha * smoothstep(3.5, 6.5, size);
    for (const r of owner.rows ?? []) {
      const h = (r.heading ? HEADING_H : ROW_H) * t.s;
      if (y > p.height) break;
      if (y + h >= 0) {
        if (size < 4.5) {
          // Too small to read: a texture of lines that hints at the content.
          ctx.globalAlpha = alpha * 0.35;
          ctx.fillStyle = r.heading ? '#b39b52' : theme.faint;
          ctx.fillRect(x, y + h * 0.45, Math.min((r.key.length + (r.value?.length ?? 0) + 3) * size * 0.6, (owner.cw ?? 100) * t.s - 20 * t.s), Math.max(size * 0.35, 0.6));
        } else if (r.heading) {
          ctx.globalAlpha = textAlpha;
          ctx.font = `700 ${10.5 * t.s}px ${theme.sans}`;
          ctx.fillStyle = '#8a6d1f';
          ctx.fillText(r.key.toUpperCase(), x, y + h * 0.72);
        } else {
          ctx.globalAlpha = textAlpha;
          ctx.font = `${size}px ${theme.mono}`;
          ctx.fillStyle = theme.ink;
          ctx.fillText(r.key, x, y + h * 0.72);
          if (r.value) {
            const kw = this.measure(ctx.font, r.key);
            ctx.fillStyle = theme.muted;
            ctx.fillText(this.fit(` = ${r.value}`, (owner.cw ?? 300) * t.s - kw - 20 * t.s), x + kw, y + h * 0.72);
          }
        }
      }
      y += h;
    }
    ctx.globalAlpha = 1;
  }

  // ----- edges -----------------------------------------------------------------------

  private bounds(e: Edge): [number, number, number, number] {
    let b = this.edgeBounds.get(e);
    if (!b) {
      let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
      for (let i = 0; i < e.points.length; i += 2) {
        x0 = Math.min(x0, e.points[i]);
        x1 = Math.max(x1, e.points[i]);
        y0 = Math.min(y0, e.points[i + 1]);
        y1 = Math.max(y1, e.points[i + 1]);
      }
      b = [x0, y0, x1, y1];
      this.edgeBounds.set(e, b);
    }
    return b;
  }

  /** Draws an edge; `state` is true when lit, false when faded beside lit ones. */
  private edge(e: Edge, t: Xf, p: Paint, alpha: number, lw: number, state: boolean | null, owner: number, container: number) {
    const pts = e.points;
    if (pts.length < 4) return;
    const [x0, y0, x1, y1] = this.bounds(e);
    if (!intersects({ x: t.x + x0 * t.s, y: t.y + y0 * t.s, w: (x1 - x0) * t.s + 1, h: (y1 - y0) * t.s + 1 }, p.width, p.height, 10)) return;
    const ctx = this.ctx;
    const lit = state === true;
    const dim = state === false;
    if (alpha > 0.3 && (x1 - x0 + y1 - y0) * t.s > 6) {
      const screen = new Array<number>(pts.length);
      for (let i = 0; i < pts.length; i += 2) {
        screen[i] = t.x + pts[i] * t.s;
        screen[i + 1] = t.y + pts[i + 1] * t.s;
      }
      this.edgeHits.push({ edge: e, owner, container, pts: screen });
    }
    // Settings and sizes are faint; conditions are dotted: neither is the data itself.
    const context = e.kind === 'context';
    const control = e.kind === 'control';
    let color = context || control ? theme.context : e.kind === 'artifact' ? theme.artifact : e.kind === 'exec' ? theme.faintEdge : theme.edge;
    let a = alpha * (context ? 0.55 : control ? 0.8 : 0.95) * (dim ? 0.35 : 1);
    let width = context || control ? lw * 0.85 : lw;
    if (lit) {
      color = theme.accent;
      a = Math.max(alpha, 0.9);
      width = lw + 0.7;
    }
    ctx.globalAlpha = a;
    ctx.strokeStyle = color;
    ctx.fillStyle = color;
    ctx.lineWidth = width;
    ctx.lineJoin = 'round';
    ctx.setLineDash(e.kind === 'exec' ? [5, 4] : e.kind === 'artifact' ? [6, 3] : control ? [1.5, 2.5].map(d => d * Math.max(width, 1)) : []);
    const n = pts.length / 2;
    const X = (i: number) => t.x + pts[i * 2] * t.s;
    const Y = (i: number) => t.y + pts[i * 2 + 1] * t.s;
    const head = e.open ? 0 : clamp(7 * Math.sqrt(t.s), 3, 8) * (context || control ? 0.8 : 1);
    // Shorten the last segment so the line ends at the arrow's base.
    const ex = X(n - 1);
    const ey = Y(n - 1);
    const dx = ex - X(n - 2);
    const dy = ey - Y(n - 2);
    const len = Math.hypot(dx, dy) || 1;
    const ux = dx / len;
    const uy = dy / len;
    const r = clamp(7 * t.s, 0, 10);
    ctx.beginPath();
    ctx.moveTo(X(0), Y(0));
    for (let i = 1; i < n - 1; i++) {
      const lenIn = Math.hypot(X(i) - X(i - 1), Y(i) - Y(i - 1));
      const lenOut = Math.hypot(X(i + 1) - X(i), Y(i + 1) - Y(i));
      ctx.arcTo(X(i), Y(i), X(i + 1), Y(i + 1), Math.min(r, lenIn / 2, lenOut / 2));
    }
    ctx.lineTo(ex - ux * head * 0.8, ey - uy * head * 0.8);
    ctx.stroke();
    ctx.setLineDash([]);
    if (head > 0) {
      ctx.beginPath();
      ctx.moveTo(ex, ey);
      ctx.lineTo(ex - ux * head - uy * head * 0.42, ey - uy * head + ux * head * 0.42);
      ctx.lineTo(ex - ux * head + uy * head * 0.42, ey - uy * head - ux * head * 0.42);
      ctx.closePath();
      ctx.fill();
    }
    ctx.globalAlpha = 1;
  }

  private edgeLabel(e: Edge, t: Xf, p: Paint, alpha: number, state: boolean | null, placed: Box[]) {
    if (!e.label || !e.labelAt || alpha < 0.03) return;
    const x = t.x + e.labelAt[0] * t.s;
    const y = t.y + e.labelAt[1] * t.s;
    if (x < -200 || y < -20 || x > p.width + 20 || y > p.height + 20) return;
    const ctx = this.ctx;
    const lit = state === true;
    const size = Math.min(11 * t.s, 13.5);
    ctx.font = `${size}px ${theme.mono}`;
    // Labels never overprint each other; the one for a highlighted edge wins.
    const r = { x: x - 1, y: y - size, w: this.measure(ctx.font, e.label) + 2, h: size + 2 };
    if (!lit && placed.some(q => r.x < q.x + q.w && q.x < r.x + r.w && r.y < q.y + q.h && q.y < r.y + r.h)) return;
    placed.push(r);
    ctx.globalAlpha = state === false ? alpha * 0.4 : alpha;
    ctx.lineWidth = 3;
    ctx.strokeStyle = theme.labelHalo;
    ctx.lineJoin = 'round';
    ctx.strokeText(e.label, x, y);
    // Values read in ink; settings stay muted like their lines.
    ctx.fillStyle = lit ? theme.accent : e.kind === 'data' ? theme.labelInk : theme.muted;
    ctx.fillText(e.label, x, y);
    ctx.globalAlpha = 1;
  }

  // ----- text ------------------------------------------------------------------------

  private measure(font: string, text: string): number {
    const key = `${font}|${text}`;
    let w = this.widths.get(key);
    if (w === undefined) {
      this.ctx.font = font;
      w = this.ctx.measureText(text).width;
      if (this.widths.size > 8000) this.widths.clear();
      this.widths.set(key, w);
    }
    return w;
  }

  /** Ellipsizes `text` to `max` pixels in the current font. */
  private fit(text: string, max: number): string {
    const font = this.ctx.font;
    if (max <= 4) return '';
    if (this.measure(font, text) <= max) return text;
    let lo = 0;
    let hi = text.length;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (this.measure(font, text.slice(0, mid) + '…') <= max) lo = mid;
      else hi = mid - 1;
    }
    this.ctx.font = font;
    return lo > 0 ? text.slice(0, lo).trimEnd() + '…' : '';
  }
}

function mix(a: string, b: string, t: number): string {
  const pa = parse(a);
  const pb = parse(b);
  const c = pa.map((v, i) => Math.round(v + (pb[i] - v) * t));
  return `rgb(${c[0]},${c[1]},${c[2]})`;
}

const parsed = new Map<string, number[]>();
function parse(hex: string): number[] {
  let v = parsed.get(hex);
  if (!v) {
    const h = hex.replace('#', '');
    v = [0, 2, 4].map(i => parseInt(h.slice(i, i + 2), 16));
    parsed.set(hex, v);
  }
  return v;
}
