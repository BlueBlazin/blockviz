//! What the renderer draws with: the part of a canvas's 2D context that
//! src/canvas/renderer.ts uses, so the renderer is ported line for line and
//! draws the same on screen (GPUI) as into an SVG.

use crate::theme::Color;
use std::f64::consts::PI;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Family {
    Sans,
    Mono,
}

/// The font text is set in: `px` pixels of `family` at `weight`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Font {
    pub px: f64,
    pub family: Family,
    pub weight: u16,
}

impl Default for Font {
    fn default() -> Self {
        Font { px: 10.0, family: Family::Sans, weight: 400 }
    }
}

pub trait Pen {
    fn set_fill(&mut self, c: Color);
    fn set_stroke(&mut self, c: Color);
    fn set_alpha(&mut self, a: f64);
    fn set_line_width(&mut self, w: f64);
    fn line_width(&self) -> f64;
    fn set_round_join(&mut self, round: bool);
    fn set_dash_offset(&mut self, offset: f64);
    fn set_dash(&mut self, segments: &[f64]);
    fn set_font(&mut self, f: Font);
    fn set_align(&mut self, a: Align);
    fn save(&mut self);
    fn restore(&mut self);
    /// Scales what follows, the same across as down (the renderer scales text).
    fn scale(&mut self, k: f64);

    fn begin_path(&mut self);
    fn move_to(&mut self, x: f64, y: f64);
    fn line_to(&mut self, x: f64, y: f64);
    /// A circle's arc, clockwise from angle `from` to `to`.
    fn arc(&mut self, x: f64, y: f64, r: f64, from: f64, to: f64);
    /// A corner of radius `r` from where the path is, toward (x1, y1), turning toward (x2, y2).
    fn arc_to(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, r: f64);
    fn round_rect(&mut self, x: f64, y: f64, w: f64, h: f64, r: f64);
    fn close_path(&mut self);
    fn fill(&mut self);
    fn stroke(&mut self);
    /// Clips what follows to the path, until `restore`.
    fn clip(&mut self);
    fn fill_rect(&mut self, x: f64, y: f64, w: f64, h: f64);

    fn fill_text(&mut self, text: &str, x: f64, y: f64);
    fn stroke_text(&mut self, text: &str, x: f64, y: f64);
    /// The width of `text` in the current font (before any scale).
    fn measure(&mut self, text: &str) -> f64;
}

/// A piece of a path, in final (scaled) coordinates.
#[derive(Clone, Debug, PartialEq)]
pub enum Seg {
    Move(f64, f64),
    Line(f64, f64),
    /// An arc of a circle centered at (cx, cy), from angle a0 to a1 (radians, either way round).
    Arc { cx: f64, cy: f64, r: f64, a0: f64, a1: f64 },
    Close,
}

/// A path as a canvas builds it: `arc`, `arcTo` and `roundRect` become exact
/// arcs, and what it is when it is one simple shape (a rounded rectangle or a
/// circle) is kept, so a pen can draw those directly.
#[derive(Clone, Debug, Default)]
pub struct PathRec {
    pub segs: Vec<Seg>,
    at: Option<(f64, f64)>,
    start: (f64, f64),
    /// The path is exactly one rounded rectangle (x, y, w, h, r).
    pub rect: Option<(f64, f64, f64, f64, f64)>,
    /// The path is exactly one full circle (cx, cy, r).
    pub circle: Option<(f64, f64, f64)>,
    shapes: usize,
}

impl PathRec {
    pub fn clear(&mut self) {
        *self = PathRec::default();
    }

    pub fn is_empty(&self) -> bool {
        self.segs.is_empty()
    }

    fn simple(&mut self) {
        self.shapes += 1;
        self.rect = None;
        self.circle = None;
    }

    pub fn move_to(&mut self, x: f64, y: f64) {
        self.simple();
        self.segs.push(Seg::Move(x, y));
        self.at = Some((x, y));
        self.start = (x, y);
    }

    pub fn line_to(&mut self, x: f64, y: f64) {
        if self.at.is_none() {
            return self.move_to(x, y);
        }
        self.simple();
        self.segs.push(Seg::Line(x, y));
        self.at = Some((x, y));
    }

    pub fn close(&mut self) {
        if self.at.is_some() {
            self.segs.push(Seg::Close);
            self.at = Some(self.start);
        }
    }

