//! Line diffs of two versions of a file, ported from src/ui/linediff.ts.

#![allow(dead_code)]

use std::collections::HashMap;
use std::hash::Hash;

/// Whether a line of a diff is kept, added (only in the new text) or deleted (only in the old).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpKind {
    Same,
    Add,
    Del,
}

/// One line of a diff: `a` and `b` are its indexes in the old and new lines
/// (none where it is not there).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineOp {
    pub kind: OpKind,
    pub a: Option<usize>,
    pub b: Option<usize>,
}

/// Unchanged lines left out of a diff, between two shown ones: `count` of
/// them, the first at `a` in the old lines and `b` in the new.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gap {
    pub a: usize,
    pub b: usize,
    pub count: usize,
}

/// A row of a folded diff: a line, or unchanged lines left out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Op(LineOp),
    Gap(Gap),
}

/// Edits beyond which two texts are shown as replaced whole rather than diffed.
const MAX_EDITS: usize = 4000;

/// Unchanged lines a folded diff shows either side of a change, as diffs usually do.
pub const CONTEXT: usize = 3;

fn same(a: usize, b: usize) -> LineOp {
    LineOp { kind: OpKind::Same, a: Some(a), b: Some(b) }
}

fn add(b: usize) -> LineOp {
    LineOp { kind: OpKind::Add, a: None, b: Some(b) }
}

fn del(a: usize) -> LineOp {
    LineOp { kind: OpKind::Del, a: Some(a), b: None }
}

/// The shortest edit from `a` to `b`, line by line (Myers' algorithm, after
/// the lines they share at either end). Deletions come before additions where
/// both are possible, as diffs usually read.
pub fn diff_lines<T: Eq + Hash>(a: &[T], b: &[T]) -> Vec<LineOp> {
    let mut pre = 0;
    while pre < a.len() && pre < b.len() && a[pre] == b[pre] {
        pre += 1;
    }
    let mut post = 0;
    while post < a.len() - pre && post < b.len() - pre && a[a.len() - 1 - post] == b[b.len() - 1 - post] {
        post += 1;
    }
    let mut out: Vec<LineOp> = (0..pre).map(|i| same(i, i)).collect();
    out.extend(middle(&a[pre..a.len() - post], &b[pre..b.len() - post], pre));
    out.extend((0..post).map(|i| same(a.len() - post + i, b.len() - post + i)));
    out
}

fn middle<T: Eq + Hash>(a: &[T], b: &[T], offset: usize) -> Vec<LineOp> {
    let (n, m) = (a.len(), b.len());
    let replaced = || (0..n).map(|i| del(offset + i)).chain((0..m).map(|i| add(offset + i))).collect();
    // Texts too far apart are often plainly so (a file reindented): no need to diff them to find out.
    if n == 0 || m == 0 || fewest_edits(a, b) > MAX_EDITS {
        return replaced();
    }
    let max = n + m;
    let at = |k: i32| (max as i32 + k) as usize;
    let mut v = vec![0i32; 2 * max + 2];
    let mut trace: Vec<Vec<i32>> = vec![];
    let mut found = None;
    'search: for d in 0..=max.min(MAX_EDITS) {
        // The reaches step d starts from; walking back reads them on diagonals -d..d only.
        trace.push(v[max - d..=max + d].to_vec());
        let d = d as i32;
        for k in (-d..=d).step_by(2) {
            let mut x = if k == -d || (k != d && v[at(k - 1)] < v[at(k + 1)]) { v[at(k + 1)] } else { v[at(k - 1)] + 1 };
            let mut y = x - k;
            while (x as usize) < n && (y as usize) < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[at(k)] = x;
            if x as usize >= n && y as usize >= m {
                found = Some(d);
                break 'search;
            }
        }
    }
    let Some(found) = found else { return replaced() };
    // Walk back through the furthest reaches to recover the edits.
    let mut ops = vec![];
    let (mut x, mut y) = (n as i32, m as i32);
    for d in (1..=found).rev() {
        let pv = &trace[d as usize];
        let k = x - y;
        let down = k == -d || (k != d && pv[(d + k - 1) as usize] < pv[(d + k + 1) as usize]);
        let pk = if down { k + 1 } else { k - 1 };
        let px = pv[(d + pk) as usize];
        let py = px - pk;
        while x > px && y > py {
            x -= 1;
            y -= 1;
            ops.push(same(offset + x as usize, offset + y as usize));
        }
        if down {
            y -= 1;
            ops.push(add(offset + y as usize));
        } else {
            x -= 1;
            ops.push(del(offset + x as usize));
        }
    }
    while x > 0 && y > 0 {
        x -= 1;
        y -= 1;
        ops.push(same(offset + x as usize, offset + y as usize));
    }
    ops.reverse();
    group_edits(ops)
}

