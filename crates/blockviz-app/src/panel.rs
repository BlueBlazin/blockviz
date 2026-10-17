//! The details panel for the selected block (src/ui/Inspector.tsx): what it
//! is, where it is used, its inputs and outputs, and its source with its own
//! lines marked. A view of its own, cached, so that it is laid out again only
//! when what it shows changes, not on every frame of a flight.

use crate::code;
use crate::model::Model;
use crate::theme::{syntax, Ui, MONO_FONT, UI_FONT};
use crate::Viewer;
use blockviz_core::ir::Kind;
use gpui::{
    div, prelude::*, px, AnyElement, App, Context, FontWeight, Hsla, ScrollHandle, SharedString, StyledText, TextRun, WeakEntity,
    Window,
};
use std::collections::HashMap;
use std::rc::Rc;

/// What each kind of block is called.
pub(crate) fn kind_label(k: Kind) -> &'static str {
    match k {
        Kind::Repo => "Repository",
        Kind::Dir => "Folder",
        Kind::File => "File",
        Kind::Notebook => "Notebook",
        Kind::Doc => "Document",
        Kind::Asset => "Asset",
        Kind::Class => "Class",
        Kind::Function => "Function",
        Kind::Section => "Section",
        Kind::Loop => "Loop",
        Kind::Branch => "Branch",
        Kind::Lane => "Branch arm",
        Kind::Group => "Group",
        Kind::Call => "Call",
        Kind::Op => "Step",
        Kind::Glyph => "Operator",
        Kind::Table => "Settings",
        Kind::Artifact => "File on disk",
        Kind::Symbol => "Definition",
        Kind::Input => "Input",
        Kind::Output => "Output",
        Kind::Unused => "Unused value",
    }
}

/// What a use is, when it is not a call.
fn use_kind(k: Kind) -> Option<&'static str> {
    match k {
        Kind::Symbol => Some("component"),
        Kind::Class => Some("methods"),
        Kind::File => Some("imports"),
        _ => None,
    }
}

/// The most lines of source shown.
const MAX_LINES: usize = 2000;
/// Uses listed before "Show all".
const USES_SHOWN: usize = 8;

pub struct Inspector {
    m: Rc<Model>,
    ui: Ui,
    viewer: WeakEntity<Viewer>,
    node: Option<u32>,
    all_uses: bool,
    scroll: ScrollHandle,
    /// Syntax runs of the files shown so far, by file.
    runs: HashMap<u32, Rc<Vec<u32>>>,
    /// The child to scroll into view on the next frame: the first marked line.
    reveal: Option<usize>,
}

impl Inspector {
    pub fn new(m: Rc<Model>, viewer: WeakEntity<Viewer>) -> Self {
        Inspector { m, ui: crate::theme::ui(), viewer, node: None, all_uses: false, scroll: ScrollHandle::new(), runs: HashMap::new(), reveal: None }
    }

    /// Shows blocks of a new analysis of the project.
    pub fn set_model(&mut self, m: Rc<Model>, cx: &mut Context<Self>) {
        self.m = m;
        self.runs.clear();
        self.node = None;
        cx.notify();
    }

    pub fn show(&mut self, node: Option<u32>, cx: &mut Context<Self>) {
        if node != self.node {
            self.node = node;
            self.all_uses = false;
            self.scroll.set_offset(gpui::point(px(0.0), px(0.0)));
            cx.notify();
        }
    }

    fn runs(&mut self, file: u32) -> Rc<Vec<u32>> {
        let m = self.m.clone();
        self.runs
            .entry(file)
            .or_insert_with(|| {
                let path = &m.scene.files[file as usize].path;
                Rc::new(m.file_text(file).map(|t| blockviz_core::highlight::highlight(path, t)).unwrap_or_default())
            })
            .clone()
    }

    /// Asks the viewer to do something, from a click in the panel: once the
    /// click is handled, since what the viewer does updates the panel.
    fn act(&self, cx: &mut App, f: impl FnOnce(&mut Viewer, &mut Context<Viewer>) + 'static) {
        let viewer = self.viewer.clone();
        cx.defer(move |cx| {
            if let Some(v) = viewer.upgrade() {
                v.update(cx, f);
            }
        });
    }

    fn label(&self, text: impl Into<SharedString>) -> gpui::Div {
        div().text_size(px(11.0)).text_color(self.ui.faint).font_weight(FontWeight::SEMIBOLD).mb(px(5.0)).child(text.into())
    }

