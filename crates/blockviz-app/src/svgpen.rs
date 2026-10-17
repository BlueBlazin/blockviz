//! Draws as the canvas pen does, into an SVG (src/canvas/svg.ts): a diagram is
//! exported as it is drawn on screen. Its transforms are scales, the same
//! across as down (the renderer scales text).

use crate::pen::{Align, Family, Font, Pen};
use crate::theme::{Color, MONO_FONT, SANS};
use std::f64::consts::PI;

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
    clips: usize,
}

pub struct SvgPen<'a> {
    s: State,
    saved: Vec<State>,
    /// The path being built, in SVG's syntax and the SVG's pixels.
    d: String,
    at: Option<(f64, f64)>,
    start: (f64, f64),
    out: Vec<String>,
    ids: usize,
    measurer: Box<dyn FnMut(&str, Font) -> f64 + 'a>,
}

/// A number as written into the SVG: to a hundredth of a pixel.
fn num(v: f64) -> String {
    let r = (v * 100.0).round() / 100.0;
    if r == r.trunc() {
        format!("{}", r as i64)
    } else {
        format!("{r}")
    }
}

/// Text as XML takes it: its markup escaped, and without the control characters XML cannot hold.
fn escape(s: &str) -> String {
    s.chars()
        .filter(|&c| !matches!(c, '\0'..='\x08' | '\x0b' | '\x0c' | '\x0e'..='\x1f'))
        .map(|c| match c {
            '&' => "&amp;".to_string(),
            '<' => "&lt;".to_string(),
            '>' => "&gt;".to_string(),
            '"' => "&quot;".to_string(),
            c => c.to_string(),
        })
        .collect()
}

/// The CSS font family of a pen family, as the web writes it.
pub fn css_family(f: Family) -> &'static str {
    match f {
        Family::Sans => "\"Helvetica Neue\", Helvetica, -apple-system, \"Segoe UI\", Arial, sans-serif",
        Family::Mono => "\"SF Mono\", ui-monospace, Menlo, Consolas, monospace",
    }
}

impl<'a> SvgPen<'a> {
    /// `measurer` measures text in a font: the window's text system in the app.
    pub fn new(measurer: impl FnMut(&str, Font) -> f64 + 'a) -> Self {
        SvgPen {
            s: State { fill: Color::hex(0), stroke: Color::hex(0), alpha: 1.0, line_width: 1.0, round_join: false, dash_offset: 0.0, dash: vec![], font: Font::default(), align: Align::Left, k: 1.0, clips: 0 },
            saved: vec![],
            d: String::new(),
            at: None,
            start: (0.0, 0.0),
            out: vec![],
            ids: 0,
            measurer: Box::new(measurer),
        }
    }

    /// Measures by estimate, as the core's layout does: for drawing without a window.
    pub fn estimating() -> SvgPen<'static> {
        SvgPen::new(|text: &str, f: Font| match f.family {
            Family::Sans => blockviz_core::text::sans_width(text, f.px, f.weight >= 600),
            Family::Mono => blockviz_core::text::mono_width(text, f.px),
        })
    }

    /// The drawing as an SVG document of `width` × `height` pixels.
    pub fn svg(&self, width: f64, height: f64) -> String {
        let (w, h) = (num(width), num(height));
        format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">\n{}\n</svg>\n", self.out.join("\n"))
    }

    fn mv(&mut self, x: f64, y: f64) {
        self.d += &format!("M{} {}", num(x), num(y));
        self.at = Some((x, y));
        self.start = (x, y);
    }

    fn ln(&mut self, x: f64, y: f64) {
        if self.at.is_none() {
            return self.mv(x, y);
        }
        self.d += &format!("L{} {}", num(x), num(y));
        self.at = Some((x, y));
    }

    fn strokes(&self) -> String {
        let k = self.s.k;
        let mut s = format!(" stroke=\"{}\" stroke-width=\"{}\"", self.s.stroke.css(), num(self.s.line_width * k));
        if self.s.round_join {
            s += " stroke-linejoin=\"round\"";
        }
        if !self.s.dash.is_empty() {
            s += &format!(" stroke-dasharray=\"{}\"", self.s.dash.iter().map(|v| num(v * k)).collect::<Vec<_>>().join(" "));
            if self.s.dash_offset != 0.0 {
                s += &format!(" stroke-dashoffset=\"{}\"", num(self.s.dash_offset * k));
            }
        }
        s
    }

    fn alpha(&self) -> String {
        if self.s.alpha < 1.0 {
            format!(" opacity=\"{}\"", (self.s.alpha * 1000.0).round() / 1000.0)
        } else {
            String::new()
        }
    }

    fn text(&mut self, text: &str, x: f64, y: f64, paint: String) {
        let anchor = match self.s.align {
            Align::Center => " text-anchor=\"middle\"",
            Align::Right => " text-anchor=\"end\"",
            Align::Left => "",
        };
        let f = self.s.font;
        let weight = if f.weight == 400 { String::new() } else { format!(" font-weight=\"{}\"", f.weight) };
        // Spaces SVG would collapse are kept (` = value` after a table's key).
        let keep = if text.starts_with(char::is_whitespace) || text.ends_with(char::is_whitespace) || text.contains("  ") { " xml:space=\"preserve\"" } else { "" };
        let k = self.s.k;
        self.out.push(format!(
            "<text x=\"{}\" y=\"{}\" font-family=\"{}\" font-size=\"{}\"{weight}{anchor}{paint}{}{keep}>{}</text>",
            num(x * k),
            num(y * k),
            escape(css_family(f.family)),
            num(f.px * k),
            self.alpha(),
            escape(text)
        ));
    }
}

