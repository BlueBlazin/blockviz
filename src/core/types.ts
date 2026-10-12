/** Mirror of the Rust scene IR (crates/blockviz-core/src/ir.rs). */

export type Kind =
  | 'repo' | 'dir' | 'file' | 'notebook' | 'doc' | 'asset'
  | 'class' | 'function' | 'section' | 'loop' | 'branch' | 'lane' | 'group'
  | 'call' | 'op' | 'glyph' | 'table' | 'artifact' | 'symbol' | 'input' | 'output';

export type Shape = 'box' | 'region' | 'glyph' | 'port';
export type EdgeKind = 'data' | 'control' | 'uses' | 'exec' | 'artifact';

export interface Rect { x: number; y: number; w: number; h: number }
export interface Span { file: number; start: number; end: number; line: number; endLine: number }
export interface Row { key: string; value?: string; note?: string; heading?: boolean }

export interface Edge {
  from: number;
  to: number;
  kind: EdgeKind;
  label?: string;
  /** Polyline in the parent's interior coordinates: x0, y0, x1, y1, ... */
  points: number[];
  labelAt?: [number, number];
  /** The line continues into a region instead of ending in an arrowhead. */
  open?: boolean;
}

export interface SceneNode {
  id: number;
  parent?: number;
  kind: Kind;
  shape: Shape;
  category: string;
  title: string;
  subtitle?: string;
  doc?: string;
  /** Source shown for the block when it is not simply the file's text at `span`. */
  code?: string;
  badges?: string[];
  effects?: string[];
  inputs?: string[];
  outputs?: string[];
  span?: Span;
  /** Interior borrowed from another node: a call shows its callee. */
  ref?: number;
  /** Where "go to definition" leads. */
  link?: number;
  /** Blocks that use this one without linking to it (a component's calls). */
  usedBy?: number[];
  x: number;
  y: number;
  w: number;
  h: number;
  inner?: Rect;
  cw?: number;
  ch?: number;
  children?: number[];
  edges?: Edge[];
  rows?: Row[];
}

export interface FileMeta { path: string; language: string; bytes: number; lines: number; analyzed: boolean; source?: string }

export interface Scene {
  version: number;
  title: string;
  description?: string;
  root: number;
  nodes: SceneNode[];
  files: FileMeta[];
  diagnostics: string[];
  stats: { files: number; pythonFiles: number; lines: number; nodes: number; maxDepth: number };
}

export interface InputFile { path: string; text?: string; bytes: number }
export interface Project { name: string; files: InputFile[]; source: string; description?: string }

/** How a file differs between the two versions compared. */
export type FileChange = { path: string; status: 'added' | 'removed' | 'modified' | 'renamed'; previous?: string };

/** What two versions of a repository are: a pull request, or two branches compared. */
export interface CompareInfo {
  kind: 'pr' | 'compare';
  /** `owner/repo`. */
  repo: string;
  number?: number;
  /** The pull request's title, or `base...head`. */
  title: string;
  /** The comparison on GitHub. */
  url: string;
  /** `open`, `closed` or `merged` for a pull request. */
  state?: string;
  baseRef: string;
  headRef: string;
  /** The commits compared: where the head branched off the base, and the head. */
  baseSha: string;
  headSha: string;
  files: FileChange[];
}

/** Two versions of a repository, read as a change from `base` to `head`. */
export interface Comparison { base: Project; head: Project; info: CompareInfo }

export type AnalysisResult = { ok: true; scene: Scene; elapsed: number } | { ok: false; error: string };
