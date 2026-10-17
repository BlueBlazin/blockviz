//! Lines with rounded corners cut to the screen, from src/canvas/geometry.ts.

use crate::camera::Rect;

/// A stretch of a line kept on screen: its points, the corner radius at each,
/// and how far along the whole line it starts (so a dash pattern stays in step).
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub pts: Vec<f64>,
    pub radii: Vec<f64>,
    pub at: f64,
}

/// One edge of a rectangle, for cutting a segment to it: the segment's points
/// at t with p·t ≤ q are on its inner side. Where they begin: −∞ if they do
/// from the start, ∞ if there are none.
fn enters(p: f64, q: f64) -> f64 {
    if p < 0.0 {
        q / p
    } else if p == 0.0 && q < 0.0 {
        f64::INFINITY
    } else {
        f64::NEG_INFINITY
    }
}

/// …and where they end: ∞ if they never do.
fn leaves(p: f64, q: f64) -> f64 {
    if p > 0.0 {
        q / p
    } else {
        f64::INFINITY
    }
}

/// How much shorter rounding corner k by radius r makes a line: the arc
/// replaces the two tangents into the corner.
fn corner_saving(pts: &[f64], k: usize, r: f64) -> f64 {
    let (ix, iy) = (pts[2 * k] - pts[2 * k - 2], pts[2 * k + 1] - pts[2 * k - 1]);
    let (ox, oy) = (pts[2 * k + 2] - pts[2 * k], pts[2 * k + 3] - pts[2 * k + 1]);
    let (li, lo) = (ix.hypot(iy), ox.hypot(oy));
    if r == 0.0 || li == 0.0 || lo == 0.0 {
        return 0.0;
    }
    let turn = ((ix * ox + iy * oy) / (li * lo)).clamp(-1.0, 1.0).acos();
    if turn > std::f64::consts::PI - 1e-6 {
        0.0
    } else {
        2.0 * r * (turn / 2.0).tan() - r * turn
    }
}

