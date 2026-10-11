import { boxOf, covers, frame, smoothPath, viewOf, xfOf, zoomAt, type Box, type View, type Xf } from './geometry';
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
    let t: Xf = { x: 0, y: 0, s: 1 };
    for (let i = upTo + 1; i < path.length - 1; i++) t = this.model.interior(t, this.model.node(path[i])) ?? t;
    return boxOf(t, this.model.node(path[path.length - 1]));
  }

  /**
   * Plans a smooth flight to frame the node at the end of `target` (an
   * instance path). Returns a function of t∈[0,1] that positions the camera.
   */
  flight(target: number[], width: number, height: number, pad = 0.1): { duration: number; step: (t: number) => void } {
    let common = 0;
    while (common < this.path.length && common < target.length && this.path[common] === target[common]) common++;
    const lca = Math.max(0, common - 1);
    const base = target.slice(0, lca + 1);
    const xfs = this.transforms();
    const from: View = viewOf(xfs[lca], width, height);
    const to: View = target.length - 1 > lca ? frame(this.boxIn(target, lca), width, height, pad) : this.contentView(target[lca], width, height, pad);
    const path = smoothPath(from, to);
    const duration = Math.min(1400, Math.max(320, path.length * 260));
    return {
      duration,
      step: (t: number) => {
        this.path = base.slice();
        this.xf = xfOf(path.at(t), width, height);
        this.reanchor(width, height);
      },
    };
  }

  /** View framing a node's whole interior in its own interior frame. */
  contentView(id: number, width: number, height: number, pad = 0.06): View {
    const owner = this.model.owner(this.model.node(id)) ?? this.model.node(id);
    return frame({ x: 0, y: 0, w: owner.cw ?? 1, h: owner.ch ?? 1 }, width, height, pad);
  }

  fitRoot(width: number, height: number) {
    this.path = [this.model.root];
    this.xf = xfOf(this.contentView(this.model.root, width, height, 0.05), width, height);
  }
}
