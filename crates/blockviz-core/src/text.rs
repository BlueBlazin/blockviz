//! Approximate text metrics so the layout engine can size boxes without a
//! browser. Widths are tuned against the renderer's sans and mono stacks
//! (SF / Helvetica-like); the renderer ellipsizes if a guess is short.

/// Advance width of one character in em units for a Helvetica-like face.
fn sans_advance(c: char) -> f64 {
    match c {
        'i' | 'j' | 'l' | '\'' | '|' | '.' | ',' | ':' | ';' | '!' => 0.24,
        'I' | ' ' => 0.28,
        'f' | 't' | 'r' | '(' | ')' | '[' | ']' | '{' | '}' | '/' | '\\' => 0.34,
        '-' => 0.36,
        '"' | '*' => 0.40,
        's' | 'c' | 'z' | 'J' => 0.50,
        'm' => 0.83,
        'w' => 0.74,
        'M' => 0.86,
        'W' => 0.93,
        'a'..='z' => 0.55,
        'A'..='Z' => 0.67,
        '0'..='9' => 0.56,
        '_' | '=' | '+' | '<' | '>' | '~' | '^' | '#' | '$' => 0.57,
        '%' | '&' | '@' => 0.80,
        '→' | '×' | '·' | '…' => 0.60,
        c if (c as u32) >= 0x2E80 => 1.0,
        _ => 0.60,
    }
}

pub fn sans_width(s: &str, size: f64, bold: bool) -> f64 {
    let w: f64 = s.chars().map(sans_advance).sum();
    w * size * if bold { 1.06 } else { 1.0 }
}

pub fn mono_width(s: &str, size: f64) -> f64 {
    s.chars().count() as f64 * size * 0.602
}

/// Collapses whitespace so multi-line source reads as one line.
pub fn squash(s: &str) -> String {
    squash_head(s, usize::MAX)
}

/// The first `n` characters of `squash(s)`, read no further: enough to tell
/// whether the text fits in fewer, however long it is.
pub fn squash_head(s: &str, n: usize) -> String {
    let mut out = String::with_capacity(s.len().min(n));
    let mut left = n;
    for word in s.split_whitespace() {
        if !out.is_empty() {
            if left == 0 {
                break;
            }
            out.push(' ');
            left -= 1;
        }
        // A word no longer in bytes than what is left fits in characters.
        let cut = if word.len() > left { word.char_indices().nth(left).map(|(i, _)| i) } else { None };
        match cut {
            Some(i) => {
                out.push_str(&word[..i]);
                break;
            }
            None => {
                out.push_str(word);
                left -= word.chars().count();
            }
        }
    }
    out
}

/// Shortens to at most `max` characters with an ellipsis.
pub fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    // End at a word: "a single pro…" reads as a typo.
    if let Some(i) = out.rfind(' ').filter(|&i| i > out.len() / 2) {
        out.truncate(i);
    }
    out = out.trim_end().trim_end_matches([',', ';', ':']).to_string();
    out.push('…');
    out
}

/// Turns a comment like `# model init` or `# helps estimate the loss, ...` into
/// a heading: first letter capitalised, trimmed to a readable length at a
/// natural boundary. Returns `None` for decorative rulers and code-like comments.
pub fn heading(comment: &str) -> Option<String> {
    let t = comment.trim().trim_start_matches('#').trim();
    // Cell and folding markers introduce a title: `# %% Load data`, `# region Setup`.
    let t = t.strip_prefix("%%").or_else(|| t.strip_prefix("region ")).map(str::trim).unwrap_or(t);
    if is_directive(t) || t.len() < 2 || t.chars().all(|c| !c.is_alphanumeric()) {
        return None;
    }
    if looks_like_code(t) {
        return None;
    }
    let mut t = t.trim_end_matches([':', '.']).to_string();
    // Long comments are cut at the earliest natural boundary that still says something.
    while t.chars().count() > 40 {
        let seps = [" (", ", ", "; ", " - ", " — ", ": ", ". ", " in case ", " because ", " so that ", " so ", " if ", " when ", " which ", " that ", " using ", " with ", " from ", " for ", " over ", " to ", " on "];
        let cut = seps.iter().filter_map(|sep| t.match_indices(sep).map(|(i, _)| i).find(|&i| t[..i].chars().count() >= 14)).min();
        match cut {
            Some(i) => t.truncate(i),
            None => break,
        }
    }
    if t.chars().count() > 44 {
        let mut cut = String::new();
        for word in t.split_whitespace() {
            if cut.chars().count() + word.chars().count() + 1 > 42 {
                break;
            }
            if !cut.is_empty() {
                cut.push(' ');
            }
            cut.push_str(word);
        }
        cut.push('…');
        t = cut;
    }
    let mut chars = t.chars();
    let first = chars.next()?;
    Some(first.to_uppercase().chain(chars).collect())
}