    fn chips(&self, label: &str, values: &[String], mono: bool) -> Option<AnyElement> {
        if values.is_empty() {
            return None;
        }
        let ui = &self.ui;
        let chips = values.iter().map(|v| {
            div()
                .px(px(7.0))
                .py(px(2.0))
                .rounded(px(5.0))
                .bg(ui.chip)
                .text_color(ui.chip_ink)
                .text_size(px(if mono { 11.5 } else { 12.0 }))
                .when(mono, |d| d.font_family(MONO_FONT))
                .child(v.clone())
        });
        Some(div().my(px(10.0)).child(self.label(label.to_uppercase())).child(div().flex().flex_wrap().gap(px(5.0)).children(chips)).into_any_element())
    }

    fn button(&self, id: &'static str, text: impl Into<SharedString>, dim: Option<SharedString>) -> gpui::Stateful<gpui::Div> {
        let ui = &self.ui;
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(6.0))
            .px(px(10.0))
            .py(px(6.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(ui.line)
            .text_color(ui.ink)
            .font_weight(FontWeight::MEDIUM)
            .cursor_pointer()
            .hover(|s| s.bg(ui.hover).border_color(ui.line_strong))
            .child(text.into())
            .when_some(dim, |d, t| d.child(div().max_w(px(120.0)).truncate().font_weight(FontWeight::NORMAL).text_color(ui.muted).child(t)))
    }

    fn used_by(&self, id: u32, cx: &mut Context<Self>) -> Option<AnyElement> {
        let users = self.m.users(id);
        if users.is_empty() {
            return None;
        }
        let ui = &self.ui;
        let shown = if self.all_uses { users } else { &users[..users.len().min(USES_SHOWN)] };
        let title = self.m.node(id).title.clone();
        let rows = shown.iter().enumerate().map(|(k, &u)| {
            let n = self.m.node(u);
            let name = self.m.where_is(u).map_or_else(|| n.title.clone(), |w| w.0);
            let kind = use_kind(n.kind).map(|k| if n.title != title { format!(" · {k} {}", n.title) } else { format!(" · {k}") });
            let at = self.m.file_of(n).zip(n.span.as_ref()).map(|(f, s)| format!("{}:{}", f.rsplit('/').next().unwrap_or(f), s.line));
            div()
                .id(("use", k))
                .flex()
                .justify_between()
                .gap(px(10.0))
                .px(px(9.0))
                .py(px(5.0))
                .when(k > 0, |d| d.border_t_1().border_color(ui.line))
                .cursor_pointer()
                .hover(|s| s.bg(ui.hover))
                .on_click(cx.listener(move |this, _, _, cx| this.act(cx, move |v, cx| v.go_to(u, cx))))
                .child(div().flex().min_w_0().truncate().font_weight(FontWeight::MEDIUM).child(name).when_some(kind, |d, k| d.child(div().font_weight(FontWeight::NORMAL).text_color(ui.muted).child(k))))
                .when_some(at, |d, at| d.child(div().flex_none().font_family(MONO_FONT).text_size(px(11.0)).text_color(ui.faint).child(at)))
        });
        let count = (users.len() > 1).then(|| format!("  {}", users.len()));
        let mut field = div()
            .my(px(10.0))
            .child(self.label(format!("USED BY{}", count.unwrap_or_default())))
            .child(div().flex().flex_col().border_1().border_color(ui.line).rounded(px(8.0)).overflow_hidden().children(rows));
        if users.len() > shown.len() {
            field = field.child(
                div()
                    .id("all-uses")
                    .mt(px(5.0))
                    .text_size(px(12.0))
                    .text_color(ui.accent)
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.all_uses = true;
                        cx.notify();
                    }))
                    .child(format!("Show all {}", users.len())),
            );
        }
        Some(field.into_any_element())
    }
}

impl Render for Inspector {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = crate::theme::ui();
        let panel = div()
            .size_full()
            .flex()
            .flex_col()
            .bg(ui.panel)
            .border_1()
            .border_color(ui.line)
            .rounded(px(12.0))
            .shadow_md()
            .font_family(UI_FONT)
            .text_size(px(13.0))
            .text_color(ui.ink)
            .occlude();
        let Some(id) = self.node else { return panel };
        let m = self.m.clone();
        let node = m.node(id);
        let mut body: Vec<AnyElement> = vec![];

