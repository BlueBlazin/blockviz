//! Source text with line, comment and blank-line lookups.
//!
//! The AST drops comments, but comments are the strongest signal of how a
//! human chunks a script ("# model init", "# training loop"), so they are
//! recovered from the token stream.

use crate::ir::Span;
use ruff_text_size::{Ranged, TextRange};

pub struct Comment {
    pub line: u32,
    pub text: String,
    /// The comment is alone on its line (not trailing code).
    pub own_line: bool,
}

pub struct Source {
    pub text: String,
    pub file: u32,
    line_starts: Vec<usize>,
    comments: Vec<Comment>,
    blank: Vec<bool>,
}

impl Source {
    /// `comments` are the ranges of comment tokens reported by the parser.
    pub fn new(text: String, file: u32, comments: &[TextRange]) -> Self {
        let mut line_starts = vec![0];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        let blank = text.split('\n').map(|l| l.trim().is_empty()).collect();
        let mut s = Source { text, file, line_starts, comments: vec![], blank };
        let mut found = vec![];
        for range in comments {
            let start = usize::from(range.start());
            let line = s.line_of(start);
            let line_start = s.line_starts[(line - 1) as usize];
            let own_line = s.text[line_start..start].trim().is_empty();
            let text = s.text[start..usize::from(range.end())].to_string();
            found.push(Comment { line, text, own_line });
        }
        s.comments = found;
        s
    }

    /// 1-based line of a byte offset.
    pub fn line_of(&self, offset: usize) -> u32 {
        self.line_starts.partition_point(|&p| p <= offset) as u32
    }

    pub fn slice(&self, r: TextRange) -> &str {
        self.text.get(usize::from(r.start())..usize::from(r.end())).unwrap_or("")
    }

    pub fn of(&self, n: &impl Ranged) -> &str {
        self.slice(n.range())
    }

    pub fn span(&self, r: TextRange) -> Span {
        let (start, end) = (usize::from(r.start()), usize::from(r.end()));
        Span {
            file: self.file,
            start: start as u32,
            end: end as u32,
            line: self.line_of(start),
            end_line: self.line_of(end.saturating_sub(1).max(start)),
        }
    }

    pub fn is_blank(&self, line: u32) -> bool {
        self.blank.get((line as usize).wrapping_sub(1)).copied().unwrap_or(true)
    }

    /// Own-line comments directly above `line` (no blank line in between), top to bottom.
    pub fn comments_above(&self, line: u32) -> Vec<&Comment> {
        let mut out = vec![];
        let mut l = line.saturating_sub(1);
        while l >= 1 {
            match self.comments.iter().find(|c| c.line == l && c.own_line) {
                Some(c) => out.push(c),
                None => break,
            }
            l -= 1;
        }
        out.reverse();
        out
    }

    pub fn trailing_comment(&self, line: u32) -> Option<&str> {
        self.comments.iter().find(|c| c.line == line && !c.own_line).map(|c| c.text.trim_start_matches('#').trim())
    }

    /// Own-line comments strictly between two lines (used for headings inside settings runs).
    pub fn comments_between(&self, after: u32, before: u32) -> Vec<&Comment> {
        self.comments.iter().filter(|c| c.own_line && c.line > after && c.line < before).collect()
    }

    /// Is there a blank line between the end of one statement and the start of the next
    /// (ignoring the comment block that introduces the next one)?
    pub fn paragraph_break(&self, prev_end: u32, next_start: u32) -> bool {
        let first_comment = self.comments_above(next_start).first().map(|c| c.line).unwrap_or(next_start);
        (prev_end + 1..first_comment).any(|l| self.is_blank(l))
    }
}
