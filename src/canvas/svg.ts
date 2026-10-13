import { placement, READABLE } from './geometry';
import type { SceneModel } from './model';
import { Renderer, type Paint, type Pen } from './renderer';
import type { Theme } from './theme';

/**
 * Draws as a canvas does, into an SVG: the part of a canvas's 2D context the
 * renderer draws with, so a diagram is exported as it is drawn on screen.
 * Its transforms are scales, the same across as down (the renderer scales text).
 */
export class SvgPen implements Pen {
  fillStyle: string | CanvasGradient | CanvasPattern = '#000000';
  strokeStyle: string | CanvasGradient | CanvasPattern = '#000000';
  globalAlpha = 1;
  lineWidth = 1;
  lineJoin: CanvasLineJoin = 'miter';
  lineDashOffset = 0;
  font = '10px sans-serif';
  textAlign: CanvasTextAlign = 'start';
  private dash: number[] = [];
  private k = 1;
  /** The path being built, in SVG's syntax and the SVG's pixels: where it is, and where its last part started. */
  private d = '';
  private at: [number, number] | null = null;
  private start: [number, number] = [0, 0];
  private out: string[] = [];
  private saved: Record<string, unknown>[] = [];
  /** Clipping groups opened since the last `save`, closed by `restore`. */
  private clips = 0;
  private ids = 0;

  /** `measurer` measures text: a canvas's context, in the browser. */
  constructor(private measurer: Pick<CanvasRenderingContext2D, 'font' | 'measureText'>) {}

  /** The drawing as an SVG document of `width` × `height` pixels. */
  svg(width: number, height: number): string {
    const w = num(width);
    const h = num(height);
    return `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}">\n${this.out.join('\n')}\n</svg>\n`;
  }

  measureText(text: string): { width: number } {
    this.measurer.font = this.font;
    return this.measurer.measureText(text);
  }

  save() {
    const { fillStyle, strokeStyle, globalAlpha, lineWidth, lineJoin, lineDashOffset, font, textAlign, dash, k, clips } = this;
    this.saved.push({ fillStyle, strokeStyle, globalAlpha, lineWidth, lineJoin, lineDashOffset, font, textAlign, dash, k, clips });
    this.clips = 0;
  }

  restore() {
    const s = this.saved.pop();
    if (!s) return;
    if (this.clips) this.out.push('</g>'.repeat(this.clips));
    Object.assign(this, s);
  }

  scale(by: number) {
    this.k *= by;
  }

  setLineDash(segments: number[]) {
    this.dash = [...segments];
  }

  // ----- paths -----------------------------------------------------------------------

  beginPath() {
    this.d = '';
    this.at = null;
  }

  moveTo(x: number, y: number) {
    this.move(x * this.k, y * this.k);
  }

  lineTo(x: number, y: number) {
    this.line(x * this.k, y * this.k);
  }

  closePath() {
    if (!this.at) return;
    this.d += 'Z';
    this.at = this.start;
  }

  arc(x: number, y: number, radius: number, from: number, to: number, counterclockwise = false) {
    const k = this.k;
    const r = radius * k;
    const turn = 2 * Math.PI;
    const d = counterclockwise ? from - to : to - from;
    const sweep = d >= turn ? turn : ((d % turn) + turn) % turn;
    const dir = counterclockwise ? -1 : 1;
    const point = (a: number): [number, number] => [x * k + r * Math.cos(a), y * k + r * Math.sin(a)];
    this.line(...point(from));
    // In halves at most: one SVG arc cannot end where it starts.
    for (const a of sweep > Math.PI ? [from + (dir * sweep) / 2, from + dir * sweep] : [from + dir * sweep]) {
      this.at = point(a);
      this.d += `A${num(r)} ${num(r)} 0 0 ${counterclockwise ? 0 : 1} ${num(this.at[0])} ${num(this.at[1])}`;
    }
  }

