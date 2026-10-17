//! The window driven headlessly with GPUI's test platform: clicks, keys and
//! what the panels, bars and dialogs show.

use crate::camera::Framing;
use crate::live::{self, File, Files};
use crate::model::Model;
use crate::panel::Content;
use crate::viewer::{Dialog, Menu, Viewer};
use gpui::{point, px, Entity, Modifiers, Pixels, Point, TestAppContext, VisualTestContext};
use std::path::PathBuf;
use std::rc::Rc;

const MODEL_PY: &str = "class Block:\n    def __init__(self):\n        self.attn = Attention()\n\n    def forward(self, x):\n        x = x + self.attn(x)\n        x = x + self.mlp(x)\n        return x\n\n\nclass Attention:\n    def __call__(self, x):\n        q = self.q(x)\n        return q\n";
const TRAIN_PY: &str = "from model import Block\nb = Block()\nb.forward(1)\n";

fn files(list: &[(&str, &str)]) -> Files {
    list.iter().map(|(p, t)| (p.to_string(), File { text: Some(t.to_string()), bytes: t.len() as u64 })).collect()
}

fn model(f: &Files) -> Model {
    Model::new(blockviz_core::analyze("t", live::inputs(f)), live::texts(f))
}

/// Draws the window as the next frame would (the test platform draws only on input),
/// twice: a frame can ask for another, as the breadcrumbs do when the view moved.
fn frame(cx: &mut VisualTestContext) {
    for _ in 0..2 {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear());
    }
    cx.run_until_parked();
}

fn open(cx: &mut TestAppContext) -> (Entity<Viewer>, &mut VisualTestContext) {
    cx.update(|cx| {
        crate::input::bind_keys(cx);
        crate::menus::bind(cx);
    });
    let f = files(&[("model.py", MODEL_PY), ("train.py", TRAIN_PY)]);
    let m = model(&f);
    let (view, cx) = cx.add_window_view(|window, cx| Viewer::new(window, cx));
    view.update(cx, |v, cx| v.show(PathBuf::from("/tmp/t"), f, m, cx));
    frame(cx);
    (view, cx)
}

fn titled(v: &Viewer, title: &str) -> u32 {
    v.m.as_ref().unwrap().scene.nodes.iter().find(|n| n.title == title).unwrap().id
}

fn center(b: gpui::Bounds<Pixels>) -> Point<Pixels> {
    point(b.origin.x + b.size.width / 2.0, b.origin.y + b.size.height / 2.0)
}

/// Finishes the flight under way, as its last frame would.
fn land(view: &Entity<Viewer>, cx: &mut VisualTestContext) {
    view.update(cx, |v, cx| {
        if let Some((_, plan)) = v.flight.take() {
            let (w, h) = (v.width, v.height);
            let m = v.m.clone().unwrap();
            v.camera.step(&m, &plan, 1.0, w, h);
            cx.notify();
        }
    });
    frame(cx);
}

fn content(view: &Entity<Viewer>, cx: &mut VisualTestContext) -> Content {
    view.read_with(cx, |v, cx| v.inspector.read(cx).content())
}

#[gpui::test]
fn a_click_opens_the_panel_and_a_line_of_source_selects_its_block(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let (file, at) = view.read_with(cx, |v, _| {
        let file = titled(v, "model.py");
        let r = v.hits.iter().find(|h| h.id == file).unwrap().rect;
        (file, point(px((r.x + r.w / 2.0) as f32), px((r.y + 6.0) as f32)))
    });
    cx.simulate_click(at, Modifiers::none());
    frame(cx);
    assert_eq!(view.read_with(cx, |v, _| v.selected), Some(file));
    assert_eq!(content(&view, cx), Content::Block(file));
    let panel = cx.debug_bounds("inspector").expect("the panel is open");
    // A step is shown in its function, its own line marked; clicking that line selects the block drawn for it.
    let step = view.update(cx, |v, cx| {
        let forward = titled(v, "forward");
        let m = v.m.clone().unwrap();
        let step = m.block_at(forward, 6);
        assert_ne!(step, forward);
        v.select(Some(step), Some(m.canonical_path(step)), cx);
        step
    });
    frame(cx);
    let line = cx.debug_bounds("marked-line").expect("its line is marked");
    assert!(panel.contains(&line.origin), "{line:?} in {panel:?}");
    cx.simulate_click(center(line), Modifiers::none());
    frame(cx);
    assert_eq!(view.read_with(cx, |v, _| v.selected), Some(step));
    // Its close button closes it, and so does Esc.
    let close = cx.debug_bounds("panel-close").expect("a close button");
    cx.simulate_click(center(close), Modifiers::none());
    frame(cx);
    assert_eq!(view.read_with(cx, |v, _| v.selected), None);
    view.update(cx, |v, cx| v.select(Some(step), None, cx));
    frame(cx);
    cx.simulate_keystrokes("escape");
    frame(cx);
    assert_eq!(view.read_with(cx, |v, _| v.selected), None);
}

