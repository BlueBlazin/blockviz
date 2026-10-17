//! Visual language, ported from src/canvas/theme.ts (the diagram) and
//! src/styles.css (the panels and bars around it): pastel fills by role,
//! hairline outlines, gray regions for repeated or conditional structure, and
//! monochrome arrows. On a dark page the same, its lightness turned over.

use gpui::Hsla;

/// A color as the web app writes them: 8-bit channels and an opacity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: f32,
}

impl Color {
    pub const fn hex(v: u32) -> Self {
        Color { r: (v >> 16) as u8, g: (v >> 8) as u8, b: v as u8, a: 1.0 }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: f32) -> Self {
        Color { r, g, b, a }
    }

    pub fn with_alpha(self, a: f32) -> Self {
        Color { a: self.a * a, ..self }
    }

    pub fn hsla(self) -> Hsla {
        gpui::Rgba { r: self.r as f32 / 255.0, g: self.g as f32 / 255.0, b: self.b as f32 / 255.0, a: self.a }.into()
    }

    /// As CSS writes it, for SVG.
    pub fn css(self) -> String {
        if self.a >= 1.0 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!("rgba({}, {}, {}, {})", self.r, self.g, self.b, (self.a * 1000.0).round() / 1000.0)
        }
    }
}

impl From<Color> for Hsla {
    fn from(c: Color) -> Hsla {
        c.hsla()
    }
}

/// `a` moved toward `b` by `t`, rounded to whole channels as the canvas mixes.
pub fn mix(a: Color, b: Color, t: f64) -> Color {
    let m = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * t).round() as u8;
    Color { r: m(a.r, b.r), g: m(a.g, b.g), b: m(a.b, b.b), a: 1.0 }
}

#[derive(Clone, Copy, Debug)]
pub struct Swatch {
    pub fill: Color,
    pub tint: Color,
}

/// Colors of a change being read: outlines and text (`ink`), and the fill blocks are tinted with.
#[derive(Clone, Copy, Debug)]
pub struct ChangeColors {
    pub ink: Color,
    pub tint: Color,
}

