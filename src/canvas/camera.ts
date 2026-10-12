import { boxOf, covers, frameAround, frameIn, frameReadable, NO_INSETS, smoothPath, viewOf, xfOf, zoomAt, type Box, type Insets, type View, type Xf } from './geometry';
import type { SceneNode } from '../core/types';

/** How a flight frames its target: `open` reads it (zooming until its
 * diagram is legible), `fit` shows all of it. */
export type Framing = 'open' | 'fit';

/** A planned camera move: its duration, how far it goes (0: nowhere), and its frames. */
export interface Plan { duration: number; length: number; step: (t: number) => void }

/** Exactly where the camera is: its anchor path and the anchor's interior transform. */
export interface CameraState { path: number[]; xf: Xf }
import type { SceneModel } from './model';

/**
 * A camera anchored to the deepest container that fills the viewport.
 *
 * `xf` maps the anchor's interior to the screen; every other transform is
 * derived from it, so precision never degrades no matter how deep the zoom
 * (a root-relative camera would run out of float precision around 1e12×).
 */
export class Camera {
  path: number[];
  xf: Xf = { x: 0, y: 0, s: 1 };
  /** Overlays covering the screen's edges; framing centres in what is left. */
  insets: Insets = NO_INSETS;

  constructor(private model: SceneModel) {
    this.path = [model.root];
  }

  /** Interior transforms along the anchor path, root first. */
  transforms(): Xf[] {
    const out: Xf[] = new Array(this.path.length);
    out[this.path.length - 1] = this.xf;
    for (let i = this.path.length - 1; i > 0; i--) out[i - 1] = this.model.exterior(out[i], this.model.node(this.path[i]));
    return out;
  }

  zoom(factor: number, px: number, py: number) {
    const s = this.xf.s * factor;
    // Keep zoom inside sane bounds relative to the whole scene.
    const rootScale = this.transforms()[0].s * factor;
    if (rootScale < 0.02 && factor < 1) return;
    if (s > 50 && factor > 1 && !this.canDescend()) return;
    this.xf = zoomAt(this.xf, factor, px, py);
  }

  pan(dx: number, dy: number) {
    this.xf = { x: this.xf.x + dx, y: this.xf.y + dy, s: this.xf.s };
  }

  private canDescend(): boolean {
    const n = this.model.node(this.path[this.path.length - 1]);
    const owner = this.model.owner(n);
    return !!owner?.children?.some(c => this.model.owner(this.model.node(c)));
  }

  /** Moves the anchor up while it no longer fills the screen, and down into any child that does. */
  reanchor(width: number, height: number) {
    while (this.path.length > 1) {
      const xfs = this.transforms();
      const n = this.model.node(this.path[this.path.length - 1]);
      if (covers(boxOf(xfs[xfs.length - 2], n), width, height)) break;
      this.xf = xfs[xfs.length - 2];
      this.path.pop();
    }
    for (let guard = 0; guard < 96; guard++) {
      const here = this.model.node(this.path[this.path.length - 1]);
      const owner = this.model.owner(here);
      if (!owner) break;
      let next: number | null = null;
      for (const c of owner.children ?? []) {
        const n = this.model.node(c);
        const o = this.model.owner(n);
        if (!o || this.path.some(id => this.model.owner(this.model.node(id)) === o && o !== n)) continue;
        if (covers(boxOf(this.xf, n), width, height)) {
          next = c;
          break;
        }
      }
      if (next === null) break;
      this.xf = this.model.interior(this.xf, this.model.node(next))!;
      this.path.push(next);
    }
  }

  /** The box of the node at the end of `path`, in the interior frame of `path[upTo]`. */
  boxIn(path: number[], upTo: number): Box {
    return this.place(path, upTo).box;
  }