#[gpui::test]
fn breadcrumbs_follow_a_flight_and_back_returns_where_the_reader_was(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let start = view.read_with(cx, |v, _| (v.camera.path.clone(), v.camera.xf));
    view.update(cx, |v, cx| {
        let path = v.m.as_ref().unwrap().canonical_path(titled(v, "forward"));
        v.fly_to(&path, Framing::Open, 0.08, cx);
    });
    land(&view, cx);
    let crumbs = view.read_with(cx, |v, _| v.crumbs.iter().map(|&c| v.m.as_ref().unwrap().node(c).title.clone()).collect::<Vec<_>>());
    assert_eq!(crumbs.last().map(String::as_str), Some("forward"), "{crumbs:?}");
    assert!(view.read_with(cx, |v, _| v.history.can_back()));
    // Back (⌥←) flies to exactly where the view was.
    cx.simulate_keystrokes("alt-left");
    land(&view, cx);
    let back = view.read_with(cx, |v, _| (v.camera.path.clone(), v.camera.xf));
    assert_eq!(back.0, start.0);
    assert!((back.1.s - start.1.s).abs() < 1e-6 && (back.1.x - start.1.x).abs() < 1e-3);
    assert!(view.read_with(cx, |v, _| v.history.can_forward()));
    // A crumb flies back out to fit its block.
    view.update(cx, |v, cx| {
        let path = v.m.as_ref().unwrap().canonical_path(titled(v, "forward"));
        v.fly_to(&path, Framing::Open, 0.08, cx);
    });
    land(&view, cx);
    let root = cx.debug_bounds("crumb-root").expect("crumbs are shown");
    cx.simulate_click(center(root), Modifiers::none());
    frame(cx);
    land(&view, cx);
    let root_id = view.read_with(cx, |v, _| v.m.as_ref().unwrap().root());
    assert_eq!(view.read_with(cx, |v, _| v.camera.path.clone()), [root_id]);
}

#[gpui::test]
fn keys_zoom_pan_fit_and_go_home(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let s0 = view.read_with(cx, |v, _| v.camera.transforms(v.m.as_ref().unwrap())[0].s);
    cx.simulate_keystrokes("=");
    frame(cx);
    let s1 = view.read_with(cx, |v, _| v.camera.transforms(v.m.as_ref().unwrap())[0].s);
    assert!((s1 / s0 - 1.5).abs() < 1e-6, "{s0} → {s1}");
    let x = view.read_with(cx, |v, _| v.camera.transforms(v.m.as_ref().unwrap())[0].x);
    cx.simulate_keystrokes("left");
    frame(cx);
    let x1 = view.read_with(cx, |v, _| v.camera.transforms(v.m.as_ref().unwrap())[0].x);
    assert!((x1 - x - 90.0).abs() < 1e-6);
    cx.simulate_keystrokes("h");
    frame(cx);
    assert!(view.read_with(cx, |v, _| v.flight.is_some()));
}

