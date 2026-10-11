//! blockviz-core: turns a codebase into a tree of nested, laid-out diagrams.
//!
//! `analyze_project` is the single boundary crossing from JavaScript: files in,
//! a fully laid-out scene out (JSON). Everything runs in a worker, so the UI
//! thread only ever renders.

pub mod ir;
pub mod layout;
pub mod notebook;
pub mod outline;
pub mod project;
pub mod python;
pub mod text;

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
    files: Vec<WireFile>,
}

pub fn analyze(name: &str, files: Vec<project::InFile>) -> ir::Scene {
    project::build(name, files)
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
            let scene = analyze(&p.name, files);
            serde_json::json!({ "ok": true, "scene": scene }).to_string()
        }
        Err(e) => serde_json::json!({ "ok": false, "error": format!("Invalid input: {e}") }).to_string(),
    }
}