/// The colors a diagram is drawn in.
#[derive(Clone, Debug)]
pub struct Theme {
    pub dark: bool,
    /// The page under the diagram.
    pub background: Color,
    /// What a block drawn as a blank card is filled with, and what text over a busy picture is backed with.
    pub paper: Color,
    pub ink: Color,
    pub muted: Color,
    pub faint: Color,
    pub outline: Color,
    /// The outline of an operation's box, lighter than a definition's.
    pub op_outline: Color,
    pub region_outline: Color,
    pub lane_outline: Color,
    pub edge: Color,
    pub context: Color,
    pub faint_edge: Color,
    pub artifact: Color,
    pub accent: Color,
    pub accent_soft: Color,
    pub label_halo: Color,
    pub label_ink: Color,
    pub effect: Color,
    /// A table's headings, and their lines while too small to read.
    pub heading: Color,
    pub heading_texture: Color,
    swatches: Vec<(&'static str, Swatch)>,
    pub added: ChangeColors,
    pub changed: ChangeColors,
    pub removed: ChangeColors,
}

const SWATCHES: [(&str, u32, u32); 29] = [
    // Neural-network vocabulary (matches the colors of the Transformer figure).
    ("attention", 0xfbdcbc, 0xfef6ee),
    ("mlp", 0xc6e5f5, 0xf1f9fd),
    ("norm", 0xeff3c4, 0xfbfcef),
    ("embed", 0xf6d2d6, 0xfdf3f4),
    ("linear", 0xd9daf3, 0xf5f5fc),
    ("activation", 0xcfe9d0, 0xf2faf2),
    ("dropout", 0xe6e6e4, 0xf8f8f7),
    ("loss", 0xf3cdd5, 0xfdf2f4),
    // General code roles.
    ("component", 0xf9e2cb, 0xfef8f2),
    ("function", 0xd6e7fa, 0xf4f8fe),
    ("endpoint", 0xe6d9f7, 0xf8f4fd),
    ("class", 0xdde0f8, 0xf6f7fe),
    ("library", 0xe9e9f1, 0xf8f8fb),
    ("io", 0xcdeadf, 0xf1faf6),
    ("config", 0xf7edc8, 0xfdfaee),
    ("data", 0xf7d6de, 0xfdf3f5),
    ("op", 0xffffff, 0xffffff),
    ("error", 0xf9d6d3, 0xfef3f2),
    ("kernel", 0xcdeadf, 0xf1faf6),
    ("section", 0xe7ebf1, 0xf9fafc),
    ("file", 0xe4e9f0, 0xfbfcfd),
    ("dir", 0xeceef1, 0xfbfbfc),
    ("doc", 0xefefea, 0xfbfbf9),
    ("asset", 0xefefea, 0xfbfbf9),
    ("meta", 0xf1f1f1, 0xfbfbfb),
    ("region", 0xf1f1ef, 0xf1f1ef),
    ("lane", 0xfafaf9, 0xfafaf9),
    ("repo", 0xffffff, 0xffffff),
    ("neutral", 0xeef0f3, 0xfafbfc),
];

/// The dark page's lightness: keep in step with `--bg` in styles.css.
const PAGE_L: f64 = 0.098;

/// A light theme's color on the dark page: the same hue, as far above the
/// page in lightness as it was below white, and about as colorful (its
/// chroma, not its saturation, which near white overstates a faint tint).
/// White is the page itself.
fn night(c: Color) -> Color {
    let (r, g, b) = (c.r as f64 / 255.0, c.g as f64 / 255.0, c.b as f64 / 255.0);
    let max = r.max(g).max(b);
    let chroma = max - r.min(g).min(b);
    let h = if chroma == 0.0 {
        0.0
    } else if max == r {
        ((g - b) / chroma + 6.0) % 6.0
    } else if max == g {
        (b - r) / chroma + 2.0
    } else {
        (r - g) / chroma + 4.0
    };
    // A gray is the page's own gray: its hue and chroma.
    let (hue, ch) = if chroma != 0.0 { (h, chroma * 0.75) } else { (11.0 / 3.0, 6.0 / 255.0) };
    rgb(hue, ch, PAGE_L + (1.0 - (max - chroma / 2.0)) * 1.25)
}

/// The color of hue `h` (0 to 6), chroma `c` and lightness `l`.
fn rgb(h: f64, c: f64, l: f64) -> Color {
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let (r, g, b) = if h < 1.0 {
        (c, x, 0.0)
    } else if h < 2.0 {
        (x, c, 0.0)
    } else if h < 3.0 {
        (0.0, c, x)
    } else if h < 4.0 {
        (0.0, x, c)
    } else if h < 5.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    let v = |v: f64| ((v + l - c / 2.0) * 255.0).round().clamp(0.0, 255.0) as u8;
    Color { r: v(r), g: v(g), b: v(b), a: 1.0 }
}

impl Theme {
    pub fn light() -> Self {
        let h = Color::hex;
        Theme {
            dark: false,
            background: h(0xffffff),
            paper: h(0xffffff),
            ink: h(0x1b1e23),
            muted: h(0x5b626d),
            faint: h(0x8b929c),
            outline: h(0x2c3036),
            op_outline: h(0x7d838c),
            region_outline: h(0xb9bdc3),
            lane_outline: h(0xcbcfd4),
            edge: h(0x2c3036),
            context: h(0xa3aab4),
            faint_edge: h(0x7c838e),
            artifact: h(0x9a5566),
            accent: h(0x2563eb),
            accent_soft: Color::rgba(37, 99, 235, 0.14),
            label_halo: h(0xffffff),
            label_ink: h(0x3d434c),
            effect: h(0xd97706),
            heading: h(0x8a6d1f),
            heading_texture: h(0xb39b52),
            swatches: SWATCHES.iter().map(|&(k, f, t)| (k, Swatch { fill: h(f), tint: h(t) })).collect(),
            added: ChangeColors { ink: h(0x1a7f37), tint: h(0xc9f2d4) },
            changed: ChangeColors { ink: h(0xbf8700), tint: h(0xfbecb3) },
            removed: ChangeColors { ink: h(0xcf222e), tint: h(0xffd8d6) },
        }
    }

    pub fn dark() -> Self {
        let h = Color::hex;
        let page = h(0x16181c);
        Theme {
            dark: true,
            background: page,
            paper: page,
            ink: h(0xe6e8eb),
            muted: h(0xa3a9b3),
            faint: h(0x7c838d),
            outline: h(0x9aa2ad),
            op_outline: h(0x6b727c),
            region_outline: h(0x454a53),
            lane_outline: h(0x3b4048),
            edge: h(0xb4bbc5),
            context: h(0x646b75),
            faint_edge: h(0x8a919b),
            artifact: h(0xd08a9b),
            accent: h(0x5b9bff),
            accent_soft: Color::rgba(91, 155, 255, 0.24),
            label_halo: page,
            label_ink: h(0xc3c8cf),
            effect: h(0xf59e0b),
            heading: h(0xd1ae55),
            heading_texture: h(0x8c7a45),
            swatches: SWATCHES.iter().map(|&(k, f, t)| (k, Swatch { fill: night(h(f)), tint: night(h(t)) })).collect(),
            added: ChangeColors { ink: h(0x3fb950), tint: h(0x1e4a2b) },
            changed: ChangeColors { ink: h(0xd29922), tint: h(0x4a3b12) },
            removed: ChangeColors { ink: h(0xf85149), tint: h(0x552321) },
        }
    }

