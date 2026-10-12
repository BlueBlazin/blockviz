import { describe, expect, it } from 'vitest';
import type { Scene, SceneNode } from '../core/types';
import { Camera } from './camera';
import { SceneModel } from './model';

// A repository 100 units wide holding, in its top-left corner, a file whose
// diagram is ten times smaller and holds one step; the rest is empty.
function scene(): SceneModel {
  const n = (id: number, over: Partial<SceneNode>): SceneNode => ({ id, kind: 'function', shape: 'box', category: 'function', title: `n${id}`, x: 0, y: 0, w: 10, h: 10, ...over });
  const nodes: SceneNode[] = [
    n(0, { kind: 'repo', w: 100, h: 100, children: [1], inner: { x: 0, y: 0, w: 100, h: 100 }, cw: 100, ch: 100 }),
    n(1, { kind: 'file', parent: 0, children: [2], inner: { x: 0, y: 0, w: 10, h: 10 }, cw: 100, ch: 100 }),
    n(2, { kind: 'op', parent: 1, x: 40, y: 40, w: 20, h: 20 }),
  ];
  const s: Scene = { version: 1, title: 'repo', root: 0, nodes, files: [], diagnostics: [], stats: { files: 1, pythonFiles: 1, lines: 1, nodes: 3, maxDepth: 2 } };
  return new SceneModel(s, { name: 'repo', source: 'test', files: [] });
}

/** Zooms in at a screen point as far as the camera goes; the interior scale where it stops. */
function zoomIn(cam: Camera, px: number, py: number): number {
  for (let i = 0; i < 500; i++) {
    cam.zoom(1.2, px, py);
    cam.reanchor(1000, 1000);
  }
  return cam.xf.s;
}

describe('zooming in', () => {
  it('stops in empty space as it does over a block with nothing inside', () => {
    const cam = new Camera(scene());
    cam.fitRoot(1000, 1000);
    // A point in the empty part of the repository, 80 units across and down.
    const s = zoomIn(cam, cam.xf.x + 80 * cam.xf.s, cam.xf.y + 80 * cam.xf.s);
    expect(cam.path).toEqual([0]);
    expect(s).toBeLessThanOrEqual(50);
    expect(s).toBeGreaterThan(50 / 1.2);
  });

  it('goes as deep as the innermost diagram under the point', () => {
    const cam = new Camera(scene());
    cam.fitRoot(1000, 1000);
    // The middle of the file's one step, 5 units across and down.
    const s = zoomIn(cam, cam.xf.x + 5 * cam.xf.s, cam.xf.y + 5 * cam.xf.s);
    expect(cam.path).toEqual([0, 1]);
    expect(s).toBeLessThanOrEqual(50);
    expect(s).toBeGreaterThan(50 / 1.2);
    expect(cam.transforms()[0].s).toBeGreaterThan(400);
  });
});
