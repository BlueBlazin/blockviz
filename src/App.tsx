import { ChevronRight, CircleHelp, Download, FolderOpen, Home, Info, Maximize, Minus, Plus, Search as SearchIcon, X } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Analyzer } from './core/analyzer';
import { droppedProject, exampleProject, folderProject, githubProject, pastedProject } from './core/loaders';
import type { Project, Scene } from './core/types';
import { Engine, type ViewState } from './canvas/engine';
import type { Insets } from './canvas/geometry';
import { SceneModel } from './canvas/model';
import { legend, swatch } from './canvas/theme';
import { Highlights } from './core/highlights';
import { clampPanel, DEFAULT_PANEL, Inspector, type PanelSize } from './ui/Inspector';
import { OpenDialog, type OpenRequest } from './ui/OpenDialog';
import { Search } from './ui/Search';

interface Loaded { project: Project; scene: Scene; model: SceneModel; highlights: Highlights; elapsed: number }

const storage = {
  get(key: string) { try { return localStorage.getItem(key) ?? ''; } catch { return ''; } },
  set(key: string, value: string) { try { localStorage.setItem(key, value); } catch { /* optional */ } },
};

/** Screen margins covered by the top bar, footer and (when open) the details panel. */
function overlayInsets(panelOpen: boolean, panel: PanelSize): Insets {
  const narrow = window.matchMedia('(max-width: 760px)').matches;
  return {
    top: narrow ? 100 : 56,
    right: panelOpen && !narrow ? panel.w + 24 : 12,
    bottom: panelOpen && narrow ? (panel.h || window.innerHeight * 0.46) + 24 : 52,
    left: 12,
  };
}

