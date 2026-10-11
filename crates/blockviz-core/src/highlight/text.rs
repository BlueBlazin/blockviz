//! Config, data and document formats: mostly line-oriented, where what
//! matters is telling keys, values, sections and comments apart.

use super::code::{find, line_end, number_end, string_end};
use super::{code, embed, Hl, Runs};

/// `(start, end)` of every line, without its newline.
fn lines(text: &str) -> impl Iterator<Item = (usize, usize)> + '_ {
    let mut at = 0;
    text.split('\n').map(move |l| {
        let s = at;
        at += l.len() + 1;
        (s, s + l.len())
    })
}

fn indent(b: &[u8], mut i: usize, end: usize) -> usize {
    while i < end && (b[i] == b' ' || b[i] == b'\t') {
        i += 1;
    }
    i
}

fn trim_end(b: &[u8], start: usize, mut end: usize) -> usize {
    while end > start && b[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    end
}

/// A `#` that starts a comment: at the line start or after whitespace.
fn hash_comment(b: &[u8], i: usize, line_start: usize) -> bool {
    b[i] == b'#' && (i == line_start || b[i - 1] == b' ' || b[i - 1] == b'\t')
}

fn scalar(word: &str) -> Hl {
    let w = word.trim();
    if matches!(w.to_ascii_lowercase().as_str(), "true" | "false" | "yes" | "no" | "on" | "off" | "null" | "none" | "~") {
        Hl::Constant
    } else if !w.is_empty() && (w.starts_with(|c: char| c.is_ascii_digit()) || (w.len() > 1 && w.starts_with(['-', '+', '.']) && w[1..].starts_with(|c: char| c.is_ascii_digit()))) && w.parse::<f64>().is_ok() {
        Hl::Number
    } else {
        Hl::String
    }
}

pub(super) fn json(text: &str, start: usize, end: usize, runs: &mut Runs) {
    let b = text.as_bytes();
    let mut i = start;
    while i < end {
        let c = b[i];
        if c == b'"' {
            let j = string_end(b, i, end, b'"', true, false);
            let k = b[j..end].iter().position(|c| !c.is_ascii_whitespace()).map_or(end, |p| j + p);
            runs.mark(i, j, if k < end && b[k] == b':' { Hl::Key } else { Hl::String });
            i = j;
        } else if c == b'/' && b.get(i + 1) == Some(&b'/') {
            let j = line_end(b, i, end);
            runs.mark(i, j, Hl::Comment);
            i = j;
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            let j = find(b, i + 2, end, b"*/").map_or(end, |k| k + 2);
            runs.mark(i, j, Hl::Comment);
            i = j;
        } else if c.is_ascii_digit() || (c == b'-' && b.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            let j = number_end(b, i + 1, end);
            runs.mark(i, j, Hl::Number);
            i = j;
        } else if c.is_ascii_alphabetic() {
            let mut j = i;
            while j < end && b[j].is_ascii_alphabetic() {
                j += 1;
            }
            if matches!(&text[i..j], "true" | "false" | "null") {
                runs.mark(i, j, Hl::Constant);
            }
            i = j;
        } else {
            i += 1;
        }
    }
}

pub(super) fn yaml(text: &str, runs: &mut Runs) {
    let b = text.as_bytes();
    // While inside a `|` or `>` block scalar: the indent of the key that opened it.
    let mut block: Option<usize> = None;
    for (ls, le) in lines(text) {
        let p0 = indent(b, ls, le);
        let blank = trim_end(b, p0, le) == p0;
        if let Some(key_col) = block {
            if blank || p0 - ls > key_col {
                runs.mark(p0, le, Hl::String);
                continue;
            }
            block = None;
        }
        if blank {
            continue;
        }
        if b[p0] == b'#' {
            runs.mark(p0, le, Hl::Comment);
            continue;
        }
        if p0 == ls && (b[ls..le].starts_with(b"---") || b[ls..le].starts_with(b"...")) {
            runs.mark(ls, ls + 3, Hl::Meta);
            yaml_value(text, ls + 3, le, runs);
            continue;
        }
        let mut p = p0;
        while p < le && b[p] == b'-' && (p + 1 == le || b[p + 1] == b' ') {
            runs.mark(p, p + 1, Hl::Keyword);
            p = indent(b, p + 1, le);
        }
        let key_col = p - ls;
        if let Some((ke, colon)) = yaml_key(b, p, le) {
            runs.mark(p, ke, Hl::Key);
            p = colon + 1;
        }
        let v = indent(b, p, le);
        let ve = trim_end(b, v, le);
        let indicator = &text[v..ve];
        let indicator = indicator.split(" #").next().unwrap_or("").trim();
        if !indicator.is_empty() && (indicator.starts_with('|') || indicator.starts_with('>')) && indicator[1..].chars().all(|c| c.is_ascii_digit() || c == '+' || c == '-') {
            runs.mark(v, v + indicator.len(), Hl::Keyword);
            block = Some(key_col);
            yaml_value(text, v + indicator.len(), le, runs);
            continue;
        }
        yaml_value(text, p, le, runs);
    }
}

/// `key:` at `p` → (end of the key text, index of the colon).
fn yaml_key(b: &[u8], p: usize, le: usize) -> Option<(usize, usize)> {
    if p >= le || matches!(b[p], b'[' | b'{' | b'#' | b'&' | b'*' | b'!' | b'|' | b'>') {
        return None;
    }
    let mut j = p;
    if b[p] == b'"' || b[p] == b'\'' {
        j = string_end(b, p, le, b[p], b[p] == b'"', false);
    }
    while j < le {
        if b[j] == b':' && (j + 1 == le || b[j + 1] == b' ' || b[j + 1] == b'\t') {
            return Some((trim_end(b, p, j), j));
        }
        if b[j] == b'#' && j > p && b[j - 1] == b' ' {
            return None;
        }
        j += 1;
    }
    None
}

fn yaml_value(text: &str, start: usize, le: usize, runs: &mut Runs) {
    let b = text.as_bytes();
    let mut i = start;
    let mut flow = 0usize;
    while i < le {
        let c = b[i];
        if c == b' ' || c == b'\t' {
            i += 1;
        } else if c == b'#' && (i == start || b[i - 1] == b' ' || b[i - 1] == b'\t') {
            runs.mark(i, le, Hl::Comment);
            return;
        } else if c == b'"' || c == b'\'' {
            let j = string_end(b, i, le, c, c == b'"', false);
            runs.mark(i, j, Hl::String);
            i = j;
        } else if matches!(c, b'[' | b'{') {
            flow += 1;
            i += 1;
        } else if matches!(c, b']' | b'}') {
            flow = flow.saturating_sub(1);
            i += 1;
        } else if c == b',' || (c == b':' && flow > 0) {
            i += 1;
        } else if matches!(c, b'&' | b'*' | b'!') {
            let mut j = i + 1;
            while j < le && !b[j].is_ascii_whitespace() && !matches!(b[j], b',' | b']' | b'}') {
                j += 1;
            }
            runs.mark(i, j, if c == b'!' { Hl::Meta } else { Hl::Type });
            i = j;
        } else {
            // A plain scalar: to the end of the line, a comment, or (in flow) a delimiter.
            let mut j = i;
            while j < le {
                if b[j] == b'#' && b[j - 1] == b' ' {
                    break;
                }
                if flow > 0 && matches!(b[j], b',' | b']' | b'}') {
                    break;
                }
                if flow > 0 && b[j] == b':' && b.get(j + 1).is_none_or(|c| *c == b' ') {
                    break;
                }
                j += 1;
            }
            let e = trim_end(b, i, j);
            if flow > 0 && j < le && b[j] == b':' {
                runs.mark(i, e, Hl::Key);
            } else {
                runs.mark(i, e, scalar(&text[i..e]));
            }
            i = j.max(i + 1);
        }
    }
}

pub(super) fn toml(text: &str, runs: &mut Runs) {
    let b = text.as_bytes();
    // Inside a multi-line string: its delimiter.
    let mut multi: Option<&'static [u8]> = None;
    for (ls, le) in lines(text) {
        let mut p = ls;
        if let Some(q) = multi {
            match find(b, ls, le, q) {
                Some(k) => {
                    runs.mark(ls, k + 3, Hl::String);
                    p = k + 3;
                }
                None => {
                    runs.mark(ls, le, Hl::String);
                    continue;
                }
            }
        } else {
            p = indent(b, p, le);
            if p < le && b[p] == b'#' {
                runs.mark(p, le, Hl::Comment);
                continue;
            }
            if p < le && b[p] == b'[' {
                let close = find(b, p, le, b"]").map_or(le, |k| if b.get(k + 1) == Some(&b']') { k + 2 } else { k + 1 });
                runs.mark(p, close, Hl::Type);
                p = close;
            } else if let Some(eq) = toml_key(b, p, le) {
                runs.mark(p, trim_end(b, p, eq), Hl::Key);
                p = eq + 1;
            }
        }
        multi = toml_value(text, p, le, runs);
    }
}

/// Index of the `=` after a key at `p`.
fn toml_key(b: &[u8], p: usize, le: usize) -> Option<usize> {
    let mut j = p;
    while j < le {
        match b[j] {
            b'=' => return (j > p).then_some(j),
            b'"' | b'\'' => j = string_end(b, j, le, b[j], b[j] == b'"', false),
            c if c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.' | b' ' | b'\t') || c >= 0x80 => j += 1,
            _ => return None,
        }
    }
    None
}

/// Lexes a value; returns the delimiter of a multi-line string left open.
fn toml_value(text: &str, start: usize, le: usize, runs: &mut Runs) -> Option<&'static [u8]> {
    let b = text.as_bytes();
    let mut i = start;
    while i < le {
        let c = b[i];
        if c == b'#' {
            runs.mark(i, le, Hl::Comment);
            return None;
        }
        if c == b'"' || c == b'\'' {
            let triple: &'static [u8] = if c == b'"' { b"\"\"\"" } else { b"'''" };
            if b[i..le].starts_with(triple) {
                match find(b, i + 3, le, triple) {
                    Some(k) => {
                        runs.mark(i, k + 3, Hl::String);
                        i = k + 3;
                        continue;
                    }
                    None => {
                        runs.mark(i, le, Hl::String);
                        return Some(triple);
                    }
                }
            }
            let j = string_end(b, i, le, c, c == b'"', false);
            runs.mark(i, j, Hl::String);
            i = j;
        } else if c.is_ascii_digit() || ((c == b'-' || c == b'+') && b.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            let mut j = i + 1;
            while j < le && (b[j].is_ascii_alphanumeric() || matches!(b[j], b'.' | b'_' | b':' | b'-' | b'+')) {
                j += 1;
            }
            runs.mark(i, j, Hl::Number);
            i = j;
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let mut j = i;
            while j < le && (b[j].is_ascii_alphanumeric() || matches!(b[j], b'_' | b'-')) {
                j += 1;
            }
            let k = indent(b, j, le);
            if matches!(&text[i..j], "true" | "false" | "inf" | "nan") {
                runs.mark(i, j, Hl::Constant);
            } else if k < le && b[k] == b'=' {
                runs.mark(i, j, Hl::Key);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    None
}

/// INI, `.cfg`, `.conf`, `.env`, `.properties`.
pub(super) fn ini(text: &str, runs: &mut Runs) {
    let b = text.as_bytes();
    for (ls, le) in lines(text) {
        let mut p = indent(b, ls, le);
        if p >= le {
            continue;
        }
        if b[p] == b'#' || b[p] == b';' {
            runs.mark(p, le, Hl::Comment);
            continue;
        }
        if b[p] == b'[' {
            let close = find(b, p, le, b"]").map_or(le, |k| k + 1);
            runs.mark(p, close, Hl::Type);
            continue;
        }
        if b[p..le].starts_with(b"export ") {
            runs.mark(p, p + 6, Hl::Keyword);
            p = indent(b, p + 6, le);
        }
        let Some(sep) = b[p..le].iter().position(|&c| c == b'=' || c == b':').map(|k| p + k) else { continue };
        runs.mark(p, trim_end(b, p, sep), Hl::Key);
        let v = indent(b, sep + 1, le);
        let mut ve = le;
        if let Some(k) = (v..le).find(|&k| (b[k] == b'#' || b[k] == b';') && k > v && b[k - 1] == b' ') {
            ve = k;
        }
        let ve_trim = trim_end(b, v, ve);
        if v < ve_trim {
            let first = b[v];
            if first == b'"' || first == b'\'' {
                runs.mark(v, ve_trim, Hl::String);
            } else {
                let class = scalar(&text[v..ve_trim]);
                if class != Hl::String {
                    runs.mark(v, ve_trim, class);
                }
            }
        }
        if ve < le {
            runs.mark(ve, le, Hl::Comment);
        }
    }
}

/// `requirements.txt`: package names, version pins, options, comments.
pub(super) fn requirements(text: &str, runs: &mut Runs) {
    let b = text.as_bytes();
    for (ls, le) in lines(text) {
        let p = indent(b, ls, le);
        if p >= le {
            continue;
        }
        let comment = (p..le).find(|&k| hash_comment(b, k, p));
        let end = comment.unwrap_or(le);
        if b[p] == b'-' {
            let w = (p..end).find(|&k| b[k] == b' ' || b[k] == b'=').unwrap_or(end);
            runs.mark(p, w, Hl::Keyword);
        } else if b[p] != b'#' {
            let mut j = p;
            while j < end && (b[j].is_ascii_alphanumeric() || matches!(b[j], b'_' | b'-' | b'.')) {
                j += 1;
            }
            runs.mark(p, j, Hl::Key);
            let marker = (j..end).find(|&k| b[k] == b';').unwrap_or(end);
            let mut k = j;
            while k < marker {
                if b[k].is_ascii_digit() {
                    let e = (k..marker).find(|&e| !(b[e].is_ascii_alphanumeric() || matches!(b[e], b'.' | b'*' | b'+'))).unwrap_or(marker);
                    runs.mark(k, e, Hl::Number);
                    k = e;
                } else {
                    k += 1;
                }
            }
            if marker < end {
                runs.mark(marker, trim_end(b, marker, end), Hl::Meta);
            }
        }
        if let Some(c) = comment {
            runs.mark(c, le, Hl::Comment);
        }
    }
}

pub(super) fn markdown(text: &str, runs: &mut Runs) {
    let b = text.as_bytes();
    // An open fence: its marker character, length, info string and content start.
    let mut fence: Option<(u8, usize, String, usize)> = None;
    let mut comment = false;
    for (ls, le) in lines(text) {
        let p = indent(b, ls, le);
        let ind = p - ls;
        if let Some((ch, n, lang, from)) = &fence {
            let run = b[p..le].iter().take_while(|&&c| c == *ch).count();
            if ind < 4 && run >= *n && trim_end(b, p + run, le) == p + run {
                if !embed(lang, text, *from, ls, runs) {
                    runs.mark(*from, ls, Hl::Code);
                }
                runs.mark(p, le, Hl::Meta);
                fence = None;
            }
            continue;
        }
        if comment {
            match find(b, ls, le, b"-->") {
                Some(k) => {
                    runs.mark(ls, k + 3, Hl::Comment);
                    comment = false;
                    inline(text, k + 3, le, runs, &mut comment);
                }
                None => runs.mark(ls, le, Hl::Comment),
            }
            continue;
        }
        if p >= le {
            continue;
        }
        let c = b[p];
        if ind < 4 && (c == b'`' || c == b'~') {
            let run = b[p..le].iter().take_while(|&&x| x == c).count();
            if run >= 3 {
                let info = text[p + run..le].trim();
                let lang = info.split(|ch: char| ch.is_whitespace() || ch == '{' || ch == ',').next().unwrap_or("").to_string();
                runs.mark(p, le, Hl::Meta);
                fence = Some((c, run, lang, (le + 1).min(text.len())));
                continue;
            }
        }
        if ind < 4 && c == b'#' {
            let hashes = b[p..le].iter().take_while(|&&x| x == b'#').count();
            if hashes <= 6 && (p + hashes == le || b[p + hashes] == b' ') {
                runs.mark(p, le, Hl::Heading);
                continue;
            }
        }
        if ind < 4 && c == b'>' {
            runs.mark(p, le, Hl::Comment);
            continue;
        }
        let t = &text[p..trim_end(b, p, le)];
        if t.len() >= 3 && (t.bytes().all(|x| x == b'-' || x == b' ') || t.bytes().all(|x| x == b'=') || t.bytes().all(|x| x == b'*' || x == b' ') || t.bytes().all(|x| x == b'_')) {
            runs.mark(p, le, Hl::Meta);
            continue;
        }
        let mut q = p;
        if matches!(c, b'-' | b'*' | b'+') && b.get(p + 1) == Some(&b' ') {
            runs.mark(p, p + 1, Hl::Keyword);
            q = p + 2;
        } else if c.is_ascii_digit() {
            let d = b[p..le].iter().take_while(|x| x.is_ascii_digit()).count();
            if matches!(b.get(p + d), Some(b'.' | b')')) && b.get(p + d + 1) == Some(&b' ') {
                runs.mark(p, p + d + 1, Hl::Keyword);
                q = p + d + 2;
            }
        }
        inline(text, q, le, runs, &mut comment);
    }
    if let Some((_, _, lang, from)) = fence {
        if from < text.len() && !embed(&lang, text, from, text.len(), runs) {
            runs.mark(from, text.len(), Hl::Code);
        }
    }
}

/// Inline Markdown: code spans, strong emphasis, links, HTML.
fn inline(text: &str, start: usize, le: usize, runs: &mut Runs, comment: &mut bool) {
    let b = text.as_bytes();
    let mut i = start;
    while i < le {
        let c = b[i];
        if c == b'\\' {
            i += 2;
            continue;
        }
        if c == b'`' {
            let n = b[i..le].iter().take_while(|&&x| x == b'`').count();
            let tick = &b[i..i + n];
            if let Some(k) = find(b, i + n, le, tick) {
                runs.mark(i, k + n, Hl::Code);
                i = k + n;
            } else {
                i += n;
            }
            continue;
        }
        if (c == b'*' || c == b'_') && b.get(i + 1) == Some(&c) {
            let pair = [c, c];
            if let Some(k) = find(b, i + 2, le, &pair) {
                runs.mark(i, k + 2, Hl::Strong);
                i = k + 2;
                continue;
            }
        }
        if c == b'[' || (c == b'!' && b.get(i + 1) == Some(&b'[')) {
            let open = if c == b'!' { i + 1 } else { i };
            if let Some(k) = find(b, open, le, b"](") {
                if let Some(e) = find(b, k + 2, le, b")") {
                    runs.mark(i, e + 1, Hl::Link);
                    i = e + 1;
                    continue;
                }
            }
        }
        if c == b'<' {
            if b[i..le].starts_with(b"<!--") {
                match find(b, i + 4, le, b"-->") {
                    Some(k) => {
                        runs.mark(i, k + 3, Hl::Comment);
                        i = k + 3;
                    }
                    None => {
                        runs.mark(i, le, Hl::Comment);
                        *comment = true;
                        return;
                    }
                }
                continue;
            }
            if let Some(k) = find(b, i, le, b">") {
                let inner = &text[i + 1..k];
                if inner.starts_with("http") {
                    runs.mark(i, k + 1, Hl::Link);
                    i = k + 1;
                    continue;
                }
                if inner.starts_with(|ch: char| ch.is_ascii_alphabetic() || ch == '/') && !inner.contains('<') {
                    runs.mark(i, k + 1, Hl::Tag);
                    i = k + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
}

/// HTML, XML, SVG, Vue and Svelte templates, with `<script>` and `<style>`
/// bodies handed to the script and CSS lexers.
pub(super) fn markup(text: &str, start: usize, end: usize, runs: &mut Runs) {
    let b = text.as_bytes();
    let mut i = start;
    while i < end {
        let c = b[i];
        let rest = &b[i..end];
        if rest.starts_with(b"<!--") {
            let j = find(b, i + 4, end, b"-->").map_or(end, |k| k + 3);
            runs.mark(i, j, Hl::Comment);
            i = j;
        } else if rest.starts_with(b"<![CDATA[") {
            let j = find(b, i, end, b"]]>").map_or(end, |k| k + 3);
            runs.mark(i, j, Hl::String);
            i = j;
        } else if rest.starts_with(b"<!") || rest.starts_with(b"<?") {
            let j = find(b, i, end, b">").map_or(end, |k| k + 1);
            runs.mark(i, j, Hl::Meta);
            i = j;
        } else if c == b'<' && i + 1 < end && (b[i + 1].is_ascii_alphabetic() || b[i + 1] == b'/') {
            let closing = b[i + 1] == b'/';
            let ns = i + 1 + closing as usize;
            let mut ne = ns;
            while ne < end && (b[ne].is_ascii_alphanumeric() || matches!(b[ne], b':' | b'_' | b'.' | b'-')) {
                ne += 1;
            }
            runs.mark(i, ne, Hl::Tag);
            let name = text[ns..ne].to_ascii_lowercase();
            let mut k = ne;
            let mut self_closing = false;
            while k < end {
                let d = b[k];
                if d == b'>' {
                    runs.mark(k, k + 1, Hl::Tag);
                    k += 1;
                    break;
                }
                if d == b'/' && b.get(k + 1) == Some(&b'>') {
                    runs.mark(k, k + 2, Hl::Tag);
                    k += 2;
                    self_closing = true;
                    break;
                }
                if d == b'"' || d == b'\'' {
                    let j = string_end(b, k, end, d, false, true);
                    runs.mark(k, j, Hl::String);
                    k = j;
                } else if d.is_ascii_whitespace() || d == b'=' {
                    k += 1;
                } else {
                    let mut j = k;
                    while j < end && !b[j].is_ascii_whitespace() && !matches!(b[j], b'=' | b'>' | b'"' | b'\'') && !(b[j] == b'/' && b.get(j + 1) == Some(&b'>')) {
                        j += 1;
                    }
                    let unquoted_value = k > ne && b[k - 1] == b'=';
                    runs.mark(k, j.max(k + 1), if unquoted_value { Hl::String } else { Hl::Attr });
                    k = j.max(k + 1);
                }
            }
            i = k;
            if !closing && !self_closing && (name == "script" || name == "style") {
                let close = find_ci(b, i, end, if name == "script" { b"</script" } else { b"</style" }).unwrap_or(end);
                if name == "script" {
                    code::lex(text, i, close, code::script(), runs);
                } else {
                    css(text, i, close, runs);
                }
                i = close;
            }
        } else if c == b'&' {
            let mut j = i + 1;
            while j < end && j < i + 12 && (b[j].is_ascii_alphanumeric() || b[j] == b'#') {
                j += 1;
            }
            if j < end && b[j] == b';' && j > i + 1 {
                runs.mark(i, j + 1, Hl::Constant);
                i = j + 1;
            } else {
                i += 1;
            }
        } else if rest.starts_with(b"{{") {
            let j = find(b, i + 2, end, b"}}").unwrap_or(end);
            runs.mark(i, i + 2, Hl::Interp);
            code::lex(text, i + 2, j, code::script(), runs);
            if j < end {
                runs.mark(j, j + 2, Hl::Interp);
            }
            i = (j + 2).min(end);
        } else {
            i += 1;
        }
    }
}

fn find_ci(b: &[u8], from: usize, end: usize, pat: &[u8]) -> Option<usize> {
    if from >= end || pat.len() > end - from {
        return None;
    }
    b[from..end].windows(pat.len()).position(|w| w.eq_ignore_ascii_case(pat)).map(|k| from + k)
}

#[derive(Clone, Copy, PartialEq)]
enum CssAt {
    Selector,
    Property,
    Value,
}

pub(super) fn css(text: &str, start: usize, end: usize, runs: &mut Runs) {
    let b = text.as_bytes();
    let mut i = start;
    let mut depth = 0usize;
    // Whether the statement starting here is a selector (it reaches `{` first).
    let statement = |from: usize, depth: usize| {
        let stop = b[from..end].iter().position(|c| matches!(c, b'{' | b';' | b'}')).map(|k| b[from + k]);
        if depth == 0 || stop == Some(b'{') { CssAt::Selector } else { CssAt::Property }
    };
    let mut at = statement(i, 0);
    while i < end {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let rest = &b[i..end];
        if rest.starts_with(b"/*") {
            let j = find(b, i + 2, end, b"*/").map_or(end, |k| k + 2);
            runs.mark(i, j, Hl::Comment);
            i = j;
            continue;
        }
        if rest.starts_with(b"//") && (i == start || b[i - 1].is_ascii_whitespace()) {
            let j = line_end(b, i, end);
            runs.mark(i, j, Hl::Comment);
            i = j;
            continue;
        }
        match c {
            b'{' => {
                depth += 1;
                i += 1;
                at = statement(i, depth);
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                i += 1;
                at = statement(i, depth);
            }
            b';' => {
                i += 1;
                at = statement(i, depth);
            }
            b'"' | b'\'' => {
                let j = string_end(b, i, end, c, true, false);
                runs.mark(i, j, Hl::String);
                i = j;
            }
            b'@' => {
                let j = css_ident_end(b, i + 1, end);
                runs.mark(i, j.max(i + 1), Hl::Keyword);
                i = j.max(i + 1);
            }
            b'!' if rest[1..].starts_with(b"important") => {
                runs.mark(i, i + 10, Hl::Keyword);
                i += 10;
            }
            b':' if at == CssAt::Property => {
                at = CssAt::Value;
                i += 1;
            }
            b':' if at == CssAt::Selector => {
                let s = if b.get(i + 1) == Some(&b':') { i + 2 } else { i + 1 };
                let j = css_ident_end(b, s, end);
                runs.mark(i, j, Hl::Attr);
                i = j.max(i + 1);
            }
            b'$' => {
                let j = css_ident_end(b, i + 1, end);
                runs.mark(i, j, Hl::Param);
                i = j.max(i + 1);
            }
            b'-' if b.get(i + 1) == Some(&b'-') => {
                let j = css_ident_end(b, i, end);
                runs.mark(i, j, if at == CssAt::Property { Hl::Key } else { Hl::Param });
                i = j.max(i + 1);
            }
            b'#' | b'.' if at == CssAt::Selector => {
                let j = css_ident_end(b, i + 1, end);
                runs.mark(i, j, Hl::Type);
                i = j.max(i + 1);
            }
            b'#' if at == CssAt::Value => {
                let j = css_ident_end(b, i + 1, end);
                runs.mark(i, j, Hl::Number);
                i = j.max(i + 1);
            }
            _ if c.is_ascii_digit() || ((c == b'.' || c == b'-') && b.get(i + 1).is_some_and(u8::is_ascii_digit)) => {
                let mut j = i + 1;
                while j < end && (b[j].is_ascii_alphanumeric() || b[j] == b'.' || b[j] == b'%') {
                    j += 1;
                }
                runs.mark(i, j, Hl::Number);
                i = j;
            }
            _ if c.is_ascii_alphabetic() || c == b'-' || c == b'_' => {
                let j = css_ident_end(b, i, end);
                let hl = if b.get(j) == Some(&b'(') {
                    Hl::Call
                } else {
                    match at {
                        CssAt::Selector => Hl::Tag,
                        CssAt::Property => Hl::Key,
                        CssAt::Value => Hl::Constant,
                    }
                };
                runs.mark(i, j, hl);
                i = j.max(i + 1);
            }
            _ => i += 1,
        }
    }
}

fn css_ident_end(b: &[u8], mut i: usize, end: usize) -> usize {
    while i < end && (b[i].is_ascii_alphanumeric() || matches!(b[i], b'-' | b'_') || b[i] >= 0x80) {
        i += 1;
    }
    i
}

pub(super) fn makefile(text: &str, runs: &mut Runs) {
    let b = text.as_bytes();
    let shell = code::shell();
    for (ls, le) in lines(text) {
        if b.get(ls) == Some(&b'\t') {
            code::lex(text, ls, le, shell, runs);
            continue;
        }
        let p = indent(b, ls, le);
        if p >= le {
            continue;
        }
        if b[p] == b'#' {
            runs.mark(p, le, Hl::Comment);
            continue;
        }
        let word_end = (p..le).find(|&k| b[k].is_ascii_whitespace()).unwrap_or(le);
        let word = &text[p..word_end];
        if matches!(word, "include" | "-include" | "sinclude" | "ifeq" | "ifneq" | "ifdef" | "ifndef" | "else" | "endif" | "define" | "endef" | "export" | "unexport" | "override" | "vpath") {
            runs.mark(p, word_end, Hl::Keyword);
            code::lex(text, word_end, le, shell, runs);
            continue;
        }
        let assign = (p..le).find(|&k| b[k] == b'=');
        let colon = (p..le).find(|&k| b[k] == b':' && b.get(k + 1) != Some(&b'='));
        match (assign, colon) {
            (Some(eq), c) if c.is_none_or(|c| c > eq || b[c + 1..eq].iter().all(|x| matches!(x, b':' | b'?' | b'+' | b'!'))) => {
                let mut k = eq;
                while k > p && matches!(b[k - 1], b':' | b'?' | b'+' | b'!') {
                    k -= 1;
                }
                runs.mark(p, trim_end(b, p, k), Hl::Key);
                code::lex(text, eq + 1, le, shell, runs);
            }
            (_, Some(c)) => {
                runs.mark(p, trim_end(b, p, c), Hl::Function);
                code::lex(text, c + 1, le, shell, runs);
            }
            _ => code::lex(text, p, le, shell, runs),
        }
    }
}

const DOCKER_INSTRUCTIONS: &[&str] = &["FROM", "RUN", "CMD", "LABEL", "MAINTAINER", "EXPOSE", "ENV", "ADD", "COPY", "ENTRYPOINT", "VOLUME", "USER", "WORKDIR", "ARG", "ONBUILD", "STOPSIGNAL", "HEALTHCHECK", "SHELL"];

pub(super) fn dockerfile(text: &str, runs: &mut Runs) {
    let b = text.as_bytes();
    let shell = code::shell();
    let mut continued = false;
    for (ls, le) in lines(text) {
        let p = indent(b, ls, le);
        let e = trim_end(b, ls, le);
        let next_continues = e > ls && b[e - 1] == b'\\';
        if p >= le {
            continued = false;
            continue;
        }
        if b[p] == b'#' && !continued {
            runs.mark(p, le, Hl::Comment);
            continue;
        }
        let mut q = p;
        if !continued {
            let w = (p..le).find(|&k| b[k].is_ascii_whitespace()).unwrap_or(le);
            if DOCKER_INSTRUCTIONS.iter().any(|i| i.eq_ignore_ascii_case(&text[p..w])) {
                runs.mark(p, w, Hl::Keyword);
                q = w;
            }
        }
        code::lex(text, q, le, shell, runs);
        continued = next_continues;
    }
}
