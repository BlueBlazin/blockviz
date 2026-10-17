//! What the app remembers between runs (the web keeps these in localStorage):
//! the color scheme chosen, the Tidy arrows switch, the details panel's
//! width, and the folders opened lately.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Settings {
    /// "light" or "dark" when chosen; following the system otherwise.
    pub scheme: Option<String>,
    pub trunks: bool,
    pub panel_w: Option<f64>,
    /// Folders opened, most recent first.
    pub recent: Vec<String>,
}

/// How many folders the Open dialog lists.
const RECENT: usize = 8;

fn path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Application Support/blockviz/settings.json"))
}

impl Settings {
    pub fn load() -> Self {
        path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    /// Saved as soon as something changes; failing to save only forgets it.
    pub fn save(&self) {
        let Some(p) = path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(t) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(p, t);
        }
    }

    pub fn opened(&mut self, folder: &std::path::Path) {
        let f = folder.to_string_lossy().to_string();
        self.recent.retain(|r| *r != f);
        self.recent.insert(0, f);
        self.recent.truncate(RECENT);
        self.save();
    }
}