    pub fn swatch(&self, category: &str) -> Swatch {
        self.swatches.iter().find(|(k, _)| *k == category).or_else(|| self.swatches.iter().find(|(k, _)| *k == "neutral")).unwrap().1
    }

    pub fn change(&self, s: crate::render::Mark) -> ChangeColors {
        match s {
            crate::render::Mark::Added => self.added,
            crate::render::Mark::Changed => self.changed,
            crate::render::Mark::Removed => self.removed,
        }
    }
}

/// Categories whose names carry meaning in a legend.
pub const LEGEND: [(&str, &str); 14] = [
    ("function", "Function / method"),
    ("endpoint", "Route, command or task"),
    ("class", "Class"),
    ("component", "Sub-module"),
    ("library", "Library call"),
    ("io", "I/O & side effects"),
    ("config", "Settings"),
    ("data", "File on disk"),
    ("attention", "Attention"),
    ("mlp", "Feed-forward"),
    ("norm", "Normalization"),
    ("embed", "Embedding"),
    ("linear", "Linear / projection"),
    ("activation", "Activation"),
];

/// The colors of the panels and bars around the diagram (src/styles.css).
#[derive(Clone, Debug)]
pub struct Ui {
    pub dark: bool,
    pub bg: Hsla,
    pub glass: Hsla,
    pub ink: Hsla,
    pub ink_strong: Hsla,
    pub muted: Hsla,
    pub faint: Hsla,
    pub line: Hsla,
    pub line_strong: Hsla,
    pub panel: Hsla,
    pub hover: Hsla,
    pub accent: Hsla,
    pub accent_bg: Hsla,
    pub accent_bg_strong: Hsla,
    pub chip: Hsla,
    pub chip_ink: Hsla,
    pub on_strong: Hsla,
    pub heading: Hsla,
    pub code_bg: Hsla,
    pub code_ink: Hsla,
    pub code_no: Hsla,
    pub code_hover: Hsla,
    pub code_marked: Hsla,
    pub code_gap: Hsla,
    pub scrim: Hsla,
    pub error_bg: Hsla,
    pub error_line: Hsla,
    pub error_ink: Hsla,
    pub logo: [Hsla; 3],
    pub added: Hsla,
    pub added_bg: Hsla,
    pub changed: Hsla,
    pub changed_strong: Hsla,
    pub changed_bg: Hsla,
    pub removed: Hsla,
    pub removed_bg: Hsla,
    pub renamed: Hsla,
    shadow_colors: [Hsla; 3],
}

impl Ui {
    pub fn new(dark: bool) -> Self {
        let p = |light: u32, night: u32| Color::hex(if dark { night } else { light }).hsla();
        let a = |light: Color, night: Color| (if dark { night } else { light }).hsla();
        Ui {
            dark,
            bg: p(0xffffff, 0x16181c),
            glass: a(Color::rgba(255, 255, 255, 0.92), Color::rgba(22, 24, 28, 0.9)),
            ink: p(0x17191d, 0xe6e8eb),
            ink_strong: p(0x000000, 0xffffff),
            muted: p(0x5d636e, 0xa3a9b3),
            faint: p(0x8d939d, 0x7c838d),
            line: p(0xe4e6ea, 0x2d3138),
            line_strong: p(0xd3d6db, 0x3d424a),
            panel: p(0xffffff, 0x1d2025),
            hover: p(0xf3f4f6, 0x272b31),
            accent: p(0x2563eb, 0x5b9bff),
            accent_bg: p(0xeef3fe, 0x1b2a45),
            accent_bg_strong: p(0xe1eafd, 0x22365a),
            chip: p(0xf3f4f6, 0x272b31),
            chip_ink: p(0x343840, 0xc9cdd3),
            on_strong: p(0xffffff, 0x101215),
            heading: p(0x8a6d1f, 0xd1ae55),
            code_bg: p(0xf8f9fa, 0x181b1f),
            code_ink: p(0x1f2328, 0xe6edf3),
            code_no: p(0xa3a9b2, 0x5c636d),
            code_hover: p(0xeef1f5, 0x22262c),
            code_marked: p(0xe3ebfd, 0x1b2a45),
            code_gap: p(0xf0f2f5, 0x1f2328),
            scrim: a(Color::rgba(250, 250, 251, 0.6), Color::rgba(10, 11, 13, 0.55)),
            error_bg: p(0xfff4f2, 0x3a1d1b),
            error_line: p(0xf3cbc5, 0x6b2b26),
            error_ink: p(0x8a2a1d, 0xffb4a9),
            logo: [p(0xfbdcbc, 0x5d462e), p(0xc6e5f5, 0x314854), p(0xeff3c4, 0x545734)],
            added: p(0x1a7f37, 0x3fb950),
            added_bg: a(Color::hex(0xe6ffec), Color::rgba(46, 160, 67, 0.18)),
            changed: p(0x9a6700, 0xd29922),
            changed_strong: p(0xbf8700, 0xbb8009),
            changed_bg: a(Color::hex(0xfff8c5), Color::rgba(187, 128, 9, 0.2)),
            removed: p(0xcf222e, 0xf85149),
            removed_bg: a(Color::hex(0xffebe9), Color::rgba(248, 81, 73, 0.17)),
            renamed: p(0x8250df, 0xa371f7),
            shadow_colors: [
                a(Color::rgba(16, 24, 40, 0.04), Color::rgba(0, 0, 0, 0.3)),
                a(Color::rgba(16, 24, 40, 0.07), Color::rgba(0, 0, 0, 0.35)),
                a(Color::rgba(16, 24, 40, 0.18), Color::rgba(0, 0, 0, 0.55)),
            ],
        }
    }