  /** A corner of radius `radius` from where the path is, towards (x1, y1), turning towards (x2, y2). */
  arcTo(x1: number, y1: number, x2: number, y2: number, radius: number) {
    const k = this.k;
    const [px, py] = [x1 * k, y1 * k];
    const r = radius * k;
    if (!this.at) return this.move(px, py);
    const [ax, ay] = [this.at[0] - px, this.at[1] - py];
    const [bx, by] = [x2 * k - px, y2 * k - py];
    const la = Math.hypot(ax, ay);
    const lb = Math.hypot(bx, by);
    const cross = ax * by - ay * bx;
    // A corner that does not turn, or has no radius, is the point itself.
    if (!r || !la || !lb || Math.abs(cross) <= 1e-9 * la * lb) return this.line(px, py);
    const angle = Math.acos(Math.max(-1, Math.min(1, (ax * bx + ay * by) / (la * lb))));
    const reach = r / Math.tan(angle / 2);
    this.line(px + (ax / la) * reach, py + (ay / la) * reach);
    this.at = [px + (bx / lb) * reach, py + (by / lb) * reach];
    this.d += `A${num(r)} ${num(r)} 0 0 ${cross < 0 ? 1 : 0} ${num(this.at[0])} ${num(this.at[1])}`;
  }

  roundRect(x: number, y: number, w: number, h: number, radius = 0) {
    const k = this.k;
    [x, y, w, h] = [x * k, y * k, w * k, h * k];
    const r = Math.max(0, Math.min(radius * k, w / 2, h / 2));
    const corner = (ex: number, ey: number) => (r ? `A${num(r)} ${num(r)} 0 0 1 ${num(ex)} ${num(ey)}` : '');
    this.d += `M${num(x + r)} ${num(y)}H${num(x + w - r)}${corner(x + w, y + r)}V${num(y + h - r)}${corner(x + w - r, y + h)}`
      + `H${num(x + r)}${corner(x, y + h - r)}V${num(y + r)}${corner(x + r, y)}Z`;
    this.at = this.start = [x + r, y];
  }

  fill() {
    if (this.d) this.out.push(`<path d="${this.d}" fill="${color(this.fillStyle)}"${this.alpha()}/>`);
  }

  stroke() {
    if (this.d) this.out.push(`<path d="${this.d}" fill="none"${this.strokes()}${this.alpha()}/>`);
  }

  clip() {
    const id = `clip${++this.ids}`;
    this.out.push(`<clipPath id="${id}"><path d="${this.d}"/></clipPath><g clip-path="url(#${id})">`);
    this.clips++;
  }

  fillRect(x: number, y: number, w: number, h: number) {
    const k = this.k;
    // A canvas fills a rectangle of negative size towards its other side.
    if (w < 0) [x, w] = [x + w, -w];
    if (h < 0) [y, h] = [y + h, -h];
    this.out.push(`<rect x="${num(x * k)}" y="${num(y * k)}" width="${num(w * k)}" height="${num(h * k)}" fill="${color(this.fillStyle)}"${this.alpha()}/>`);
  }

  // ----- text ------------------------------------------------------------------------

  fillText(text: string, x: number, y: number) {
    this.text(text, x, y, ` fill="${color(this.fillStyle)}"`);
  }

  strokeText(text: string, x: number, y: number) {
    this.text(text, x, y, ` fill="none"${this.strokes()}`);
  }

  private text(text: string, x: number, y: number, paint: string) {
    // As the renderer sets it: `600 10px "Helvetica Neue", …`.
    const [, weight = '400', size = '10', family = 'sans-serif'] = /^(?:(\d+) )?([\d.]+)px (.+)$/.exec(this.font) ?? [];
    const anchor = this.textAlign === 'center' ? 'middle' : this.textAlign === 'right' || this.textAlign === 'end' ? 'end' : 'start';
    // Spaces SVG would collapse are kept (` = value` after a table's key), on the text itself, where browsers read it.
    const keep = /^\s|\s$|\s\s/.test(text) ? ' xml:space="preserve"' : '';
    this.out.push(`<text x="${num(x * this.k)}" y="${num(y * this.k)}" font-family="${escape(family)}" font-size="${num(+size * this.k)}"`
      + `${weight === '400' ? '' : ` font-weight="${weight}"`}${anchor === 'start' ? '' : ` text-anchor="${anchor}"`}${paint}${this.alpha()}${keep}>${escape(text)}</text>`);
  }