#[gpui::test]
fn search_finds_a_method_by_its_qualified_name_and_flies_there(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    cx.simulate_keystrokes("/");
    frame(cx);
    assert!(cx.debug_bounds("search").is_some());
    let typed = |v: &Viewer, cx: &gpui::App| match &v.dialog {
        Some(Dialog::Search(s)) => Some(s.input.read(cx).content.to_string()),
        _ => None,
    };
    // The key that opened the palette is not typed into it.
    assert_eq!(view.read_with(cx, |v, cx| typed(v, cx)), Some(String::new()));
    cx.simulate_input("Block.forward");
    frame(cx);
    let forward = view.read_with(cx, |v, _| titled(v, "forward"));
    let first = view.read_with(cx, |v, _| match &v.dialog {
        Some(Dialog::Search(s)) => s.results.first().copied(),
        _ => None,
    });
    assert_eq!(first, Some(forward));
    // Typing never runs the diagram's keys.
    assert!(view.read_with(cx, |v, _| v.flight.is_none()));
    cx.simulate_keystrokes("down up enter");
    frame(cx);
    view.read_with(cx, |v, _| {
        assert!(v.dialog.is_none());
        assert_eq!(v.selected, Some(forward));
        assert!(v.flight.is_some());
    });
    cx.simulate_keystrokes("cmd-k");
    frame(cx);
    assert!(view.read_with(cx, |v, _| matches!(v.dialog, Some(Dialog::Search(_)))));
    cx.simulate_keystrokes("escape");
    frame(cx);
    assert!(view.read_with(cx, |v, _| v.dialog.is_none()));
}

#[gpui::test]
fn following_a_value_lists_where_it_comes_from_and_goes(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    // Open Block.forward and follow `x`: the value panel names it.
    view.update(cx, |v, cx| {
        let path = v.m.as_ref().unwrap().canonical_path(titled(v, "forward"));
        v.fly_to(&path, Framing::Open, 0.08, cx);
    });
    land(&view, cx);
    let edge = view.read_with(cx, |v, _| v.m.as_ref().unwrap().node(titled(v, "forward")).edges.iter().position(|e| e.label == "x").map(|i| (titled(v, "forward"), i))).expect("an edge carrying x");
    view.update(cx, |v, cx| v.select_edge_for_test(edge, cx));
    frame(cx);
    match content(&view, cx) {
        Content::Value { owner, edges, .. } => {
            assert_eq!(owner, edge.0);
            assert!(edges.len() >= 2, "{edges:?}");
        }
        c => panic!("not a value: {c:?}"),
    }
    assert!(view.read_with(cx, |v, _| v.trace.is_some()));
    cx.simulate_keystrokes("escape");
    frame(cx);
    assert!(view.read_with(cx, |v, _| v.value.is_none()));
}

#[gpui::test]
fn a_comparison_lists_the_changes_and_steps_through_them(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    // The base is the code before `mlp` was added, and before `attn` was renamed.
    let old = files(&[("model.py", &MODEL_PY.replace("        x = x + self.mlp(x)\n", "")), ("train.py", TRAIN_PY)]);
    view.update(cx, |v, cx| {
        let base = Rc::new(model(&old));
        let head = v.m.clone().unwrap();
        let changes = crate::git::changes(&old, &v.project.as_ref().unwrap().files);
        let diff = Rc::new(crate::diff::SceneDiff::new(base.clone(), head, changes));
        v.compare = Some(crate::viewer::Compared { rev: "HEAD".into(), kind: "Uncommitted changes".into(), label: "HEAD".into(), title: "Working tree vs HEAD".into(), base_files: old.clone(), base, diff });
        v.changes_open = true;
        v.sync_panel(cx);
        cx.notify();
    });
    frame(cx);
    assert_eq!(content(&view, cx), Content::Changes);
    let total = view.read_with(cx, |v, _| v.compare.as_ref().unwrap().diff.changes.len());
    assert!(total >= 1);
    cx.simulate_keystrokes("n");
    frame(cx);
    assert_eq!(view.read_with(cx, |v, _| v.change_index()), Some(0));
    assert!(matches!(content(&view, cx), Content::Block(_) | Content::Removed(_)));
    // Esc goes back to the list, and again closes it.
    view.update(cx, |v, cx| v.show_changes(cx));
    frame(cx);
    cx.simulate_keystrokes("escape");
    frame(cx);
    assert!(!view.read_with(cx, |v, _| v.changes_open));
}

