//! The window: the diagram and everything around it, ported from src/App.tsx
//! (the app's state) and src/canvas/engine.ts (input, camera and frames). The
//! bars, dialogs and menus are in shell.rs; the details panels in panel.rs.

use crate::anchor;
use crate::camera::{Camera, Framing, Insets, Plan, Xf};
use crate::diff::SceneDiff;
use crate::git::{self, Commit, Repo};
use crate::gpuipen::GpuiPen;
use crate::history::{Entry, History};
use crate::input::TextInput;
use crate::lines::distance_to_polyline;
use crate::live::{self, Ask, Files, Update};
use crate::model::Model;
use crate::panel::{Content, Inspector, PanelState};
use crate::render::{hit_at, path_of, Changes, EdgeHit, EdgeId, Hit, Paint, Renderer, Trace};
use crate::settings::Settings;
use crate::svgpen::SvgPen;
use crate::theme::{Theme, Ui};
use gpui::{
    canvas, div, prelude::*, px, AnyElement, App, Bounds, Context, CursorStyle, Entity, FocusHandle, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollDelta, ScrollWheelEvent, Subscription, Window,
};
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

/// The project on screen.
pub struct Project {
    pub folder: PathBuf,
    pub ask: Sender<Ask>,
    pub repo: Option<Repo>,
    pub files: Files,
    /// How long reading and analyzing it took, for the footer.
    pub elapsed: Duration,
}

/// A change being read: the base version, and how the code on screen differs from it.
pub struct Compared {
    /// The revision compared with, as git names it.
    pub rev: String,
    /// What it is: "Uncommitted changes", "This branch", or a commit.
    pub kind: String,
    /// Its name in the chip and panel (`HEAD`, `main`, `a1b2c3d`).
    pub label: String,
    /// The panel's heading for it.
    pub title: String,
    pub base_files: Files,
    pub base: Rc<Model>,
    pub diff: Rc<SceneDiff>,
}

/// A press that may still be a click: where it started, where it was last, and whether it has moved.
pub struct Press {
    pub at: Point<Pixels>,
    pub last: Point<Pixels>,
    pub moved: bool,
}

/// A dialog over the diagram.
pub enum Dialog {
    Open,
    Search(Search),
}

/// The search palette's field, the results for what is in it, and the one Enter picks.
pub struct Search {
    pub input: Entity<TextInput>,
    pub query: String,
    pub results: Vec<u32>,
    pub active: usize,
    pub scroll: gpui::ScrollHandle,
    pub _edits: Subscription,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Menu {
    Download,
    Compare,
    Crumbs,
}

/// The details panel's default width, and its narrowest.
pub const PANEL_W: f64 = 380.0;
pub const PANEL_MIN: f64 = 300.0;

const fn ease(t: f64) -> f64 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        let u = -2.0 * t + 2.0;
        1.0 - u * u * u / 2.0
    }
}

pub struct Viewer {
    pub m: Option<Rc<Model>>,
    pub project: Option<Project>,
    /// Counts projects opened, so analyses of one left behind are not shown.
    generation: u64,
    pub compare: Option<Compared>,
    pub camera: Camera,
    pub width: f64,
    pub height: f64,
    renderer: Renderer,
    pub hits: Vec<Hit>,
    edge_hits: Vec<EdgeHit>,
    pub hover: Option<u32>,
    hover_edge: Option<EdgeHit>,
    /// A block pointed out from outside the canvas (a line of its source, a row of a panel).
    peeked: Option<u32>,
    pub selected: Option<u32>,
    pub selected_path: Option<Vec<u32>>,
    /// A value being followed (an edge clicked), and its path.
    pub value: Option<EdgeId>,
    pub trace: Option<Trace>,
    related: Option<(u32, HashSet<u32>)>,
    /// Reading a change: a block it removed, as its index among the changes.
    pub removed_sel: Option<usize>,
    /// Reading a change: the list of its changes is open (when nothing else is in the panel).
    pub changes_open: bool,
    pub flight: Option<(Instant, Plan)>,
    /// Still showing the initial whole-project framing (nothing has moved the camera).
    framed: bool,
    press: Option<Press>,
    pub crumbs: Vec<u32>,
    pub history: History,
    pub theme: Theme,
    pub ui: Ui,
    /// The scheme chosen (Some(true): dark), or None to follow the system.
    pub scheme: Option<bool>,
    pub system_dark: bool,
    pub dialog: Option<Dialog>,
    pub legend: bool,
    pub menu: Option<Menu>,
    pub busy: Option<String>,
    pub error: Option<String>,
    pub panel_w: f64,
    pub resizing: Option<(f32, f64)>,
    pub inspector: Entity<Inspector>,
    pub focus: FocusHandle,
    pub settings: Settings,
    pub trunks: bool,
    pub cursor: CursorStyle,
    /// The commits the Compare menu offers.
    pub log: Vec<Commit>,
    pub default_branch: Option<String>,
    pub branch: Option<String>,
    /// The window's title, set when it is drawn.
    pub title: String,
    _appearance: Option<Subscription>,
}

