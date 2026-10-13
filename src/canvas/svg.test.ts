import { describe, expect, it } from 'vitest';
import type { Project, Scene, SceneNode } from '../core/types';
import { SceneModel } from './model';
import { Renderer } from './renderer';
import { SvgPen } from './svg';

/** Text 6 pixels a character wide. */
const pen = () => new SvgPen({ font: '', measureText: (t: string) => ({ width: t.length * 6 }) as TextMetrics });
const body = (p: SvgPen) => p.svg(100, 100).split('\n').slice(1, -2);

describe('drawing as SVG', () => {
  it('fills and strokes a path as a canvas would', () => {
    const p = pen();
    p.beginPath();
    p.roundRect(10, 20, 100, 50, 6);
    p.fillStyle = '#d6e7fa';
    p.globalAlpha = 0.5;
    p.fill();
    p.globalAlpha = 1;
    p.strokeStyle = '#2c3036';
    p.lineWidth = 1.5;
    p.setLineDash([4, 3]);
    p.lineDashOffset = 2;
    p.stroke();
    expect(body(p)).toEqual([
      '<path d="M16 20H104A6 6 0 0 1 110 26V64A6 6 0 0 1 104 70H16A6 6 0 0 1 10 64V26A6 6 0 0 1 16 20Z" fill="#d6e7fa" opacity="0.5"/>',
      '<path d="M16 20H104A6 6 0 0 1 110 26V64A6 6 0 0 1 104 70H16A6 6 0 0 1 10 64V26A6 6 0 0 1 16 20Z" fill="none" stroke="#2c3036" stroke-width="1.5" stroke-dasharray="4 3" stroke-dashoffset="2"/>',
    ]);
  });

  it('rounds a corner where the line turns, and goes straight where it does not', () => {
    const p = pen();
    p.beginPath();
    p.moveTo(0, 0);
    p.arcTo(10, 0, 10, 10, 4);
    p.arcTo(10, 20, 10, 30, 4);
    p.lineTo(10, 30);
    p.stroke();
    expect(body(p)[0]).toContain('d="M0 0L6 0A4 4 0 0 1 10 4L10 20L10 30"');
  });

  it('draws a whole circle in two halves', () => {
    const p = pen();
    p.beginPath();
    p.arc(50, 50, 10, 0, Math.PI * 2);
    p.fill();
    expect(body(p)[0]).toContain('d="M60 50A10 10 0 0 1 40 50A10 10 0 0 1 60 50"');
  });

  it('writes text at the size it is scaled to, escaped', () => {
    const p = pen();
    p.font = '600 8.41px "Helvetica Neue", sans-serif';
    p.fillStyle = '#1b1e23';
    p.textAlign = 'center';
    p.save();
    p.scale(2);
    p.fillText('a < b & "c"', 20, 10);
    p.restore();
    p.textAlign = 'left';
    p.fillText(' = x', 1, 2);
    expect(body(p)).toEqual([
      '<text x="40" y="20" font-family="&quot;Helvetica Neue&quot;, sans-serif" font-size="16.82" font-weight="600" text-anchor="middle" fill="#1b1e23">a &lt; b &amp; &quot;c&quot;</text>',
      '<text x="1" y="2" font-family="&quot;Helvetica Neue&quot;, sans-serif" font-size="8.41" font-weight="600" fill="#1b1e23" xml:space="preserve"> = x</text>',
    ]);
  });

  it('clips what is drawn until the state is restored', () => {
    const p = pen();
    p.beginPath();
    p.roundRect(0, 0, 10, 10, 0);
    p.save();
    p.clip();
    p.fillStyle = '#ffffff';
    p.fillRect(0, 0, 10, 4);
    p.restore();
    p.fillRect(0, 0, 1, -1);
    expect(body(p)).toEqual([
      '<clipPath id="clip1"><path d="M0 0H10V10H0V0Z"/></clipPath><g clip-path="url(#clip1)">',
      '<rect x="0" y="0" width="10" height="4" fill="#ffffff"/>',
      '</g>',
      '<rect x="0" y="-1" width="1" height="1" fill="#000000"/>',
    ]);
  });
});

describe('a block as SVG', () => {
  it('is drawn alone, with its name', () => {
    const n = (id: number, title: string, extra: Partial<SceneNode>): SceneNode => ({ id, kind: 'function', shape: 'box', category: 'function', title, x: 0, y: 0, w: 10, h: 10, ...extra });
    const nodes = [
      n(0, 'repo', { kind: 'repo', children: [1, 2], w: 400, h: 200, cw: 400, ch: 200 }),
      n(1, 'forward', { parent: 0, x: 10, y: 10, w: 180, h: 80 }),
      n(2, 'backward', { parent: 0, x: 210, y: 10, w: 180, h: 80 }),
    ];
    const scene: Scene = { version: 1, title: 'repo', root: 0, nodes, files: [], diagnostics: [], stats: { files: 0, pythonFiles: 0, lines: 0, nodes: 3, maxDepth: 1 } };
    const project: Project = { name: 'repo', source: 'test', files: [] };
    const p = pen();
    new Renderer(p, new SceneModel(scene, project)).drawBlock(nodes[1], { x: 2, y: 2, s: 1 }, { width: 200, height: 100, hover: null, selected: null, path: [], xfs: [] });
    const svg = p.svg(200, 100);
    expect(svg).toContain('>forward</text>');
    expect(svg).not.toContain('backward');
    expect(svg).toContain('<path d="M18 12H');
  });
});