#[gpui::test]
fn the_toolbar_opens_its_menus_and_switches_the_scheme(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    let dark = view.read_with(cx, |v, _| v.dark());
    view.update(cx, |v, cx| v.toggle_scheme(cx));
    frame(cx);
    assert_eq!(view.read_with(cx, |v, _| (v.dark(), v.theme.dark)), (!dark, !dark));
    view.update(cx, |v, cx| v.toggle_scheme(cx));
    view.update(cx, |v, cx| {
        v.menu = Some(Menu::Download);
        cx.notify();
    });
    frame(cx);
    cx.simulate_keystrokes("escape");
    frame(cx);
    assert!(view.read_with(cx, |v, _| v.menu.is_none()));
    // The legend lists what this diagram draws.
    view.update(cx, |v, cx| {
        v.legend = true;
        cx.notify();
    });
    frame(cx);
}

#[gpui::test]
fn compare_reads_the_folder_at_a_revision_and_shows_what_changed(cx: &mut TestAppContext) {
    let dir = std::env::temp_dir().join(format!("blockviz-compare-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let git = |args: &[&str]| {
        let ok = std::process::Command::new("git").args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null"]).args(args).current_dir(&dir).output().unwrap().status.success();
        assert!(ok, "git {args:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("model.py"), MODEL_PY.replace("        x = x + self.mlp(x)\n", "")).unwrap();
    std::fs::write(dir.join("train.py"), TRAIN_PY).unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "first"]);
    // The working tree adds a step.
    std::fs::write(dir.join("model.py"), MODEL_PY).unwrap();
    cx.update(crate::input::bind_keys);
    let f = live::read(&dir);
    let m = model(&f);
    let (view, cx) = cx.add_window_view(|window, cx| Viewer::new(window, cx));
    view.update(cx, |v, cx| {
        v.show(dir.clone(), f, m, cx);
        let repo = crate::git::Repo::find(&dir).expect("a repository");
        v.project.as_mut().unwrap().repo = Some(repo);
        v.start_compare("HEAD".into(), "Uncommitted changes".into(), "HEAD".into(), "Working tree vs HEAD".into(), cx);
    });
    for _ in 0..50 {
        frame(cx);
        if view.read_with(cx, |v, _| v.compare.is_some()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let (files, changes) = view.read_with(cx, |v, _| {
        let c = v.compare.as_ref().expect("comparing");
        (c.diff.files.iter().map(|f| f.path.clone()).collect::<Vec<_>>(), c.diff.changes.len())
    });
    assert_eq!(files, ["model.py"]);
    assert!(changes >= 1);
    assert_eq!(content(&view, cx), Content::Changes);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[gpui::test]
fn the_menu_bar_drives_the_view(cx: &mut TestAppContext) {
    let (view, cx) = open(cx);
    // Choosing a menu item dispatches its action to the window.
    cx.dispatch_action(crate::menus::Search);
    frame(cx);
    assert!(view.read_with(cx, |v, _| matches!(v.dialog, Some(Dialog::Search(_)))));
    cx.dispatch_action(crate::menus::Search);
    frame(cx);
    assert!(view.read_with(cx, |v, _| v.dialog.is_none()));
    let s0 = view.read_with(cx, |v, _| v.camera.transforms(v.m.as_ref().unwrap())[0].s);
    cx.dispatch_action(crate::menus::ZoomIn);
    frame(cx);
    let s1 = view.read_with(cx, |v, _| v.camera.transforms(v.m.as_ref().unwrap())[0].s);
    assert!((s1 / s0 - 1.5).abs() < 1e-6);
    cx.dispatch_action(crate::menus::ToggleLegend);
    frame(cx);
    assert!(view.read_with(cx, |v, _| v.legend));
    cx.simulate_keystrokes("cmd-0");
    frame(cx);
    assert!(view.read_with(cx, |v, _| v.flight.is_some()));
    // The menus list the folders opened lately.
    let menus = crate::menus::menus(&["/a/one".into(), "/b/two".into()]);
    let names: Vec<String> = menus.iter().map(|m| m.name.to_string()).collect();
    assert_eq!(names, ["blockviz", "File", "Edit", "View", "Compare", "Window"]);
}
