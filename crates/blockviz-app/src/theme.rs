//! The light theme of src/canvas/theme.ts: pastel fills by role, hairline
//! outlines, gray regions.

use gpui::{rgb, Hsla};

pub struct Swatch {
    pub fill: Hsla,
    pub tint: Hsla,
}

fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

pub fn swatch(category: &str) -> Swatch {
    let (fill, tint) = match category {
        "attention" => (0xfbdcbc, 0xfef6ee),
        "mlp" => (0xc6e5f5, 0xf1f9fd),
        "norm" => (0xeff3c4, 0xfbfcef),
        "embed" => (0xf6d2d6, 0xfdf3f4),
        "linear" => (0xd9daf3, 0xf5f5fc),
        "activation" => (0xcfe9d0, 0xf2faf2),
        "dropout" => (0xe6e6e4, 0xf8f8f7),
        "loss" => (0xf3cdd5, 0xfdf2f4),
        "component" => (0xf9e2cb, 0xfef8f2),
        "function" => (0xd6e7fa, 0xf4f8fe),
        "endpoint" => (0xe6d9f7, 0xf8f4fd),
        "class" => (0xdde0f8, 0xf6f7fe),
        "library" => (0xe9e9f1, 0xf8f8fb),
        "io" | "kernel" => (0xcdeadf, 0xf1faf6),
        "config" => (0xf7edc8, 0xfdfaee),
        "data" => (0xf7d6de, 0xfdf3f5),
        "op" | "repo" => (0xffffff, 0xffffff),
        "error" => (0xf9d6d3, 0xfef3f2),
        "section" => (0xe7ebf1, 0xf9fafc),
        "file" => (0xe4e9f0, 0xfbfcfd),
        "dir" => (0xeceef1, 0xfbfbfc),
        "doc" | "asset" => (0xefefea, 0xfbfbf9),
        "meta" => (0xf1f1f1, 0xfbfbfb),
        "region" => (0xf1f1ef, 0xf1f1ef),
        "lane" => (0xfafaf9, 0xfafaf9),
        _ => (0xeef0f3, 0xfafbfc),
    };
    Swatch { fill: c(fill), tint: c(tint) }
}

pub struct Theme {
    pub background: Hsla,
    pub paper: Hsla,
    pub ink: Hsla,
    pub muted: Hsla,
    pub outline: Hsla,
    pub op_outline: Hsla,
    pub region_outline: Hsla,
    pub lane_outline: Hsla,
    pub edge: Hsla,
    pub artifact: Hsla,
    pub label_ink: Hsla,
    pub accent: Hsla,
}

pub fn light() -> Theme {
    Theme {
        background: c(0xffffff),
        paper: c(0xffffff),
        ink: c(0x1b1e23),
        muted: c(0x5b626d),
        outline: c(0x2c3036),
        op_outline: c(0x7d838c),
        region_outline: c(0xb9bdc3),
        lane_outline: c(0xcbcfd4),
        edge: c(0x2c3036),
        artifact: c(0x9a5566),
        label_ink: c(0x3d434c),
        accent: c(0x2563eb),
    }
}

/// `a` moved toward `b` by `t`, in RGB as the canvas mixes.
pub fn mix(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b) = (a.to_rgb(), b.to_rgb());
    gpui::Rgba { r: a.r + (b.r - a.r) * t, g: a.g + (b.g - a.g) * t, b: a.b + (b.b - a.b) * t, a: 1.0 }.into()
}

/// The colors of the panels and bars around the diagram (src/styles.css, light).
pub struct Ui {
    pub ink: Hsla,
    pub muted: Hsla,
    pub faint: Hsla,
    pub line: Hsla,
    pub line_strong: Hsla,
    pub panel: Hsla,
    pub glass: Hsla,
    pub hover: Hsla,
    pub accent: Hsla,
    pub accent_bg: Hsla,
    pub chip: Hsla,
    pub chip_ink: Hsla,
    pub code_bg: Hsla,
    pub code_ink: Hsla,
    pub code_no: Hsla,
    pub code_hover: Hsla,
    pub code_marked: Hsla,
}

pub fn ui() -> Ui {
    Ui {
        ink: c(0x17191d),
        muted: c(0x5d636e),
        faint: c(0x8d939d),
        line: c(0xe4e6ea),
        line_strong: c(0xd3d6db),
        panel: c(0xffffff),
        glass: gpui::Rgba { r: 1.0, g: 1.0, b: 1.0, a: 0.92 }.into(),
        hover: c(0xf3f4f6),
        accent: c(0x2563eb),
        accent_bg: c(0xeef3fe),
        chip: c(0xf3f4f6),
        chip_ink: c(0x343840),
        code_bg: c(0xf8f9fa),
        code_ink: c(0x1f2328),
        code_no: c(0xa3a9b2),
        code_hover: c(0xeef1f5),
        code_marked: c(0xe3ebfd),
    }
}

/// The color of a syntax class from the core's highlighter (`highlight::Hl`), as styles.css colors `.t1`…`.t21`.
pub fn syntax(class: u32) -> Option<Hsla> {
    let hex = match class {
        1 | 13 => 0xcf222e,
        2 | 20 => 0x0a3069,
        3 => 0x6e7781,
        4 | 5 | 6 | 12 | 14 | 16 | 17 => 0x0550ae,
        7 | 8 | 10 => 0x8250df,
        9 => 0x953800,
        15 => 0x116329,
        19 => 0x0969da,
        21 => 0x6639ba,
        _ => return None,
    };
    Some(c(hex))
}

/// The UI font and the code font.
pub const UI_FONT: &str = ".SystemUIFont";
pub const MONO_FONT: &str = "Menlo";