export default function App() {
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [busy, setBusy] = useState<string | null>('Starting…');
  const [error, setError] = useState<string | null>(null);
  const [view, setView] = useState<ViewState>({ path: [], zoom: 1 });
  const [selected, setSelected] = useState<number | null>(null);
  const [selectedPath, setSelectedPath] = useState<number[] | null>(null);
  const [dialog, setDialog] = useState<'open' | 'search' | 'help' | null>(null);
  const [showLegend, setShowLegend] = useState(false);
  const [dragging, setDragging] = useState(false);
  const [panel, setPanel] = useState<PanelSize>(() => clampPanel({ w: Number(storage.get('blockviz.panel.w')) || DEFAULT_PANEL.w, h: Number(storage.get('blockviz.panel.h')) || 0 }));
  const canvas = useRef<HTMLCanvasElement>(null);
  const engine = useRef<Engine | null>(null);
  const analyzer = useRef(new Analyzer());
  const request = useRef(0);

  const load = useCallback(async (make: (progress: (m: string) => void) => Promise<Project>) => {
    const id = ++request.current;
    setError(null);
    setBusy('Loading…');
    try {
      const project = await make(m => { if (id === request.current) setBusy(m); });
      if (id !== request.current) return;
      if (!project.files.length) throw new Error('No files found.');
      setBusy(`Analyzing ${project.files.length} files…`);
      const result = await analyzer.current.analyze(project);
      if (id !== request.current) return;
      if (!result.ok) throw new Error(result.error);
      setSelected(null);
      setSelectedPath(null);
      const model = new SceneModel(result.scene, project);
      setLoaded({ project, scene: result.scene, model, highlights: new Highlights(analyzer.current, model), elapsed: result.elapsed });
      setDialog(null);
      document.title = `${result.scene.title} · blockviz`;
    } catch (e) {
      if (id !== request.current || String(e) === 'Error: cancelled') return;
      setError(e instanceof Error ? e.message : String(e));
      setDialog(d => d ?? 'open');
    } finally {
      if (id === request.current) setBusy(null);
    }
  }, []);

  const open = useCallback((r: OpenRequest) => {
    if (r.kind === 'example') void load(() => exampleProject());
    if (r.kind === 'github') {
      storage.set('blockviz.github', r.url);
      history.replaceState(null, '', `#github=${encodeURIComponent(r.url.replace(/^https?:\/\//, ''))}`);
      void load(p => githubProject(r.url, p));
    }
    if (r.kind === 'folder') void load(p => folderProject(r.files, p));
    if (r.kind === 'paste') void load(async () => pastedProject(r.filename, r.text));
  }, [load]);

  // First load: a shared GitHub link, else the bundled example.
  useEffect(() => {
    const hash = new URLSearchParams(location.hash.slice(1)).get('github');
    if (hash) void load(p => githubProject(hash, p));
    else void load(() => exampleProject());
    return () => analyzer.current.cancel();
  }, [load]);

  useEffect(() => {
    if (!loaded || !canvas.current) return;
    const e = new Engine(canvas.current, loaded.model, {
      onView: setView,
      onSelect: (id, path) => { setSelected(id); setSelectedPath(path); },
    }, overlayInsets(false, DEFAULT_PANEL));
    engine.current = e;
    if (import.meta.env.DEV) (window as unknown as { __engine: Engine }).__engine = e;
    return () => { e.dispose(); if (engine.current === e) engine.current = null; };
  }, [loaded]);

  useEffect(() => engine.current?.select(selected), [selected]);

  // Framing keeps blocks clear of the top bar, footer and details panel.
  const panelOpen = selected !== null;
  useEffect(() => {
    engine.current?.setInsets(overlayInsets(panelOpen, panel));
  }, [panelOpen, panel, loaded]);

  useEffect(() => {
    const t = setTimeout(() => { storage.set('blockviz.panel.w', String(panel.w)); storage.set('blockviz.panel.h', String(panel.h)); }, 300);
    return () => clearTimeout(t);
  }, [panel]);

  useEffect(() => {
    const fit = () => setPanel(p => clampPanel(p));
    window.addEventListener('resize', fit);
    return () => window.removeEventListener('resize', fit);
  }, []);

  const goTo = useCallback((id: number) => {
    setSelected(id);
    setSelectedPath(loaded?.model.canonicalPath(id) ?? null);
    // The panel opens with this selection: frame the block beside it.
    engine.current?.setInsets(overlayInsets(true, panel));
    engine.current?.flyToNode(id);
  }, [loaded, panel]);

  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      const typing = ['INPUT', 'TEXTAREA', 'SELECT'].includes((e.target as HTMLElement).tagName);
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') { e.preventDefault(); setDialog(d => (d === 'search' ? null : 'search')); return; }
      if (e.key === 'Escape') {
        if (dialog) { if (dialog !== 'open' || loaded) setDialog(null); return; }
        if (selected !== null) { setSelected(null); setSelectedPath(null); return; }
        engine.current?.zoomOut();
        return;
      }
      if (typing || dialog || e.metaKey || e.ctrlKey || e.altKey) return;
      const eng = engine.current;
      if (!eng) return;
      if (e.key === '/') { e.preventDefault(); setDialog('search'); }
      else if (e.key === 'f') eng.fit();
      else if (e.key === 'h' || e.key === '0') eng.home();
      else if (e.key === '+' || e.key === '=') eng.zoomBy(1.5);
      else if (e.key === '-' || e.key === '_') eng.zoomBy(1 / 1.5);
      else if (e.key === 'Enter' && selectedPath) eng.flyTo(selectedPath);
      else if (e.key === '?') setDialog('help');
      else if (e.key.startsWith('Arrow')) {
        e.preventDefault();
        const d = 90;
        eng.panBy(e.key === 'ArrowLeft' ? d : e.key === 'ArrowRight' ? -d : 0, e.key === 'ArrowUp' ? d : e.key === 'ArrowDown' ? -d : 0);
      }
    };
    window.addEventListener('keydown', key);
    return () => window.removeEventListener('keydown', key);
  }, [dialog, selected, selectedPath, loaded]);

  const crumbs = useMemo(() => {
    if (!loaded) return [];
    return view.path.map((id, i) => ({ id, i, title: loaded.model.node(id).title })).filter(c => c.title);
  }, [view.path, loaded]);

  const node = loaded && selected !== null ? loaded.model.node(selected) : null;
  const stats = loaded?.scene.stats;

  return (
    <main
      className={`app ${dragging ? 'dragging' : ''}`}
      onDragOver={e => { e.preventDefault(); setDragging(true); }}
      onDragLeave={e => { if (e.currentTarget === e.target) setDragging(false); }}
      onDrop={e => {
        e.preventDefault();
        setDragging(false);
        const items = e.dataTransfer.items;
        if (items?.length) void load(p => droppedProject(items, p));
      }}
    >
      <canvas ref={canvas} tabIndex={0} aria-label="Code diagram. Scroll or pinch to zoom, drag to pan, double-click a block to zoom into it, press / to search." />

      <nav className="topbar" aria-label="Location">
        <button className="brand" onClick={() => engine.current?.home()} title="Whole project (H)">
          <span className="logo" aria-hidden="true"><i /><i /><i /></span>blockviz
        </button>
        <span className="sep" />
        {crumbs.length <= 1 && <span className="crumb-root">{loaded?.scene.title}</span>}
        {view.path.length <= 1 && loaded?.scene.description && <span className="tagline" title={loaded.scene.description}>{loaded.scene.description}</span>}
        <div className="crumbs">
          {crumbs.length > 1 && crumbs.map((c, k) => (
            <span key={`${c.id}-${c.i}`} className="crumb">
              {k > 0 && <ChevronRight size={13} />}
              <button onClick={() => engine.current?.flyTo(view.path.slice(0, c.i + 1).length > 1 ? view.path.slice(0, c.i + 1) : [view.path[0]], 0.06)}>{c.title}</button>
            </span>
          ))}
        </div>
      </nav>

      <div className="toolbar">
        <button className="search-button" onClick={() => setDialog('search')} aria-label="Search">
          <SearchIcon size={15} /><span>Search</span><kbd>⌘K</kbd>
        </button>
        <button className="icon" title="Open a codebase" aria-label="Open a codebase" onClick={() => setDialog('open')}><FolderOpen size={17} /></button>
        <button className="icon" title="Export PNG" aria-label="Export PNG" onClick={() => engine.current?.exportPng(loaded?.scene.title ?? 'diagram')}><Download size={17} /></button>
        <button className="icon" title="Help (?)" aria-label="Help" onClick={() => setDialog('help')}><CircleHelp size={17} /></button>
      </div>

      <div className="zoombar">
        <button className="icon" aria-label="Zoom out" title="Zoom out (−)" onClick={() => engine.current?.zoomBy(1 / 1.5)}><Minus size={15} /></button>
        <button className="icon" aria-label="Zoom in" title="Zoom in (+)" onClick={() => engine.current?.zoomBy(1.5)}><Plus size={15} /></button>
        <button className="icon" aria-label="Fit current view" title="Fit (F)" onClick={() => engine.current?.fit()}><Maximize size={15} /></button>
        <button className="icon" aria-label="Whole project" title="Whole project (H)" onClick={() => engine.current?.home()}><Home size={15} /></button>
      </div>

      <div className="footer">
        <button className={`legend-toggle ${showLegend ? 'on' : ''}`} onClick={() => setShowLegend(v => !v)} aria-expanded={showLegend}>
          <Info size={13} />Legend
        </button>
        {stats && <span className="stats">{stats.files} files · {stats.lines.toLocaleString()} lines · {stats.nodes.toLocaleString()} blocks · {Math.round(loaded!.elapsed)} ms</span>}
      </div>

      {showLegend && (
        <aside className="legend" aria-label="Legend">
          <header><strong>Legend</strong><button className="icon" aria-label="Close legend" onClick={() => setShowLegend(false)}><X size={14} /></button></header>
          <div className="legend-grid">
            {legend.map(([cat, label]) => <span key={cat}><i style={{ background: swatch(cat).fill }} />{label}</span>)}
          </div>
          <div className="legend-lines">
            <span><svg width="34" height="10"><line x1="0" y1="5" x2="26" y2="5" stroke="#2c3036" strokeWidth="1.4" /><path d="M26 1.5 L33 5 L26 8.5Z" fill="#2c3036" /></svg>value flows into</span>
            <span><svg width="34" height="10"><line x1="0" y1="5" x2="27" y2="5" stroke="#a3aab4" strokeWidth="1.1" /><path d="M27 2 L33 5 L27 8Z" fill="#a3aab4" /></svg>settings / sizes</span>
            <span><svg width="34" height="10"><line x1="0" y1="5" x2="26" y2="5" stroke="#9a5566" strokeWidth="1.4" strokeDasharray="6 3" /><path d="M26 1.5 L33 5 L26 8.5Z" fill="#9a5566" /></svg>file written → read</span>
            <span><svg width="34" height="10"><line x1="0" y1="5" x2="26" y2="5" stroke="#2c3036" strokeWidth="1.4" strokeDasharray="5 4" /><path d="M26 1.5 L33 5 L26 8.5Z" fill="#2c3036" /></svg>code executed</span>
            <span><svg width="34" height="14"><circle cx="17" cy="7" r="6" fill="#fff" stroke="#2c3036" /><path d="M13.5 7h7M17 3.5v7" stroke="#2c3036" /></svg>add (residual)</span>
            <span><svg width="34" height="14"><circle cx="17" cy="7" r="3.5" fill="#d97706" /></svg>has side effects</span>
          </div>
          <p>Bigger blocks contain more. Zoom into any block with a corner mark to see inside.</p>
        </aside>
      )}

      {node && loaded && (
        <Inspector
          model={loaded.model}
          highlights={loaded.highlights}
          node={node}
          size={panel}
          onResize={setPanel}
          onClose={() => { setSelected(null); setSelectedPath(null); }}
          onZoom={() => selectedPath && engine.current?.flyTo(selectedPath)}
          onGo={goTo}
        />
      )}

      {busy && <div className="busy" role="status"><span className="spinner" />{busy}</div>}
      {error && !dialog && <div className="toast" role="alert">{error}<button className="icon" aria-label="Dismiss" onClick={() => setError(null)}><X size={14} /></button></div>}
      {dragging && <div className="drop-hint">Drop a folder to visualize it</div>}

      {dialog === 'open' && (
        <OpenDialog onOpen={open} onClose={() => setDialog(null)} canClose={!!loaded} lastUrl={storage.get('blockviz.github') || 'github.com/karpathy/nanoGPT'} />
      )}
      {dialog === 'open' && error && <div className="toast over" role="alert">{error}</div>}
      {dialog === 'search' && loaded && <Search model={loaded.model} onPick={id => { setDialog(null); goTo(id); }} onClose={() => setDialog(null)} />}
      {dialog === 'help' && (
        <div className="scrim" onMouseDown={e => { if (e.target === e.currentTarget) setDialog(null); }}>
          <section className="help" role="dialog" aria-modal="true" aria-label="Help">
            <header><h1>Reading the diagram</h1><button className="icon" aria-label="Close" onClick={() => setDialog(null)}><X size={16} /></button></header>
            <p>Every level is a dataflow diagram read top to bottom: inputs enter at the top, results leave at the bottom, and arrows show what flows into what. Zooming into a block reveals the diagram inside it: repository → files → classes and functions → the steps inside them → the code they call.</p>
            <div className="keys">
              {[['Zoom', 'Scroll, pinch, + / −'], ['Pan', 'Drag, two-finger scroll, arrows'], ['Zoom into a block', 'Double-click, or Enter'], ['Back out', 'Esc'], ['Fit view', 'F'], ['Whole project', 'H'], ['Search', '/ or ⌘K']].map(([a, b]) => <div key={a}><span>{a}</span><kbd>{b}</kbd></div>)}
            </div>
            <h2>What becomes a block</h2>
            <p>Calls into your own code, sub-modules, library operations and I/O. Glue such as reshapes and arithmetic on sizes is folded into the step it feeds; prints and asserts become side-effect tags. Comment-headed paragraphs in long scripts become sections, and runs of constants become settings tables.</p>
            <h2>Limits</h2>
            {loaded?.scene.diagnostics.map((d, i) => <p key={i} className="dim">{d}</p>)}
            <p className="dim">Non-Python files are outlined (definitions, keys, headings) rather than analyzed for dataflow.</p>
          </section>
        </div>
      )}
    </main>
  );
}
