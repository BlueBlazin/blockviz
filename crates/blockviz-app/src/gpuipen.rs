//! Draws as a canvas does, with GPUI: rounded rectangles and circles are the
//! GPU's own quads, other paths are tessellated, dashes are cut as a canvas
//! cuts them, and text is shaped by the window's text system.

use crate::pen::{dashes, Align, Family, Font, Pen, PathRec};
use crate::svgpen::screen_family;
use crate::theme::Color;
use gpui::{point, px, size, App, BorderStyle, Bounds, Corners, Edges, FontWeight, PathBuilder, Pixels, SharedString, TextRun, Window};
use lyon::tessellation::{LineJoin, StrokeOptions};

#[derive(Clone)]
struct State {
    fill: Color,
    stroke: Color,
    alpha: f64,
    line_width: f64,
    round_join: bool,
    dash_offset: f64,
    dash: Vec<f64>,
    font: Font,
    align: Align,
    k: f64,
    /// The rounded rectangle what is drawn is clipped to (x, y, w, h, r), in final coordinates.
    clip: Option<(f64, f64, f64, f64, f64)>,
}

pub struct GpuiPen<'w> {
    window: &'w mut Window,
    cx: &'w mut App,
    s: State,
    saved: Vec<State>,
    path: PathRec,
    /// Where the canvas is in the window.
    origin: (f64, f64),
}

/// Paths are flattened to within this many pixels of their curves.
const TOLERANCE: f64 = 0.15;

pub fn gpui_font(f: Family, weight: u16) -> gpui::Font {
    gpui::Font { weight: FontWeight(weight as f32), ..gpui::font(screen_family(f)) }
}

impl<'w> GpuiPen<'w> {
    pub fn new(window: &'w mut Window, cx: &'w mut App, origin: (f64, f64)) -> Self {
        GpuiPen {
            window,
            cx,
            s: State { fill: Color::hex(0), stroke: Color::hex(0), alpha: 1.0, line_width: 1.0, round_join: false, dash_offset: 0.0, dash: vec![], font: Font::default(), align: Align::Left, k: 1.0, clip: None },
            saved: vec![],
            path: PathRec::default(),
            origin,
        }
    }

    fn p(&self, x: f64, y: f64) -> gpui::Point<Pixels> {
        point(px((x + self.origin.0) as f32), px((y + self.origin.1) as f32))
    }

    fn bounds(&self, x: f64, y: f64, w: f64, h: f64) -> Bounds<Pixels> {
        Bounds::new(self.p(x, y), size(px(w as f32), px(h as f32)))
    }

    fn color(&self, c: Color) -> gpui::Hsla {
        c.with_alpha(self.s.alpha as f32).hsla()
    }

    fn quad(&mut self, b: Bounds<Pixels>, radii: [f64; 4], fill: gpui::Hsla, border: f64, border_color: gpui::Hsla) {
        let r = Corners { top_left: px(radii[0] as f32), top_right: px(radii[1] as f32), bottom_right: px(radii[2] as f32), bottom_left: px(radii[3] as f32) };
        self.window.paint_quad(gpui::quad(b, r, fill, Edges::all(px(border as f32)), border_color, BorderStyle::Solid));
    }

    fn polylines(&mut self, lines: &[(Vec<(f64, f64)>, bool)], fill: bool, color: gpui::Hsla) {
        let mut pb = if fill {
            PathBuilder::fill()
        } else {
            let mut o = StrokeOptions::default().with_line_width((self.s.line_width * self.s.k) as f32).with_tolerance(TOLERANCE as f32);
            o = o.with_line_join(if self.s.round_join { LineJoin::Round } else { LineJoin::MiterClip }).with_miter_limit(10.0);
            PathBuilder::stroke(px(0.0)).with_style(gpui::PathStyle::Stroke(o))
        };
        let mut any = false;
        for (line, closed) in lines {
            if line.len() < 2 {
                continue;
            }
            any = true;
            pb.move_to(self.p(line[0].0, line[0].1));
            for &(x, y) in &line[1..] {
                pb.line_to(self.p(x, y));
            }
            if *closed {
                pb.close();
            }
        }
        if any {
            if let Ok(path) = pb.build() {
                self.window.paint_path(path, color);
            }
        }
    }