    pub fn arc(&mut self, cx: f64, cy: f64, r: f64, from: f64, to: f64) {
        let first = self.segs.is_empty();
        let sweep = {
            let d = to - from;
            if d >= 2.0 * PI {
                2.0 * PI
            } else {
                ((d % (2.0 * PI)) + 2.0 * PI) % (2.0 * PI)
            }
        };
        let (sx, sy) = (cx + r * from.cos(), cy + r * from.sin());
        self.line_to(sx, sy);
        self.segs.push(Seg::Arc { cx, cy, r, a0: from, a1: from + sweep });
        let end = from + sweep;
        self.at = Some((cx + r * end.cos(), cy + r * end.sin()));
        if first && (sweep - 2.0 * PI).abs() < 1e-9 {
            self.shapes = 1;
            self.circle = Some((cx, cy, r));
        }
    }

    pub fn arc_to(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, r: f64) {
        let Some((x0, y0)) = self.at else { return self.move_to(x1, y1) };
        let (ax, ay) = (x0 - x1, y0 - y1);
        let (bx, by) = (x2 - x1, y2 - y1);
        let (la, lb) = (ax.hypot(ay), bx.hypot(by));
        let cross = ax * by - ay * bx;
        // A corner that does not turn, or has no radius, is the point itself.
        if r == 0.0 || la == 0.0 || lb == 0.0 || cross.abs() <= 1e-9 * la * lb {
            return self.line_to(x1, y1);
        }
        let angle = ((ax * bx + ay * by) / (la * lb)).clamp(-1.0, 1.0).acos();
        let reach = r / (angle / 2.0).tan();
        let (t0x, t0y) = (x1 + ax / la * reach, y1 + ay / la * reach);
        let (t1x, t1y) = (x1 + bx / lb * reach, y1 + by / lb * reach);
        self.line_to(t0x, t0y);
        // The center is `r` from both tangent points, on the inside of the corner.
        let (bisx, bisy) = (ax / la + bx / lb, ay / la + by / lb);
        let bl = bisx.hypot(bisy);
        let d = r / (angle / 2.0).sin();
        let (cx, cy) = (x1 + bisx / bl * d, y1 + bisy / bl * d);
        let a0 = (t0y - cy).atan2(t0x - cx);
        let mut a1 = (t1y - cy).atan2(t1x - cx);
        // The short way round.
        while a1 - a0 > PI {
            a1 -= 2.0 * PI;
        }
        while a0 - a1 > PI {
            a1 += 2.0 * PI;
        }
        self.segs.push(Seg::Arc { cx, cy, r, a0, a1 });
        self.at = Some((t1x, t1y));
    }

    pub fn round_rect(&mut self, x: f64, y: f64, w: f64, h: f64, radius: f64) {
        let first = self.segs.is_empty();
        let r = radius.min(w / 2.0).min(h / 2.0).max(0.0);
        self.move_to(x + r, y);
        self.line_to(x + w - r, y);
        if r > 0.0 {
            self.segs.push(Seg::Arc { cx: x + w - r, cy: y + r, r, a0: -PI / 2.0, a1: 0.0 });
        }
        self.line_to(x + w, y + h - r);
        if r > 0.0 {
            self.segs.push(Seg::Arc { cx: x + w - r, cy: y + h - r, r, a0: 0.0, a1: PI / 2.0 });
        }
        self.line_to(x + r, y + h);
        if r > 0.0 {
            self.segs.push(Seg::Arc { cx: x + r, cy: y + h - r, r, a0: PI / 2.0, a1: PI });
        }
        self.line_to(x, y + r);
        if r > 0.0 {
            self.segs.push(Seg::Arc { cx: x + r, cy: y + r, r, a0: PI, a1: 1.5 * PI });
        }
        self.close();
        self.at = Some((x + r, y));
        if first {
            self.shapes = 1;
            self.rect = Some((x, y, w, h, r));
        }
    }

    /// The path as polylines (one per subpath, with whether it is closed), arcs
    /// in steps no further than `tolerance` from the curve.
    pub fn flatten(&self, tolerance: f64) -> Vec<(Vec<(f64, f64)>, bool)> {
        let mut out: Vec<(Vec<(f64, f64)>, bool)> = vec![];
        for s in &self.segs {
            match *s {
                Seg::Move(x, y) => out.push((vec![(x, y)], false)),
                Seg::Line(x, y) => {
                    if out.is_empty() {
                        out.push((vec![], false));
                    }
                    out.last_mut().unwrap().0.push((x, y));
                }
                Seg::Arc { cx, cy, r, a0, a1 } => {
                    if out.is_empty() {
                        out.push((vec![], false));
                    }
                    let step = if r > tolerance { 2.0 * (1.0 - tolerance / r).acos() } else { PI / 2.0 };
                    let n = (((a1 - a0).abs() / step.max(1e-3)).ceil() as usize).clamp(1, 256);
                    let line = &mut out.last_mut().unwrap().0;
                    for i in 1..=n {
                        let a = a0 + (a1 - a0) * i as f64 / n as f64;
                        line.push((cx + r * a.cos(), cy + r * a.sin()));
                    }
                }
                Seg::Close => {
                    if let Some(last) = out.last_mut() {
                        last.1 = true;
                        if let Some(&first) = last.0.first() {
                            let start = first;
                            out.push((vec![start], false));
                        }
                    }
                }
            }
        }
        out.retain(|(p, _)| p.len() >= 2);
        out
    }
}

