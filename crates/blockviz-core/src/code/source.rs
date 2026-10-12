//! Source text with line, comment and blank-line lookups.
//!
//! The AST drops comments, but comments are the strongest signal of how a
//! human chunks a script ("# model init", "# training loop"), so front ends
//! recover them from their token streams.

use super::syntax::{Ranged, TextRange};
use crate::ir::Span;

pub struct Comment {
    pub line: u32,
    /// The comment's words without its markers (`#`, `//`, `/* */`).
    pub text: String,
    /// The comment is alone on its line (not trailing code).
    pub own_line: bool,
    /// Documentation of the item below it (Rust's `///`), not a heading.
    pub doc: bool,
}

pub struct Source {
    pub text: String,
    pub file: u32,
    /// Shortened code is written in the module's own syntax.
    pub lang: super::syntax::Lang,
    line_starts: Vec<usize>,
    /// In line order: a line's are found by binary search.
    comments: Vec<Comment>,
    blank: Vec<bool>,
}

impl Source {
    /// `comments` are comment lines: where each starts, and its words without
    /// comment markers. A block comment contributes one entry per line.
    pub fn new(text: String, file: u32, comments: &[(TextRange, String)]) -> Self {
        let mut line_starts = vec![0];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        let blank = text.split('\n').map(|l| l.trim().is_empty()).collect();
        let mut s = Source { text, file, lang: super::syntax::Lang::Python, line_starts, comments: vec![], blank };
        let mut found = vec![];
        for (range, body) in comments {
            let start = usize::from(range.start());
            let line = s.line_of(start);
            let line_start = s.line_starts[(line - 1) as usize];
            let own_line = s.text.get(line_start..start).is_some_and(|t| t.trim().is_empty());
            found.push(Comment { line, text: body.clone(), own_line, doc: false });
        }
        found.sort_by_key(|c| c.line);
        s.comments = found;
        s
    }

    /// Marks the comments starting at these offsets as documentation.
    pub fn mark_docs(&mut self, starts: &[usize]) {
        let lines: crate::hash::Set<u32> = starts.iter().map(|&o| self.line_of(o)).collect();
        for c in &mut self.comments {
            if lines.contains(&c.line) {
                c.doc = true;
            }
        }
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

    /// The comments on lines `lines`.
    fn on_lines(&self, lines: std::ops::Range<u32>) -> &[Comment] {
        let start = self.comments.partition_point(|c| c.line < lines.start);
        let end = self.comments.partition_point(|c| c.line < lines.end).max(start);
        &self.comments[start..end]
    }

    /// Own-line comments directly above `line` (no blank line in between), top to bottom.
    pub fn comments_above(&self, line: u32) -> Vec<&Comment> {
        let mut out = vec![];
        let mut l = line.saturating_sub(1);
        while l >= 1 {
            match self.on_lines(l..l + 1).iter().find(|c| c.own_line) {
                Some(c) => out.push(c),
                None => break,
            }
            l -= 1;
        }
        out.reverse();
        out
    }

    /// The description in the comment block above a line: its prose, before
    /// any `@param`-style tags.
    pub fn prose_above(&self, line: u32) -> String {
        let prose: Vec<&str> = self.comments_above(line).iter().map(|c| c.text.trim()).take_while(|t| !t.starts_with('@')).collect();
        crate::text::summary(&prose.join(" "))
    }

    pub fn trailing_comment(&self, line: u32) -> Option<&str> {
        self.on_lines(line..line + 1).iter().find(|c| !c.own_line).map(|c| c.text.trim())
    }

    /// Own-line comments strictly between two lines (used for headings inside settings runs).
    pub fn comments_between(&self, after: u32, before: u32) -> Vec<&Comment> {
        self.on_lines(after + 1..before).iter().filter(|c| c.own_line).collect()
    }

    /// Is there a blank line between the end of one statement and the start of the next
    /// (ignoring the comment block that introduces the next one)?
    pub fn paragraph_break(&self, prev_end: u32, next_start: u32) -> bool {
        let first_comment = self.comments_above(next_start).first().map(|c| c.line).unwrap_or(next_start);
        (prev_end + 1..first_comment).any(|l| self.is_blank(l))
    }
}