        // What it is, and a way to close the panel.
        let kind = kind_label(node.kind);
        body.push(
            div()
                .flex()
                .justify_between()
                .items_center()
                .child(
                    div()
                        .flex()
                        .text_size(px(11.0))
                        .text_color(ui.faint)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(kind.to_uppercase())
                        .when(!node.subtitle.is_empty(), |d| d.child(div().font_weight(FontWeight::MEDIUM).child(format!(" · {}", node.subtitle)))),
                )
                .child(
                    div()
                        .id("close")
                        .debug_selector(|| "panel-close".into())
                        .px(px(6.0))
                        .rounded(px(6.0))
                        .text_color(ui.muted)
                        .cursor_pointer()
                        .hover(|s| s.bg(ui.hover))
                        .on_click(cx.listener(|this, _, _, cx| this.act(cx, |v, cx| v.select(None, None, cx))))
                        .child("✕"),
                )
                .into_any_element(),
        );
        let title = if node.title.is_empty() { kind.to_string() } else { node.title.clone() };
        let op = node.kind == Kind::Op;
        body.push(
            div()
                .mt(px(2.0))
                .mb(px(6.0))
                .font_weight(FontWeight::BOLD)
                .text_size(px(if op { 14.5 } else { 18.0 }))
                .when(op, |d| d.font_family(MONO_FONT))
                .child(title)
                .into_any_element(),
        );
        if !node.doc.is_empty() {
            body.push(div().mb(px(10.0)).text_color(ui.muted).line_height(px(19.5)).child(node.doc.clone()).into_any_element());
        }

        // Zoom in, and go to where it is defined.
        let target = node.link.or(node.reference).filter(|&t| t != id);
        let mut actions = div().flex().flex_wrap().gap(px(6.0)).mt(px(4.0)).mb(px(12.0));
        if m.owner(node).is_some() {
            actions = actions.child(self.button("zoom", "Zoom in", None).on_click(cx.listener(|this, _, _, cx| this.act(cx, |v, cx| v.open_selected(cx)))));
        }
        if let Some(t) = target {
            let def = m.node(t);
            let what = if def.kind == Kind::Class { "Go to class" } else { "Go to definition" };
            actions = actions.child(self.button("definition", what, Some(def.title.clone().into())).on_click(cx.listener(move |this, _, _, cx| this.act(cx, move |v, cx| v.go_to(t, cx)))));
        }
        body.push(actions.into_any_element());

        body.extend(self.used_by(id, cx));
        body.extend(self.chips("Inputs", &node.inputs, true));
        body.extend(self.chips("Outputs", &node.outputs, true));
        body.extend(self.chips("Tags", &node.badges, false));
        let effects: Vec<String> = node.effects.iter().filter(|e| !e.starts_with("__")).cloned().collect();
        body.extend(self.chips("Side effects", &effects, false));
        if !node.rows.is_empty() {
            let rows = node.rows.iter().take(80).map(|r| {
                if r.heading {
                    div().mt(px(6.0)).mb(px(2.0)).text_size(px(10.5)).font_weight(FontWeight::BOLD).text_color(rgb_heading()).child(r.key.to_uppercase())
                } else {
                    div()
                        .py(px(1.5))
                        .flex()
                        .font_family(MONO_FONT)
                        .text_size(px(11.5))
                        .text_color(ui.muted)
                        .child(div().text_color(ui.ink).child(r.key.clone()))
                        .when(!r.value.is_empty(), |d| d.child(format!(" = {}", r.value)))
                }
            });
            body.push(
                div()
                    .my(px(10.0))
                    .child(self.label(format!("{} ENTRIES", node.rows.len())))
                    .child(div().border_1().border_color(ui.line).rounded(px(8.0)).px(px(8.0)).py(px(6.0)).children(rows))
                    .into_any_element(),
            );
        }