/// Tool directives, notes and links (`# noqa`, `# type: ignore`, `# TODO`,
/// `# https://…`) annotate code; they never title it.
pub fn is_directive(comment: &str) -> bool {
    let t = comment.trim().trim_start_matches('#').trim().to_ascii_lowercase();
    const PREFIXES: &[&str] = &[
        "noqa", "type:", "noinspection", "pylint:", "pyright:", "mypy:", "fmt:", "isort:", "pragma", "nosec", "ruff:", "flake8", "coding:", "coding=", "-*-", "!", "todo", "fixme", "xxx", "hack", "note:", "nb:", "http://", "https://", "see ", "see:", "cf.", "endregion", "in[", "pyformat:", "yapf:", "black:", "spellchecker:", "cspell:", "+build", "go:",
    ];
    PREFIXES.iter().any(|p| t.starts_with(p))
}

/// Commented-out code (`x = 1`, `foo(bar)`, `>>> ...`) is not prose.
fn looks_like_code(t: &str) -> bool {
    if t.starts_with(">>>") || t.starts_with("$ ") || t.starts_with("import ") || t.starts_with("from ") {
        return true;
    }
    // A command line: a program, then its flags (`vivid --color-mode 8-bit …`).
    let words: Vec<&str> = t.split_whitespace().collect();
    let program = words.first().is_some_and(|w| w.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-_./".contains(c)));
    if program && words.iter().skip(1).take(2).any(|w| w.len() > 2 && w.starts_with("--") && w[2..].starts_with(|c: char| c.is_ascii_alphabetic())) {
        return true;
    }
    let ident_end = t
        .char_indices()
        .find(|(_, c)| !(c.is_alphanumeric() || *c == '_' || *c == '.' || *c == '[' || *c == ']' || *c == '\''))
        .map(|(i, _)| i)
        .unwrap_or(t.len());
    if ident_end == 0 {
        return false;
    }
    let rest = t[ident_end..].trim_start();
    (rest.starts_with('=') && !rest.starts_with("==")) || (rest.starts_with('(') && t.trim_end().ends_with(')'))
}

/// First sentence of a docstring or comment block, for one-line summaries.
pub fn summary(doc: &str) -> String {
    // reStructuredText headings (`title\n~~~~~`) are decoration, not prose.
    let lines: Vec<&str> = doc.lines().map(|l| l.trim()).collect();
    let mut kept: Vec<&str> = vec![];
    for (i, l) in lines.iter().enumerate() {
        let rule = |s: &str| s.len() >= 3 && s.chars().all(|c| "~=-^*#_+".contains(c));
        if rule(l) || lines.get(i + 1).is_some_and(|n| rule(n) && !l.contains(' ')) {
            continue;
        }
        kept.push(l);
    }
    let text = squash(&kept.join(" "));
    let mut end = text.len();
    for (i, c) in text.char_indices() {
        if (c == '.' || c == '!' || c == '?') && text[i + 1..].starts_with(' ') && i > 12 {
            // `i.e.`, `e.g.`, `etc.` do not end a sentence.
            let word = text[..i].rsplit(|c: char| c.is_whitespace() || c == '(').next().unwrap_or("");
            if c == '.' && (word.contains('.') || matches!(word.to_ascii_lowercase().as_str(), "e.g" | "i.e" | "etc" | "vs" | "cf" | "approx" | "incl" | "resp" | "viz" | "al")) {
                continue;
            }
            end = i + 1;
            break;
        }
    }
    clip(&text[..end], 160)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn squashes_whitespace() {
        assert_eq!(squash("  f(a,\n\t  b)  "), "f(a, b)");
        assert_eq!(squash_head("  f(a,\n\t  b)  ", 5), "f(a, ");
        assert_eq!(squash_head("f(a, b)", 99), "f(a, b)");
    }
    #[test]
    fn clips_at_words() {
        assert_eq!(clip("returns a reactive and mutable ref object", 30), "returns a reactive and…");
        assert_eq!(clip("get_password_hash, verify_password", 30), "get_password_hash…");
        assert_eq!(clip("short", 30), "short");
        // A single long word is still cut.
        assert_eq!(clip("abcdefghijklmnopqrstuvwxyz", 10), "abcdefghi…");
    }
    #[test]
    fn headings_from_comments() {
        assert_eq!(heading("# model init").as_deref(), Some("Model init"));
        assert_eq!(heading("# -----------------------------------------------------------------------------"), None);
        assert_eq!(
            heading("# init these up here, can override if init_from='resume' (i.e. from a checkpoint)").as_deref(),
            Some("Init these up here")
        );
        assert_eq!(heading("# x = foo(1)"), None);
        assert_eq!(heading("# look for the meta pickle in case it is available in the dataset folder").as_deref(), Some("Look for the meta pickle"));
        assert_eq!(heading("# various inits, derived attributes, I/O setup").as_deref(), Some("Various inits, derived attributes"));
        assert_eq!(heading("# attempt to derive vocab_size from the dataset").as_deref(), Some("Attempt to derive vocab_size"));
    }
    #[test]
    fn widths_are_monotonic() {
        assert!(sans_width("mmm", 13., false) > sans_width("iii", 13., false));
        assert!(mono_width("abc", 12.) > mono_width("ab", 12.));
    }
}