impl Pen for SvgPen<'_> {
    fn set_fill(&mut self, c: Color) {
        self.s.fill = c;
    }
    fn set_stroke(&mut self, c: Color) {
        self.s.stroke = c;
    }
    fn set_alpha(&mut self, a: f64) {
        self.s.alpha = a;
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
        let mut s = self.s.clone();
        s.clips = self.s.clips;
        self.saved.push(s);
        self.s.clips = 0;
    }
    fn restore(&mut self) {
        let Some(s) = self.saved.pop() else { return };
        if self.s.clips > 0 {
            self.out.push("</g>".repeat(self.s.clips));
        }
        self.s = s;
    }
    fn scale(&mut self, k: f64) {
        self.s.k *= k;
    }

    fn begin_path(&mut self) {
        self.d.clear();
        self.at = None;
    }
    fn move_to(&mut self, x: f64, y: f64) {
        self.mv(x * self.s.k, y * self.s.k);
    }
    fn line_to(&mut self, x: f64, y: f64) {
        self.ln(x * self.s.k, y * self.s.k);
    }
    fn close_path(&mut self) {
        if self.at.is_none() {
            return;
        }
        self.d.push('Z');
        self.at = Some(self.start);
    }
    fn arc(&mut self, x: f64, y: f64, radius: f64, from: f64, to: f64) {
        let k = self.s.k;
        let r = radius * k;
        let turn = 2.0 * PI;
        let d = to - from;
        let sweep = if d >= turn { turn } else { ((d % turn) + turn) % turn };
        let point = |a: f64| (x * k + r * a.cos(), y * k + r * a.sin());
        let (sx, sy) = point(from);
        self.ln(sx, sy);
        // In halves at most: one SVG arc cannot end where it starts.
        let ends: Vec<f64> = if sweep > PI { vec![from + sweep / 2.0, from + sweep] } else { vec![from + sweep] };
        for a in ends {
            let p = point(a);
            self.at = Some(p);
            self.d += &format!("A{} {} 0 0 1 {} {}", num(r), num(r), num(p.0), num(p.1));
        }
    }
    fn arc_to(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, radius: f64) {
        let k = self.s.k;
        let (px, py) = (x1 * k, y1 * k);
        let r = radius * k;
        let Some(at) = self.at else { return self.mv(px, py) };
        let (ax, ay) = (at.0 - px, at.1 - py);
        let (bx, by) = (x2 * k - px, y2 * k - py);
        let (la, lb) = (ax.hypot(ay), bx.hypot(by));
        let cross = ax * by - ay * bx;
        if r == 0.0 || la == 0.0 || lb == 0.0 || cross.abs() <= 1e-9 * la * lb {
            return self.ln(px, py);
        }
        let angle = ((ax * bx + ay * by) / (la * lb)).clamp(-1.0, 1.0).acos();
        let reach = r / (angle / 2.0).tan();
        self.ln(px + ax / la * reach, py + ay / la * reach);
        let end = (px + bx / lb * reach, py + by / lb * reach);
        self.at = Some(end);
        self.d += &format!("A{} {} 0 0 {} {} {}", num(r), num(r), if cross < 0.0 { 1 } else { 0 }, num(end.0), num(end.1));
    }
    fn round_rect(&mut self, x: f64, y: f64, w: f64, h: f64, radius: f64) {
        let k = self.s.k;
        let (x, y, w, h) = (x * k, y * k, w * k, h * k);
        let r = (radius * k).min(w / 2.0).min(h / 2.0).max(0.0);
        let corner = |ex: f64, ey: f64| if r > 0.0 { format!("A{} {} 0 0 1 {} {}", num(r), num(r), num(ex), num(ey)) } else { String::new() };
        self.d += &format!(
            "M{} {}H{}{}V{}{}H{}{}V{}{}Z",
            num(x + r),
            num(y),
            num(x + w - r),
            corner(x + w, y + r),
            num(y + h - r),
            corner(x + w - r, y + h),
            num(x + r),
            corner(x, y + h - r),
            num(y + r),
            corner(x + r, y)
        );
        self.at = Some((x + r, y));
        self.start = (x + r, y);
    }
    fn fill(&mut self) {
        if !self.d.is_empty() {
            self.out.push(format!("<path d=\"{}\" fill=\"{}\"{}/>", self.d, self.s.fill.css(), self.alpha()));
        }
    }
    fn stroke(&mut self) {
        if !self.d.is_empty() {
            self.out.push(format!("<path d=\"{}\" fill=\"none\"{}{}/>", self.d, self.strokes(), self.alpha()));
        }
    }
    fn clip(&mut self) {
        self.ids += 1;
        self.out.push(format!("<clipPath id=\"clip{0}\"><path d=\"{1}\"/></clipPath><g clip-path=\"url(#clip{0})\">", self.ids, self.d));
        self.s.clips += 1;
    }
    fn fill_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        let k = self.s.k;
        // A canvas fills a rectangle of negative size toward its other side.
        let (x, w) = if w < 0.0 { (x + w, -w) } else { (x, w) };
        let (y, h) = if h < 0.0 { (y + h, -h) } else { (y, h) };
        self.out.push(format!("<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\"{}/>", num(x * k), num(y * k), num(w * k), num(h * k), self.s.fill.css(), self.alpha()));
    }

    fn fill_text(&mut self, text: &str, x: f64, y: f64) {
        let paint = format!(" fill=\"{}\"", self.s.fill.css());
        self.text(text, x, y, paint);
    }
    fn stroke_text(&mut self, text: &str, x: f64, y: f64) {
        let paint = format!(" fill=\"none\"{}", self.strokes());
        self.text(text, x, y, paint);
    }
    fn measure(&mut self, text: &str) -> f64 {
        (self.measurer)(text, self.s.font)
    }
}