    /// Shapes `text` in the current font at its size on screen.
    fn shape(&mut self, text: &str, color: gpui::Hsla) -> gpui::ShapedLine {
        let f = self.s.font;
        // Sizes in steps of a quarter pixel: a zoom shapes each name at a few sizes, not one per frame.
        let size = ((f.px * self.s.k) * 4.0).round() / 4.0;
        let run = TextRun { len: text.len(), font: gpui_font(f.family, f.weight), color, background_color: None, underline: None, strikethrough: None };
        self.window.text_system().shape_line(SharedString::from(text.to_string()), px(size as f32), &[run], None)
    }

    fn draw_text(&mut self, text: &str, x: f64, y: f64, color: gpui::Hsla, halo: f64) {
        if text.is_empty() || color.a <= 0.0 {
            return;
        }
        let line = self.shape(text, color);
        let width = f64::from(line.width);
        let (x, y) = (x * self.s.k, y * self.s.k);
        let left = match self.s.align {
            Align::Left => x,
            Align::Center => x - width / 2.0,
            Align::Right => x - width,
        };
        let ascent = f64::from(line.ascent);
        let height = line.ascent + line.descent;
        if halo > 0.0 {
            // An outline round the letters: the text drawn around itself.
            for i in 0..8 {
                let a = i as f64 * std::f64::consts::PI / 4.0;
                let o = self.p(left + halo * a.cos(), y - ascent + halo * a.sin());
                let _ = line.paint(o, height, self.window, self.cx);
            }
        } else {
            let o = self.p(left, y - ascent);
            let _ = line.paint(o, height, self.window, self.cx);
        }
    }
}