  // ----- helpers ---------------------------------------------------------------------

  /** Moves to, or draws a line to, a point in the SVG's pixels. */
  private move(x: number, y: number) {
    this.d += `M${num(x)} ${num(y)}`;
    this.at = this.start = [x, y];
  }

  private line(x: number, y: number) {
    if (!this.at) return this.move(x, y);
    this.d += `L${num(x)} ${num(y)}`;
    this.at = [x, y];
  }

  private strokes(): string {
    const k = this.k;
    let s = ` stroke="${color(this.strokeStyle)}" stroke-width="${num(this.lineWidth * k)}"`;
    if (this.lineJoin !== 'miter') s += ` stroke-linejoin="${this.lineJoin}"`;
    if (this.dash.length) s += ` stroke-dasharray="${this.dash.map(v => num(v * k)).join(' ')}"`;
    if (this.dash.length && this.lineDashOffset) s += ` stroke-dashoffset="${num(this.lineDashOffset * k)}"`;
    return s;
  }

  private alpha(): string {
    return this.globalAlpha < 1 ? ` opacity="${Math.round(this.globalAlpha * 1000) / 1000}"` : '';
  }
}

/** A number as written into the SVG: to a hundredth of a pixel. */
const num = (v: number) => String(Math.round(v * 100) / 100);

/** The renderer paints in colours only. */
const color = (c: string | CanvasGradient | CanvasPattern) => escape(c as string);

/** Text as XML takes it: its markup escaped, and without the control characters XML cannot hold. */
const escape = (s: string) => s.replace(/[\0-\x08\v\f\x0e-\x1f]/g, '').replace(/[&<>"]/g, c => `&${{ '&': 'amp', '<': 'lt', '>': 'gt', '"': 'quot' }[c]};`);

let measurer: CanvasRenderingContext2D | null = null;

/** An SVG of `width` × `height` pixels, drawn by `draw`. */
export function svgOf(width: number, height: number, draw: (pen: SvgPen) => void): string {
  const pen = new SvgPen((measurer ??= document.createElement('canvas').getContext('2d')!));
  draw(pen);
  return pen.svg(width, height);
}

/** The most pixels across a block is exported at: a larger one is drawn smaller, as a screen that size would show it. */
const LARGEST = 4096;
/** Room around a block, for the rings and badges drawn over its edge. */
const MARGIN = 12;

/**
 * Block `id` alone as SVG in `theme`'s colours, with its diagram at the size
 * names are read at on screen; `paint` adds a change being read, or a value
 * being followed.
 */
export function blockSvg(model: SceneModel, id: number, theme: Theme, paint: Pick<Paint, 'diff' | 'trace'> = {}): string {
  const n = model.node(id);
  const owner = model.owner(n);
  // The scale of the diagram the block is in at which its own diagram reads at READABLE.
  const s = Math.min(owner && n.inner ? READABLE / placement(n, owner.cw ?? 1, owner.ch ?? 1).k : 1, LARGEST / Math.max(n.w, n.h));
  const t = { x: MARGIN - n.x * s, y: MARGIN - n.y * s, s };
  const width = Math.ceil(n.w * s) + 2 * MARGIN;
  const height = Math.ceil(n.h * s) + 2 * MARGIN;
  const p: Paint = { width, height, hover: null, selected: null, path: [], xfs: [], ...paint };
  return svgOf(width, height, pen => {
    const r = new Renderer(pen, model, theme);
    // The project itself is drawn as its diagram, as the view of all of it shows it.
    if (n.id === model.root) r.draw({ ...p, path: [n.id], xfs: [t] });
    else r.drawBlock(n, t, p);
  });
}
