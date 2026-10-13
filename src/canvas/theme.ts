import type { Status } from './diff';

/**
 * Visual language, modelled on classic architecture figures: pastel fills by
 * role, hairline outlines, grey regions for repeated or conditional
 * structure, and monochrome arrows. On a dark page the same, its lightness
 * turned over.
 */

export interface Swatch { fill: string; tint: string; ink?: string }

/** The colours a diagram is drawn in. */
export interface Theme {
  /** The page under the diagram. */
  background: string;
  /** What a block drawn as a blank card is filled with, and what text over a busy picture is backed with. */
  paper: string;
  ink: string;
  muted: string;
  faint: string;
  outline: string;
  /** The outline of an operation's box, lighter than a definition's. */
  opOutline: string;
  regionOutline: string;
  laneOutline: string;
  edge: string;
  context: string;
  faintEdge: string;
  artifact: string;
  accent: string;
  accentSoft: string;
  labelHalo: string;
  labelInk: string;
  effect: string;
  /** A table's headings, and their lines while too small to read. */
  heading: string;
  headingTexture: string;
  sans: string;
  mono: string;
  /** Fills by a block's role (see `swatch`). */
  swatches: Record<string, Swatch>;
  /** Colours of a change being read (a pull request): outlines and text (`ink`), and the fill blocks are tinted with. */
  change: Record<Status, { ink: string; tint: string }>;
}

const swatches: Record<string, Swatch> = {
  // Neural-network vocabulary (matches the colours of the Transformer figure).
  attention: { fill: '#fbdcbc', tint: '#fef6ee' },
  mlp: { fill: '#c6e5f5', tint: '#f1f9fd' },
  norm: { fill: '#eff3c4', tint: '#fbfcef' },
  embed: { fill: '#f6d2d6', tint: '#fdf3f4' },
  linear: { fill: '#d9daf3', tint: '#f5f5fc' },
  activation: { fill: '#cfe9d0', tint: '#f2faf2' },
  dropout: { fill: '#e6e6e4', tint: '#f8f8f7' },
  loss: { fill: '#f3cdd5', tint: '#fdf2f4' },
  // General code roles.
  component: { fill: '#f9e2cb', tint: '#fef8f2' },
  function: { fill: '#d6e7fa', tint: '#f4f8fe' },
  endpoint: { fill: '#e6d9f7', tint: '#f8f4fd' },
  class: { fill: '#dde0f8', tint: '#f6f7fe' },
  library: { fill: '#e9e9f1', tint: '#f8f8fb' },
  io: { fill: '#cdeadf', tint: '#f1faf6' },
  config: { fill: '#f7edc8', tint: '#fdfaee' },
  data: { fill: '#f7d6de', tint: '#fdf3f5' },
  op: { fill: '#ffffff', tint: '#ffffff' },
  error: { fill: '#f9d6d3', tint: '#fef3f2' },
  kernel: { fill: '#cdeadf', tint: '#f1faf6' },
  section: { fill: '#e7ebf1', tint: '#f9fafc' },
  file: { fill: '#e4e9f0', tint: '#fbfcfd' },
  dir: { fill: '#eceef1', tint: '#fbfbfc' },
  doc: { fill: '#efefea', tint: '#fbfbf9' },
  asset: { fill: '#efefea', tint: '#fbfbf9' },
  meta: { fill: '#f1f1f1', tint: '#fbfbfb' },
  region: { fill: '#f1f1ef', tint: '#f1f1ef' },
  lane: { fill: '#fafaf9', tint: '#fafaf9' },
  repo: { fill: '#ffffff', tint: '#ffffff' },
  neutral: { fill: '#eef0f3', tint: '#fafbfc' },
};

const fonts = {
  sans: '"Helvetica Neue", Helvetica, -apple-system, "Segoe UI", Arial, sans-serif',
  mono: '"SF Mono", ui-monospace, Menlo, Consolas, monospace',
};

