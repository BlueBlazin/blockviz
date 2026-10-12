//! blockviz-core: turns a codebase into a tree of nested, laid-out diagrams.
//!
//! `analyze_project` is the single boundary crossing from JavaScript: files in,
//! a fully laid-out scene out (JSON). Everything runs in a worker, so the UI
//! thread only ever renders.

pub mod code;
pub mod compose;
pub mod go;
pub mod hash;
#[cfg(target_arch = "wasm32")]
mod headroom;
pub mod highlight;
pub mod invocations;
pub mod js;
pub mod ir;
pub mod layout;
pub mod naming;
pub mod notebook;
pub mod outline;
pub mod project;
pub mod python;
pub mod rust;
pub mod text;
pub mod vocabulary;

use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

#[derive(Deserialize)]
struct WireFile {
    path: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    bytes: u64,
}

#[derive(Deserialize)]
struct WireProject {
    #[serde(default)]
    name: String,
    /// A maintainer-written one-liner (GitHub's repository description).
    #[serde(default)]
    description: String,
    files: Vec<WireFile>,
    /// Layout style: one trunk per block (see `layout::Options`).
    #[serde(default)]
    shared_trunks: bool,
}

/// `analyze_project`'s answer, serialized straight from the scene: going
/// through a `serde_json::Value` copies the whole scene first (4× the time,
/// and on a large repository nearly twice the memory).
#[derive(Serialize)]
struct Analyzed<'a> {
    ok: bool,
    scene: &'a ir::Scene,
}

/// `analyze_project`'s reply, in the pieces it is written in. The page
/// copies them out one at a time: joined here, they would need one block of
/// memory as large as the reply, which a fragmented heap rarely has free, so
/// WebAssembly memory would grow by the reply's size just to hold it.
#[wasm_bindgen]
#[derive(Default)]
pub struct Reply {
    full: std::collections::VecDeque<Vec<u8>>,
    last: Vec<u8>,
}

#[wasm_bindgen]
impl Reply {
    /// Its length in bytes.
    #[wasm_bindgen(getter)]
    pub fn len(&self) -> usize {
        self.full.iter().map(Vec::len).sum::<usize>() + self.last.len()
    }

    /// The next piece, freed here as it is handed over.
    pub fn next(&mut self) -> Option<Vec<u8>> {
        self.full.pop_front().or_else(|| Some(std::mem::take(&mut self.last)).filter(|p| !p.is_empty()))
    }
}

impl std::io::Write for Reply {
    // serde_json writes every token with this: inlined, as `Vec`'s is, it costs no more.
    #[inline]
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        if self.last.capacity() - self.last.len() < buf.len() {
            let full = std::mem::replace(&mut self.last, Vec::with_capacity(buf.len().max(1 << 16)));
            self.full.push_back(full);
        }
        self.last.extend_from_slice(buf);
        Ok(())
    }
    #[inline]
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.write_all(buf).map(|_| buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub fn analyze(name: &str, files: Vec<project::InFile>) -> ir::Scene {
    project::build(name, files)
}

/// `analyze` with a choice of layout style.
pub fn analyze_with(name: &str, files: Vec<project::InFile>, opts: layout::Options) -> ir::Scene {
    project::build_with(name, files, opts)
}

/// Syntax classes for one file's text, as `[len, class, len, class, ...]`
/// runs over its UTF-8 bytes (see `highlight::Hl` for the classes).
#[wasm_bindgen]
pub fn highlight_source(path: &str, text: &str) -> Vec<u32> {
    highlight::highlight(path, text)
}

/// `{"name": "...", "files": [{"path", "text", "bytes"}]}` → `{"ok": true, "scene": ...}`,
/// both UTF-8 JSON: as bytes, the reply is handed on (and kept) without decoding it.
/// The request is taken, not borrowed, to be freed once read: it is as large
/// as every file's text.
#[wasm_bindgen]
pub fn analyze_project(input: Vec<u8>) -> Reply {
    let parsed: Result<WireProject, _> = serde_json::from_slice(&input);
    drop(input);
    let mut reply = Reply::default();
    match parsed {
        Ok(p) => {
            let files = p
                .files
                .into_iter()
                .map(|f| {
                    let bytes = if f.bytes > 0 { f.bytes } else { f.text.as_ref().map(|t| t.len() as u64).unwrap_or(0) };
                    project::InFile { path: f.path, text: f.text, bytes }
                })
                .collect();
            let mut scene = analyze_with(&p.name, files, layout::Options { shared_trunks: p.shared_trunks });
            if !p.description.trim().is_empty() {
                scene.description = p.description.trim().to_string();
            }
            serde_json::to_writer(&mut reply, &Analyzed { ok: true, scene: &scene }).unwrap();
        }
        Err(e) => serde_json::to_writer(&mut reply, &serde_json::json!({ "ok": false, "error": format!("Invalid input: {e}") })).unwrap(),
    }
    reply
}
