import { ArrowLeft, ArrowRight, Check, CircleHelp, Download, FolderOpen, Home, Info, Link, Maximize, Minus, Plus, Search as SearchIcon, X } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Analyzer } from './core/analyzer';
import { droppedProject, exampleProject, folderProject, githubProject, pastedProject } from './core/loaders';
import type { Project, Scene } from './core/types';
import { Engine, type EdgeRef, type ViewState } from './canvas/engine';
import { traceValue } from './canvas/trace';
import { ValuePanel } from './ui/ValuePanel';
import type { Insets } from './canvas/geometry';
import { SceneModel } from './canvas/model';
import { legend, swatch } from './canvas/theme';
import { Highlights } from './core/highlights';
import { clampPanel, DEFAULT_PANEL, Inspector, type PanelSize } from './ui/Inspector';
import { OpenDialog, type OpenRequest } from './ui/OpenDialog';
import { Search } from './ui/Search';
import { Breadcrumbs } from './ui/Breadcrumbs';
import { hashParam, hashParams, Navigation, pushProject } from './ui/navigation';

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
  /** A value being followed (an edge clicked). */
  const [value, setValue] = useState<EdgeRef | null>(null);
  const [hoverId, setHoverId] = useState<number | null>(null);
  const [panel, setPanel] = useState<PanelSize>(() => clampPanel({ w: Number(storage.get('blockviz.panel.w')) || DEFAULT_PANEL.w, h: Number(storage.get('blockviz.panel.h')) || 0 }));
  const canvas = useRef<HTMLCanvasElement>(null);
  const engine = useRef<Engine | null>(null);
  const analyzer = useRef(new Analyzer());
  const request = useRef(0);
  const nav = useRef<Navigation | null>(null);
  const [steps, setSteps] = useState({ back: false, forward: false });
  const [copied, setCopied] = useState(false);

  const load = useCallback(async (make: (progress: (m: string) => void) => Promise<Project>) => {
    const id = ++request.current;
    // The project on screen stops writing the address the new one is loaded into.
    nav.current?.stop();
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
      setValue(null);
      setView({ path: [], zoom: 1 });
      const model = new SceneModel(result.scene, project);
      setLoaded({ project, scene: result.scene, model, highlights: new Highlights(analyzer.current, model), elapsed: result.elapsed });
      setDialog(null);
      document.title = `${result.scene.title} · blockviz`;
    } catch (e) {
      if (id !== request.current || String(e) === 'Error: cancelled') return;
      nav.current?.resume();
      setError(e instanceof Error ? e.message : String(e));
      setDialog(d => d ?? 'open');
    } finally {
      if (id === request.current) setBusy(null);
    }
  }, []);

  const open = useCallback((r: OpenRequest) => {
    // The URL names the project only when a link can load it again.
    if (r.kind !== 'github') pushProject(nav.current, location.pathname + location.search);
    if (r.kind === 'example') void load(() => exampleProject());
    if (r.kind === 'github') {
      storage.set('blockviz.github', r.url);
      pushProject(nav.current, `#github=${encodeURIComponent(r.url.replace(/^https?:\/\//, ''))}`);
      void load(p => githubProject(r.url, p));
    }
    if (r.kind === 'folder') void load(p => folderProject(r.files, p));
    if (r.kind === 'paste') void load(async () => pastedProject(r.filename, r.text));
  }, [load]);

  // First load: a shared GitHub link, else the bundled example.
  const loadFromUrl = useCallback(() => {
    const repo = hashParam('github');
    if (repo) void load(p => githubProject(repo, p));
    else void load(() => exampleProject());
  }, [load]);
  useEffect(() => {
    loadFromUrl();
    return () => analyzer.current.cancel();
  }, [loadFromUrl]);

  useEffect(() => {
    if (!loaded || !canvas.current) return;
    const e = new Engine(canvas.current, loaded.model, {
      onView: v => { setView(v); nav.current?.viewChanged(); },
      onJump: () => nav.current?.jump(),
      onSelect: (id, path) => { setSelected(id); setSelectedPath(path); setValue(null); },
      onSelectEdge: edge => { setValue(edge); setSelected(null); setSelectedPath(null); },
      onHover: id => setHoverId(id),
    }, overlayInsets(false, DEFAULT_PANEL));
    engine.current = e;
    if (import.meta.env.DEV) (window as unknown as { __engine: Engine }).__engine = e;
    const repo = hashParams().get('github');
    const n = new Navigation(
      loaded.model, e, `${loaded.project.source}#${loaded.scene.nodes.length}`, repo ? [['github', repo]] : [],
      sel => { setSelected(sel?.id ?? null); setSelectedPath(sel?.path ?? null); },
      () => setSteps({ back: n.canBack, forward: n.canForward }),
    );
    nav.current = n;
    setSteps({ back: n.canBack, forward: n.canForward });
    const pop = (ev: PopStateEvent) => {
      // An entry of another project (or a link to one): load that project.
      if (!n.pop(ev.state)) loadFromUrl();
      else if ((hashParam('github') ?? '') !== (repo ? decodeURIComponent(repo) : '')) loadFromUrl();
    };
    window.addEventListener('popstate', pop);
    setValue(null);
    return () => {
      window.removeEventListener('popstate', pop);
      n.dispose();
      if (nav.current === n) nav.current = null;
      e.dispose(); setHoverId(null); if (engine.current === e) engine.current = null;
    };
  }, [loaded, loadFromUrl]);

  useEffect(() => nav.current?.setSelection(selected !== null && selectedPath ? { id: selected, path: selectedPath } : null), [selected, selectedPath]);

  useEffect(() => engine.current?.select(selected), [selected]);

  const traced = useMemo(() => {
    const start = value && loaded?.model.nodes[value.owner]?.edges?.[value.index];
    if (!value || !start) return null;
    const model = loaded!.model;
    return { owner: value.owner, start, ...traceValue(id => model.node(id), value.owner, start) };
  }, [value, loaded]);
  useEffect(() => engine.current?.trace(traced && { scope: traced.scope, edges: traced.edges }), [traced]);

  // Framing keeps blocks clear of the top bar, footer and details panel.
  const panelOpen = selected !== null || value !== null;
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

  /** Selects a block for a line of source, staying in the instance being read. */
  const pick = useCallback((id: number) => {
    if (!loaded) return;
    const path = loaded.model.instancePath(id, selectedPath ?? view.path);
    // Read through a call, the call is where the definition is drawn.
    setSelected(path[path.length - 1]);
    setSelectedPath(path);
    engine.current?.reveal(path);
  }, [loaded, selectedPath, view.path]);

  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      const typing = ['INPUT', 'TEXTAREA', 'SELECT'].includes((e.target as HTMLElement).tagName);
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') { e.preventDefault(); setDialog(d => (d === 'search' ? null : 'search')); return; }
      if (e.key === 'Escape') {
        if (dialog) { if (dialog !== 'open' || loaded) setDialog(null); return; }
        if (value !== null) { setValue(null); return; }
        if (selected !== null) { setSelected(null); setSelectedPath(null); return; }
        engine.current?.zoomOut();
        return;
      }
      // Back and forward, as in a browser.
      if (!typing && !dialog && ((e.altKey && e.key === 'ArrowLeft') || ((e.metaKey || e.ctrlKey) && e.key === '['))) { e.preventDefault(); history.back(); return; }
      if (!typing && !dialog && ((e.altKey && e.key === 'ArrowRight') || ((e.metaKey || e.ctrlKey) && e.key === ']'))) { e.preventDefault(); history.forward(); return; }
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
  }, [dialog, selected, selectedPath, loaded, value]);

  const crumbs = useMemo(() => {
    if (!loaded) return [];
    // A path left from the previous project may name nodes this one lacks.
    return view.path.map((id, i) => ({ id, i, title: loaded.model.node(id)?.title ?? '' })).filter(c => c.title);
  }, [view.path, loaded]);

  const node = loaded && selected !== null ? loaded.model.node(selected) : null;
  const stats = loaded?.scene.stats;
  // Inside code reached through a call (… › block › attn): where that code is defined.
  const borrowed = useMemo(() => (loaded ? loaded.model.borrowedDefinition(view.path) : null), [loaded, view.path]);

  return (
    <main
      className={`app ${dragging ? 'dragging' : ''}`}
      onDragOver={e => { e.preventDefault(); setDragging(true); }}
      onDragLeave={e => { if (e.currentTarget === e.target) setDragging(false); }}
      onDrop={e => {
        e.preventDefault();
        setDragging(false);
        const items = e.dataTransfer.items;
        if (items?.length) {
          pushProject(nav.current, location.pathname + location.search);
          void load(p => droppedProject(items, p));
        }
      }}
    >
      <canvas ref={canvas} tabIndex={0} aria-label="Code diagram. Scroll or pinch to zoom, drag to pan, double-click a block to zoom into it, press / to search." />

      <nav className="topbar" aria-label="Location">
        <button className="brand" onClick={() => engine.current?.home()} title="Whole project (H)">
          <span className="logo" aria-hidden="true"><i /><i /><i /></span>blockviz
        </button>
        <span className="history">
          <button className="icon" aria-label="Back" title="Back (Alt+←)" disabled={!steps.back} onClick={() => history.back()}><ArrowLeft size={15} /></button>
          <button className="icon" aria-label="Forward" title="Forward (Alt+→)" disabled={!steps.forward} onClick={() => history.forward()}><ArrowRight size={15} /></button>
        </span>
        {crumbs.length <= 1 && <span className="crumb-root">{loaded?.scene.title}</span>}
        {view.path.length <= 1 && loaded?.scene.description && <span className="tagline" title={loaded.scene.description}>{loaded.scene.description}</span>}
        {crumbs.length > 1 && (
          <Breadcrumbs crumbs={crumbs} onGo={c => engine.current?.flyTo(view.path.slice(0, c.i + 1).length > 1 ? view.path.slice(0, c.i + 1) : [view.path[0]], 'fit', 0.06)} />
        )}
      </nav>
      {borrowed && loaded && (
        <button className="defined-in" title="Go to where this code is defined" onClick={() => goTo(borrowed.id)}>
          defined in <strong>{loaded.model.qualified(borrowed.id)}</strong>
          {borrowed.span && <span className="dim"> · {loaded.model.fileOf(borrowed)?.split('/').pop()}:{borrowed.span.line}</span>}
        </button>
      )}

      <div className="toolbar">
        <button className="search-button" onClick={() => setDialog('search')} aria-label="Search">
          <SearchIcon size={15} /><span>Search</span><kbd>⌘K</kbd>
        </button>
        <button
          className="icon"
          title={copied ? 'Link copied' : 'Copy a link to this view'}
          aria-label="Copy a link to this view"
          onClick={() => {
            nav.current?.viewChanged();
            // The URL is brought up to date a moment after the view settles.
            setTimeout(() => void navigator.clipboard?.writeText(location.href).then(() => { setCopied(true); setTimeout(() => setCopied(false), 1400); }), 300);
          }}
        >{copied ? <Check size={17} /> : <Link size={17} />}</button>
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
            <span><svg width="34" height="10"><line x1="0" y1="5" x2="27" y2="5" stroke="#a3aab4" strokeWidth="1.1" /><path d="M27 2 L33 5 L27 8Z" fill="#a3aab4" /></svg>settings, sizes, types</span>
            <span><svg width="34" height="10"><line x1="0" y1="5" x2="27" y2="5" stroke="#a3aab4" strokeWidth="1.2" strokeDasharray="1.5 2.5" /><path d="M27 2 L33 5 L27 8Z" fill="#a3aab4" /></svg>decides whether it runs</span>
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
          hover={hoverId}
          size={panel}
          onResize={setPanel}
          onClose={() => { setSelected(null); setSelectedPath(null); engine.current?.peek(null); }}
          onZoom={() => selectedPath && engine.current?.flyTo(selectedPath)}
          onGo={goTo}
          onPick={pick}
          onPeek={id => engine.current?.peek(id)}
        />
      )}

      {traced && loaded && !node && (
        <ValuePanel
          model={loaded.model}
          owner={traced.owner}
          edges={traced.edges}
          start={traced.start}
          size={panel}
          onClose={() => { setValue(null); engine.current?.peek(null); }}
          onPick={id => { setValue(null); pick(id); }}
          onPeek={id => engine.current?.peek(id)}
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
              {[['Zoom', 'Scroll, pinch, + / −'], ['Pan', 'Drag, two-finger scroll, arrows'], ['Zoom into a block', 'Double-click, or Enter'], ['Back out', 'Esc'], ['Back / forward', 'Alt+← / Alt+→'], ['Fit view', 'F'], ['Whole project', 'H'], ['Search', '/ or ⌘K'], ['Follow a value', 'Click an arrow'], ['Find a line\'s block', 'Click the line']].map(([a, b]) => <div key={a}><span>{a}</span><kbd>{b}</kbd></div>)}
            </div>
            <h2>Finding your way</h2>
            <p>The details panel shows a step's code inside its function with its lines marked; click a line to select its block. <em>Used by</em> lists every place a definition or component is used, and those places are outlined in the diagram. Inside code reached through a call, the line under the breadcrumbs says where it is defined. The address bar always names the current view, so a link reopens it.</p>
            <h2>What becomes a block</h2>
            <p>Calls into your own code, sub-modules, library operations and I/O. Glue such as reshapes and arithmetic on sizes is folded into the step it feeds; prints and asserts become side-effect tags. Comment-headed paragraphs in long scripts become sections, and runs of constants become settings tables.</p>
            <h2>Limits</h2>
            {loaded?.scene.diagnostics.map((d, i) => <p key={i} className="dim">{d}</p>)}
            <p className="dim">Python, JavaScript, TypeScript, Rust and Go are analyzed for dataflow; other files are outlined (definitions, keys, headings).</p>
          </section>
        </div>
      )}
    </main>
  );
}
