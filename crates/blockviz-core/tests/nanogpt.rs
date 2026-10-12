//! Abstraction-quality tests against karpathy/nanoGPT (MIT, vendored in fixtures).
//! These pin down what a reader should see at each zoom level.

use blockviz_core::ir::{EdgeKind, Kind, Node, Scene};
use std::path::Path;

fn load(dir: &Path, root: &Path, out: &mut Vec<blockviz_core::project::InFile>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().collect();
    entries.sort_by_key(|e| e.path());
    for e in entries {
        let p = e.path();
        let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
        if p.is_dir() {
            load(&p, root, out);
        } else {
            let bytes = e.metadata().unwrap().len();
            out.push(blockviz_core::project::InFile { path: rel, text: std::fs::read_to_string(&p).ok(), bytes });
        }
    }
}

fn scene() -> Scene {
    scene_with(blockviz_core::layout::Options::default())
}

fn scene_with(opts: blockviz_core::layout::Options) -> Scene {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/nanogpt");
    let mut files = vec![];
    load(&root, &root, &mut files);
    blockviz_core::analyze_with("nanoGPT", files, opts)
}

fn child<'a>(s: &'a Scene, parent: &Node, title: &str) -> &'a Node {
    parent
        .children
        .iter()
        .map(|&c| &s.nodes[c as usize])
        .find(|n| n.title == title)
        .unwrap_or_else(|| panic!("{} has no child {title:?}; has {:?}", parent.title, parent.children.iter().map(|&c| &s.nodes[c as usize].title).collect::<Vec<_>>()))
}

fn path<'a>(s: &'a Scene, titles: &[&str]) -> &'a Node {
    let mut n = &s.nodes[s.root as usize];
    for t in titles {
        n = child(s, n, t);
    }
    n
}

fn steps<'a>(s: &'a Scene, n: &Node) -> Vec<&'a Node> {
    n.children.iter().map(|&c| &s.nodes[c as usize]).filter(|c| !matches!(c.kind, Kind::Input | Kind::Output)).collect()
}

fn has_edge(s: &Scene, n: &Node, from: &str, to: &str) -> bool {
    n.edges.iter().any(|e| s.nodes[e.from as usize].title == from && s.nodes[e.to as usize].title == to)
}

#[test]
fn repo_level_is_a_pipeline_of_programs_and_artifacts() {
    let s = scene();
    let root = &s.nodes[s.root as usize];
    for t in ["model.py", "train.py", "sample.py", "configurator.py", "config/", "data/", "train.bin", "ckpt.pt", "meta.pkl"] {
        child(&s, root, t);
    }
    assert!(root.edges.iter().any(|e| s.nodes[e.from as usize].title == "model.py" && s.nodes[e.to as usize].title == "train.py" && e.label.contains("GPT")));
    assert!(root.edges.iter().any(|e| e.kind == EdgeKind::Exec && s.nodes[e.from as usize].title == "configurator.py"));
    assert!(has_edge(&s, root, "train.py", "ckpt.pt") && has_edge(&s, root, "ckpt.pt", "sample.py"));
    assert!(has_edge(&s, root, "data/", "train.bin") && has_edge(&s, root, "train.bin", "train.py"));
}

#[test]
fn no_syntax_noise() {
    let s = scene();
    for n in &s.nodes {
        assert!(!n.title.contains("__name__"), "syntax leaked into a block: {}", n.title);
        assert!(!n.title.starts_with("import "), "imports are not blocks: {}", n.title);
        assert_ne!(n.title, "print", "prints are effects, not blocks");
        assert_ne!(n.title, "no_grad", "context managers are not blocks");
    }
}

#[test]
fn model_composition() {
    let s = scene();
    let model = path(&s, &["model.py"]);
    let titles: Vec<&str> = steps(&s, model).iter().map(|n| n.title.as_str()).collect();
    assert_eq!(titles, ["LayerNorm", "CausalSelfAttention", "MLP", "Block", "GPTConfig", "GPT"]);
    assert!(has_edge(&s, model, "Block", "GPT"));
    assert!(has_edge(&s, model, "CausalSelfAttention", "Block"));
    assert!(!path(&s, &["model.py", "GPTConfig"]).rows.is_empty(), "dataclass reads as a table");
}

