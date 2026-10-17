//! The reader's trail through a project (src/ui/navigation.ts): every jump
//! (search, go to, zooming into a block, breadcrumbs) is a step Back undoes,
//! and Back returns to the exact view and selection.

use crate::anchor::map_path;
use crate::camera::Xf;
use crate::model::Model;

#[derive(Clone, Debug)]
pub struct Entry {
    /// The camera: its anchor path and the anchor's interior transform.
    pub path: Vec<u32>,
    pub xf: Xf,
    /// What was selected, and the instance path it was selected through.
    pub sel: Option<(u32, Vec<u32>)>,
}

#[derive(Default)]
pub struct History {
    back: Vec<Entry>,
    forward: Vec<Entry>,
}

/// The most steps kept back.
const KEPT: usize = 200;

impl History {
    /// A jump is about to leave `here`: it becomes a step back.
    pub fn jump(&mut self, here: Entry) {
        self.back.push(here);
        if self.back.len() > KEPT {
            self.back.remove(0);
        }
        self.forward.clear();
    }

    pub fn back(&mut self, here: Entry) -> Option<Entry> {
        let to = self.back.pop()?;
        self.forward.push(here);
        Some(to)
    }

    pub fn forward(&mut self, here: Entry) -> Option<Entry> {
        let to = self.forward.pop()?;
        self.back.push(here);
        Some(to)
    }

    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    pub fn clear(&mut self) {
        self.back.clear();
        self.forward.clear();
    }

    /// Carries the trail into a new analysis of the same project: each step to
    /// the same blocks, as far as they are still there.
    pub fn carry(&mut self, old: &Model, new: &Model) {
        for e in self.back.iter_mut().chain(self.forward.iter_mut()) {
            let path = map_path(old, new, &e.path);
            if path.len() < e.path.len() {
                // The anchor is gone: the step shows its nearest container whole.
                e.xf = Xf { x: 0.0, y: 0.0, s: 0.0 };
            }
            e.path = path;
            e.sel = e.sel.take().and_then(|(_, p)| {
                let q = map_path(old, new, &p);
                (q.len() == p.len()).then(|| (*q.last().unwrap(), q))
            });
        }
    }
}
