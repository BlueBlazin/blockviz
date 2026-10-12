import type { CameraState } from '../canvas/camera';
import type { Engine } from '../canvas/engine';
import { decodePath, encodePath } from '../canvas/location';
import type { SceneModel } from '../canvas/model';

export interface Selection { id: number; path: number[] }

/** A history entry: where the reader was, exactly, on which analysis. */
interface Entry { key: string; i: number; view: CameraState; sel: Selection | null }

const isEntry = (s: unknown): s is Entry => !!s && typeof s === 'object' && 'key' in s && 'view' in s;

/** The URL fragment's parameters, values still escaped (a path's segments are escaped one by one). */
export function hashParams(hash = location.hash): Map<string, string> {
  const out = new Map<string, string>();
  for (const part of hash.replace(/^#/, '').split('&')) {
    if (!part) continue;
    const eq = part.indexOf('=');
    out.set(eq < 0 ? part : part.slice(0, eq), eq < 0 ? '' : part.slice(eq + 1));
  }
  return out;
}

/** A plain (not path) fragment parameter, unescaped. */
export function hashParam(name: string): string | null {
  const v = hashParams().get(name);
  if (v === undefined) return null;
  try { return decodeURIComponent(v.replace(/\+/g, ' ')); } catch { return v; }
}

/**
 * The reader's trail through one project, kept in the browser's history:
 * every jump (search, go to, zooming into a block, breadcrumbs) is a step
 * Back undoes, and the URL always names the current view and selection
 * (`#at=model.py/GPT/forward&sel=…`), so it can be shared or reloaded.
 */
export class Navigation {
  index = 0;
  max = 0;
  private sel: Selection | null = null;
  private timer = 0;

  constructor(
    private model: SceneModel,
    private engine: Engine,
    /** Identifies the analysis, so entries from another project are not replayed here. */
    private key: string,
    /** URL parameters that load the project (`github=…`, escaped), kept in every entry. */
    private base: [string, string][],
    private select: (sel: Selection | null) => void,
    private changed: () => void,
  ) {
    const state: unknown = history.state;
    if (isEntry(state) && state.key === key) this.index = this.max = state.i;
    this.fromUrl();
    this.write(false);
  }

  /** Goes where the URL says (a shared link), without adding a step. */
  fromUrl() {
    const params = hashParams();
    const at = params.get('at') ?? null;
    const sel = params.get('sel') ?? null;
    if (at !== null) this.engine.flyTo(decodePath(this.model, at), 'fit', 0.04, false);
    // A link names its selection, or none.
    const path = sel === null ? [] : decodePath(this.model, sel);
    this.select(path.length > 1 ? { id: path[path.length - 1], path } : null);
  }

  get canBack() { return this.index > 0; }
  get canForward() { return this.index < this.max; }

  setSelection(sel: Selection | null) {
    this.sel = sel;
    this.schedule();
  }

  /** The view moved: keep the current entry (and URL) up to date. */
  viewChanged() {
    this.schedule();
  }

  /** A jump is starting: the current entry keeps where the reader was, and a new one follows. */
  jump() {
    this.write(false);
    this.index++;
    this.max = this.index;
    this.write(true);
    this.changed();
  }

  /** Back or forward landed on `state`; false when it belongs to another project. */
  pop(state: unknown): boolean {
    if (!isEntry(state)) {
      this.fromUrl();
      return true;
    }
    if (state.key !== this.key) return false;
    this.index = state.i;
    this.engine.restore(state.view);
    this.select(state.sel);
    this.changed();
    return true;
  }

  dispose() {
    clearTimeout(this.timer);
  }

  private schedule() {
    clearTimeout(this.timer);
    this.timer = window.setTimeout(() => this.write(false), 250);
  }

  private write(push: boolean) {
    clearTimeout(this.timer);
    const entry: Entry = { key: this.key, i: this.index, view: this.engine.state(), sel: this.sel };
    const params = [...this.base];
    const at = encodePath(this.model, this.engine.focus());
    if (at) params.push(['at', at]);
    if (this.sel) params.push(['sel', encodePath(this.model, this.sel.path)]);
    const hash = params.map(([k, v]) => `${k}=${v}`).join('&');
    const url = `${location.pathname}${location.search}${hash ? `#${hash}` : ''}`;
    if (push) history.pushState(entry, '', url);
    else history.replaceState(entry, '', url);
  }
}
