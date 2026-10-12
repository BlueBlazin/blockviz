/** Pure camera and placement math (no DOM). */
import type { SceneNode } from '../core/types';

/** Maps a content coordinate system to the screen: screen = (x + u·s, y + v·s). */
export interface Xf { x: number; y: number; s: number }
export interface Box { x: number; y: number; w: number; h: number }
/** A view described in some content frame: center and visible width. */
export interface View { cx: number; cy: number; w: number }

export const boxOf = (t: Xf, n: Pick<SceneNode, 'x' | 'y' | 'w' | 'h'>): Box => ({ x: t.x + n.x * t.s, y: t.y + n.y * t.s, w: n.w * t.s, h: n.h * t.s });

export const intersects = (b: Box, w: number, h: number, margin = 0) => b.x < w + margin && b.y < h + margin && b.x + b.w > -margin && b.y + b.h > -margin;

export const covers = (b: Box, w: number, h: number) => b.x <= 0 && b.y <= 0 && b.x + b.w >= w && b.y + b.h >= h;

/** Interior placement of a box: the content (cw×ch) fitted and centred inside `inner`. */
export function placement(n: SceneNode, cw: number, ch: number) {
  const inner = n.inner!;
  const k = Math.min(inner.w / Math.max(cw, 1e-9), inner.h / Math.max(ch, 1e-9));
  return { k, ox: inner.x + (inner.w - cw * k) / 2, oy: inner.y + (inner.h - ch * k) / 2 };
}

export function childXf(t: Xf, n: SceneNode, cw: number, ch: number): Xf {
  const p = placement(n, cw, ch);
  return { x: t.x + (n.x + p.ox) * t.s, y: t.y + (n.y + p.oy) * t.s, s: t.s * p.k };
}

export function parentXf(c: Xf, n: SceneNode, cw: number, ch: number): Xf {
  const p = placement(n, cw, ch);
  const s = c.s / p.k;
  return { x: c.x - (n.x + p.ox) * s, y: c.y - (n.y + p.oy) * s, s };
}

export function zoomAt(t: Xf, factor: number, px: number, py: number): Xf {
  return { x: px - (px - t.x) * factor, y: py - (py - t.y) * factor, s: t.s * factor };
}

export function viewOf(t: Xf, width: number, height: number): View {
  return { cx: (width / 2 - t.x) / t.s, cy: (height / 2 - t.y) / t.s, w: width / t.s };
}

export function xfOf(v: View, width: number, height: number): Xf {
  const s = width / v.w;
  return { x: width / 2 - v.cx * s, y: height / 2 - v.cy * s, s };
}

/** The view that frames a box with padding, keeping the screen's aspect. */
export function frame(b: Box, width: number, height: number, pad = 0.12): View {
  const aspect = width / Math.max(height, 1);
  const w = Math.max(b.w, b.h * aspect) * (1 + pad * 2);
  return { cx: b.x + b.w / 2, cy: b.y + b.h / 2, w };
}

/** Screen margins covered by overlays (toolbars, the details panel). */
export interface Insets { top: number; right: number; bottom: number; left: number }
export const NO_INSETS: Insets = { top: 0, right: 0, bottom: 0, left: 0 };

/** Like `frame`, but centres the box in the part of the screen overlays leave visible. */
export function frameIn(b: Box, width: number, height: number, pad: number, i: Insets): View {
  const vw = Math.max(width - i.left - i.right, width * 0.3);
  const vh = Math.max(height - i.top - i.bottom, height * 0.3);
  const v = frame(b, vw, vh, pad);
  const w = (v.w * width) / vw;
  const k = w / width;
  return { cx: v.cx - ((i.left - i.right) / 2) * k, cy: v.cy - ((i.top - i.bottom) / 2) * k, w };
}

/** Interior scale at which names (13 units) read comfortably, and the least
 * that counts as readable. */
export const READABLE = 1;
const READABLE_MIN = 0.8;

