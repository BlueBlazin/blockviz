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
    }
}

/// `a` moved toward `b` by `t`, in RGB as the canvas mixes.
pub fn mix(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b) = (a.to_rgb(), b.to_rgb());
    gpui::Rgba { r: a.r + (b.r - a.r) * t, g: a.g + (b.g - a.g) * t, b: a.b + (b.b - a.b) * t, a: 1.0 }.into()
}
