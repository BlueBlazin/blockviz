/**
 * Visual language, modelled on classic architecture figures: pastel fills by
 * role, dark hairline outlines, grey regions for repeated or conditional
 * structure, and monochrome arrows.
 */

export interface Swatch { fill: string; tint: string; ink?: string }

const light: Record<string, Swatch> = {
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

export const theme = {
  background: '#ffffff',
  grid: '#eceef1',
  ink: '#1b1e23',
  muted: '#5b626d',
  faint: '#8b929c',
  outline: '#2c3036',
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
  sans: '"Helvetica Neue", Helvetica, -apple-system, "Segoe UI", Arial, sans-serif',
  mono: '"SF Mono", ui-monospace, Menlo, Consolas, monospace',
};

export function swatch(category: string): Swatch {
  return light[category] ?? light.neutral;
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
