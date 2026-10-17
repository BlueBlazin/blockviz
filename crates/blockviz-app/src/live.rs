//! Keeps a folder's analysis current. A thread watches the folder; when files
//! change it reads just those, waits for a burst of changes (a save is often
//! several events, a checkout many) to settle, and analyzes the project again,
//! reusing what the core parsed of every file that did not change.

use crate::model::Model;
use blockviz_core::project::{build_cached, ignored, Cache, InFile};
use notify::{EventKind, RecursiveMode, Watcher};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::time::{Duration, Instant};

/// How long the folder must be quiet before analyzing again.
const SETTLE: Duration = Duration::from_millis(120);
/// Files over this size are listed without their text (the core's own limit).
pub const MAX_TEXT: u64 = 2_000_000;

/// A file as the project holds it: its text (none when unreadable or too large) and size.
#[derive(Clone)]
pub struct File {
    pub text: Option<String>,
    pub bytes: u64,
}

/// The project's files, by path relative to its folder.
pub type Files = BTreeMap<String, File>;

/// A new analysis of the project, made after files changed.
pub struct Update {
    pub model: Model,
    /// The files that changed since the last one.
    pub changed: Vec<String>,
    pub took: Duration,
    /// Files whose parse was reused.
    pub reused: usize,
}

pub fn read(root: &Path) -> Files {
    let mut out = Files::new();
    walk(root, root, &mut out);
    out
}

fn walk(dir: &Path, root: &Path, out: &mut Files) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        let rel = relative(root, &p);
        if p.is_dir() {
            if !ignored(&format!("{rel}/x")) {
                walk(&p, root, out);
            }
        } else if !ignored(&rel) {
            out.insert(rel, read_file(&p));
        }
    }
}

fn read_file(p: &Path) -> File {
    let bytes = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    let text = if bytes < MAX_TEXT { std::fs::read_to_string(p).ok() } else { None };
    File { text, bytes }
}

fn relative(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/")
}

/// The files as the core takes them.
pub fn inputs(files: &Files) -> Vec<InFile> {
    files.iter().map(|(path, f)| InFile { path: path.clone(), text: f.text.clone(), bytes: f.bytes }).collect()
}

pub fn texts(files: &Files) -> HashMap<String, String> {
    files.iter().filter_map(|(p, f)| Some((p.clone(), f.text.clone()?))).collect()
}

/// Brings `files` up to date with what is on disk at `paths` (files or
/// folders, added, changed or removed). Returns the files whose text changed.
pub fn apply(root: &Path, files: &mut Files, paths: impl IntoIterator<Item = PathBuf>) -> Vec<String> {
    let mut changed = vec![];
    for p in paths {
        let rel = relative(root, &p);
        if rel.is_empty() || ignored(&format!("{rel}/x")) {
            continue;
        }
        let prefix = format!("{rel}/");
        if p.is_dir() {
            // A folder added, moved in or out, or changed wholesale: read it again.
            let mut now = Files::new();
            walk(&p, root, &mut now);
            let gone: Vec<String> = files.range(prefix.clone()..).take_while(|(k, _)| k.starts_with(&prefix)).map(|(k, _)| k.clone()).filter(|k| !now.contains_key(k)).collect();
            for k in gone {
                files.remove(&k);
                changed.push(k);
            }
            for (k, f) in now {
                if files.get(&k).map(|old| &old.text) != Some(&f.text) {
                    files.insert(k.clone(), f);
                    changed.push(k);
                }
            }
        } else if p.is_file() {
            if ignored(&rel) {
                continue;
            }
            let f = read_file(&p);
            if files.get(&rel).map(|old| &old.text) != Some(&f.text) {
                files.insert(rel.clone(), f);
                changed.push(rel);
            }
        } else {
            // Gone: a file, or a folder and everything in it.
            if files.remove(&rel).is_some() {
                changed.push(rel.clone());
            }
            let under: Vec<String> = files.range(prefix.clone()..).take_while(|(k, _)| k.starts_with(&prefix)).map(|(k, _)| k.clone()).collect();
            for k in under {
                files.remove(&k);
                changed.push(k);
            }
        }
    }
    changed.sort();
    changed.dedup();
    changed
}

/// Watches `root` on a thread of its own and sends a new analysis whenever
/// its files change. Stops when `send` reports the receiver is gone.
pub fn watch(root: PathBuf, name: String, mut files: Files, mut cache: Cache, send: impl Fn(Update) -> bool + Send + 'static) {
    std::thread::spawn(move || {
        let (tx, rx) = channel();
        let mut watcher = match notify::recommended_watcher(move |e| {
            let _ = tx.send(e);
        }) {
            Ok(w) => w,
            Err(e) => return eprintln!("Not watching {}: {e}", root.display()),
        };
        if let Err(e) = watcher.watch(&root, RecursiveMode::Recursive) {
            return eprintln!("Not watching {}: {e}", root.display());
        }
        let paths_of = |e: notify::Result<notify::Event>| match e {
            // Reading a file is not changing it.
            Ok(e) if !matches!(e.kind, EventKind::Access(_)) => e.paths,
            _ => vec![],
        };
        while let Ok(first) = rx.recv() {
            let mut paths = paths_of(first);
            loop {
                match rx.recv_timeout(SETTLE) {
                    Ok(e) => paths.extend(paths_of(e)),
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
            paths.sort();
            paths.dedup();
            let changed = apply(&root, &mut files, paths);
            if changed.is_empty() {
                continue;
            }
            let start = Instant::now();
            let scene = build_cached(&name, inputs(&files), blockviz_core::layout::Options::default(), &mut cache);
            let model = Model::new(scene, texts(&files));
            let update = Update { model, changed, took: start.elapsed(), reused: cache.reused() };
            if !send(update) {
                return;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applying_changes_reads_only_what_changed_on_disk() {
        let dir = std::env::temp_dir().join(format!("blockviz-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("pkg")).unwrap();
        std::fs::write(dir.join("a.py"), "x = 1\n").unwrap();
        std::fs::write(dir.join("pkg/b.py"), "y = 2\n").unwrap();
        std::fs::create_dir_all(dir.join("node_modules/dep")).unwrap();
        std::fs::write(dir.join("node_modules/dep/index.js"), "").unwrap();
        let mut files = read(&dir);
        assert_eq!(files.keys().collect::<Vec<_>>(), ["a.py", "pkg/b.py"]);

        // A file saved with the same text is no change; a new text is.
        std::fs::write(dir.join("a.py"), "x = 1\n").unwrap();
        assert!(apply(&dir, &mut files, [dir.join("a.py")]).is_empty());
        std::fs::write(dir.join("a.py"), "x = 3\n").unwrap();
        assert_eq!(apply(&dir, &mut files, [dir.join("a.py")]), ["a.py"]);
        assert_eq!(files["a.py"].text.as_deref(), Some("x = 3\n"));

        // Ignored folders stay out; a folder removed takes its files with it.
        std::fs::write(dir.join("node_modules/dep/index.js"), "changed").unwrap();
        assert!(apply(&dir, &mut files, [dir.join("node_modules/dep/index.js")]).is_empty());
        std::fs::remove_dir_all(dir.join("pkg")).unwrap();
        assert_eq!(apply(&dir, &mut files, [dir.join("pkg")]), ["pkg/b.py"]);
        assert!(!files.contains_key("pkg/b.py"));

        // A folder moved in is read whole.
        std::fs::create_dir_all(dir.join("new")).unwrap();
        std::fs::write(dir.join("new/c.py"), "z = 1\n").unwrap();
        assert_eq!(apply(&dir, &mut files, [dir.join("new")]), ["new/c.py"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
