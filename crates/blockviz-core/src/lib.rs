//! blockviz-core: turns a codebase into a tree of nested, laid-out diagrams.
//!
//! `analyze_project` is the single boundary crossing from JavaScript: files in,
//! a fully laid-out scene out (JSON). Everything runs in a worker, so the UI
//! thread only ever renders.

pub mod code;
pub mod compose;
pub mod highlight;
pub mod invocations;
pub mod ir;
pub mod layout;
pub mod naming;
pub mod notebook;
pub mod outline;
pub mod project;
pub mod python;
pub mod text;
pub mod vocabulary;

use serde::Deserialize;
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
}

pub fn analyze(name: &str, files: Vec<project::InFile>) -> ir::Scene {
    project::build(name, files)
}

/// Syntax classes for one file's text, as `[len, class, len, class, ...]`
/// runs over its UTF-8 bytes (see `highlight::Hl` for the classes).
#[wasm_bindgen]
pub fn highlight_source(path: &str, text: &str) -> Vec<u32> {
    highlight::highlight(path, text)
}

/// `{"name": "...", "files": [{"path", "text", "bytes"}]}` → `{"ok": true, "scene": ...}`.
#[wasm_bindgen]
pub fn analyze_project(input: &str) -> String {
    let parsed: Result<WireProject, _> = serde_json::from_str(input);
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
            let mut scene = analyze(&p.name, files);
            if !p.description.trim().is_empty() {
                scene.description = p.description.trim().to_string();
            }
            serde_json::json!({ "ok": true, "scene": scene }).to_string()
        }
        Err(e) => serde_json::json!({ "ok": false, "error": format!("Invalid input: {e}") }).to_string(),
    }
}