/** The part of the screen overlays leave visible. */
function visible(width: number, height: number, i: Insets) {
  return { vw: Math.max(width - i.left - i.right, width * 0.3), vh: Math.max(height - i.top - i.bottom, height * 0.3) };
}

/** The view at screen scale `s` (pixels per frame unit) that puts frame point
 * (x, y) at screen point (sx, sy). */
function viewAt(x: number, y: number, sx: number, sy: number, s: number, width: number, height: number): View {
  return { cx: x + (width / 2 - sx) / s, cy: y + (height / 2 - sy) / s, w: width / s };
}

/**
 * Frames a box so the diagram inside it can be read. `k` is the size of one
 * interior unit in frame units. The whole box when that is readable already;
 * otherwise as much as readable text needs, never past the box's full width,
 * starting at its top (diagrams read top to bottom).
 */
export function frameReadable(b: Box, k: number, width: number, height: number, pad: number, i: Insets): View {
  const whole = frameIn(b, width, height, pad, i);
  const s0 = width / whole.w;
  if (s0 * k >= READABLE_MIN) return whole;
  const { vw, vh } = visible(width, height, i);
  const s = Math.max(s0, Math.min(READABLE / k, vw / (b.w * 1.06)));
  const sy = b.h * s <= vh - 32 ? i.top + vh / 2 - (b.h * s) / 2 : i.top + 16;
  return viewAt(b.x + b.w / 2, b.y, i.left + vw / 2, sy, s, width, height);
}

/**
 * Frames a block with nothing inside among its neighbours: centred, at the
 * size its diagram reads at (`k`: one unit of the diagram it sits in, in frame
 * units), and never filling more than about half the screen.
 */
export function frameAround(b: Box, k: number, width: number, height: number, i: Insets): View {
  const { vw, vh } = visible(width, height, i);
  const s = Math.min((READABLE * 1.15) / k, (vw * 0.55) / Math.max(b.w, 1e-9), (vh * 0.55) / Math.max(b.h, 1e-9));
  return viewAt(b.x + b.w / 2, b.y + b.h / 2, i.left + vw / 2, i.top + vh / 2, s, width, height);
}

/**
 * Smooth and efficient zooming and panning (van Wijk & Nuij, 2003): the
 * perceptually shortest path between two views zooms out, pans, then zooms in.
 */
export function smoothPath(a: View, b: View, rho = 1.35) {
  const dx = b.cx - a.cx;
  const dy = b.cy - a.cy;
  const d = Math.hypot(dx, dy);
  const w0 = a.w;
  const w1 = b.w;
  if (d < 1e-9 * Math.max(w0, w1)) {
    const S = Math.abs(Math.log(w1 / w0)) / rho;
    return { length: S, at: (t: number): View => ({ cx: a.cx, cy: a.cy, w: w0 * Math.exp(rho * S * t * Math.sign(Math.log(w1 / w0))) }) };
  }
  const r = (i: 0 | 1) => {
    const wi = i ? w1 : w0;
    const bi = (w1 * w1 - w0 * w0 + (i ? -1 : 1) * rho ** 4 * d * d) / (2 * wi * rho * rho * d);
    return Math.log(-bi + Math.sqrt(bi * bi + 1));
  };
  const r0 = r(0);
  const r1 = r(1);
  const S = (r1 - r0) / rho;
  const at = (t: number): View => {
    const s = t * S;
    const u = (w0 / (rho * rho * d)) * (Math.cosh(r0) * Math.tanh(rho * s + r0) - Math.sinh(r0));
    const w = (w0 * Math.cosh(r0)) / Math.cosh(rho * s + r0);
    return { cx: a.cx + dx * u, cy: a.cy + dy * u, w };
  };
  return { length: Number.isFinite(S) ? S : 0, at };
}

export const clamp = (v: number, lo: number, hi: number) => Math.max(lo, Math.min(hi, v));
export const smoothstep = (a: number, b: number, v: number) => {
  const t = clamp((v - a) / (b - a), 0, 1);
  return t * t * (3 - 2 * t);
};