/// The font a pen family is set in on screen.
pub fn screen_family(f: Family) -> &'static str {
    match f {
        Family::Sans => SANS,
        Family::Mono => MONO_FONT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_paths_and_text_as_the_canvas_draws_them() {
        let mut p = SvgPen::estimating();
        p.set_fill(Color::hex(0xff0000));
        p.begin_path();
        p.round_rect(0.0, 0.0, 40.0, 10.0, 8.0);
        p.fill();
        p.set_stroke(Color::hex(0x0000ff));
        p.set_dash(&[5.0, 3.0]);
        p.set_alpha(0.5);
        p.begin_path();
        p.move_to(0.0, 0.0);
        p.arc_to(10.0, 0.0, 10.0, 10.0, 4.0);
        p.stroke();
        p.set_align(Align::Center);
        p.save();
        p.scale(2.0);
        p.fill_text("a < b", 5.0, 5.0);
        p.restore();
        let svg = p.svg(100.0, 50.0);
        assert!(svg.contains("<path d=\"M5 0H35A5 5 0 0 1 40 5V5A5 5 0 0 1 35 10H5A5 5 0 0 1 0 5V5A5 5 0 0 1 5 0Z\" fill=\"#ff0000\"/>"), "{svg}");
        assert!(svg.contains("M0 0L6 0A4 4 0 0 1 10 4"), "{svg}");
        assert!(svg.contains("stroke-dasharray=\"5 3\""));
        assert!(svg.contains("x=\"10\" y=\"10\""));
        assert!(svg.contains("text-anchor=\"middle\""));
        assert!(svg.contains("a &lt; b"));
    }
}
