//! Turns a span of a file plus its syntax runs into display lines (src/ui/code.ts).

#[derive(Clone, Debug, PartialEq)]
pub struct Seg {
    pub text: String,
    /// The syntax class (`highlight::Hl`); 0 is plain.
    pub cls: u32,
    /// Outside the span, on one of its lines: shown faded, so a call is read in its statement.
    pub dim: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub no: u32,
    pub segs: Vec<Seg>,
}

#[cfg(test)]
impl Line {
    pub fn text(&self) -> String {
        self.segs.iter().map(|s| s.text.as_str()).collect()
    }
}

/// The whole lines a span touches, as classed segments, and how many lines
/// past `max_lines` were left out. Common indentation is removed. `runs` is
/// `[len, class, ...]` over the file's UTF-8 bytes; empty, everything is plain.
pub fn lines(text: &str, runs: &[u32], start: usize, end: usize, first_line: u32, max_lines: usize) -> (Vec<Line>, usize) {
    let bytes = text.as_bytes();
    let start = start.min(bytes.len());
    let mut end = end.max(start).min(bytes.len());
    while end > start && matches!(bytes[end - 1], b'\n' | b'\r') {
        end -= 1;
    }
    let from = if start == 0 { 0 } else { bytes[..start].iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1) };
    let to = bytes[end..].iter().position(|&b| b == b'\n').map_or(bytes.len(), |i| end + i);

    let mut out = vec![Line { no: first_line, segs: vec![] }];
    let mut full = false;
    // Splits [a, b) at newlines into the current and following lines.
    let mut emit = |a: usize, b: usize, cls: u32, out: &mut Vec<Line>| {
        let mut s = a;
        while s < b && !full {
            let e = bytes[s..b].iter().position(|&c| c == b'\n').map_or(b, |i| s + i);
            if e > s {
                let piece = String::from_utf8_lossy(&bytes[s..e]);
                let piece = piece.strip_suffix('\r').unwrap_or(&piece).to_string();
                out.last_mut().unwrap().segs.push(Seg { text: piece, cls, dim: e <= start || s >= end });
            }
            if e < b {
                if out.len() >= max_lines {
                    full = true;
                } else {
                    let no = out.last().unwrap().no + 1;
                    out.push(Line { no, segs: vec![] });
                }
                s = e + 1;
            } else {
                s = e;
            }
        }
    };
    // Also splits at the span's ends, where dimming starts and stops.
    let cuts: Vec<usize> = [start, end].into_iter().filter(|&c| c > from && c < to).collect();
    let mut piece = |a: usize, b: usize, cls: u32, out: &mut Vec<Line>| {
        let mut s = a;
        for &c in &cuts {
            if c > s && c < b {
                emit(s, c, cls, out);
                s = c;
            }
        }
        emit(s, b, cls, out);
    };
    if runs.len() < 2 {
        piece(from, to, 0, &mut out);
    } else {
        let mut at = 0usize;
        for pair in runs.chunks_exact(2) {
            if at >= to {
                break;
            }
            let next = at + pair[0] as usize;
            if next > from {
                piece(at.max(from), next.min(to), pair[1], &mut out);
            }
            at = next;
        }
        if at < to {
            piece(at.max(from), to, 0, &mut out);
        }
    }
    let more = if out.len() >= max_lines && to > from { bytes[from..to].iter().filter(|&&b| b == b'\n').count() + 1 - out.len() } else { 0 };
    (dedent(out), more)
}

fn leading(s: &str) -> usize {
    s.len() - s.trim_start_matches([' ', '\t']).len()
}

/// Removes the indentation every non-blank line shares.
fn dedent(lines: Vec<Line>) -> Vec<Line> {
    let indent = |l: &Line| {
        let mut n = 0;
        for s in &l.segs {
            let ws = leading(&s.text);
            n += ws;
            if ws < s.text.len() {
                return Some(n);
            }
        }
        None
    };
    let cut = lines.iter().filter_map(indent).min().unwrap_or(0);
    if cut == 0 {
        return lines;
    }
    lines
        .into_iter()
        .map(|l| {
            let mut left = cut;
            let mut segs = vec![];
            for s in l.segs {
                if left == 0 {
                    segs.push(s);
                    continue;
                }
                let take = left.min(leading(&s.text));
                if take == s.text.len() {
                    left -= take;
                    continue;
                }
                segs.push(Seg { text: s.text[take..].to_string(), ..s });
                left = 0;
            }
            Line { no: l.no, segs }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "class A:\n    def f(self):\n        x = self.attn(x)\n        return x\n";

    fn texts(l: &[Line]) -> Vec<String> {
        l.iter().map(Line::text).collect()
    }

    #[test]
    fn shows_whole_lines_dedented_numbered_from_the_span() {
        let start = SRC.find("def").unwrap();
        let (l, _) = lines(SRC, &[], start, SRC.len(), 2, 2000);
        assert_eq!(texts(&l), ["def f(self):", "    x = self.attn(x)", "    return x"]);
        assert_eq!(l.iter().map(|l| l.no).collect::<Vec<_>>(), [2, 3, 4]);
    }

    #[test]
    fn dims_the_rest_of_the_statement_around_a_call() {
        let start = SRC.find("self.attn").unwrap();
        let end = start + "self.attn(x)".len();
        let (l, _) = lines(SRC, &[], start, end, 3, 2000);
        assert_eq!(texts(&l), ["x = self.attn(x)"]);
        assert_eq!(l[0].segs.iter().map(|s| (s.text.as_str(), s.dim)).collect::<Vec<_>>(), [("x = ", true), ("self.attn(x)", false)]);
    }

    #[test]
    fn applies_syntax_runs_and_splits_them_at_line_ends() {
        let s = "a = 'x'\nb\n";
        let (l, _) = lines(s, &[4, 0, 3, 2, 3, 0], 0, s.len(), 1, 2000);
        assert_eq!(l[0].segs, [Seg { text: "a = ".into(), cls: 0, dim: false }, Seg { text: "'x'".into(), cls: 2, dim: false }]);
        assert_eq!(texts(&l), ["a = 'x'", "b"]);
    }

    #[test]
    fn keeps_multi_byte_characters_whole() {
        let s = "π = \"日本\"\n";
        let (l, _) = lines(s, &[5, 0, 8, 2, 1, 0], 0, s.len(), 1, 2000);
        assert_eq!(l[0].segs.iter().map(|x| x.text.as_str()).collect::<Vec<_>>(), ["π = ", "\"日本\""]);
    }

    #[test]
    fn caps_very_long_spans() {
        let s = (0..50).map(|i| format!("l{i}")).collect::<Vec<_>>().join("\n");
        let (l, more) = lines(&s, &[], 0, s.len(), 1, 10);
        assert_eq!(l.len(), 10);
        assert_eq!(more, 40);
    }
}
