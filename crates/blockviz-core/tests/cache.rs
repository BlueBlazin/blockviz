//! Analyzing again with a cache of what the front ends made of each file
//! gives exactly the scene a fresh analysis gives, and reparses only what
//! changed (`project::build_cached`).

use blockviz_core::layout::Options;
use blockviz_core::project::{build_cached, build_with, Cache, InFile};
use std::path::Path;

fn load(dir: &Path, root: &Path, out: &mut Vec<InFile>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().collect();
    entries.sort_by_key(|e| e.path());
    for e in entries {
        let p = e.path();
        if p.is_dir() {
            load(&p, root, out);
        } else {
            let text = std::fs::read_to_string(&p).ok();
            let bytes = text.as_ref().map_or(0, |t| t.len() as u64);
            out.push(InFile { path: p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"), text, bytes });
        }
    }
}

fn files(dir: &Path) -> Vec<InFile> {
    let mut out = vec![];
    load(dir, dir, &mut out);
    out
}

fn copy(files: &[InFile]) -> Vec<InFile> {
    files.iter().map(|f| InFile { path: f.path.clone(), text: f.text.clone(), bytes: f.bytes }).collect()
}

fn json(files: Vec<InFile>, cache: Option<&mut Cache>) -> String {
    let scene = match cache {
        Some(c) => build_cached("p", files, Options::default(), c),
        None => build_with("p", files, Options::default()),
    };
    serde_json::to_string(&scene).unwrap()
}

/// The projects the corpus tests read, in every language.
fn projects() -> Vec<std::path::PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut out = vec![root.join("nanogpt")];
    for corpus in ["corpus", "corpus-js", "corpus-rs", "corpus-go"] {
        let mut dirs: Vec<_> = std::fs::read_dir(root.join(corpus)).unwrap().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        dirs.sort();
        out.extend(dirs);
    }
    out
}

/// A code file the front ends cache (Python, JavaScript, TypeScript, Rust), to edit.
fn cached_kind(path: &str) -> bool {
    [".py", ".js", ".ts", ".tsx", ".jsx", ".rs"].iter().any(|e| path.ends_with(e)) && !path.contains(".min.")
}

#[test]
fn a_cached_analysis_is_the_fresh_one_and_reparses_only_what_changed() {
    for dir in projects() {
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        let files = files(&dir);
        let fresh = json(copy(&files), None);
        let mut cache = Cache::default();
        assert_eq!(json(copy(&files), Some(&mut cache)), fresh, "{name}: first cached analysis");
        let kept = cache.len();
        assert_eq!(json(copy(&files), Some(&mut cache)), fresh, "{name}: analysis from the cache");
        assert_eq!(cache.reused(), kept, "{name}: every kept file is reused");

        // Edit one file's body: only it is parsed again, and the scene is the fresh one.
        let Some(i) = files.iter().position(|f| cached_kind(&f.path) && f.text.as_ref().is_some_and(|t| t.len() > 200)) else { continue };
        let mut edited = copy(&files);
        let comment = if edited[i].path.ends_with(".py") { "\n# edited\n" } else { "\n// edited\n" };
        let text = edited[i].text.as_mut().unwrap();
        text.push_str(comment);
        edited[i].bytes = text.len() as u64;
        let fresh = json(copy(&edited), None);
        assert_eq!(json(copy(&edited), Some(&mut cache)), fresh, "{name}: after editing {}", edited[i].path);
        assert_eq!(cache.reused(), kept - 1, "{name}: only {} is parsed again", edited[i].path);

        // Add a module next to it that it declares or imports: how imports resolve changes,
        // and what was lowered with the old resolution is not reused.
        let mut added = copy(&edited);
        let at = &edited[i].path;
        let dir = at.rsplit_once('/').map_or("", |(d, _)| d);
        let (path, text, import) = match at.rsplit('.').next().unwrap() {
            "py" => (format!("{dir}/added.py"), "def added():\n    return 1\n".to_string(), "\nfrom .added import added\n"),
            "rs" => (format!("{dir}/added.rs"), "pub fn added() -> u32 {\n    1\n}\n".to_string(), "\nmod added;\n"),
            _ => (format!("{dir}/added.ts"), "export function added() {\n  return 1;\n}\n".to_string(), "\nimport { added } from './added';\n"),
        };
        let path = path.trim_start_matches('/').to_string();
        added.push(InFile { bytes: text.len() as u64, path, text: Some(text) });
        added[i].text.as_mut().unwrap().push_str(import);
        added.sort_by(|a, b| a.path.cmp(&b.path));
        let mut warm = Cache::default();
        json(copy(&edited), Some(&mut warm));
        assert_eq!(json(copy(&added), Some(&mut warm)), json(copy(&added), None), "{name}: after adding a module beside {at}");

        // Remove it: the scene is the fresh one, and the cache forgets it.
        let mut removed = copy(&edited);
        removed.remove(i);
        assert_eq!(json(copy(&removed), Some(&mut cache)), json(copy(&removed), None), "{name}: after removing {}", edited[i].path);
    }
}
