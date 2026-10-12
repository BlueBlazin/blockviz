import type { Edge, SceneNode } from '../core/types';
import { boxOf, clamp, intersects, rectLine, smoothstep, visibleRuns, type Box, type Xf } from './geometry';
import type { RemovedEdge, SceneDiff, Status } from './diff';
import type { SceneModel } from './model';
import { changeColor, swatch, theme } from './theme';

/** A block drawn this frame. `clear` when the reader can see what it is (its
 * name is legible, or the box is large), so clicks never land on a block
 * too small to make out. `face` when its name is on its face: it is closed,
 * or what is drawn inside it is only a thumbnail. */
export interface Hit { id: number; box: Box; parent: number; clear: boolean; face?: boolean }

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
  /** How the code differs from a base version (a pull request): what changed is coloured, the rest faded. */
  diff?: SceneDiff | null;
}

/** How faint what a change did not touch is drawn. */
const UNTOUCHED = 0.4;

const ROW_H = 17;
/** Largest on-screen size of a table's rows, as for names. */
const ROW_MAX = 15;
const HEADING_H = 22;
/** Interior scale at which a box starts / finishes opening. */
const OPEN_FROM = 0.028;
const OPEN_TO = 0.075;

/** The smallest name, in pixels, a reader can make out. */
const LEGIBLE = 7;