#[test]
fn transformer_block_reads_like_the_paper() {
    let s = scene();
    let fwd = path(&s, &["model.py", "Block", "forward"]);
    let titles: Vec<&str> = steps(&s, fwd).iter().map(|n| n.title.as_str()).collect();
    assert_eq!(titles, ["ln_1", "attn", "+", "ln_2", "mlp", "+"]);
    // Residual: the block input feeds the first add directly.
    assert!(has_edge(&s, fwd, "x", "+"), "residual connection from x");
    let attn = child(&s, fwd, "attn");
    assert_eq!(attn.subtitle, "CausalSelfAttention");
    assert_eq!(s.nodes[attn.reference.unwrap() as usize].title, "forward", "attn zooms into CausalSelfAttention.forward");
}

#[test]
fn gpt_forward_has_repeated_blocks_and_both_outputs() {
    let s = scene();
    let fwd = path(&s, &["model.py", "GPT", "forward"]);
    let titles: Vec<&str> = steps(&s, fwd).iter().map(|n| n.title.as_str()).collect();
    for t in ["wte", "wpe", "+", "drop", "ln_f"] {
        assert!(titles.contains(&t), "{t} missing from {titles:?}");
    }
    let lp = steps(&s, fwd).into_iter().find(|n| n.kind == Kind::Loop).expect("loop over blocks");
    assert!(lp.badges.iter().any(|b| b == "× n_layer"), "{:?}", lp.badges);
    let block = steps(&s, lp)[0];
    assert_eq!(block.title, "block");
    let out = fwd.children.iter().map(|&c| &s.nodes[c as usize]).find(|n| n.kind == Kind::Output).unwrap();
    assert_eq!(out.title, "logits, loss");
}

#[test]
fn training_script_is_sectioned_by_its_comments() {
    let s = scene();
    let train = path(&s, &["train.py"]);
    for t in ["Model init", "Training loop", "get_batch", "estimate_loss", "get_lr", "configurator.py"] {
        child(&s, train, t);
    }
    assert!(steps(&s, train).len() <= 24, "top level stays readable: {}", steps(&s, train).len());
}

#[test]
fn notebooks_and_non_python_files_are_handled() {
    let s = scene();
    let nb = path(&s, &["scaling_laws.ipynb"]);
    assert_eq!(nb.kind, Kind::Notebook);
    assert!(!nb.children.is_empty());
    let docs = path(&s, &["Docs & assets"]);
    assert!(docs.children.len() >= 3);
}

#[test]
fn layout_invariants() {
    let s = scene();
    for n in &s.nodes {
        let kids: Vec<&Node> = n.children.iter().map(|&c| &s.nodes[c as usize]).collect();
        for k in &kids {
            assert!(k.x >= -0.5 && k.y >= -0.5 && k.x + k.w <= n.cw + 0.5 && k.y + k.h <= n.ch + 0.5, "{} escapes {}", k.title, n.title);
        }
        for (i, a) in kids.iter().enumerate() {
            for b in &kids[i + 1..] {
                let overlap = a.x < b.x + b.w - 0.5 && b.x < a.x + a.w - 0.5 && a.y < b.y + b.h - 0.5 && b.y < a.y + a.h - 0.5;
                assert!(!overlap || a.h == 0. || b.h == 0., "{} overlaps {} in {}", a.title, b.title, n.title);
            }
        }
        for e in &n.edges {
            assert!(e.points.len() >= 4 && e.points.len() % 2 == 0, "edge without route in {}", n.title);
        }
    }
}

#[test]
fn analysis_is_deterministic() {
    let a = serde_json::to_string(&scene()).unwrap();
    let b = serde_json::to_string(&scene()).unwrap();
    assert_eq!(a, b);
}