        // Its source: a step is read in the definition around it, its own lines marked.
        let mut reveal = None;
        if let Some(view) = m.source_view(node) {
            if let Some(text) = m.file_text(view.span.file) {
                let span = node.span.as_ref().unwrap();
                let file = m.file_of(node).unwrap_or_default().to_string();
                let marked = (view.root != id).then_some((span.line, span.end_line));
                let root = m.node(view.root);
                let place = if view.root != id && root.kind != Kind::File { format!(" · in {}", root.title) } else { String::new() };
                let lines = if span.line == span.end_line { format!("line {}", span.line) } else { format!("lines {}–{}", span.line, span.end_line) };
                body.push(
                    div()
                        .mt(px(14.0))
                        .mb(px(5.0))
                        .flex()
                        .font_family(MONO_FONT)
                        .text_size(px(11.0))
                        .text_color(ui.faint)
                        .font_weight(FontWeight::MEDIUM)
                        .child(div().text_color(ui.muted).child(file))
                        .child(format!("{place} · {lines}"))
                        .into_any_element(),
                );
                let runs = self.runs(view.span.file);
                let (lines, more) = code::lines(text, &runs, view.span.start as usize, view.span.end as usize, view.span.line, MAX_LINES);
                let digits = lines.last().map_or(1, |l| l.no.to_string().len());
                let count = lines.len();
                let view_root = view.root;
                for (k, line) in lines.into_iter().enumerate() {
                    let is_marked = marked.is_some_and(|(a, b)| line.no >= a && line.no <= b);
                    if is_marked && reveal.is_none() {
                        reveal = Some(body.len());
                    }
                    let first_marked = is_marked && reveal == Some(body.len());
                    body.push(self.code_line(line, digits, is_marked, first_marked, k == 0, k + 1 == count && more == 0, view_root, cx));
                }
                if more > 0 {
                    body.push(div().px(px(12.0)).pt(px(6.0)).text_size(px(11.5)).text_color(ui.faint).child(format!("… {more} more lines")).into_any_element());
                }
            }
        } else if !node.code.is_empty() {
            body.push(
                div()
                    .mt(px(14.0))
                    .p(px(10.0))
                    .bg(ui.code_bg)
                    .border_1()
                    .border_color(ui.line)
                    .rounded(px(8.0))
                    .font_family(MONO_FONT)
                    .text_size(px(11.5))
                    .child(node.code.clone())
                    .into_any_element(),
            );
        }
        if reveal != self.reveal {
            self.reveal = reveal;
            if let Some(i) = reveal {
                self.scroll.scroll_to_item(i);
            }
        }
        panel.child(div().id("inspector-body").flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.scroll).px(px(16.0)).pt(px(14.0)).pb(px(16.0)).children(body))
    }
}

/// The color of a table's headings.
fn rgb_heading() -> Hsla {
    gpui::rgb(0x8a6d1f).into()
}

impl Inspector {
    /// A line of source: its number and its code, colored by syntax. Clicking
    /// it selects the block drawn for it.
    #[allow(clippy::too_many_arguments)]
    fn code_line(&self, line: code::Line, digits: usize, marked: bool, first_marked: bool, first: bool, last: bool, root: u32, cx: &mut Context<Self>) -> AnyElement {
        let ui = &self.ui;
        let mut text = String::new();
        let mut runs = vec![];
        let font = gpui::font(MONO_FONT);
        for s in &line.segs {
            let color = syntax(s.cls).unwrap_or(ui.code_ink);
            text.push_str(&s.text);
            runs.push(TextRun { len: s.text.len(), font: font.clone(), color: if s.dim { color.opacity(0.42) } else { color }, background_color: None, underline: None, strikethrough: None });
        }
        if text.is_empty() {
            text.push(' ');
            runs.push(TextRun { len: 1, font: font.clone(), color: ui.code_ink, background_color: None, underline: None, strikethrough: None });
        }
        let no = line.no;
        div()
            .id(("line", no as usize))
            .when(first_marked, |d| d.debug_selector(|| "marked-line".into()))
            .flex()
            .bg(if marked { ui.code_marked } else { ui.code_bg })
            .border_l_2()
            .border_color(if marked { ui.accent } else { gpui::transparent_black() })
            .when(first, |d| d.mt(px(0.0)).pt(px(8.0)).rounded_t(px(8.0)))
            .when(last, |d| d.pb(px(8.0)).rounded_b(px(8.0)))
            .font_family(MONO_FONT)
            .text_size(px(11.5))
            .line_height(px(18.4))
            .cursor_pointer()
            .hover(|s| if marked { s } else { s.bg(ui.code_hover) })
            .on_click(cx.listener(move |this, _, _, cx| {
                let m = this.m.clone();
                this.act(cx, move |v, cx| v.pick(m.block_at(root, no), cx));
            }))
            .child(
                div()
                    .flex_none()
                    .w(px(digits as f32 * 7.0))
                    .ml(px(8.0))
                    .mr(px(10.0))
                    .text_right()
                    .text_color(if marked { ui.accent } else { ui.code_no })
                    .child(no.to_string()),
            )
            .child(div().flex_1().min_w_0().pr(px(10.0)).child(StyledText::new(text).with_runs(runs)))
            .into_any_element()
    }
}
