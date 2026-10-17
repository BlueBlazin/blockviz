//! Keeps the reader where they were when the project is analyzed again: block
//! ids are the new scene's, so blocks are matched by where they are, and the
//! view is moved so that the block it showed stays put on screen.

use crate::camera::{box_of, zoom_at, Camera, Rect, Xf};
use crate::model::Model;
use crate::paint::{self, Hit};
use blockviz_core::ir::Node;

/// The child of `new_owner` that is `old_child` of `old_owner`: the one of the
/// same kind and title, counting those that share both in order.
fn same_child(old: &Model, old_owner: &Node, old_child: u32, new: &Model, new_owner: &Node) -> Option<u32> {
    let c = old.node(old_child);
    let alike = |m: &Model, n: u32| {
        let k = m.node(n);
        k.kind == c.kind && k.title == c.title
    };
    let nth = old_owner.children.iter().filter(|&&s| alike(old, s)).position(|&s| s == old_child)?;
    new_owner.children.iter().copied().filter(|&s| alike(new, s)).nth(nth)
}

/// An instance path of the old scene in the new one, as far as its blocks are
/// still there: root first, ending at the deepest one found.
pub fn map_path(old: &Model, new: &Model, path: &[u32]) -> Vec<u32> {
    let mut out = vec![new.root()];
    for w in path.windows(2) {
        let (Some(old_owner), Some(new_owner)) = (old.owner(old.node(w[0])), new.owner(new.node(*out.last().unwrap()))) else { break };
        match same_child(old, old_owner, w[1], new, new_owner) {
            Some(c) => out.push(c),
            None => break,
        }
    }
    out
}

/// Where the block at the end of `path` is on screen, through the camera: the
/// path runs through the camera's anchor, or ends on its way.
fn on_screen(camera: &Camera, m: &Model, path: &[u32]) -> Option<Rect> {
    let xfs = camera.transforms(m);
    let k = camera.path.len();
    if path.len() <= k {
        // A block the anchor is in: drawn in the interior of the one before it.
        let i = path.len() - 1;
        return (path == &camera.path[..path.len()] && i > 0).then(|| box_of(xfs[i - 1], m.node(path[i])));
    }
    if path[..k] != camera.path[..] {
        return None;
    }
    let mut t: Xf = camera.xf;
    for (j, &id) in path[k..].iter().enumerate() {
        let n = m.node(id);
        if j + 1 == path.len() - k {
            return Some(box_of(t, n));
        }
        t = m.interior(t, n)?;
    }
    None
}