  /** The node at the end of `path` in the interior frame of `path[upTo]`: its
   * box, the scale of the diagram it sits in, and of its own interior. */
  private place(path: number[], upTo: number): { node: SceneNode; box: Box; outer: number; inner: number | null } {
    let t: Xf = { x: 0, y: 0, s: 1 };
    for (let i = upTo + 1; i < path.length - 1; i++) t = this.model.interior(t, this.model.node(path[i])) ?? t;
    const node = this.model.node(path[path.length - 1]);
    return { node, box: boxOf(t, node), outer: t.s, inner: this.model.interior(t, node)?.s ?? null };
  }

  /**
   * Plans a smooth flight to frame the node at the end of `target` (an
   * instance path). Returns a function of t∈[0,1] that positions the camera.
   */
  flight(target: number[], width: number, height: number, how: Framing = 'fit', pad = 0.1): Plan {
    let common = 0;
    while (common < this.path.length && common < target.length && this.path[common] === target[common]) common++;
    // Fitting a block shows all of it, its title too: it is framed in the diagram around it.
    if (how === 'fit' && target.length > 1) common = Math.min(common, target.length - 1);
    const lca = Math.max(0, common - 1);
    const base = target.slice(0, lca + 1);
    const xfs = this.transforms();
    const from: View = viewOf(xfs[lca], width, height);
    const to = this.framing(target, lca, width, height, how, pad);
    const path = smoothPath(from, to);
    const duration = Math.min(1400, Math.max(320, path.length * 260));
    return {
      duration,
      length: path.length,
      step: (t: number) => {
        this.path = base.slice();
        this.xf = xfOf(path.at(t), width, height);
        this.reanchor(width, height);
      },
    };
  }

  state(): CameraState {
    return { path: this.path.slice(), xf: { ...this.xf } };
  }

  /** Plans a flight back to a saved state (on the same scene). */
  flightTo(saved: CameraState, width: number, height: number): Plan {
    let common = 0;
    while (common < this.path.length && common < saved.path.length && this.path[common] === saved.path[common]) common++;
    const lca = Math.max(0, common - 1);
    const base = saved.path.slice(0, lca + 1);
    // The saved transform, carried up to the common ancestor's interior.
    let t = saved.xf;
    for (let i = saved.path.length - 1; i > lca; i--) t = this.model.exterior(t, this.model.node(saved.path[i]));
    const path = smoothPath(viewOf(this.transforms()[lca], width, height), viewOf(t, width, height));
    return {
      duration: Math.min(1400, Math.max(320, path.length * 260)),
      length: path.length,
      step: (u: number) => {
        this.path = base.slice();
        this.xf = xfOf(path.at(u), width, height);
        this.reanchor(width, height);
      },
    };
  }

  /** Where a flight to `target` ends, in the interior frame of `target[lca]`. */
  private framing(target: number[], lca: number, width: number, height: number, how: Framing, pad: number): View {
    if (target.length - 1 <= lca) {
      if (how === 'fit') return this.contentView(target[lca], width, height, pad);
      const owner = this.model.owner(this.model.node(target[lca])) ?? this.model.node(target[lca]);
      return frameReadable({ x: 0, y: 0, w: owner.cw ?? 1, h: owner.ch ?? 1 }, 1, width, height, pad, this.insets);
    }
    const { node, box, outer, inner } = this.place(target, lca);
    // A block with nothing inside is shown among its neighbours, not blown up alone.
    if (!this.model.owner(node)) return frameAround(box, outer, width, height, this.insets);
    if (how === 'open' && inner) return frameReadable(box, inner, width, height, pad, this.insets);
    return frameIn(box, width, height, pad, this.insets);
  }

  /** View framing a node's whole interior in its own interior frame. */
  contentView(id: number, width: number, height: number, pad = 0.06): View {
    const owner = this.model.owner(this.model.node(id)) ?? this.model.node(id);
    return frameIn({ x: 0, y: 0, w: owner.cw ?? 1, h: owner.ch ?? 1 }, width, height, pad, this.insets);
  }

  fitRoot(width: number, height: number) {
    this.path = [this.model.root];
    this.xf = xfOf(this.contentView(this.model.root, width, height, 0.05), width, height);
  }
}
