//! Reads a folder on disk into the files the core analyzes, as the CLI does.

use blockviz_core::project::{ignored, InFile};
use std::path::Path;

/// Files over this size are listed without their text (the core's own limit).
const MAX_TEXT: u64 = 2_000_000;

pub fn read(root: &Path) -> Vec<InFile> {
    let mut out = vec![];
    walk(root, root, &mut out);
    out
}

fn walk(dir: &Path, root: &Path, out: &mut Vec<InFile>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.path());
    for e in entries {
        let p = e.path();
        let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
        if p.is_dir() {
            if !ignored(&format!("{rel}/x")) {
                walk(&p, root, out);
            }
        } else {
            let bytes = e.metadata().map(|m| m.len()).unwrap_or(0);
            let text = if bytes < MAX_TEXT { std::fs::read_to_string(&p).ok() } else { None };
            out.push(InFile { path: rel, text, bytes });
        }
    }
}