    /// The color of a syntax class from the core's highlighter (`highlight::Hl`), as styles.css colors `.t1`…`.t21`.
    pub fn syntax(&self, class: u32) -> Option<Hsla> {
        let (light, night) = match class {
            1 | 13 => (0xcf222e, 0xff7b72),
            2 | 20 => (0x0a3069, 0xa5d6ff),
            3 => (0x6e7781, 0x8b949e),
            4 | 5 | 6 | 12 | 14 | 16 | 17 => (0x0550ae, 0x79c0ff),
            7 | 8 | 10 => (0x8250df, 0xd2a8ff),
            9 => (0x953800, 0xffa657),
            11 => (0x1f2328, 0xe6edf3),
            15 => (0x116329, 0x7ee787),
            19 => (0x0969da, 0x58a6ff),
            21 => (0x6639ba, 0xd2a8ff),
            _ => return None,
        };
        Some(Color::hex(if self.dark { night } else { light }).hsla())
    }

    /// The `--shadow` of a floating panel.
    pub fn shadow(&self) -> Vec<gpui::BoxShadow> {
        use gpui::{point, px, BoxShadow};
        vec![
            BoxShadow { color: self.shadow_colors[0], offset: point(px(0.0), px(1.0)), blur_radius: px(2.0), spread_radius: px(0.0) },
            BoxShadow { color: self.shadow_colors[1], offset: point(px(0.0), px(6.0)), blur_radius: px(20.0), spread_radius: px(0.0) },
        ]
    }

    /// The `--shadow-large` of a dialog.
    pub fn shadow_large(&self) -> Vec<gpui::BoxShadow> {
        use gpui::{point, px, BoxShadow};
        vec![BoxShadow { color: self.shadow_colors[2], offset: point(px(0.0), px(24.0)), blur_radius: px(60.0), spread_radius: px(0.0) }]
    }
}

/// The interface font: `-apple-system`, as the web asks for it.
pub const UI_FONT: &str = ".SystemUIFont";
/// The code font: the web asks for "SF Mono", ui-monospace; GPUI reaches the
/// system's monospace by this name (see `fonts_resolve`).
pub const MONO_FONT: &str = "Menlo";
/// The diagram's names (theme.ts).
pub const SANS: &str = "Helvetica Neue";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn night_colors_match_the_web() {
        // As theme.ts's night() computes them: white is the page itself.
        assert_eq!(night(Color::hex(0xffffff)).css(), "#16181c");
        assert_eq!(mix(Color::hex(0x000000), Color::hex(0xffffff), 0.5).css(), "#808080");
        assert_eq!(Theme::dark().swatch("nothing").fill.css(), night(Color::hex(0xeef0f3)).css());
    }
}