/// Edits any diff from `a` to `b` takes at least: one for each copy of a line the other has fewer of.
fn fewest_edits<T: Eq + Hash>(a: &[T], b: &[T]) -> usize {
    let mut extra: HashMap<&T, i64> = HashMap::new();
    for l in a {
        *extra.entry(l).or_default() += 1;
    }
    for l in b {
        *extra.entry(l).or_default() -= 1;
    }
    extra.values().map(|e| e.unsigned_abs() as usize).sum()
}

/// Within each run of edits, deletions before additions.
fn group_edits(ops: Vec<LineOp>) -> Vec<LineOp> {
    let mut out = Vec::with_capacity(ops.len());
    let mut i = 0;
    while i < ops.len() {
        if ops[i].kind == OpKind::Same {
            out.push(ops[i]);
            i += 1;
            continue;
        }
        let start = i;
        while i < ops.len() && ops[i].kind != OpKind::Same {
            i += 1;
        }
        let run = &ops[start..i];
        out.extend(run.iter().filter(|o| o.kind == OpKind::Del));
        out.extend(run.iter().filter(|o| o.kind == OpKind::Add));
    }
    out
}

/// A diff with long runs of unchanged lines folded to `context` lines either
/// side of a change. Lines `keep` says to show (the selected block's) are
/// never folded away.
pub fn fold(ops: &[LineOp], context: usize, keep: impl Fn(&LineOp) -> bool) -> Vec<Row> {
    let show: Vec<bool> = ops.iter().map(|o| o.kind != OpKind::Same || keep(o)).collect();
    let mut near = show.clone();
    for (i, _) in show.iter().enumerate().filter(|(_, &s)| s) {
        for n in &mut near[i.saturating_sub(context)..=(i + context).min(ops.len() - 1)] {
            *n = true;
        }
    }
    let mut out = vec![];
    let mut i = 0;
    while i < ops.len() {
        if near[i] {
            out.push(Row::Op(ops[i]));
            i += 1;
            continue;
        }
        let mut j = i;
        while j < ops.len() && !near[j] {
            j += 1;
        }
        // Folding one line saves nothing.
        if j - i == 1 {
            out.push(Row::Op(ops[i]));
        } else {
            // Only unchanged lines fold, and those are in both texts.
            let (a, b) = (ops[i].a.expect("a kept line"), ops[i].b.expect("a kept line"));
            out.push(Row::Gap(Gap { a, b, count: j - i }));
        }
        i = j;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A diff as text: ` x`, `+x`, `-x`, and `… n` for folded lines.
    fn show(a: &[&str], b: &[&str], rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                Row::Gap(g) => format!("… {}", g.count),
                Row::Op(o) => match o.kind {
                    OpKind::Same => format!(" {}", b[o.b.unwrap()]),
                    OpKind::Add => format!("+{}", b[o.b.unwrap()]),
                    OpKind::Del => format!("-{}", a[o.a.unwrap()]),
                },
            })
            .collect()
    }

    fn rows(ops: Vec<LineOp>) -> Vec<Row> {
        ops.into_iter().map(Row::Op).collect()
    }

    /// diff_lines as first written, keeping every step's reaches on all diagonals: the diff it finds is the one to find.
    fn reference(a: &[String], b: &[String]) -> Vec<LineOp> {
        let mut pre = 0;
        while pre < a.len() && pre < b.len() && a[pre] == b[pre] {
            pre += 1;
        }
        let mut post = 0;
        while post < a.len() - pre && post < b.len() - pre && a[a.len() - 1 - post] == b[b.len() - 1 - post] {
            post += 1;
        }
        let mut out: Vec<LineOp> = (0..pre).map(|i| same(i, i)).collect();
        out.extend(reference_middle(&a[pre..a.len() - post], &b[pre..b.len() - post], pre));
        out.extend((0..post).map(|i| same(a.len() - post + i, b.len() - post + i)));
        out
    }

    fn reference_middle(a: &[String], b: &[String], offset: usize) -> Vec<LineOp> {
        let (n, m) = (a.len() as i32, b.len() as i32);
        let replaced = || (0..a.len()).map(|i| del(offset + i)).chain((0..b.len()).map(|i| add(offset + i))).collect();
        if n == 0 || m == 0 {
            return replaced();
        }
        let max = n + m;
        let at = |k: i32| (max + k) as usize;
        let mut v = vec![0i32; 2 * max as usize + 2];
        let mut trace = vec![];
        let mut found = None;
        'search: for d in 0..=max.min(MAX_EDITS as i32) {
            trace.push(v.clone());
            for k in (-d..=d).step_by(2) {
                let mut x = if k == -d || (k != d && v[at(k - 1)] < v[at(k + 1)]) { v[at(k + 1)] } else { v[at(k - 1)] + 1 };
                let mut y = x - k;
                while x < n && y < m && a[x as usize] == b[y as usize] {
                    x += 1;
                    y += 1;
                }
                v[at(k)] = x;
                if x >= n && y >= m {
                    found = Some(d);
                    break 'search;
                }
            }
        }
        let Some(found) = found else { return replaced() };
        let mut ops = vec![];
        let (mut x, mut y) = (n, m);
        for d in (1..=found).rev() {
            let pv = &trace[d as usize];
            let k = x - y;
            let down = k == -d || (k != d && pv[at(k - 1)] < pv[at(k + 1)]);
            let pk = if down { k + 1 } else { k - 1 };
            let px = pv[at(pk)];
            let py = px - pk;
            while x > px && y > py {
                x -= 1;
                y -= 1;
                ops.push(same(offset + x as usize, offset + y as usize));
            }
            if down {
                y -= 1;
                ops.push(add(offset + y as usize));
            } else {
                x -= 1;
                ops.push(del(offset + x as usize));
            }
        }
        while x > 0 && y > 0 {
            x -= 1;
            y -= 1;
            ops.push(same(offset + x as usize, offset + y as usize));
        }
        ops.reverse();
        group_edits(ops)
    }

    #[test]
    fn finds_the_shortest_edit_deletions_before_additions() {
        let a = ["def f(x):", "    y = g(x)", "    return y"];
        let b = ["def f(x):", "    y = g(x, strict=True)", "    z = h(y)", "    return z"];
        assert_eq!(
            show(&a, &b, &rows(diff_lines(&a, &b))),
            [" def f(x):", "-    y = g(x)", "-    return y", "+    y = g(x, strict=True)", "+    z = h(y)", "+    return z"]
        );
    }

    #[test]
    fn keeps_lines_that_moved_around_edits() {
        let a = ["a", "b", "c", "d", "e"];
        let b = ["a", "x", "c", "d", "y", "e"];
        assert_eq!(show(&a, &b, &rows(diff_lines(&a, &b))), [" a", "-b", "+x", " c", " d", "+y", " e"]);
    }

    #[test]
    fn handles_empty_sides() {
        let none: [&str; 0] = [];
        assert_eq!(show(&none, &["a"], &rows(diff_lines(&none, &["a"]))), ["+a"]);
        assert_eq!(show(&["a"], &none, &rows(diff_lines(&["a"], &none))), ["-a"]);
        assert_eq!(diff_lines(&none, &none), []);
    }

    #[test]
    fn finds_the_diff_it_always_found_for_any_texts() {
        // Seeded, so a failure repeats: texts of a few kinds of line (many equally short diffs), and texts edited.
        let mut seed: u64 = 1;
        let mut random = |n: usize| {
            seed = seed * 16807 % 2147483647;
            (seed as f64 / 2147483647.0 * n as f64).floor() as usize
        };
        for i in 0..2000 {
            let len = random(40);
            let kinds = 1 + random(6);
            let a: Vec<String> = (0..len).map(|_| format!("l{}", random(kinds))).collect();
            let b: Vec<String> = if i % 2 == 1 {
                let len = random(40);
                let kinds = 1 + random(6);
                (0..len).map(|_| format!("l{}", random(kinds))).collect()
            } else {
                a.iter()
                    .flat_map(|l| match random(5) {
                        0 => vec![],
                        1 | 2 => vec![l.clone()],
                        3 => vec![l.clone(), "new".to_string()],
                        _ => vec!["new".to_string()],
                    })
                    .collect()
            };
            assert_eq!(diff_lines(&a, &b), reference(&a, &b), "{a:?} → {b:?}");
        }
    }

    #[test]
    fn diffs_texts_up_to_4000_edits_apart_and_shows_ones_further_apart_as_replaced() {
        let lines = |prefix: &'static str| (0..1000).map(move |i| format!("{prefix}{i}"));
        let a: Vec<String> = lines("a").chain(["kept".to_string()]).chain(lines("b")).collect();
        let b: Vec<String> = lines("c").chain(["kept".to_string()]).chain(lines("d")).collect();
        let kept = |ops: Vec<LineOp>| ops.iter().filter(|o| o.kind == OpKind::Same).count();
        assert_eq!(kept(diff_lines(&a, &b)), 1);
        let more: Vec<String> = a.iter().cloned().chain(["more".to_string()]).collect();
        assert_eq!(kept(diff_lines(&more, &b)), 0);
    }

    #[test]
    fn folds_long_unchanged_runs_keeping_context_and_the_lines_asked_for() {
        let a: Vec<String> = (0..20).map(|i| format!("l{i}")).collect();
        let b: Vec<String> = a.iter().enumerate().map(|(i, l)| if i == 10 { "changed".to_string() } else { l.clone() }).collect();
        let (a, b): (Vec<&str>, Vec<&str>) = (a.iter().map(String::as_str).collect(), b.iter().map(String::as_str).collect());
        assert_eq!(show(&a, &b, &fold(&diff_lines(&a, &b), 2, |_| false)), ["… 8", " l8", " l9", "-l10", "+changed", " l11", " l12", "… 7"]);
        let kept = fold(&diff_lines(&a, &b), 2, |o| o.b == Some(1));
        assert_eq!(show(&a, &b, &kept)[..5], [" l0", " l1", " l2", " l3", "… 4"]);
    }
}