impl Pen for GpuiPen<'_> {
    fn set_fill(&mut self, c: Color) {
        self.s.fill = c;
    }
    fn set_stroke(&mut self, c: Color) {
        self.s.stroke = c;
    }
    fn set_alpha(&mut self, a: f64) {
        self.s.alpha = a.clamp(0.0, 1.0);
    }
    fn set_line_width(&mut self, w: f64) {
        self.s.line_width = w;
    }
    fn line_width(&self) -> f64 {
        self.s.line_width
    }
    fn set_round_join(&mut self, round: bool) {
        self.s.round_join = round;
    }
    fn set_dash_offset(&mut self, offset: f64) {
        self.s.dash_offset = offset;
    }
    fn set_dash(&mut self, segments: &[f64]) {
        self.s.dash = segments.to_vec();
    }
    fn set_font(&mut self, f: Font) {
        self.s.font = f;
    }
    fn set_align(&mut self, a: Align) {
        self.s.align = a;
    }
    fn save(&mut self) {
        self.saved.push(self.s.clone());
    }
    fn restore(&mut self) {
        if let Some(s) = self.saved.pop() {
            self.s = s;
        }
    }
    fn scale(&mut self, k: f64) {
        self.s.k *= k;
    }

    fn begin_path(&mut self) {
        self.path.clear();
    }
    fn move_to(&mut self, x: f64, y: f64) {
        let k = self.s.k;
        self.path.move_to(x * k, y * k);
    }
    fn line_to(&mut self, x: f64, y: f64) {
        let k = self.s.k;
        self.path.line_to(x * k, y * k);
    }
    fn arc(&mut self, x: f64, y: f64, r: f64, from: f64, to: f64) {
        let k = self.s.k;
        self.path.arc(x * k, y * k, r * k, from, to);
    }
    fn arc_to(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, r: f64) {
        let k = self.s.k;
        self.path.arc_to(x1 * k, y1 * k, x2 * k, y2 * k, r * k);
    }
    fn round_rect(&mut self, x: f64, y: f64, w: f64, h: f64, r: f64) {
        let k = self.s.k;
        self.path.round_rect(x * k, y * k, w * k, h * k, r * k);
    }
    fn close_path(&mut self) {
        self.path.close();
    }

    fn fill(&mut self) {
        let color = self.color(self.s.fill);
        if let Some((x, y, w, h, r)) = self.path.rect {
            let b = self.bounds(x, y, w, h);
            return self.quad(b, [r; 4], color, 0.0, color);
        }
        if let Some((cx, cy, r)) = self.path.circle {
            let b = self.bounds(cx - r, cy - r, 2.0 * r, 2.0 * r);
            return self.quad(b, [r; 4], color, 0.0, color);
        }
        let lines = self.path.flatten(TOLERANCE);
        self.polylines(&lines, true, color);
    }

    fn stroke(&mut self) {
        let color = self.color(self.s.stroke);
        let lw = self.s.line_width * self.s.k;
        if lw <= 0.0 || self.path.is_empty() {
            return;
        }
        if self.s.dash.is_empty() {
            // A stroke is centered on its path: the quad's border is drawn inside its bounds.
            let h = lw / 2.0;
            if let Some((x, y, w, ht, r)) = self.path.rect {
                let b = self.bounds(x - h, y - h, w + lw, ht + lw);
                return self.quad(b, [r + h; 4], gpui::transparent_black(), lw, color);
            }
            if let Some((cx, cy, r)) = self.path.circle {
                let b = self.bounds(cx - r - h, cy - r - h, 2.0 * r + lw, 2.0 * r + lw);
                return self.quad(b, [r + h; 4], gpui::transparent_black(), lw, color);
            }
            let lines = self.path.flatten(TOLERANCE);
            return self.polylines(&lines, false, color);
        }
        let k = self.s.k;
        let dash: Vec<f64> = self.s.dash.iter().map(|d| d * k).collect();
        let offset = self.s.dash_offset * k;
        let cut: Vec<(Vec<(f64, f64)>, bool)> = self.path.flatten(TOLERANCE).iter().flat_map(|(l, closed)| dashes(l, *closed, &dash, offset)).map(|l| (l, false)).collect();
        self.polylines(&cut, false, color);
    }

    fn clip(&mut self) {
        // The renderer clips only to a box's own outline, to keep its header band inside it.
        self.s.clip = self.path.rect;
    }

    fn fill_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        let k = self.s.k;
        let (mut x, mut y, mut w, mut h) = (x * k, y * k, w * k, h * k);
        if w < 0.0 {
            x += w;
            w = -w;
        }
        if h < 0.0 {
            y += h;
            h = -h;
        }
        let color = self.color(self.s.fill);
        let mut radii = [0.0; 4];
        if let Some((cx, cy, cw, ch, r)) = self.s.clip {
            // What of the rectangle is inside the clip, rounded where it reaches the clip's corners.
            let (x0, y0) = (x.max(cx), y.max(cy));
            let (x1, y1) = ((x + w).min(cx + cw), (y + h).min(cy + ch));
            if x1 <= x0 || y1 <= y0 {
                return;
            }
            let e = 0.01;
            let left = (x0 - cx).abs() < e;
            let right = (x1 - (cx + cw)).abs() < e;
            let top = (y0 - cy).abs() < e;
            let bottom = (y1 - (cy + ch)).abs() < e;
            radii = [if top && left { r } else { 0.0 }, if top && right { r } else { 0.0 }, if bottom && right { r } else { 0.0 }, if bottom && left { r } else { 0.0 }];
            (x, y, w, h) = (x0, y0, x1 - x0, y1 - y0);
        }
        let b = self.bounds(x, y, w, h);
        self.quad(b, radii, color, 0.0, color);
    }

    fn fill_text(&mut self, text: &str, x: f64, y: f64) {
        let color = self.color(self.s.fill);
        self.draw_text(text, x, y, color, 0.0);
    }

    fn stroke_text(&mut self, text: &str, x: f64, y: f64) {
        let color = self.color(self.s.stroke);
        let halo = self.s.line_width * self.s.k / 2.0;
        self.draw_text(text, x, y, color, halo);
    }

    fn measure(&mut self, text: &str) -> f64 {
        let f = self.s.font;
        let run = TextRun { len: text.len(), font: gpui_font(f.family, f.weight), color: gpui::black(), background_color: None, underline: None, strikethrough: None };
        f64::from(self.window.text_system().layout_line(text, px(f.px as f32), &[run], None).width)
    }
}