export const LIGHT: Theme = {
  background: '#ffffff',
  paper: '#ffffff',
  ink: '#1b1e23',
  muted: '#5b626d',
  faint: '#8b929c',
  outline: '#2c3036',
  opOutline: '#7d838c',
  regionOutline: '#b9bdc3',
  laneOutline: '#cbcfd4',
  edge: '#2c3036',
  context: '#a3aab4',
  faintEdge: '#7c838e',
  artifact: '#9a5566',
  accent: '#2563eb',
  accentSoft: 'rgba(37, 99, 235, 0.14)',
  labelHalo: '#ffffff',
  labelInk: '#3d434c',
  effect: '#d97706',
  heading: '#8a6d1f',
  headingTexture: '#b39b52',
  ...fonts,
  swatches,
  change: {
    added: { ink: '#1a7f37', tint: '#c9f2d4' },
    changed: { ink: '#bf8700', tint: '#fbecb3' },
    removed: { ink: '#cf222e', tint: '#ffd8d6' },
  },
};

/** The page of the dark theme, and its lightness: keep in step with `--bg` in styles.css. */
const NIGHT = '#16181c';
const PAGE_L = 0.098;

export const DARK: Theme = {
  background: NIGHT,
  paper: NIGHT,
  ink: '#e6e8eb',
  muted: '#a3a9b3',
  faint: '#7c838d',
  outline: '#9aa2ad',
  opOutline: '#6b727c',
  regionOutline: '#454a53',
  laneOutline: '#3b4048',
  edge: '#b4bbc5',
  context: '#646b75',
  faintEdge: '#8a919b',
  artifact: '#d08a9b',
  accent: '#5b9bff',
  accentSoft: 'rgba(91, 155, 255, 0.24)',
  labelHalo: NIGHT,
  labelInk: '#c3c8cf',
  effect: '#f59e0b',
  heading: '#d1ae55',
  headingTexture: '#8c7a45',
  ...fonts,
  swatches: Object.fromEntries(Object.entries(swatches).map(([role, s]) => [role, { fill: night(s.fill), tint: night(s.tint) }])),
  change: {
    added: { ink: '#3fb950', tint: '#1e4a2b' },
    changed: { ink: '#d29922', tint: '#4a3b12' },
    removed: { ink: '#f85149', tint: '#552321' },
  },
};

export function swatch(theme: Theme, category: string): Swatch {
  return theme.swatches[category] ?? theme.swatches.neutral;
}

/**
 * A light theme's colour on the dark page: the same hue, as far above the
 * page in lightness as it was below white, and about as colourful (its
 * chroma, not its saturation, which near white overstates a faint tint).
 * White is the page itself.
 */
function night(hex: string): string {
  const [r, g, b] = [1, 3, 5].map(i => parseInt(hex.slice(i, i + 2), 16) / 255);
  const max = Math.max(r, g, b);
  const c = max - Math.min(r, g, b);
  const h = !c ? 0 : max === r ? ((g - b) / c + 6) % 6 : max === g ? (b - r) / c + 2 : (r - g) / c + 4;
  // A grey is the page's own grey: its hue and chroma.
  const [hue, chroma] = c ? [h, c * 0.75] : [11 / 3, 6 / 255];
  return rgb(hue, chroma, PAGE_L + (1 - (max - c / 2)) * 1.25);
}

/** The colour of hue `h` (0 to 6), chroma `c` and lightness `l`. */
function rgb(h: number, c: number, l: number): string {
  const x = c * (1 - Math.abs((h % 2) - 1));
  const [r, g, b] = h < 1 ? [c, x, 0] : h < 2 ? [x, c, 0] : h < 3 ? [0, c, x] : h < 4 ? [0, x, c] : h < 5 ? [x, 0, c] : [c, 0, x];
  return `#${[r, g, b].map(v => Math.round((v + l - c / 2) * 255).toString(16).padStart(2, '0')).join('')}`;
}

/** Categories whose names carry meaning in a legend. */
export const legend: [string, string][] = [
  ['function', 'Function / method'],
  ['endpoint', 'Route, command or task'],
  ['class', 'Class'],
  ['component', 'Sub-module'],
  ['library', 'Library call'],
  ['io', 'I/O & side effects'],
  ['config', 'Settings'],
  ['data', 'File on disk'],
  ['attention', 'Attention'],
  ['mlp', 'Feed-forward'],
  ['norm', 'Normalisation'],
  ['embed', 'Embedding'],
  ['linear', 'Linear / projection'],
  ['activation', 'Activation'],
];