impl Viewer {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
        let settings = Settings::load();
        let scheme = settings.scheme.as_deref().map(|s| s == "dark");
        let system_dark = matches!(window.appearance(), gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark);
        let dark = scheme.unwrap_or(system_dark);
        let me = cx.entity().downgrade();
        let ui = Ui::new(dark);
        let inspector = cx.new(|_| Inspector::new(me, ui.clone()));
        let appearance = cx.observe_window_appearance(window, |v, window, cx| {
            v.system_dark = matches!(window.appearance(), gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark);
            v.apply_scheme(cx);
        });
        let trunks = settings.trunks;
        let panel_w = settings.panel_w.unwrap_or(PANEL_W);
        Viewer {
            m: None,
            project: None,
            generation: 0,
            compare: None,
            camera: Camera { path: vec![0], xf: Xf { x: 0.0, y: 0.0, s: 1.0 }, insets: Insets::default() },
            width: 0.0,
            height: 0.0,
            renderer: Renderer::default(),
            hits: vec![],
            edge_hits: vec![],
            hover: None,
            hover_edge: None,
            peeked: None,
            selected: None,
            selected_path: None,
            value: None,
            trace: None,
            related: None,
            removed_sel: None,
            changes_open: true,
            flight: None,
            framed: true,
            press: None,
            crumbs: vec![],
            history: History::default(),
            theme: if dark { Theme::dark() } else { Theme::light() },
            ui,
            scheme,
            system_dark,
            dialog: None,
            legend: false,
            menu: None,
            busy: None,
            error: None,
            panel_w,
            resizing: None,
            inspector,
            focus,
            settings,
            trunks,
            cursor: CursorStyle::OpenHand,
            log: vec![],
            default_branch: None,
            branch: None,
            title: "blockviz".into(),
            _appearance: Some(appearance),
        }
    }

    // ----- the scheme -----------------------------------------------------------------

    pub fn dark(&self) -> bool {
        self.scheme.unwrap_or(self.system_dark)
    }

    fn apply_scheme(&mut self, cx: &mut Context<Self>) {
        let dark = self.dark();
        if self.theme.dark != dark {
            self.theme = if dark { Theme::dark() } else { Theme::light() };
            self.ui = Ui::new(dark);
            self.sync_panel(cx);
        }
        cx.notify();
    }

    /// The toolbar's switch: the other scheme, remembered; picking the system's own goes back to following it.
    pub fn toggle_scheme(&mut self, cx: &mut Context<Self>) {
        let dark = !self.dark();
        self.scheme = (dark != self.system_dark).then_some(dark);
        self.settings.scheme = self.scheme.map(|d| if d { "dark".into() } else { "light".into() });
        self.settings.save();
        self.apply_scheme(cx);
    }

    // ----- projects -------------------------------------------------------------------

    /// Opens a folder: analyzes it on a thread of its own, which then follows its changes.
    pub fn open_folder(&mut self, folder: PathBuf, cx: &mut Context<Self>) {
        let folder = folder.canonicalize().unwrap_or(folder);
        if !folder.is_dir() {
            self.error = Some(format!("{} is not a folder.", folder.display()));
            return cx.notify();
        }
        if let Some(p) = self.project.take() {
            let _ = p.ask.send(Ask::Stop);
        }
        self.generation += 1;
        let generation = self.generation;
        self.error = None;
        self.dialog = None;
        let name = folder.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        self.busy = Some(format!("Analyzing {name}…"));
        let (send, mut updates) = futures::channel::mpsc::unbounded::<Update>();
        let ask = live::open(folder.clone(), self.trunks, move |u| send.unbounded_send(u).is_ok());
        self.project = Some(Project { folder: folder.clone(), ask, repo: None, files: Files::new(), elapsed: Duration::ZERO });
        self.settings.opened(&folder);
        self.title = format!("{name} · blockviz");
        cx.spawn(async move |this, cx| {
            use futures::StreamExt;
            while let Some(u) = updates.next().await {
                let still = this.update(cx, |v, cx| {
                    if v.generation != generation {
                        return false;
                    }
                    v.take(u, cx);
                    true
                });
                if !matches!(still, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    /// Stops opening a folder: the project on screen, if any, stays.
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if self.m.is_none() {
            if let Some(p) = self.project.take() {
                let _ = p.ask.send(Ask::Stop);
            }
            self.dialog = Some(Dialog::Open);
        }
        self.generation += 1;
        self.busy = None;
        cx.notify();
    }

    /// Takes in an analysis of the project: its first, or a new one after files changed.
    fn take(&mut self, u: Update, cx: &mut Context<Self>) {
        let new = Rc::new(u.model);
        if let Some(p) = self.project.as_mut() {
            p.files = u.files;
            p.elapsed = u.read + u.took;
        }
        if u.first || self.m.is_none() {
            self.busy = None;
            let repo = self.project.as_ref().and_then(|p| Repo::find(&p.folder));
            self.branch = repo.as_ref().and_then(Repo::branch);
            self.default_branch = repo.as_ref().and_then(Repo::default_branch);
            if let Some(p) = self.project.as_mut() {
                p.repo = repo;
            }
            self.m = Some(new);
            self.compare = None;
            self.selected = None;
            self.selected_path = None;
            self.value = None;
            self.trace = None;
            self.removed_sel = None;
            self.changes_open = true;
            self.history.clear();
            self.hits.clear();
            self.edge_hits.clear();
            self.hover = None;
            self.hover_edge = None;
            self.peeked = None;
            self.flight = None;
            self.framed = true;
            self.crumbs.clear();
            self.camera = Camera::new(self.m.as_ref().unwrap());
            self.camera.insets = self.insets();
            if self.width > 0.0 {
                self.camera.fit_root(self.m.as_ref().unwrap(), self.width, self.height);
            }
        } else {
            let old = self.m.replace(new.clone()).unwrap();
            if !u.changed.is_empty() {
                let files = if u.changed.len() <= 3 { u.changed.join(", ") } else { format!("{} files", u.changed.len()) };
                println!("{files} changed: analyzed again in {:.0} ms, {} files' parse reused", u.took.as_secs_f64() * 1000.0, u.reused);
            }
            // Ids are the new scene's: the view, the selection, the trail and search carry over to the same blocks.
            self.flight = None;
            if self.width > 0.0 {
                anchor::carry(&old, &new, &mut self.camera, &self.hits, self.width, self.height);
            } else {
                self.camera.path = vec![new.root()];
            }
            self.hits.clear();
            self.edge_hits.clear();
            self.hover = None;
            self.hover_edge = None;
            self.peeked = None;
            self.related = None;
            self.history.carry(&old, &new);
            let sel = self.selected_path.take().map(|p| (p.len(), anchor::map_path(&old, &new, &p)));
            match sel {
                Some((len, path)) if path.len() == len => {
                    self.selected = path.last().copied();
                    self.selected_path = Some(path);
                }
                _ => self.selected = None,
            }
            // A value followed: the same edge of the same block, if it is still there.
            self.value = self.value.and_then(|(owner, i)| {
                let p = old.canonical_path(owner);
                let q = anchor::map_path(&old, &new, &p);
                let e = &old.node(owner).edges[i];
                let to = *q.last()?;
                (q.len() == p.len()).then_some(())?;
                new.node(to).edges.iter().position(|f| f.label == e.label && f.kind == e.kind && new.node(f.from).title == old.node(e.from).title).map(|j| (to, j))
            });
            self.trace = self.value.map(|(o, i)| crate::trace::trace_value(&|id| new.node(id), o, i));
            if let Some(Dialog::Search(s)) = self.dialog.as_mut() {
                s.results = new.find(&s.query);
                s.active = s.active.min(s.results.len().saturating_sub(1));
            }
            if self.compare.is_some() {
                self.compare_again();
            }
        }
        self.sync_panel(cx);
        cx.notify();
    }

    /// Lays the project out again, each block's values on one branching line (or not).
    pub fn set_trunks(&mut self, on: bool, cx: &mut Context<Self>) {
        self.trunks = on;
        self.settings.trunks = on;
        self.settings.save();
        if let Some(p) = &self.project {
            let _ = p.ask.send(Ask::Layout { shared_trunks: on });
        }
        // The base of a change is laid out the same way.
        if let Some(c) = &self.compare {
            let (rev, kind, label, title) = (c.rev.clone(), c.kind.clone(), c.label.clone(), c.title.clone());
            self.start_compare(rev, kind, label, title, cx);
        }
        cx.notify();
    }

    // ----- comparing with git ---------------------------------------------------------

    /// Reads the folder as it was at `rev`, analyzes it, and shows how the code on screen differs from it.
    pub fn start_compare(&mut self, rev: String, kind: String, label: String, title: String, cx: &mut Context<Self>) {
        let Some(repo) = self.project.as_ref().and_then(|p| p.repo.clone()) else { return };
        let Some(folder) = self.project.as_ref().map(|p| p.folder.clone()) else { return };
        let generation = self.generation;
        let trunks = self.trunks;
        self.menu = None;
        self.busy = Some(format!("Comparing with {label}…"));
        cx.notify();
        let task = cx.background_executor().spawn(async move {
            let files = repo.files_at(&rev);
            let name = folder.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let scene = blockviz_core::analyze_with(&name, live::inputs(&files), blockviz_core::layout::Options { shared_trunks: trunks });
            let model = Model::new(scene, live::texts(&files));
            (rev, files, model)
        });
        cx.spawn(async move |this, cx| {
            let (rev, files, model) = task.await;
            let _ = this.update(cx, |v, cx| {
                if v.generation != generation {
                    return;
                }
                v.busy = None;
                let Some(head) = v.m.clone() else { return };
                let base = Rc::new(model);
                let changes = git::changes(&files, &v.project.as_ref().unwrap().files);
                let diff = Rc::new(SceneDiff::new(base.clone(), head, changes));
                v.compare = Some(Compared { rev, kind, label, title, base_files: files, base, diff });
                v.selected = None;
                v.selected_path = None;
                v.value = None;
                v.trace = None;
                v.removed_sel = None;
                v.changes_open = true;
                v.camera.insets = v.insets();
                v.sync_panel(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// The code on screen changed: how it differs from the base, again.
    fn compare_again(&mut self) {
        let (Some(c), Some(head), Some(p)) = (self.compare.as_mut(), self.m.clone(), self.project.as_ref()) else { return };
        let changes = git::changes(&c.base_files, &p.files);
        c.diff = Rc::new(SceneDiff::new(c.base.clone(), head, changes));
        self.removed_sel = None;
    }

    pub fn stop_compare(&mut self, cx: &mut Context<Self>) {
        self.compare = None;
        self.removed_sel = None;
        self.menu = None;
        self.camera.insets = self.insets();
        self.sync_panel(cx);
        cx.notify();
    }

    /// Opens the Compare menu, with the repository's recent commits.
    pub fn open_compare_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu == Some(Menu::Compare) {
            self.menu = None;
        } else {
            self.menu = Some(Menu::Compare);
            self.log = self.project.as_ref().and_then(|p| p.repo.as_ref()).map(|r| r.log(15)).unwrap_or_default();
        }
        cx.notify();
    }

    /// "This branch": everything since the branch left the default branch.
    pub fn compare_branch(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.project.as_ref().and_then(|p| p.repo.clone()) else { return };
        let Some(base) = self.default_branch.clone() else { return };
        let Some(sha) = repo.merge_base("HEAD", &base) else {
            self.error = Some(format!("This branch has nothing in common with {base}."));
            return cx.notify();
        };
        let short = base.trim_start_matches("origin/").to_string();
        self.start_compare(sha, "This branch".into(), short.clone(), format!("Changes since leaving {short}"), cx);
    }

    // ----- the view -------------------------------------------------------------------

    /// Screen margins covered by the top bar, footer and (when open) the details panel.
    pub fn insets(&self) -> Insets {
        Insets { top: 56.0, right: if self.panel_open() { self.panel_w + 24.0 } else { 12.0 }, bottom: 52.0, left: 12.0 }
    }

    pub fn panel_open(&self) -> bool {
        self.selected.is_some() || self.value.is_some() || self.removed_sel.is_some() || (self.compare.is_some() && self.changes_open)
    }

    fn entry(&self) -> Entry {
        Entry { path: self.camera.path.clone(), xf: self.camera.xf, sel: self.selected.zip(self.selected_path.clone()) }
    }

    /// Flies to the node at the end of an instance path: `Open` to read it, `Fit` to see all of it.
    pub fn fly_to(&mut self, path: &[u32], how: Framing, pad: f64, cx: &mut Context<Self>) {
        let Some(m) = self.m.clone() else { return };
        if self.width == 0.0 {
            return;
        }
        self.camera.insets = self.insets();
        let plan = self.camera.flight(&m, path, self.width, self.height, how, pad);
        // A flight that goes nowhere (H where the view already is) is not a step back.
        if plan.length > 1e-3 {
            self.history.jump(self.entry());
        }
        self.framed = false;
        self.flight = Some((Instant::now(), plan));
        cx.notify();
    }

    pub fn fly_to_node(&mut self, id: u32, cx: &mut Context<Self>) {
        let Some(m) = self.m.clone() else { return };
        self.fly_to(&m.canonical_path(id), Framing::Open, 0.08, cx);
    }

    /// Frames all of the block the breadcrumbs end at.
    pub fn fit(&mut self, cx: &mut Context<Self>) {
        let path = self.crumbs.clone();
        if path.len() <= 1 {
            self.home(cx);
        } else {
            self.fly_to(&path, Framing::Fit, 0.04, cx);
        }
    }

    pub fn home(&mut self, cx: &mut Context<Self>) {
        if let Some(m) = self.m.clone() {
            self.fly_to(&[m.root()], Framing::Fit, 0.05, cx);
        }
    }

    pub fn zoom_by(&mut self, factor: f64, cx: &mut Context<Self>) {
        let Some(m) = self.m.clone() else { return };
        self.flight = None;
        self.camera.zoom(&m, factor, self.width / 2.0, self.height / 2.0);
        self.changed(cx);
    }

    pub fn pan_by(&mut self, dx: f64, dy: f64, cx: &mut Context<Self>) {
        self.flight = None;
        self.camera.pan(dx, dy);
        self.changed(cx);
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        self.framed = false;
        if let Some(m) = self.m.clone() {
            self.camera.reanchor(&m, self.width, self.height);
        }
        cx.notify();
    }

    /// Brings a block into view unless it is already on screen and legible.
    pub fn reveal(&mut self, path: &[u32], cx: &mut Context<Self>) {
        let Some(&id) = path.last() else { return };
        let i = self.insets();
        let seen = self.hits.iter().any(|h| {
            let r = h.rect;
            h.id == id && h.clear && r.x >= i.left && r.y >= i.top && r.x + r.w <= self.width - i.right && r.y + r.h <= self.height - i.bottom
        });
        if !seen {
            self.fly_to(path, Framing::Open, 0.08, cx);
        }
    }

    /// Back, as in a browser: the exact view and selection before the last jump.
    pub fn back(&mut self, cx: &mut Context<Self>) {
        let here = self.entry();
        if let Some(e) = self.history.back(here) {
            self.restore(e, cx);
        }
    }

    pub fn forward(&mut self, cx: &mut Context<Self>) {
        let here = self.entry();
        if let Some(e) = self.history.forward(here) {
            self.restore(e, cx);
        }
    }

    fn restore(&mut self, e: Entry, cx: &mut Context<Self>) {
        let Some(m) = self.m.clone() else { return };
        self.selected = e.sel.as_ref().map(|s| s.0);
        self.selected_path = e.sel.map(|s| s.1);
        self.value = None;
        self.trace = None;
        self.removed_sel = None;
        self.camera.insets = self.insets();
        // A step whose anchor is gone shows its nearest container whole.
        let plan = if e.xf.s == 0.0 {
            self.camera.flight(&m, &e.path, self.width, self.height, Framing::Fit, 0.05)
        } else {
            self.camera.flight_to(&m, &e.path, e.xf, self.width, self.height)
        };
        self.framed = false;
        self.flight = Some((Instant::now(), plan));
        self.sync_panel(cx);
        cx.notify();
    }

    // ----- selection ------------------------------------------------------------------

    pub fn select(&mut self, id: Option<u32>, path: Option<Vec<u32>>, cx: &mut Context<Self>) {
        self.selected = id;
        self.selected_path = path;
        self.value = None;
        self.trace = None;
        self.removed_sel = None;
        self.peeked = None;
        self.camera.insets = self.insets();
        self.sync_panel(cx);
        cx.notify();
    }

    /// An edge was clicked: follow its value.
    fn select_edge(&mut self, edge: EdgeId, cx: &mut Context<Self>) {
        let Some(m) = self.m.clone() else { return };
        self.value = Some(edge);
        self.trace = Some(crate::trace::trace_value(&|id| m.node(id), edge.0, edge.1));
        self.selected = None;
        self.selected_path = None;
        self.removed_sel = None;
        self.peeked = None;
        self.camera.insets = self.insets();
        self.sync_panel(cx);
        cx.notify();
    }

    pub fn close_value(&mut self, cx: &mut Context<Self>) {
        self.value = None;
        self.trace = None;
        self.camera.insets = self.insets();
        self.sync_panel(cx);
        cx.notify();
    }

    /// Selects a block and flies to where it is defined (a link in a panel, a search result).
    pub fn go_to(&mut self, id: u32, cx: &mut Context<Self>) {
        let Some(m) = self.m.clone() else { return };
        let path = m.canonical_path(id);
        self.select(Some(id), Some(path), cx);
        self.fly_to_node(id, cx);
    }

    /// Selects the block drawn for a line of source, staying in the instance being read.
    pub fn pick(&mut self, id: u32, cx: &mut Context<Self>) {
        let Some(m) = self.m.clone() else { return };
        let near = self.selected_path.clone().unwrap_or_else(|| self.crumbs.clone());
        let path = m.instance_path(id, &near);
        self.select(path.last().copied(), Some(path.clone()), cx);
        self.reveal(&path, cx);
    }

    /// Points out a block, as hovering it would.
    pub fn peek(&mut self, id: Option<u32>, cx: &mut Context<Self>) {
        if self.peeked != id {
            self.peeked = id;
            cx.notify();
        }
    }

    pub fn open_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.selected_path.clone() {
            self.fly_to(&path, Framing::Open, 0.08, cx);
        }
    }

    /// Goes to a change: selects its block (or shows the removed one) and flies to it.
    pub fn go_to_change(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(c) = self.compare.as_ref().and_then(|c| c.diff.changes.get(i).copied()) else { return };
        if c.status == crate::diff::Status::Removed {
            self.selected = None;
            self.selected_path = None;
            self.value = None;
            self.trace = None;
            self.removed_sel = Some(i);
            self.camera.insets = self.insets();
            self.sync_panel(cx);
            self.fly_to_node(c.at, cx);
        } else {
            self.go_to(c.id, cx);
        }
    }

    /// Where the change being read is among the changes.
    pub fn change_index(&self) -> Option<usize> {
        let d = &self.compare.as_ref()?.diff;
        if let Some(i) = self.removed_sel {
            return Some(i);
        }
        d.index_of(self.selected?, false)
    }

    /// The next (`delta` 1) or previous (-1) change from the one being read, round the list.
    pub fn step_change(&mut self, delta: i32, cx: &mut Context<Self>) {
        let n = self.compare.as_ref().map_or(0, |c| c.diff.changes.len());
        if n == 0 {
            return;
        }
        let i = match self.change_index() {
            None => if delta > 0 { 0 } else { n - 1 },
            Some(i) => ((i as i32 + delta).rem_euclid(n as i32)) as usize,
        };
        self.go_to_change(i, cx);
    }

    pub fn show_changes(&mut self, cx: &mut Context<Self>) {
        self.selected = None;
        self.selected_path = None;
        self.value = None;
        self.trace = None;
        self.removed_sel = None;
        self.changes_open = true;
        self.camera.insets = self.insets();
        self.sync_panel(cx);
        cx.notify();
    }

    /// Tells the details panel what to show.
    pub fn sync_panel(&mut self, cx: &mut Context<Self>) {
        let Some(m) = self.m.clone() else {
            return self.inspector.update(cx, |i, cx| i.set(None, cx));
        };
        let content = if let Some(id) = self.selected {
            Content::Block(id)
        } else if let Some(i) = self.removed_sel {
            Content::Removed(i)
        } else if let (Some(v), Some(t)) = (self.value, &self.trace) {
            Content::Value { owner: v.0, start: v.1, edges: t.edges.iter().copied().collect() }
        } else if self.compare.is_some() && self.changes_open {
            Content::Changes
        } else {
            Content::None
        };
        let state = PanelState {
            m,
            content,
            hover: self.hover,
            compare: self.compare.as_ref().map(|c| crate::panel::CompareView { diff: c.diff.clone(), kind: c.kind.clone(), label: c.label.clone(), title: c.title.clone() }),
            change_index: self.change_index(),
            ui: self.ui.clone(),
        };
        self.inspector.update(cx, |i, cx| i.set(Some(state), cx));
    }

    // ----- input ----------------------------------------------------------------------

    fn local(&self, p: Point<Pixels>) -> (f64, f64) {
        (f64::from(p.x), f64::from(p.y))
    }

    /// Forgets the hovered block while the view moves; the next pointer move finds it again.
    fn unhover(&mut self, cx: &mut Context<Self>) {
        let had = self.hover.is_some() || self.hover_edge.is_some();
        self.hover = None;
        self.hover_edge = None;
        if had {
            self.sync_hover(cx);
            cx.notify();
        }
    }

    fn sync_hover(&mut self, cx: &mut Context<Self>) {
        if self.selected.is_some() {
            let hover = self.hover;
            self.inspector.update(cx, |i, cx| i.set_hover(hover, cx));
        }
    }

    /// The edge within `radius` pixels of a point, drawn in the interior of hit
    /// `container` (the block under the point, so a block drawn over an edge wins).
    fn edge_at(&self, x: f64, y: f64, container: i32, radius: f64) -> Option<EdgeHit> {
        let mut best = None;
        let mut d = radius * radius;
        for h in &self.edge_hits {
            if h.container != container {
                continue;
            }
            let k = distance_to_polyline(x, y, &h.pts);
            if k <= d {
                d = k;
                best = Some(h.clone());
            }
        }
        best
    }

    pub fn scroll(&mut self, e: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.m.clone() else { return };
        self.flight = None;
        self.unhover(cx);
        let (x, y) = self.local(e.position);
        // AppKit's deltas are the opposite way round to a browser's.
        let dy = match e.delta {
            ScrollDelta::Pixels(p) => -f64::from(p.y),
            ScrollDelta::Lines(l) => -(l.y as f64) * 16.0,
        };
        // Scrolling zooms; a pinch arrives as ctrl+scroll in finer steps. Dragging pans.
        let k = if e.modifiers.control { 0.011 } else { 0.0022 };
        self.camera.zoom(&m, (-dy.clamp(-160.0, 160.0) * k).exp(), x, y);
        self.changed(cx);
    }

    pub fn down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        self.menu = None;
        if self.m.is_none() {
            return;
        }
        self.flight = None;
        if e.click_count == 2 {
            self.press = None;
            let (x, y) = self.local(e.position);
            self.open_at(x, y, cx);
            return;
        }
        self.unhover(cx);
        self.press = Some(Press { at: e.position, last: e.position, moved: false });
        self.cursor = CursorStyle::ClosedHand;
        cx.notify();
    }

    pub fn moved(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let (x, y) = self.local(e.position);
        let Some(p) = self.press.as_mut() else {
            if self.m.is_none() || self.flight.is_some() {
                return;
            }
            let i = hit_at(&self.hits, x, y, false);
            let edge = self.edge_at(x, y, i, 5.0);
            let id = if edge.is_some() { None } else { (i >= 0).then(|| self.hits[i as usize].id) };
            let mut redraw = false;
            if edge.as_ref().map(|h| h.edge) != self.hover_edge.as_ref().map(|h| h.edge) {
                self.hover_edge = edge;
                redraw = true;
            }
            if id != self.hover {
                self.hover = id;
                self.sync_hover(cx);
                redraw = true;
            }
            let cursor = if id.is_none() && self.hover_edge.is_none() { CursorStyle::OpenHand } else { CursorStyle::PointingHand };
            if cursor != self.cursor {
                self.cursor = cursor;
                redraw = true;
            }
            if redraw {
                cx.notify();
            }
            return;
        };
        if !e.dragging() {
            self.press = None;
            return;
        }
        let (dx, dy) = (f64::from(e.position.x - p.last.x), f64::from(e.position.y - p.last.y));
        p.last = e.position;
        if f64::from(e.position.x - p.at.x).hypot(f64::from(e.position.y - p.at.y)) > 4.0 {
            p.moved = true;
        }
        self.camera.pan(dx, dy);
        self.changed(cx);
    }

    pub fn up(&mut self, e: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.press.take() else { return };
        self.cursor = if self.hover.is_none() { CursorStyle::OpenHand } else { CursorStyle::PointingHand };
        if !p.moved {
            let (x, y) = self.local(e.position);
            let i = hit_at(&self.hits, x, y, false);
            if let Some(edge) = self.edge_at(x, y, i, 5.0) {
                self.select_edge(edge.edge, cx);
            } else {
                let path = (i >= 0).then(|| path_of(&self.hits, self.m.as_ref().unwrap().root(), i));
                self.select(path.as_ref().and_then(|p| p.last().copied()), path, cx);
            }
        }
        cx.notify();
    }

    pub fn left(&mut self, cx: &mut Context<Self>) {
        if self.press.is_none() {
            self.unhover(cx);
        }
    }

    /// Opens the block at a point (a double click): zooms into it, or goes to its definition.
    fn open_at(&mut self, x: f64, y: f64, cx: &mut Context<Self>) {
        let Some(m) = self.m.clone() else { return };
        let i = hit_at(&self.hits, x, y, true);
        if i < 0 {
            self.camera.zoom(&m, 2.0, x, y);
            return self.changed(cx);
        }
        let n = m.node(self.hits[i as usize].id);
        // A block with nothing inside but a definition elsewhere goes there.
        match n.link {
            Some(link) if m.owner(n).is_none() && link != n.id => {
                let path = m.canonical_path(link);
                self.select(Some(link), Some(path), cx);
                self.fly_to_node(link, cx);
            }
            _ => {
                let path = path_of(&self.hits, m.root(), i);
                self.fly_to(&path, Framing::Open, 0.08, cx);
            }
        }
    }

    pub fn key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        let key = k.key.as_str();
        let ch = k.key_char.as_deref().unwrap_or(key);
        let cmd = k.modifiers.platform || k.modifiers.control;
        if cmd && key == "k" {
            cx.stop_propagation();
            if matches!(self.dialog, Some(Dialog::Search(_))) {
                self.close_dialog(window, cx);
            } else if self.m.is_some() {
                self.open_search(window, cx);
            }
            return;
        }
        if cmd && key == "o" {
            cx.stop_propagation();
            return self.prompt_folder(cx);
        }
        // Esc closes a dialog, else the details panel (as its close button does); it never moves the view.
        if key == "escape" {
            if self.menu.take().is_some() {
                return cx.notify();
            }
            if self.dialog.is_some() {
                if self.m.is_some() || !matches!(self.dialog, Some(Dialog::Open)) {
                    self.close_dialog(window, cx);
                }
                return;
            }
            if self.selected.is_some() {
                self.select(None, None, cx);
            } else if self.removed_sel.is_some() {
                self.removed_sel = None;
                self.camera.insets = self.insets();
                self.sync_panel(cx);
            } else if self.value.is_some() {
                self.close_value(cx);
            } else if self.compare.is_some() && self.changes_open {
                self.changes_open = false;
                self.camera.insets = self.insets();
                self.sync_panel(cx);
            }
            return cx.notify();
        }
        if self.dialog.is_some() {
            return;
        }
        // Back and forward, as in a browser.
        if (k.modifiers.alt && key == "left") || (cmd && key == "[") {
            return self.back(cx);
        }
        if (k.modifiers.alt && key == "right") || (cmd && key == "]") {
            return self.forward(cx);
        }
        if cmd || k.modifiers.alt || self.m.is_none() {
            return;
        }
        match ch {
            "/" => {
                cx.stop_propagation();
                self.open_search(window, cx);
            }
            "f" => self.fit(cx),
            "h" | "0" => self.home(cx),
            "+" | "=" => self.zoom_by(1.5, cx),
            "-" | "_" => self.zoom_by(1.0 / 1.5, cx),
            "n" | "p" if self.compare.is_some() => self.step_change(if ch == "n" { 1 } else { -1 }, cx),
            _ => match key {
                "enter" => self.open_selected(cx),
                "left" => self.pan_by(90.0, 0.0, cx),
                "right" => self.pan_by(-90.0, 0.0, cx),
                "up" => self.pan_by(0.0, 90.0, cx),
                "down" => self.pan_by(0.0, -90.0, cx),
                _ => {}
            },
        }
    }

    // ----- dialogs --------------------------------------------------------------------

    pub fn close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dialog = None;
        window.focus(&self.focus);
        cx.notify();
    }

    pub fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.m.clone() else { return };
        let ui = self.ui.clone();
        let input = cx.new(|cx| TextInput::new("Jump to a file, class, function or block…", &ui, cx));
        let edits = cx.observe(&input, |v, input, cx| {
            let query = input.read(cx).content.to_string();
            let Some(m) = v.m.clone() else { return };
            if let Some(Dialog::Search(s)) = v.dialog.as_mut() {
                if s.query != query {
                    s.results = m.find(&query);
                    s.query = query;
                    s.active = 0;
                    s.scroll.scroll_to_item(0);
                    cx.notify();
                }
            }
        });
        window.focus(&gpui::Focusable::focus_handle(input.read(cx), cx));
        self.menu = None;
        self.dialog = Some(Dialog::Search(Search { input, query: String::new(), results: m.find(""), active: 0, scroll: gpui::ScrollHandle::new(), _edits: edits }));
        cx.notify();
    }

    /// Picks a search result: flies to it and selects it.
    pub fn pick_result(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Dialog::Search(s)) = &self.dialog else { return };
        let Some(&id) = s.results.get(i) else { return };
        self.close_dialog(window, cx);
        self.go_to(id, cx);
    }

    /// Asks for a folder to open.
    pub fn prompt_folder(&mut self, cx: &mut Context<Self>) {
        self.menu = None;
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Open".into()) });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                if let Some(p) = paths.into_iter().next() {
                    let _ = this.update(cx, |v, cx| v.open_folder(p, cx));
                }
            }
        })
        .detach();
    }

    // ----- downloads ------------------------------------------------------------------

    /// Measures text as the screen sets it, for drawing into an SVG.
    fn measurer(cx: &App) -> impl FnMut(&str, crate::pen::Font) -> f64 + 'static {
        let ts = cx.text_system().clone();
        move |text: &str, f: crate::pen::Font| {
            let id = ts.resolve_font(&crate::gpuipen::gpui_font(f.family, f.weight));
            text.chars().map(|c| ts.advance(id, px(f.px as f32), c).map_or(0.0, |a| f64::from(a.width))).sum()
        }
    }

    /// The view as SVG, drawn as it is on screen.
    pub fn view_svg(&mut self, cx: &App) -> Option<String> {
        let m = self.m.clone()?;
        let xfs = self.camera.transforms(&m);
        let mut pen = SvgPen::new(Self::measurer(cx));
        let related = self.related_set(&m);
        let compare = self.compare.as_ref().map(|c| c.diff.clone());
        let p = Paint {
            width: self.width,
            height: self.height,
            hover: None,
            selected: self.selected,
            path: &self.camera.path,
            xfs: &xfs,
            related: related.as_ref(),
            trace: self.trace.as_ref(),
            hover_edge: None,
            diff: compare.as_ref().map(|d| &**d as &dyn Changes),
        };
        Renderer::default().draw(&mut pen, &m, &self.theme, &p);
        Some(pen.svg(self.width, self.height))
    }

    /// Saves block `id` alone as SVG (of the base version, `base`, for a block a change removed).
    pub fn download_block(&mut self, id: u32, base: bool, cx: &mut Context<Self>) {
        let (m, diff) = match (&self.compare, base) {
            (Some(c), true) => (c.base.clone(), None),
            (c, _) => match self.m.clone() {
                Some(m) => (m, c.as_ref().map(|c| c.diff.clone())),
                None => return,
            },
        };
        let mut pen = SvgPen::new(Self::measurer(cx));
        let svg = crate::export::block_svg(&mut pen, &m, id, &self.theme, if base { None } else { self.trace.as_ref() }, diff.as_ref().map(|d| &**d as &dyn Changes));
        let n = m.node(id);
        let name = if n.title.is_empty() { crate::diff::block_name(&m, n) } else { m.qualified(id) };
        self.save(&name, "svg", svg.into_bytes(), cx);
    }

    /// Saves `bytes` where the reader says, under a name made of `title`.
    pub fn save(&mut self, title: &str, ext: &str, bytes: Vec<u8>, cx: &mut Context<Self>) {
        let name: String = title.chars().map(|c| if "\\/:*?\"<>|".contains(c) || c.is_control() { '-' } else { c }).collect::<String>().trim().chars().take(80).collect();
        let name = if name.is_empty() { "diagram".to_string() } else { name };
        let dir = std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Downloads")).unwrap_or_else(|| PathBuf::from("."));
        let rx = cx.prompt_for_new_path(&dir, Some(&format!("{name}.{ext}")));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(path))) = rx.await {
                if let Err(e) = std::fs::write(&path, bytes) {
                    let _ = this.update(cx, |v, cx| {
                        v.error = Some(format!("Could not save {}: {e}", path.display()));
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    pub fn download(&mut self, png: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        let Some(svg) = self.view_svg(cx) else { return };
        let title = self.m.as_ref().map(|m| m.scene.title.clone()).unwrap_or_default();
        if png {
            match crate::export::png(&svg, window.scale_factor()) {
                Some(bytes) => self.save(&title, "png", bytes, cx),
                None => self.error = Some("Could not make a PNG of this view.".into()),
            }
        } else {
            self.save(&title, "svg", svg.into_bytes(), cx);
        }
        cx.notify();
    }

    fn related_set(&mut self, m: &Model) -> Option<HashSet<u32>> {
        let focus = self.hover.or(self.peeked).or(self.selected)?;
        if self.related.as_ref().map(|r| r.0) != Some(focus) {
            self.related = Some((focus, m.users(focus).iter().copied().collect()));
        }
        self.related.as_ref().map(|r| r.1.clone())
    }

    // ----- frames ---------------------------------------------------------------------

    pub fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let (width, height) = (f64::from(bounds.size.width), f64::from(bounds.size.height));
        let Some(m) = self.m.clone() else {
            window.paint_quad(gpui::fill(bounds, self.theme.background.hsla()));
            (self.width, self.height) = (width, height);
            return;
        };
        self.camera.insets = self.insets();
        if self.width <= 1.0 || self.height <= 1.0 || (self.framed && (width, height) != (self.width, self.height)) {
            // A window that had no size, or still shows the whole project: keep it framed.
            self.flight = None;
            self.camera.fit_root(&m, width, height);
        } else if (width, height) != (self.width, self.height) {
            self.camera.pan((width - self.width) / 2.0, (height - self.height) / 2.0);
            self.camera.reanchor(&m, width, height);
        }
        (self.width, self.height) = (width, height);
        if let Some((start, plan)) = &self.flight {
            let t = (start.elapsed().as_secs_f64() * 1000.0 / plan.duration).clamp(0.0, 1.0);
            self.camera.step(&m, plan, ease(t), width, height);
            if t >= 1.0 {
                self.flight = None;
            } else {
                window.request_animation_frame();
            }
        }
        let xfs = self.camera.transforms(&m);
        let related = self.related_set(&m);
        let hover_edge = self.hover_edge.as_ref().map(|h| h.edge);
        let compare = self.compare.as_ref().map(|c| c.diff.clone());
        let p = Paint {
            width,
            height,
            hover: self.hover.or(self.peeked),
            selected: self.selected,
            path: &self.camera.path,
            xfs: &xfs,
            related: related.as_ref(),
            trace: self.trace.as_ref(),
            hover_edge,
            diff: compare.as_ref().map(|d| &**d as &dyn Changes),
        };
        let origin = (f64::from(bounds.origin.x), f64::from(bounds.origin.y));
        let drew = {
            let mut pen = GpuiPen::new(window, cx, origin);
            self.renderer.draw(&mut pen, &m, &self.theme, &p)
        };
        self.hits = drew.hits;
        self.edge_hits = drew.edge_hits;
        // The breadcrumbs follow the view; they are drawn again only when it is somewhere else.
        let focus = self.camera.focus(&m, width, height);
        if focus != self.crumbs {
            self.crumbs = focus;
            if self.menu == Some(Menu::Crumbs) {
                self.menu = None;
            }
            cx.notify();
        }
    }

    /// The diagram's layer: the canvas, and the pointer on it.
    pub fn canvas(&self, cx: &mut Context<Self>) -> AnyElement {
        let view = cx.entity();
        div()
            .id("diagram")
            .absolute()
            .size_full()
            .cursor(self.cursor)
            .on_scroll_wheel(cx.listener(Self::scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::down))
            .on_mouse_move(cx.listener(Self::moved))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::up))
            .on_hover(cx.listener(|v, hovered: &bool, _, cx| {
                if !*hovered {
                    v.left(cx);
                }
            }))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds: Bounds<Pixels>, (), window, cx| {
                        view.update(cx, |v, cx| v.paint(bounds, window, cx));
                    },
                )
                .size_full(),
            )
            .into_any_element()
    }

}