/// A polyline cut into its dashes: `dash` is the canvas's pattern (an odd one
/// repeats twice), `offset` how far into it the line starts.
pub fn dashes(line: &[(f64, f64)], closed: bool, dash: &[f64], offset: f64) -> Vec<Vec<(f64, f64)>> {
    let pattern: Vec<f64> = if dash.len() % 2 == 1 { dash.iter().chain(dash).copied().collect() } else { dash.to_vec() };
    let total: f64 = pattern.iter().sum();
    let mut pts = line.to_vec();
    if closed && pts.first() != pts.last() {
        pts.push(pts[0]);
    }
    if pattern.is_empty() || total <= 0.0 || pattern.iter().any(|d| *d < 0.0) {
        return vec![pts];
    }
    // Where in the pattern the line starts.
    let mut i = 0;
    let mut left = pattern[0];
    let mut skip = offset.rem_euclid(total);
    while skip > 0.0 {
        if skip >= left {
            skip -= left;
            i = (i + 1) % pattern.len();
            left = pattern[i];
        } else {
            left -= skip;
            skip = 0.0;
        }
    }
    let mut out = vec![];
    let mut cur: Vec<(f64, f64)> = if i % 2 == 0 { vec![pts[0]] } else { vec![] };
    for w in pts.windows(2) {
        let ((ax, ay), (bx, by)) = (w[0], w[1]);
        let len = (bx - ax).hypot(by - ay);
        let mut t = 0.0;
        while len - t > 1e-9 {
            let step = left.min(len - t);
            t += step;
            left -= step;
            let p = (ax + (bx - ax) * t / len, ay + (by - ay) * t / len);
            if i % 2 == 0 {
                cur.push(p);
            }
            if left <= 1e-9 {
                if i % 2 == 0 && cur.len() >= 2 {
                    out.push(std::mem::take(&mut cur));
                }
                cur.clear();
                i = (i + 1) % pattern.len();
                left = pattern[i];
                if i % 2 == 0 {
                    cur.push(p);
                }
            }
        }
    }
    if i % 2 == 0 && cur.len() >= 2 {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arc_to_rounds_a_right_angle_with_its_center_inside() {
        let mut p = PathRec::default();
        p.move_to(0.0, 0.0);
        p.arc_to(10.0, 0.0, 10.0, 10.0, 4.0);
        p.line_to(10.0, 10.0);
        let Seg::Line(x, y) = p.segs[1] else { panic!("{:?}", p.segs) };
        assert!((x - 6.0).abs() < 1e-9 && y == 0.0);
        let Seg::Arc { cx, cy, r, a0, a1 } = p.segs[2] else { panic!("{:?}", p.segs) };
        assert!((cx - 6.0).abs() < 1e-9 && (cy - 4.0).abs() < 1e-9 && r == 4.0);
        assert!((a0 + PI / 2.0).abs() < 1e-9 && a1.abs() < 1e-9);
    }

    #[test]
    fn dashes_follow_the_pattern_and_its_offset() {
        let line = [(0.0, 0.0), (10.0, 0.0)];
        let d = dashes(&line, false, &[2.0, 3.0], 0.0);
        assert_eq!(d, [vec![(0.0, 0.0), (2.0, 0.0)], vec![(5.0, 0.0), (7.0, 0.0)]]);
        let d = dashes(&line, false, &[2.0, 3.0], 1.0);
        assert_eq!(d[0], vec![(0.0, 0.0), (1.0, 0.0)]);
        assert_eq!(d[1], vec![(4.0, 0.0), (6.0, 0.0)]);
    }

    #[test]
    fn simple_shapes_are_known() {
        let mut p = PathRec::default();
        p.round_rect(1.0, 2.0, 30.0, 10.0, 8.0);
        assert_eq!(p.rect, Some((1.0, 2.0, 30.0, 10.0, 5.0)));
        let mut c = PathRec::default();
        c.arc(5.0, 5.0, 3.0, 0.0, 2.0 * PI);
        assert_eq!(c.circle, Some((5.0, 5.0, 3.0)));
        c.line_to(1.0, 1.0);
        assert_eq!(c.circle, None);
    }
}