/// Carries the camera from the old scene into the new: its anchor to the same
/// block (or the deepest one still there), and the block the reader was
/// looking at, at the center of what the overlays leave visible, to where it
/// was on screen and as wide.
pub fn carry(old: &Model, new: &Model, camera: &mut Camera, hits: &[Hit], width: f64, height: f64) {
    let i = camera.insets;
    let (cx, cy) = ((i.left + width - i.right) / 2.0, (i.top + height - i.bottom) / 2.0);
    let at = paint::hit_at(hits, cx, cy, false);
    let looked_at = (at >= 0).then(|| paint::path_of(hits, old.root(), at));
    let before = Camera { path: camera.path.clone(), xf: camera.xf, insets: camera.insets };
    let xfs = camera.transforms(old);

    let path = map_path(old, new, &camera.path);
    camera.xf = xfs[path.len() - 1];
    camera.path = path;
    if let Some(looked_at) = looked_at {
        let found = map_path(old, new, &looked_at);
        let was = on_screen(&before, old, &looked_at[..found.len()]);
        let now = on_screen(camera, new, &found);
        if let (Some(was), Some(now)) = (was, now) {
            if now.w > 0.0 {
                camera.xf = zoom_at(camera.xf, was.w / now.w, now.x, now.y);
                camera.pan(was.x - now.x, was.y - now.y);
            }
        }
    }
    camera.reanchor(new, width, height);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Framing;
    use crate::paint::Walk;

    const MODEL_PY: &str = "class Block:\n    def forward(self, x):\n        x = x + self.attn(x)\n        x = x + self.mlp(x)\n        return x\n\n\nclass MLP:\n    def forward(self, x):\n        return self.fc(x)\n";

    fn model(files: &[(&str, &str)]) -> Model {
        let inputs = files.iter().map(|(p, t)| blockviz_core::project::InFile { path: p.to_string(), text: Some(t.to_string()), bytes: t.len() as u64 }).collect();
        Model::new(blockviz_core::analyze("t", inputs), files.iter().map(|(p, t)| (p.to_string(), t.to_string())).collect())
    }

    fn path_to(m: &Model, class: &str, method: &str) -> Vec<u32> {
        let c = m.scene.nodes.iter().find(|n| n.title == class).unwrap().id;
        let f = m.node(c).children.iter().copied().find(|&k| m.node(k).title == method).unwrap();
        m.canonical_path(f)
    }

    #[test]
    fn blocks_are_found_again_by_where_they_are_even_with_namesakes() {
        let old = model(&[("model.py", MODEL_PY)]);
        // A class added above both: ids shift, and there are now two other `forward`s before MLP's.
        let new = model(&[("model.py", &format!("class Head:\n    def forward(self, x):\n        return x\n\n\n{MODEL_PY}"))]);
        for class in ["Block", "MLP"] {
            let mapped = map_path(&old, &new, &path_to(&old, class, "forward"));
            assert_eq!(mapped, path_to(&new, class, "forward"), "{class}.forward");
        }
        // A block that is gone maps as far as its parent.
        let gone = model(&[("model.py", "class Block:\n    def forward(self, x):\n        return x\n")]);
        let mapped = map_path(&old, &gone, &path_to(&old, "MLP", "forward"));
        assert_eq!(gone.node(*mapped.last().unwrap()).title, "model.py");
    }

    #[test]
    fn the_block_in_view_stays_where_it_was_on_screen() {
        let old = model(&[("model.py", MODEL_PY), ("train.py", "from model import Block\nb = Block()\n")]);
        let (w, h) = (1200.0, 800.0);
        let mut cam = Camera::new(&old);
        cam.fit_root(&old, w, h);
        let plan = cam.flight(&old, &path_to(&old, "Block", "forward"), w, h, Framing::Open, 0.08);
        cam.step(&old, &plan, 1.0, w, h);
        let theme = crate::theme::light();
        let hits = {
            let xfs = cam.transforms(&old);
            let mut walk = Walk::new(&old, &theme, w, h, &cam.path, &xfs);
            walk.draw();
            walk.hits
        };
        let target = path_to(&old, "Block", "forward");
        assert_eq!(cam.focus(&old, w, h), target);
        let was = on_screen(&cam, &old, &target).unwrap();

        // Code added above shifts everything in the file; the view follows Block.forward.
        let new = model(&[("model.py", &format!("class Head:\n    def forward(self, x):\n        y = self.a(x)\n        return self.b(y)\n\n\n{MODEL_PY}")), ("train.py", "from model import Block\nb = Block()\n")]);
        carry(&old, &new, &mut cam, &hits, w, h);
        let target = path_to(&new, "Block", "forward");
        let now = on_screen(&cam, &new, &target).expect("still in view");
        assert!((now.x - was.x).abs() < 0.5 && (now.y - was.y).abs() < 0.5 && (now.w - was.w).abs() < 0.5, "{was:?} → {now:?}");
        assert_eq!(cam.focus(&new, w, h), target);
    }

    #[test]
    fn looking_at_a_file_the_block_at_the_center_stays_put_as_code_is_added_above() {
        let body: String = (0..6).map(|i| format!("def f{i}(x):\n    y = g{i}(x)\n    return h{i}(y)\n\n\n")).collect();
        let files = |extra: &str| model(&[("model.py", &format!("{extra}{body}")), ("train.py", "from model import f1\nf1(2)\n")]);
        let old = files("");
        let (w, h) = (1200.0, 800.0);
        let mut cam = Camera::new(&old);
        cam.fit_root(&old, w, h);
        let file = old.scene.nodes.iter().find(|n| n.title == "model.py").unwrap().id;
        let plan = cam.flight(&old, &old.canonical_path(file), w, h, Framing::Fit, 0.04);
        cam.step(&old, &plan, 1.0, w, h);
        let theme = crate::theme::light();
        let xfs = cam.transforms(&old);
        let mut walk = Walk::new(&old, &theme, w, h, &cam.path, &xfs);
        walk.draw();
        let hits = walk.hits;
        let at = paint::hit_at(&hits, w / 2.0, h / 2.0, false);
        assert!(at >= 0);
        let looked_at = paint::path_of(&hits, old.root(), at);
        let was = hits[at as usize].rect;

        let new = files("def first(x):\n    a = p(x)\n    b = q(a)\n    return r(b)\n\n\n");
        carry(&old, &new, &mut cam, &hits, w, h);
        let found = map_path(&old, &new, &looked_at);
        assert_eq!(found.len(), looked_at.len(), "{} is still there", old.node(*looked_at.last().unwrap()).title);
        let now = on_screen(&cam, &new, &found).unwrap();
        assert!((now.x - was.x).abs() < 0.5 && (now.y - was.y).abs() < 0.5 && (now.w - was.w).abs() < 0.5, "{} moved: {was:?} → {now:?}", new.node(*found.last().unwrap()).title);
    }
}