/// The parts of a line with rounded corners (`radii`, one per point; the ends'
/// are ignored) inside a rectangle. A line many screens long (an outer
/// diagram's arrow, seen from deep inside) is cut to the screen before it is
/// drawn. A run goes round a corner only when the corner is inside the
/// rectangle, so the rectangle should reach far enough past the screen to hide
/// corners cut off.
pub fn visible_runs(pts: &[f64], radii: &[f64], x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Run> {
    let mut runs = vec![];
    let mut run: Option<Run> = None;
    // Length of the drawn line up to point k, its corner there included.
    let mut along = 0.0;
    let n = pts.len() / 2;
    for k in 0..n.saturating_sub(1) {
        let (ax, ay) = (pts[2 * k], pts[2 * k + 1]);
        let (dx, dy) = (pts[2 * k + 2] - ax, pts[2 * k + 3] - ay);
        let len = dx.hypot(dy);
        if k > 0 {
            along -= corner_saving(pts, k, radii[k]);
        }
        // Liang–Barsky: the part t0..t1 of this segment inside the rectangle.
        let t0 = 0f64.max(enters(-dx, ax - x0)).max(enters(dx, x1 - ax)).max(enters(-dy, ay - y0)).max(enters(dy, y1 - ay));
        let t1 = 1f64.min(leaves(-dx, ax - x0)).min(leaves(dx, x1 - ax)).min(leaves(-dy, ay - y0)).min(leaves(dy, y1 - ay));
        if t0 > t1 {
            along += len;
            continue;
        }
        // A run open here ended at this point, inside: it carries on round the corner.
        let r = run.get_or_insert_with(|| Run { pts: vec![ax + dx * t0, ay + dy * t0], radii: vec![0.0], at: along + len * t0 });
        if t1 < 1.0 {
            r.pts.extend([ax + dx * t1, ay + dy * t1]);
            r.radii.push(0.0);
            runs.push(run.take().unwrap());
        } else {
            r.pts.extend([ax + dx, ay + dy]);
            r.radii.push(if k + 2 < n { radii[k + 1] } else { 0.0 });
        }
        along += len;
    }
    runs.extend(run);
    runs
}

/// A rounded rectangle as a line round it, starting where `roundRect` starts.
pub fn rect_line(b: Rect, radius: f64) -> (Vec<f64>, Vec<f64>) {
    let r = radius.min(b.w / 2.0).min(b.h / 2.0).max(0.0);
    let Rect { x, y, w, h } = b;
    (vec![x + r, y, x + w, y, x + w, y + h, x, y + h, x, y, x + r, y], vec![0.0, r, r, r, r, 0.0])
}

/// Squared distance from a point to a polyline (x0, y0, x1, y1, …).
pub fn distance_to_polyline(px: f64, py: f64, pts: &[f64]) -> f64 {
    let mut best = f64::INFINITY;
    let mut i = 0;
    while i + 3 < pts.len() {
        let (ax, ay, bx, by) = (pts[i], pts[i + 1], pts[i + 2], pts[i + 3]);
        let (dx, dy) = (bx - ax, by - ay);
        let len = dx * dx + dy * dy;
        let t = if len > 0.0 { (((px - ax) * dx + (py - ay) * dy) / len).clamp(0.0, 1.0) } else { 0.0 };
        let (x, y) = (ax + t * dx - px, ay + t * dy - py);
        best = best.min(x * x + y * y);
        i += 2;
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(pts: &[f64], radii: &[f64], at: f64) -> Run {
        Run { pts: pts.to_vec(), radii: radii.to_vec(), at }
    }

    #[test]
    fn keeps_a_line_on_screen_whole() {
        assert_eq!(visible_runs(&[10.0, 10.0, 50.0, 10.0, 50.0, 80.0], &[0.0, 6.0, 0.0], 0.0, 0.0, 100.0, 100.0), [run(&[10.0, 10.0, 50.0, 10.0, 50.0, 80.0], &[0.0, 6.0, 0.0], 0.0)]);
    }

    #[test]
    fn starts_a_long_line_where_it_comes_on_screen_as_far_along_as_it_is_there() {
        let runs = visible_runs(&[-1000.0, 0.0, 50.0, 0.0, 50.0, 300.0], &[0.0, 10.0, 0.0], 0.0, -10.0, 100.0, 200.0);
        assert_eq!(runs, [run(&[0.0, 0.0, 50.0, 0.0, 50.0, 200.0], &[0.0, 10.0, 0.0], 1000.0)]);
    }

    #[test]
    fn counts_the_corners_passed_off_screen_as_drawn_rounded() {
        let runs = visible_runs(&[-500.0, -500.0, -500.0, 0.0, 1000.0, 0.0], &[0.0, 10.0, 0.0], 0.0, -10.0, 100.0, 10.0);
        assert_eq!(runs[0].pts, [0.0, 0.0, 100.0, 0.0]);
        assert!((runs[0].at - (1000.0 - (20.0 - 5.0 * std::f64::consts::PI))).abs() < 1e-9);
    }

    #[test]
    fn leaves_the_screen_and_comes_back_as_two_runs() {
        let runs = visible_runs(&[10.0, 5.0, 200.0, 5.0, 200.0, 50.0, 10.0, 50.0], &[0.0; 4], 0.0, 0.0, 100.0, 100.0);
        assert_eq!(runs.iter().map(|r| r.pts.clone()).collect::<Vec<_>>(), [vec![10.0, 5.0, 100.0, 5.0], vec![100.0, 50.0, 10.0, 50.0]]);
        assert_eq!(runs[1].at, 190.0 + 45.0 + 100.0);
    }

    #[test]
    fn draws_nothing_of_a_line_off_screen_or_beside_it_and_all_of_one_along_its_edge() {
        assert!(visible_runs(&[200.0, 0.0, 300.0, 0.0, 300.0, 400.0], &[0.0, 10.0, 0.0], 0.0, 0.0, 100.0, 100.0).is_empty());
        assert!(visible_runs(&[-20.0, -5.0, 120.0, -5.0], &[0.0, 0.0], 0.0, 0.0, 100.0, 100.0).is_empty());
        assert_eq!(visible_runs(&[10.0, 0.0, 90.0, 0.0], &[0.0, 0.0], 0.0, 0.0, 100.0, 100.0), [run(&[10.0, 0.0, 90.0, 0.0], &[0.0, 0.0], 0.0)]);
    }

    #[test]
    fn goes_round_a_rectangle_as_round_rect_does() {
        let (pts, radii) = rect_line(Rect { x: 0.0, y: 0.0, w: 40.0, h: 10.0 }, 8.0);
        assert_eq!(pts, [5.0, 0.0, 40.0, 0.0, 40.0, 10.0, 0.0, 10.0, 0.0, 0.0, 5.0, 0.0]);
        assert_eq!(radii, [0.0, 5.0, 5.0, 5.0, 5.0, 0.0]);
    }
}