#[test]
fn browser_reply_is_the_scene() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/nanogpt");
    let mut files = vec![];
    load(&root, &root, &mut files);
    let files: Vec<_> = files.iter().map(|f| serde_json::json!({ "path": f.path, "text": f.text, "bytes": f.bytes })).collect();
    let input = serde_json::to_vec(&serde_json::json!({ "name": "nanoGPT", "files": files })).unwrap();
    // The scene's bytes as the CLI writes them, wrapped.
    let scene = serde_json::to_vec(&scene()).unwrap();
    assert_eq!(blockviz_core::analyze_project(input), [&br#"{"ok":true,"scene":"#[..], &scene, b"}"].concat());
    let error: serde_json::Value = serde_json::from_slice(&blockviz_core::analyze_project(b"{".to_vec())).unwrap();
    assert_eq!(error["ok"], false);
}

#[test]
fn class_view_shows_components_and_method_calls() {
    let s = scene();
    let gpt = path(&s, &["model.py", "GPT"]);
    let comps = child(&s, gpt, "components");
    let transformer = child(&s, comps, "transformer");
    let h = child(&s, transformer, "h");
    assert_eq!(h.subtitle, "Block × n_layer");
    assert!(has_edge(&s, gpt, "get_num_params", "estimate_mfu"), "nested self.method() calls are found");
    assert!(has_edge(&s, gpt, "forward", "generate"), "self(...) calls forward");
}

#[test]
fn components_know_the_calls_that_use_them() {
    let s = scene();
    let attn = path(&s, &["model.py", "CausalSelfAttention"]);
    let c_attn = child(&s, child(&s, attn, "components"), "c_attn");
    let call = child(&s, child(&s, attn, "forward"), "c_attn");
    assert_eq!(c_attn.used_by, vec![call.id]);
    // Inside a group (`self.transformer.wte(idx)`), and on every branch that calls it.
    let gpt = path(&s, &["model.py", "GPT"]);
    let wte = child(&s, child(&s, child(&s, gpt, "components"), "transformer"), "wte");
    assert_eq!(wte.used_by, vec![child(&s, child(&s, gpt, "forward"), "wte").id]);
    let lm_head = child(&s, child(&s, gpt, "components"), "lm_head");
    assert_eq!(lm_head.used_by.len(), 2);
}

#[test]
fn scripts_open_with_settings_and_configuration() {
    let s = scene();
    for script in ["train.py", "sample.py", "bench.py"] {
        let f = path(&s, &[script]);
        let first: Vec<&str> = steps(&s, f).iter().take(2).map(|n| n.title.as_str()).collect();
        assert_eq!(first, ["Settings", "configurator.py"], "{script} starts with its settings");
        assert!(f.badges.iter().any(|b| b == "entry point"));
    }
    let model = path(&s, &["model.py"]);
    assert_eq!(model.subtitle, "module");
    assert!(!model.badges.iter().any(|b| b == "entry point"), "imported files are modules");
}

#[test]
fn artifacts_name_only_the_files_actually_touched() {
    let s = scene();
    let prep = s.nodes.iter().find(|n| n.title == "shakespeare_char/prepare.py").unwrap();
    let mut all = vec![];
    fn collect<'a>(s: &'a Scene, n: &'a Node, out: &mut Vec<&'a Node>) {
        out.push(n);
        for &c in &n.children {
            collect(s, &s.nodes[c as usize], out);
        }
    }
    collect(&s, prep, &mut all);
    let tofile: Vec<&&Node> = all.iter().filter(|n| n.title == "tofile").collect();
    assert_eq!(tofile.len(), 2);
    for t in tofile {
        assert!(!t.effects.iter().any(|e| e.contains("input.txt")), "data does not carry file names: {:?}", t.effects);
    }
}

#[test]
fn shared_trunks_are_a_choice() {
    // `B, T, C` feeds the three reshapes and the output: one line per value
    // it carries by default, one trunk for all of them when asked.
    let starts = |s: &Scene| {
        let f = path(s, &["model.py", "CausalSelfAttention", "forward"]);
        let btc = child(s, f, "B, T, C").id;
        let mut xs: Vec<i64> = f.edges.iter().filter(|e| e.from == btc).map(|e| e.points[0].round() as i64).collect();
        xs.sort();
        xs.dedup();
        xs.len()
    };
    assert!(starts(&scene()) > 1);
    assert_eq!(starts(&scene_with(blockviz_core::layout::Options { shared_trunks: true })), 1);
}
