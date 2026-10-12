import type { CameraState } from '../canvas/camera';
import type { Engine } from '../canvas/engine';
import { decodePath, encodePath } from '../canvas/location';
import type { SceneModel } from '../canvas/model';

export interface Selection { id: number; path: number[] }

/** A history entry: where the reader was, exactly, on which analysis. `i`
 * counts entries across projects, and `max` is the last one ahead of it. */
interface Entry { key: string; i: number; max: number; view: CameraState; sel: Selection | null }

/** The position a history state records, whatever wrote it (an entry, or a project being opened). */
function position(s: unknown): { i: number; max: number } | null {
  if (!s || typeof s !== 'object' || typeof (s as { i?: unknown }).i !== 'number') return null;
  const { i, max } = s as { i: number; max?: number };
  return { i, max: Math.max(i, typeof max === 'number' ? max : i) };
}

/** Starts a history entry for opening another project, at `url`. */
export function pushProject(from: Navigation | null, url: string) {
  try { history.pushState({ i: (from?.index ?? 0) + 1 }, '', url); } catch { /* history is a convenience */ }
}

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
  private written = '';
  /** Another project is loading: this one no longer owns the address. */
  private stopped = false;

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
    const at = position(history.state);
    if (at) {
      this.index = at.i;
      this.max = at.max;
    }
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
    if (this.stopped) return;
    this.write(false);
    this.index++;
    this.max = this.index;
    this.write(true);
    this.changed();
  }

  /** Back or forward landed on `state`; false when it belongs to another project. */
  pop(state: unknown): boolean {
    if (this.stopped) return false;
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

  /** Stops writing the address: another project is being loaded into it. */
  stop() {
    this.stopped = true;
    clearTimeout(this.timer);
  }

  /** The other project failed to load: this one, still on screen, owns the address again. */
  resume() {
    this.stopped = false;
    this.written = '';
    this.write(false);
  }

  dispose() {
    this.stop();
  }

  private schedule() {
    if (this.stopped) return;
    clearTimeout(this.timer);
    this.timer = window.setTimeout(() => this.write(false), 250);
  }

  private write(push: boolean) {
    clearTimeout(this.timer);
    if (this.stopped) return;
    const entry: Entry = { key: this.key, i: this.index, max: this.max, view: this.engine.state(), sel: this.sel };
    const params = [...this.base];
    const at = encodePath(this.model, this.engine.focus());
    if (at) params.push(['at', at]);
    if (this.sel) params.push(['sel', encodePath(this.model, this.sel.path)]);
    const hash = params.map(([k, v]) => `${k}=${v}`).join('&');
    const url = `${location.pathname}${location.search}${hash ? `#${hash}` : ''}`;
    // Redraws that did not move anything leave history alone (browsers limit how often it may change).
    const text = `${url} ${JSON.stringify(entry)}`;
    if (!push && text === this.written) return;
    this.written = text;
    try {
      if (push) history.pushState(entry, '', url);
      else history.replaceState(entry, '', url);
    } catch { /* over the browser's rate limit: the next change writes it */ }
  }
}
