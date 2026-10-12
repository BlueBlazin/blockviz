//! Highlighting must cover every byte of any file, break only on character
//! boundaries, and never panic, whatever the language.

use std::path::{Path, PathBuf};
use std::time::Instant;

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            files(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// Highlights every text file under `dir`; returns (files, bytes).
fn check_dir(dir: &Path) -> (usize, usize) {
    let mut all = Vec::new();
    files(dir, &mut all);
    let (mut n, mut bytes) = (0, 0);
    for p in all {
        let Ok(text) = std::fs::read_to_string(&p) else { continue };
        if text.len() > 2_000_000 {
            continue;
        }
        let path = p.to_string_lossy().replace('\\', "/");
        check(&path, &text);
        // Cut mid-file too: unterminated strings, comments and fences.
        let mut cut = text.len() * 2 / 3;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        check(&path, &text[..cut]);
        n += 1;
        bytes += text.len();
    }
    (n, bytes)
}

fn check(path: &str, text: &str) {
    let runs = blockviz_core::highlight::highlight(path, text);
    let mut at = 0usize;
    for pair in runs.chunks_exact(2) {
        assert!(pair[0] > 0, "{path}: empty run");
        at += pair[0] as usize;
        assert!(text.is_char_boundary(at), "{path}: run ends inside a character at {at}");
    }
    assert_eq!(at, text.len(), "{path}: runs must cover the file");
}

#[test]
fn fixtures_highlight_cleanly() {
    let (n, _) = check_dir(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures")));
    assert!(n > 20);
    let (n, _) = check_dir(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src")));
    assert!(n > 10);
}

#[test]
fn unclosed_markdown_openers_take_linear_time() {
    // Each `[`, `<` or `[a](` that never closes once scanned to the end of its
    // line: these 300 KB lines took from 25 seconds to over 3 minutes.
    for line in ["[".repeat(300_000), "a<b ".repeat(75_000), "[a](b ".repeat(50_000)] {
        let start = Instant::now();
        check("README.md", &line);
        assert!(start.elapsed().as_secs() < 10, "{:?} for {} KB", start.elapsed(), line.len() / 1000);
    }
}

/// `BLOCKVIZ_CORPUS=/path/to/repos cargo test --release --test highlight -- --ignored`
#[test]
#[ignore]
fn corpus_highlights_cleanly() {
    let dir = std::env::var("BLOCKVIZ_CORPUS").expect("set BLOCKVIZ_CORPUS");
    let t = Instant::now();
    let (n, bytes) = check_dir(Path::new(&dir));
    eprintln!("{n} files, {:.1} MB in {:.2?}", bytes as f64 / 1e6, t.elapsed());
}