/** How far past the screen a line is kept: far enough that the corners of what is cut off stay out of sight. */
const CUT = 24;

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
  /** How many closed blocks the one being drawn is in the thumbnail of. */
  private thumbnails = 0;
  /** How many faded (untouched) blocks the one being drawn is in. */
  private faded = 0;
  /** Widths of text, by its font as set and the text. */
  private widths = new Map<string, number>();
  /** The font as set (at a size on the ladder), and how much it is scaled to the size asked for. */
  private fontKey = '';
  private fontScale = 1;
  private edgeBounds = new WeakMap<Edge, [number, number, number, number]>();

  constructor(private ctx: CanvasRenderingContext2D, private model: SceneModel) {}

  draw(p: Paint) {
    const ctx = this.ctx;
    ctx.fillStyle = theme.background;
    ctx.fillRect(0, 0, p.width, p.height);
    this.hits = [];
    this.edgeHits = [];
    this.thumbnails = 0;
    this.faded = 0;
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
    const diff = p.diff;
    // A connection the change made is green; one between blocks it did not touch fades with them.
    const change = (e: Edge): 'added' | 'faded' | null => !diff ? null : diff.added.has(e) ? 'added' : !this.faded && !diff.touched(e.from) && !diff.touched(e.to) ? 'faded' : null;
    if (edgeAlpha > 0.02) {
      for (const e of owner.edges ?? []) this.edge(e, t, p, edgeAlpha, lw, lit(e), owner.id, parentHit, change(e));
      for (const r of diff?.removed.get(owner.id) ?? []) this.removedEdge(r, t, edgeAlpha, lw, p);
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
      // Labels of lit edges are placed first, so they are the ones shown.
      const all = owner.edges ?? [];
      const order = focused || traced || p.hoverEdge ? [...all].sort((a, b) => Number(!!lit(b)) - Number(!!lit(a))) : all;
      for (const e of order) this.edgeLabel(e, t, p, edgeAlpha * smoothstep(4.5, 7, t.s * 11), lit(e), placed, change(e));
    }
  }

  private node(n: SceneNode, b: Box, t: Xf, parentHit: number, pathPos: number, p: Paint, chain: number[], alpha: number) {
    switch (n.shape) {
      case 'port':
        this.port(n, b, t, alpha, p.diff?.status(n.id) ?? null);
        return;
      case 'glyph':
        this.glyph(n, b, t, p.diff && !this.faded && !p.diff.touched(n.id) ? alpha * UNTOUCHED : alpha, p);
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
    if (h?.id === n.id && size >= LEGIBLE && alpha >= 0.5) h.clear = true;
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
    // A change colours what it added or changed; what it did not touch fades.
    const diff = p.diff;
    const status = diff?.status(n.id) ?? null;
    const unfaded = this.faded ? Math.min(1, alpha / UNTOUCHED) : alpha;
    const fade = !!diff && !this.faded && !diff.touched(n.id);
    if (fade) {
      alpha *= UNTOUCHED;
      this.faded++;
    }
    const fill = status ? mix(sw.fill, changeColor[status].tint, 0.7) : sw.fill;
    // Opened, a changed block keeps its colour on the header band; the diagram inside stays plain.
    const tint = status ? mix(sw.tint, changeColor[status].tint, 0.12) : sw.tint;

    ctx.globalAlpha = alpha;
    ctx.beginPath();
    ctx.roundRect(b.x, b.y, b.w, b.h, radius);
    ctx.fillStyle = open > 0 ? mix(fill, tint, open) : fill;
    ctx.fill();
    if (open > 0.01) {
      // Header band keeps the box's identity once it is open.
      const hh = Math.min(30 * t.s, b.h);
      ctx.save();
      ctx.clip();
      ctx.globalAlpha = alpha * open;
      ctx.fillStyle = fill;
      ctx.fillRect(b.x, b.y, b.w, hh);
      ctx.restore();
      ctx.globalAlpha = alpha;
    }
    const lw = clamp(1.25 * Math.sqrt(t.s), 0.5, 1.4);
    ctx.lineWidth = selected ? 2.2 : status ? Math.max(lw + 0.5, 1.5) + (hovered ? 0.5 : 0) : hovered ? lw + 0.6 : lw;
    ctx.strokeStyle = selected ? theme.accent : status ? changeColor[status].ink : n.category === 'op' ? '#7d838c' : theme.outline;
    ctx.stroke();
    if (selected) {
      ctx.beginPath();
      ctx.roundRect(b.x - 4, b.y - 4, b.w + 8, b.h + 8, radius + 4);
      ctx.strokeStyle = theme.accentSoft;
      ctx.lineWidth = 6;
      ctx.stroke();
    } else if (p.related?.has(n.id)) {
      // A use of the block in focus: outlined so the reader sees where it is used.
      ctx.strokeStyle = theme.accent;
      ctx.lineWidth = 1.6;
      ctx.setLineDash([5, 3]);
      this.outline({ x: b.x - 3, y: b.y - 3, w: b.w + 6, h: b.h + 6 }, radius + 3, p);
      ctx.setLineDash([]);
    }
    const me = this.hit(n, b, parentHit);

    // While the header is too small to read, the name stays centred on a
    // pill over the faint interior; it moves to the header once legible.
    const legible = smoothstep(9, 10.5, 13 * t.s);
    const faceAlpha = Math.max(1 - open, open * (1 - legible));
    const thumbnail = faceAlpha >= 0.5;
    if (this.hits[me]?.id === n.id) this.hits[me].face = thumbnail;
    if (open > 0.01 && inner && owner) {
      this.seen(me, n, Math.min(13 * t.s, 19), alpha * open * legible * smoothstep(4.2, 7, Math.min(13 * t.s, 19)));
      this.header(n, b, t, alpha * open * legible);
      const shown = owner.rows?.length ? alpha * open * legible : alpha * open;
      if (thumbnail) this.thumbnails++;
      this.interior(owner, inner, me, pathPos, p, owner === n ? chain : [...chain, owner.id], shown);
      if (thumbnail) this.thumbnails--;
    }
    if (faceAlpha > 0.01) {
      const [size, shown] = this.face(n, b, t, alpha * faceAlpha, !!owner && !recursive, open > 0.2);
      this.seen(me, n, size, shown);
    }
    this.effectsMark(n, b, t, alpha);
    if (diff) this.changeMarks(n, b, t, alpha, unfaded, diff, status, p);
    if (fade) this.faded--;
    ctx.globalAlpha = 1;
  }

  /**
   * How many changes a block holds, on a badge at its top edge; a block that
   * holds changes without changing itself (a folder) is ringed. A block that
   * leads into changed code (a call of a changed function) gets a dashed ring
   * and an arrowed count, drawn even where its surroundings are faded.
   */
  private changeMarks(n: SceneNode, b: Box, t: Xf, alpha: number, unfaded: number, diff: SceneDiff, status: Status | null, p: Paint) {
    const inside = diff.inside(n.id);
    const through = inside || status === 'added' ? 0 : diff.through(n.id);
    // In a thumbnail the tint shows where changes are; the closed block's badge counts them.
    if ((!inside && !through) || b.w < 14 || b.h < 10 || this.thumbnails) return;
    const ctx = this.ctx;
    const ink = changeColor.changed.ink;
    const radius = clamp(6 * t.s, 1.2, 10) + 3;
    if (!status || through) {
      ctx.globalAlpha = through ? unfaded : alpha;
      ctx.strokeStyle = ink;
      ctx.lineWidth = 1.5;
      if (through) ctx.setLineDash([4, 3]);
      this.outline({ x: b.x - 3, y: b.y - 3, w: b.w + 6, h: b.h + 6 }, radius, p);
      ctx.setLineDash([]);
    }
    const count = inside || through;
    const words = b.w >= 130 ? ` change${count === 1 ? '' : 's'}` : '';
    const text = through ? `→ ${count}${words}` : `${count}${words}`;
    this.font(10, theme.sans, 600);
    const w = this.width(text) + 10;
    const h = 15;
    const x = Math.max(b.x + 2, b.x + b.w - w - 6);
    const y = b.y - h / 2 - (status ? 0 : 3);
    ctx.globalAlpha = through ? unfaded : alpha;
    ctx.beginPath();
    ctx.roundRect(x, y, w, h, h / 2);
    ctx.fillStyle = through ? '#ffffff' : ink;
    ctx.fill();
    if (through) {
      ctx.strokeStyle = ink;
      ctx.lineWidth = 1;
      ctx.stroke();
    }
    ctx.fillStyle = through ? ink : '#ffffff';
    ctx.textAlign = 'center';
    this.text(text, x + w / 2, y + 11);
    ctx.textAlign = 'left';
  }

  /** Draws a closed box's name; returns the title's size and opacity. */
  private face(n: SceneNode, b: Box, t: Xf, alpha: number, deep: boolean, overlay: boolean): [number, number] {
    if (alpha < 0.02) return [0, 0];
    const ctx = this.ctx;
    // An operation's title is code; a table's is a name ("External packages").
    const code = n.kind === 'op';
    // Names of boxes big enough to hold them stay readable at any zoom.
    const natural = (code ? 12 : 13) * t.s;
    const named = NAMED_KINDS.has(n.kind);
    const family = code ? theme.mono : theme.sans;
    const weight = code ? 400 : n.kind === 'call' || n.kind === 'symbol' ? 500 : 600;
    const at10 = (text: string) => {
      this.font(10, family, weight);
      return this.width(text);
    };
    // Readable minimum, but never larger than what fits the box.
    const room = b.w - 14 * Math.min(t.s, 1);
    const sizeFor = (text: string, share: number) => {
      const fits = room / Math.max(at10(text) / 10, 1);
      return clamp(natural, Math.min(11.5, b.h * share, Math.max(fits, natural)), 18);
    };
    // A box with room keeps a readable name; so does a landmark (a file, a
    // class, a file on disk) whose name still fits it at a legible size,
    // however narrow it is, unless it is in the thumbnail of a closed block
    // (whose own name is the landmark there). Other names grow and shrink
    // with the zoom, so they are never measured.
    const roomy = b.w >= (named ? 44 : 70) && b.h >= 18;
    const landmark = named && !this.thumbnails && b.h >= 14;
    const prominent = t.s >= (named ? 0.08 : 0.14) && (roomy || landmark);
    let lines = [n.title];
    let size = prominent ? sizeFor(n.title, 0.55) : natural;
    // A name of several words too long for a narrow box goes on two lines
    // rather than shrinking out of sight.
    if (prominent && size < 10 && n.title.includes(' ')) {
      const split = splitTitle(n.title);
      const longer = at10(split[0]) >= at10(split[1]) ? split[0] : split[1];
      const two = sizeFor(longer, 0.3);
      if (two > size + 0.5) {
        lines = split;
        size = two;
      }
    }
    // A landmark whose name no longer fits at a legible size is not one.
    if (!roomy && size < LEGIBLE) {
      lines = [n.title];
      size = natural;
    }
    if (size < 4.2) return [0, 0];
    const meta = [n.subtitle, ...(n.badges ?? [])].filter(Boolean).join(' · ');
    const textAlpha = alpha * smoothstep(4.2, 7, size);
    // The caption under a name is always smaller than the name.
    const ms = Math.min(size * 0.82, Math.max(10.5 * t.s, 10));
    const lh = size * 1.15;
    const extra = (lines.length - 1) * lh;
    const twoLines = !!meta && b.h >= size + extra + ms + 10;
    const cx = b.x + b.w / 2;
    const cy = b.y + b.h / 2;
    const maxW = room;
    this.font(size, family, weight);
    const titles = lines.map(l => this.fit(l, maxW));
    const title = titles.join(' ').trim();
    if (overlay && title) {
      // A soft backing keeps the name legible over the interior preview.
      const w = Math.max(...titles.map(l => this.width(l))) + size * 1.1;
      const h = (twoLines ? size + extra + ms + 6 : size + extra) + size * 0.7;
      ctx.globalAlpha = alpha * 0.92;
      ctx.fillStyle = '#ffffff';
      ctx.beginPath();
      ctx.roundRect(cx - w / 2, cy - h / 2, w, h, Math.min(h, size * 1.7) / 2.6);
      ctx.fill();
    }
    ctx.globalAlpha = textAlpha;
    ctx.textAlign = 'center';
    ctx.fillStyle = theme.ink;
    const titleY = (twoLines ? cy - ms * 0.15 : cy + size * 0.35) - extra / 2;
    titles.forEach((l, i) => this.text(l, cx, titleY + i * lh));
    if (twoLines) {
      this.font(ms, theme.sans);
      ctx.fillStyle = theme.muted;
      this.text(this.fit(meta, maxW), cx, titleY + extra + ms + 3);
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
    this.font(size, theme.sans, 600);
    ctx.fillStyle = theme.ink;
    const title = this.fit(n.title, maxW);
    this.text(title, x, y);
    const used = this.width(title) + 8 * t.s;
    const meta = [n.subtitle, ...(n.badges ?? [])].filter(Boolean).join(' · ');
    if (meta && used < maxW - 20 * t.s) {
      this.font(size * 0.82, theme.sans);
      ctx.fillStyle = theme.muted;
      this.text(this.fit(meta, maxW - used), x + used, y);
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
    const diff = p.diff;
    const status = diff?.status(n.id) ?? null;
    const unfaded = this.faded ? Math.min(1, alpha / UNTOUCHED) : alpha;
    const fade = !!diff && !this.faded && !diff.touched(n.id);
    if (fade) {
      alpha *= UNTOUCHED;
      this.faded++;
    }
    const fill = n.kind === 'table' ? swatch('config').tint : sw.fill;
    ctx.globalAlpha = alpha;
    ctx.beginPath();
    ctx.roundRect(b.x, b.y, b.w, b.h, radius);
    ctx.fillStyle = status ? mix(fill, changeColor[status].tint, 0.5) : fill;
    ctx.fill();
    ctx.lineWidth = p.selected === n.id ? 2 : status ? 1.5 : clamp(1.1 * Math.sqrt(t.s), 0.45, 1.2);
    ctx.strokeStyle = p.selected === n.id ? theme.accent : status ? changeColor[status].ink : lane ? theme.laneOutline : theme.regionOutline;
    if (lane) {
      ctx.setLineDash([4 * t.s, 3 * t.s]);
      this.outline(b, radius, p);
      ctx.setLineDash([]);
    } else ctx.stroke();
    const me = this.hit(n, b, parentHit);
    const size = 11.5 * t.s;
    if (n.title) this.seen(me, n, size, alpha * smoothstep(4.2, 7, size));
    if (n.title && size >= 4.2) {
      ctx.globalAlpha = alpha * smoothstep(4.2, 7, size);
      const code = CODE_KINDS.has(n.kind);
      const badges = (n.badges ?? []).join('  ');
      let reserve = 0;
      if (badges) {
        this.font(12 * t.s, theme.sans, 700);
        reserve = this.width(badges) + 18 * t.s;
        ctx.fillStyle = theme.ink;
        ctx.textAlign = 'right';
        this.text(badges, b.x + b.w - 10 * t.s, b.y + 17 * t.s);
        ctx.textAlign = 'left';
      }
      this.font(size, code ? theme.mono : theme.sans, code ? 400 : 600);
      ctx.fillStyle = theme.muted;
      this.text(this.fit(n.title, b.w - 20 * t.s - reserve), b.x + 10 * t.s, b.y + 17 * t.s);
    }
    const inner = pathPos >= 0 ? p.xfs[pathPos] : this.model.interior(t, n, n);
    if (inner) this.interior(n, inner, me, pathPos, p, chain, alpha);
    if (diff) this.changeMarks(n, b, t, alpha, unfaded, diff, status, p);
    if (fade) this.faded--;
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
    const status = p.diff?.status(n.id) ?? null;
    ctx.lineWidth = clamp(1.25 * Math.sqrt(t.s), 0.5, 1.4) + (status ? 0.5 : 0);
    ctx.strokeStyle = p.selected === n.id ? theme.accent : status ? changeColor[status].ink : theme.outline;
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
      this.font(r * 1.15, theme.sans, 600);
      ctx.fillStyle = theme.ink;
      ctx.textAlign = 'center';
      this.text(n.title, cx, cy + r * 0.4);
      ctx.textAlign = 'left';
    }
  }

  private port(n: SceneNode, b: Box, t: Xf, alpha: number, status: Status | null) {
    if (b.h === 0 || !n.title) return;
    const size = Math.min(11.5 * t.s, 15);
    if (size < 4.2) return;
    const ctx = this.ctx;
    ctx.globalAlpha = alpha * smoothstep(4.2, 7, size);
    this.font(size, theme.mono);
    // A new input or result is named in the colour of what was added.
    ctx.fillStyle = status ? changeColor[status].ink : theme.muted;
    ctx.textAlign = 'center';
    this.text(this.fit(n.title, Math.max(b.w + 60 * t.s, 40)), b.x + b.w / 2, b.y + b.h / 2 + size * 0.35);
    ctx.textAlign = 'left';
    ctx.globalAlpha = 1;
  }

  private rows(owner: SceneNode, t: Xf, p: Paint, alpha: number) {
    const ctx = this.ctx;
    // Rows read at most at the size names do, however far the table is
    // zoomed or however few rows give it a large scale beside its neighbours.
    const size = Math.min(11.5 * t.s, ROW_MAX);
    if (size < 3) return;
    const f = size / 11.5;
    let y = t.y + 6 * f;
    const x = t.x + 10 * f;
    const textAlpha = alpha * smoothstep(3.5, 6.5, size);
    for (const r of owner.rows ?? []) {
      const h = (r.heading ? HEADING_H : ROW_H) * f;
      if (y > p.height) break;
      if (y + h >= 0) {
        if (size < 4.5) {
          // Too small to read: a texture of lines that hints at the content.
          ctx.globalAlpha = alpha * 0.35;
          ctx.fillStyle = r.heading ? '#b39b52' : theme.faint;
          ctx.fillRect(x, y + h * 0.45, Math.min((r.key.length + (r.value?.length ?? 0) + 3) * size * 0.6, (owner.cw ?? 100) * t.s - 20 * f), Math.max(size * 0.35, 0.6));
        } else if (r.heading) {
          ctx.globalAlpha = textAlpha;
          this.font(10.5 * f, theme.sans, 700);
          ctx.fillStyle = '#8a6d1f';
          this.text(r.key.toUpperCase(), x, y + h * 0.72);
        } else {
          ctx.globalAlpha = textAlpha;
          this.font(size, theme.mono);
          ctx.fillStyle = theme.ink;
          this.text(r.key, x, y + h * 0.72);
          if (r.value) {
            const kw = this.width(r.key);
            ctx.fillStyle = theme.muted;
            this.text(this.fit(` = ${r.value}`, (owner.cw ?? 300) * t.s - kw - 20 * f), x + kw, y + h * 0.72);
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

  /** Draws an edge; `state` is true when lit, false when faded beside lit ones; `change` says
   * whether a change being read added it or left it untouched. */
  private edge(e: Edge, t: Xf, p: Paint, alpha: number, lw: number, state: boolean | null, owner: number, container: number, change: 'added' | 'faded' | null = null) {
    const pts = e.points;
    if (pts.length < 4) return;
    const [x0, y0, x1, y1] = this.bounds(e);
    if (!intersects({ x: t.x + x0 * t.s, y: t.y + y0 * t.s, w: (x1 - x0) * t.s + 1, h: (y1 - y0) * t.s + 1 }, p.width, p.height, 10)) return;
    const ctx = this.ctx;
    const lit = state === true;
    const dim = state === false;
    const screen = new Array<number>(pts.length);
    for (let i = 0; i < pts.length; i += 2) {
      screen[i] = t.x + pts[i] * t.s;
      screen[i + 1] = t.y + pts[i + 1] * t.s;
    }
    // Edges can be pointed at once their diagram is legible.
    if (alpha > 0.3 && t.s * 11 >= 6 && (x1 - x0 + y1 - y0) * t.s > 6) this.edgeHits.push({ edge: e, owner, container, pts: screen });
    // Settings and sizes are faint; conditions are dotted: neither is the data itself.
    const control = e.kind === 'control';
    let color = control ? theme.context : e.kind === 'artifact' ? theme.artifact : e.kind === 'exec' ? theme.faintEdge : theme.edge;
    let a = alpha * (control ? 0.8 : 0.95) * (dim ? 0.35 : 1);
    let width = control ? lw * 0.85 : lw;
    if (lit) {
      color = theme.accent;
      a = Math.max(alpha, 0.9);
      width = lw + 0.7;
    } else if (change === 'added') {
      color = changeColor.added.ink;
      a = Math.max(a, 0.9);
      width = lw + 0.6;
    } else if (change === 'faded') {
      a *= UNTOUCHED;
    }
    ctx.globalAlpha = a;
    ctx.strokeStyle = color;
    ctx.fillStyle = color;
    ctx.lineWidth = width;
    ctx.lineJoin = 'round';
    ctx.setLineDash(e.kind === 'exec' ? [5, 4] : e.kind === 'artifact' ? [6, 3] : control ? [1.5, 2.5].map(d => d * Math.max(width, 1)) : []);
    const n = pts.length / 2;
    const head = e.open ? 0 : clamp(7 * Math.sqrt(t.s), 3, 8) * (control ? 0.8 : 1);
    const ex = screen[2 * n - 2];
    const ey = screen[2 * n - 1];
    const dx = ex - screen[2 * n - 4];
    const dy = ey - screen[2 * n - 3];
    const len = Math.hypot(dx, dy) || 1;
    const ux = dx / len;
    const uy = dy / len;
    const r = clamp(7 * t.s, 0, 10);
    const radii = new Array<number>(n).fill(0);
    for (let i = 1; i < n - 1; i++) {
      const lenIn = Math.hypot(screen[2 * i] - screen[2 * i - 2], screen[2 * i + 1] - screen[2 * i - 1]);
      const lenOut = Math.hypot(screen[2 * i + 2] - screen[2 * i], screen[2 * i + 3] - screen[2 * i + 1]);
      radii[i] = Math.min(r, lenIn / 2, lenOut / 2);
    }
    // Shorten the last segment so the line ends at the arrow's base.
    const line = screen.slice();
    line[2 * n - 2] = ex - ux * head * 0.8;
    line[2 * n - 1] = ey - uy * head * 0.8;
    this.line(line, radii, p);
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

  private edgeLabel(e: Edge, t: Xf, p: Paint, alpha: number, state: boolean | null, placed: Box[], change: 'added' | 'faded' | null = null) {
    if (!e.label || !e.labelAt || alpha < 0.03) return;
    const x = t.x + e.labelAt[0] * t.s;
    const y = t.y + e.labelAt[1] * t.s;
    if (x < -200 || y < -20 || x > p.width + 20 || y > p.height + 20) return;
    const ctx = this.ctx;
    const lit = state === true;
    const size = Math.min(11 * t.s, 13.5);
    this.font(size, theme.mono);
    // Labels never overprint each other; the one for a highlighted edge wins.
    const r = { x: x - 1, y: y - size, w: this.width(e.label) + 2, h: size + 2 };
    if (!lit && placed.some(q => r.x < q.x + q.w && q.x < r.x + r.w && r.y < q.y + q.h && q.y < r.y + r.h)) return;
    placed.push(r);
    ctx.globalAlpha = state === false ? alpha * 0.4 : change === 'faded' && !lit ? alpha * UNTOUCHED : alpha;
    ctx.lineWidth = 3;
    ctx.strokeStyle = theme.labelHalo;
    ctx.lineJoin = 'round';
    this.text(e.label, x, y, true);
    // Values read in ink; conditions stay muted like their dotted lines.
    ctx.fillStyle = lit ? theme.accent : change === 'added' ? changeColor.added.ink : e.kind === 'control' ? theme.muted : theme.labelInk;
    this.text(e.label, x, y);
    ctx.globalAlpha = 1;
  }

  /** A connection the change removed: a dashed red curve from the bottom of one block to the top of the other. */
  private removedEdge(r: RemovedEdge, t: Xf, alpha: number, lw: number, p: Paint) {
    const a = boxOf(t, this.model.node(r.from));
    const b = boxOf(t, this.model.node(r.to));
    const x0 = a.x + a.w / 2;
    const y0 = a.y + a.h;
    const x1 = b.x + b.w / 2;
    const y1 = b.y;
    if (!intersects({ x: Math.min(x0, x1), y: Math.min(y0, y1), w: Math.abs(x1 - x0) + 1, h: Math.abs(y1 - y0) + 1 }, p.width, p.height, 20)) return;
    const ctx = this.ctx;
    const bend = Math.max(24 * t.s, Math.abs(y1 - y0) / 2);
    const head = clamp(7 * Math.sqrt(t.s), 3, 8);
    ctx.globalAlpha = Math.max(alpha, 0.85);
    ctx.strokeStyle = changeColor.removed.ink;
    ctx.fillStyle = changeColor.removed.ink;
    ctx.lineWidth = lw + 0.3;
    ctx.setLineDash([5, 4]);
    // The curve in steps fine enough to look smooth, so it can be cut to the screen.
    const ye = y1 - head * 0.8;
    const k = clamp(Math.ceil((Math.abs(x1 - x0) + Math.abs(ye - y0) + 2 * bend) / 8), 12, 512);
    const pts: number[] = [];
    for (let i = 0; i <= k; i++) {
      const u = i / k;
      const v = 1 - u;
      pts.push((v * v * v + 3 * v * v * u) * x0 + (3 * v * u * u + u * u * u) * x1, v * v * v * y0 + 3 * v * v * u * (y0 + bend) + 3 * v * u * u * (y1 - bend) + u * u * u * ye);
    }
    this.line(pts, new Array<number>(k + 1).fill(0), p);
    ctx.setLineDash([]);
    ctx.beginPath();
    ctx.moveTo(x1, y1);
    ctx.lineTo(x1 - head * 0.42, y1 - head);
    ctx.lineTo(x1 + head * 0.42, y1 - head);
    ctx.closePath();
    ctx.fill();
    if (r.label && t.s * 11 >= 4.5) {
      const size = Math.min(11 * t.s, 13.5);
      this.font(size, theme.mono);
      const mx = (x0 + x1) / 2;
      const my = (y0 + y1) / 2;
      ctx.lineWidth = 3;
      ctx.strokeStyle = theme.labelHalo;
      this.text(r.label, mx + 4, my, true);
      this.text(r.label, mx + 4, my);
    }
    ctx.globalAlpha = 1;
  }

  /** Strokes a line with rounded corners where it is on screen: a canvas dashes
   * the whole of a line, and one many screens long costs a dash every few pixels. */
  private line(pts: number[], radii: number[], p: Paint) {
    const ctx = this.ctx;
    for (const run of visibleRuns(pts, radii, -CUT, -CUT, p.width + CUT, p.height + CUT)) {
      const q = run.pts;
      ctx.beginPath();
      ctx.moveTo(q[0], q[1]);
      for (let i = 2; i < q.length - 2; i += 2) ctx.arcTo(q[i], q[i + 1], q[i + 2], q[i + 3], run.radii[i / 2]);
      ctx.lineTo(q[q.length - 2], q[q.length - 1]);
      ctx.lineDashOffset = run.at;
      ctx.stroke();
    }
    ctx.lineDashOffset = 0;
  }

  /** Strokes a rounded rectangle's outline where it is on screen. */
  private outline(b: Box, radius: number, p: Paint) {
    const r = rectLine(b, radius);
    this.line(r.pts, r.radii, p);
  }

  // ----- text ------------------------------------------------------------------------

  /**
   * Sets the font to `px` pixels of `family`. Text is laid out only at the
   * sizes of a ladder (`fontSize`) and scaled to its own size as it is drawn:
   * laid out at the size it has in each frame of a zoom, every name on
   * screen would be shaped and measured anew in every frame.
   */
  private font(px: number, family: string, weight = 400) {
    const size = fontSize(px);
    this.fontKey = `${weight} ${size}px ${family}`;
    this.fontScale = px / size;
    this.ctx.font = this.fontKey;
  }

  /** The width of `text` in the current font. */
  private width(text: string): number {
    const key = `${this.fontKey}|${text}`;
    let w = this.widths.get(key);
    if (w === undefined) {
      w = this.ctx.measureText(text).width;
      if (this.widths.size > 8000) this.widths.clear();
      this.widths.set(key, w);
    }
    return w * this.fontScale;
  }

  /** Draws `text` at (x, y) in the current font: filled, or outlined when `stroke`. */
  private text(text: string, x: number, y: number, stroke = false) {
    const ctx = this.ctx;
    const k = this.fontScale;
    ctx.save();
    ctx.scale(k, k);
    if (stroke) {
      ctx.lineWidth /= k;
      ctx.strokeText(text, x / k, y / k);
    } else ctx.fillText(text, x / k, y / k);
    ctx.restore();
  }

  /** Ellipsizes `text` to `max` pixels in the current font. Text sized to
   * fill `max` fits, whatever the rounding in working out its size. */
  private fit(text: string, max: number): string {
    if (max <= 4) return '';
    const room = max * (1 + 1e-9);
    if (this.width(text) <= room) return text;
    let lo = 0;
    let hi = text.length;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (this.width(text.slice(0, mid) + '…') <= room) lo = mid;
      else hi = mid - 1;
    }
    return lo > 0 ? text.slice(0, lo).trimEnd() + '…' : '';
  }
}

/** The size text of `px` pixels is laid out at: one of a ladder of sizes a
 * quarter of an octave apart, 10 px among them, so a name passes through a
 * few of them as the view zooms rather than a new one each frame. */
export const fontSize = (px: number) => 10 * 2 ** (Math.round(4 * Math.log2(px / 10)) / 4);

function mix(a: string, b: string, t: number): string {
  const pa = parse(a);
  const pb = parse(b);
  // Hex, so a mixed colour can be mixed again.
  return `#${pa.map((v, i) => Math.round(v + (pb[i] - v) * t).toString(16).padStart(2, '0')).join('')}`;
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

/** A title split into two lines at the space nearest its middle. */
export function splitTitle(title: string): [string, string] {
  let best = -1;
  for (let i = title.indexOf(' '); i >= 0; i = title.indexOf(' ', i + 1)) {
    if (best < 0 || Math.abs(i - title.length / 2) < Math.abs(best - title.length / 2)) best = i;
  }
  return best < 0 ? [title, ''] : [title.slice(0, best), title.slice(best + 1)];
}
