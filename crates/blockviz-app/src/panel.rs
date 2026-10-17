//! The details panel, ported from src/ui/Inspector.tsx (a block),
//! ChangesPanel.tsx (a change's list, and a block it removed), ValuePanel.tsx
//! (a value followed) and CodeView.tsx / DiffView.tsx (source, and source as it
//! changed). A view of its own, cached: laid out again only when what it shows
//! changes, not on every frame of a flight.

use crate::code::{self, Line};
use crate::diff::{describe_change, SceneDiff, Status};
use crate::linediff::{diff_lines, fold, OpKind, Row};
use crate::model::Model;
use crate::render::EdgeId;
use crate::shell::{ico, icon_button, kbd};
use crate::theme::{Ui, MONO_FONT, UI_FONT};
use crate::viewer::Viewer;
use blockviz_core::ir::{Kind, Shape, Span};
use gpui::{div, prelude::*, px, AnyElement, App, Context, FontWeight, ScrollHandle, SharedString, Stateful, StyledText, TextRun, WeakEntity, Window};
use std::collections::{HashMap, HashSet};
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

/// What the panel shows.
#[derive(Clone, PartialEq, Debug)]
pub enum Content {
    None,
    Block(u32),
    /// A block the change removed, by its index among the changes.
    Removed(usize),
    Value { owner: u32, start: usize, edges: Vec<EdgeId> },
    Changes,
}

/// A change being read, as the panels describe it.
#[derive(Clone)]
pub struct CompareView {
    pub diff: Rc<SceneDiff>,
    pub kind: String,
    pub label: String,
    pub title: String,
}

pub struct PanelState {
    pub m: Rc<Model>,
    pub content: Content,
    /// The block under the pointer in the diagram: its lines are tinted.
    pub hover: Option<u32>,
    pub compare: Option<CompareView>,
    pub change_index: Option<usize>,
    pub ui: Ui,
}

/// The most lines of source shown.
const MAX_LINES: usize = 2000;
/// Uses listed before "Show all".
const USES_SHOWN: usize = 8;
/// Longer diffs fold unchanged stretches away.
const FOLD_OVER: usize = 40;
/// The width of a character of code (11.5 px of the code font).
const CH: f32 = 6.93;

/// A row of a `.uses` list: the block it goes to (or a change's index), its
/// name, a dimmer note, where it is, and whether it is the current one.
struct UseRow {
    id: u32,
    name: String,
    dim: Option<String>,
    at: Option<String>,
    current: bool,
    /// The block pointed out while the row is under the pointer.
    peek: Option<u32>,
}

pub struct Inspector {
    viewer: WeakEntity<Viewer>,
    ui: Ui,
    state: Option<PanelState>,
    all_uses: bool,
    scroll: ScrollHandle,
    /// Syntax runs of the files shown so far, by model and file.
    runs: HashMap<(usize, u32), Rc<Vec<u32>>>,
    /// The child last scrolled into view, for what was shown.
    revealed: Option<(Content, usize)>,
    /// Unchanged stretches of a diff the reader unfolded.
    unfolded: HashSet<(Option<usize>, Option<usize>)>,
}

impl Inspector {
    pub fn new(viewer: WeakEntity<Viewer>, ui: Ui) -> Self {
        Inspector { viewer, ui, state: None, all_uses: false, scroll: ScrollHandle::new(), runs: HashMap::new(), revealed: None, unfolded: HashSet::new() }
    }

    pub fn set(&mut self, state: Option<PanelState>, cx: &mut Context<Self>) {
        let same = matches!((&self.state, &state), (Some(a), Some(b)) if a.content == b.content && Rc::ptr_eq(&a.m, &b.m));
        if !same {
            self.all_uses = false;
            self.unfolded.clear();
            self.scroll.set_offset(gpui::point(px(0.0), px(0.0)));
            self.revealed = None;
        }
        if let Some(s) = &state {
            if self.state.as_ref().is_none_or(|o| !Rc::ptr_eq(&o.m, &s.m)) {
                self.runs.clear();
            }
            self.ui = s.ui.clone();
        }
        self.state = state;
        cx.notify();
    }

    /// What the panel shows.
    #[cfg(test)]
    pub fn content(&self) -> Content {
        self.state.as_ref().map_or(Content::None, |s| s.content.clone())
    }

    pub fn set_hover(&mut self, hover: Option<u32>, cx: &mut Context<Self>) {
        if let Some(s) = self.state.as_mut() {
            if s.hover != hover {
                s.hover = hover;
                cx.notify();
            }
        }
    }

    fn runs(&mut self, m: &Rc<Model>, file: u32) -> Rc<Vec<u32>> {
        let key = (Rc::as_ptr(m) as usize, file);
        self.runs
            .entry(key)
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

    // ----- pieces -----------------------------------------------------------------------

    fn head(&self, kind: String, sub: Option<String>, title: String, mono: bool, cx: &mut Context<Self>, close: impl Fn(&mut Viewer, &mut Context<Viewer>) + Copy + 'static) -> AnyElement {
        let ui = &self.ui;
        div()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .mt(px(-4.0))
                    .mr(px(-6.0))
                    .mb(px(2.0))
                    .child(div().flex().text_size(px(11.0)).text_color(ui.faint).font_weight(FontWeight::SEMIBOLD).child(kind.to_uppercase()).when_some(sub, |d, s| d.child(div().font_weight(FontWeight::MEDIUM).child(format!(" · {s}")))))
                    .child(icon_button("close", "x", 15.0, 30.0, "Close details", ui, true).debug_selector(|| "panel-close".into()).on_click(cx.listener(move |this, _, _, cx| this.act(cx, close)))),
            )
            .child(div().mt(px(2.0)).mb(px(6.0)).font_weight(FontWeight(650.0)).text_size(px(if mono { 14.5 } else { 18.0 })).when(mono, |d| d.font_family(MONO_FONT)).child(title))
            .into_any_element()
    }

    fn doc(&self, text: impl Into<SharedString>) -> AnyElement {
        div().mb(px(10.0)).text_color(self.ui.muted).line_height(px(19.5)).child(text.into()).into_any_element()
    }

    fn label(&self, text: impl Into<SharedString>, count: Option<usize>) -> gpui::Div {
        let ui = &self.ui;
        div().flex().gap(px(4.0)).text_size(px(11.0)).text_color(ui.faint).font_weight(FontWeight::SEMIBOLD).mb(px(5.0)).child(text.into()).when_some(count, |d, n| d.child(div().font_weight(FontWeight::MEDIUM).child(n.to_string())))
    }

    fn action(&self, id: &'static str, icon: &str, text: impl Into<SharedString>, dim: Option<String>) -> Stateful<gpui::Div> {
        let ui = &self.ui;
        let (hover, line_strong) = (ui.hover, ui.line_strong);
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
            .hover(move |s| s.bg(hover).border_color(line_strong))
            .child(ico(icon, 13.0, ui.ink))
            .child(text.into())
            .when_some(dim, |d, t| d.child(div().ml(px(2.0)).max_w(px(120.0)).truncate().font_weight(FontWeight::NORMAL).text_color(ui.faint).child(t)))
    }

    fn chips(&self, label: &str, values: &[String], mono: bool) -> Option<AnyElement> {
        if values.is_empty() {
            return None;
        }
        let ui = &self.ui;
        let chips = values.iter().map(|v| div().px(px(7.0)).py(px(2.0)).rounded(px(5.0)).bg(ui.chip).text_color(ui.chip_ink).text_size(px(if mono { 11.5 } else { 12.0 })).when(mono, |d| d.font_family(MONO_FONT)).child(v.clone()));
        Some(div().my(px(10.0)).child(self.label(label.to_uppercase(), None)).child(div().flex().flex_wrap().gap(px(5.0)).children(chips)).into_any_element())
    }

    /// A list of links (`.uses`): each row names a place, points it out, and goes to it.
    fn uses(&self, rows: Vec<UseRow>, cx: &mut Context<Self>, go: impl Fn(&mut Viewer, u32, &mut Context<Viewer>) + Copy + 'static) -> AnyElement {
        let ui = &self.ui;
        let hover = ui.hover;
        let list = rows.into_iter().enumerate().map(|(k, r)| {
            let (id, peek) = (r.id, r.peek);
            div()
                .id(("use", k))
                .flex()
                .justify_between()
                .items_baseline()
                .gap(px(10.0))
                .px(px(9.0))
                .py(px(5.0))
                .when(k > 0, |d| d.border_t_1().border_color(ui.line))
                .when(r.current, |d| d.bg(ui.accent_bg).border_l_2().border_color(ui.accent))
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .on_hover(cx.listener(move |this, h: &bool, _, cx| {
                    let target = if *h { peek } else { None };
                    this.act(cx, move |v, cx| v.peek(target, cx));
                }))
                .on_click(cx.listener(move |this, _, _, cx| this.act(cx, move |v, cx| go(v, id, cx))))
                .child(div().flex().min_w_0().truncate().font_weight(FontWeight(550.0)).child(r.name).when_some(r.dim, |d, t| d.child(div().font_weight(FontWeight::NORMAL).text_color(ui.faint).child(t))))
                .when_some(r.at, |d, at| d.child(div().flex_none().font_family(MONO_FONT).text_size(px(11.0)).text_color(ui.faint).child(at)))
        });
        div().flex().flex_col().border_1().border_color(ui.line).rounded(px(8.0)).overflow_hidden().children(list).into_any_element()
    }

    /// Steps through the changes: which one this is, and buttons for the previous and next.
    fn change_steps(&self, index: Option<usize>, total: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        if total == 0 {
            return None;
        }
        let ui = &self.ui;
        let said = match index {
            Some(i) => div().flex().gap(px(3.0)).child("Change").child(div().text_color(ui.ink).font_weight(FontWeight::SEMIBOLD).child((i + 1).to_string())).child(format!("of {total}")),
            None => div().child(format!("{total} change{} in all", if total == 1 { "" } else { "s" })),
        };
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(2.0))
                .mb(px(8.0))
                .text_color(ui.faint)
                .text_size(px(12.0))
                .child(said.mr(px(4.0)))
                .child(icon_button("prev-change", "chevron-left", 15.0, 26.0, "Previous change (P)", ui, true).on_click(cx.listener(|this, _, _, cx| this.act(cx, |v, cx| v.step_change(-1, cx)))))
                .child(icon_button("next-change", "chevron-right", 15.0, 26.0, "Next change (N)", ui, true).on_click(cx.listener(|this, _, _, cx| this.act(cx, |v, cx| v.step_change(1, cx)))))
                .into_any_element(),
        )
    }

    fn badge(&self, s: Status) -> AnyElement {
        let ui = &self.ui;
        let (bg, ink, text) = match s {
            Status::Added => (ui.added_bg, ui.added, "Added"),
            Status::Changed => (ui.changed_bg, ui.changed, "Changed"),
            Status::Removed => (ui.removed_bg, ui.removed, "Removed"),
        };
        div().px(px(7.0)).py(px(1.0)).rounded(px(10.0)).text_size(px(11.0)).font_weight(FontWeight(650.0)).bg(bg).text_color(ink).child(text).into_any_element()
    }

    fn source_label(&self, file: &str, rest: String) -> AnyElement {
        let ui = &self.ui;
        div().mt(px(14.0)).mb(px(5.0)).flex().flex_wrap().font_family(MONO_FONT).text_size(px(11.0)).font_weight(FontWeight::MEDIUM).text_color(ui.muted).child(file.to_string()).child(div().text_color(ui.faint).child(rest)).into_any_element()
    }

    // ----- source -----------------------------------------------------------------------

    /// One row of a code box: the box's own border and padding around `row`.
    fn boxed(&self, row: AnyElement, first: bool, last: bool) -> AnyElement {
        let ui = &self.ui;
        div()
            .bg(ui.code_bg)
            .border_x_1()
            .border_color(ui.line)
            .font_family(MONO_FONT)
            .text_size(px(11.5))
            .line_height(px(18.4))
            .text_color(ui.code_ink)
            .when(first, |d| d.border_t_1().rounded_t(px(8.0)).pt(px(8.0)))
            .when(last, |d| d.border_b_1().rounded_b(px(8.0)).pb(px(8.0)))
            .child(row)
            .into_any_element()
    }

    /// A line of source: its number, a sign in a diff, and its code colored by
    /// syntax, wrapping under its own indentation.
    #[allow(clippy::too_many_arguments)]
    fn code_line(&self, line: &Line, digits: usize, sign: Option<OpKind>, marked: bool, tinted: bool, click: Option<(u32, u32)>, id: usize, cx: &mut Context<Self>) -> AnyElement {
        let ui = &self.ui;
        let font = gpui::font(MONO_FONT);
        let indent = line.indent();
        let mut text = String::new();
        let mut runs = vec![];
        let mut skip = true;
        for s in &line.segs {
            // The indentation is a spacer, so wrapped parts continue under the code.
            let t = if skip { s.text.trim_start_matches([' ', '\t']) } else { s.text.as_str() };
            if t.is_empty() {
                continue;
            }
            skip = false;
            let color = ui.syntax(s.cls).unwrap_or(ui.code_ink);
            let weight = if matches!(s.cls, 17 | 18) { FontWeight::SEMIBOLD } else { FontWeight::NORMAL };
            let style = if s.cls == 11 { gpui::FontStyle::Italic } else { gpui::FontStyle::Normal };
            text.push_str(t);
            runs.push(TextRun { len: t.len(), font: gpui::Font { weight, style, ..font.clone() }, color: if s.dim { color.opacity(0.42) } else { color }, background_color: None, underline: None, strikethrough: None });
        }
        if text.is_empty() {
            text.push(' ');
            runs.push(TextRun { len: 1, font: font.clone(), color: ui.code_ink, background_color: None, underline: None, strikethrough: None });
        }
        let (bg, mut no_color) = match sign {
            Some(OpKind::Add) => (Some(ui.added_bg), ui.added),
            Some(OpKind::Del) => (Some(ui.removed_bg), ui.removed),
            _ => (None, ui.code_no),
        };
        if marked {
            no_color = ui.accent;
        }
        let hover = ui.code_hover;
        let mut row = div()
            .id(("line", id))
            .flex()
            .pr(px(10.0))
            .border_l_2()
            .border_color(if marked { ui.accent } else { gpui::transparent_black() })
            .when_some(bg, |d, c| d.bg(c))
            .when(tinted && !marked, |d| d.bg(ui.code_hover))
            .when(marked, |d| d.bg(ui.code_marked).debug_selector(|| "marked-line".into()))
            .child(div().flex_none().w(px(digits as f32 * CH + 18.0)).pl(px(8.0)).pr(px(10.0)).flex().justify_end().text_color(no_color).child(line.no.to_string()));
        if let Some(kind) = sign {
            let s = match kind {
                OpKind::Same => " ",
                OpKind::Add => "+",
                OpKind::Del => "−",
            };
            row = row.child(div().flex_none().w(px(1.6 * CH)).text_color(no_color).child(s));
        }
        row = row.child(div().flex_none().w(px(indent as f32 * CH))).child(div().flex_1().min_w_0().when(sign == Some(OpKind::Del), |d| d.opacity(0.85)).child(StyledText::new(text).with_runs(runs)));
        if let Some((root, no)) = click {
            row = row
                .cursor_pointer()
                .when(!marked, |d| d.hover(move |s| s.bg(hover)))
                .on_hover(cx.listener(move |this, h: &bool, _, cx| {
                    let Some(m) = this.state.as_ref().map(|s| s.m.clone()) else { return };
                    let target = if *h { Some(m.block_at(root, no)).filter(|&b| b != root) } else { None };
                    this.act(cx, move |v, cx| v.peek(target, cx));
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let Some(m) = this.state.as_ref().map(|s| s.m.clone()) else { return };
                    let b = m.block_at(root, no);
                    this.act(cx, move |v, cx| v.pick(b, cx));
                }));
        }
        row.into_any_element()
    }

    /// Source of a span, with line numbers and syntax colors (CodeView), into `body`.
    #[allow(clippy::too_many_arguments)]
    fn code_view(&mut self, m: &Rc<Model>, span: &Span, marked: Option<(u32, u32)>, tinted: Option<(u32, u32)>, root: u32, body: &mut Vec<AnyElement>, reveal: &mut Option<usize>, cx: &mut Context<Self>) {
        let Some(text) = m.file_text(span.file) else { return };
        let runs = self.runs(m, span.file);
        let (lines, more) = code::lines(text, &runs, span.start as usize, span.end as usize, span.line, MAX_LINES, false);
        if lines.is_empty() {
            return;
        }
        let digits = lines.last().unwrap().no.to_string().len();
        let within = |r: Option<(u32, u32)>, no: u32| r.is_some_and(|(a, b)| no >= a && no <= b);
        let n = lines.len();
        for (k, l) in lines.iter().enumerate() {
            let is_marked = within(marked, l.no);
            if is_marked && reveal.is_none() {
                *reveal = Some(body.len());
            }
            let row = self.code_line(l, digits, None, is_marked, within(tinted, l.no), Some((root, l.no)), k, cx);
            body.push(self.boxed(row, k == 0, k + 1 == n && more == 0));
        }
        if more > 0 {
            let note = div().px(px(12.0)).pt(px(6.0)).font_family(UI_FONT).text_color(self.ui.faint).child(format!("… {more} more lines")).into_any_element();
            body.push(self.boxed(note, false, true));
        }
    }

    /// A block's code as it changed (DiffView), into `body`: lines removed from
    /// the base and added in the head among those kept, long unchanged stretches
    /// folded (click to unfold), never the marked lines.
    #[allow(clippy::too_many_arguments)]
    fn diff_view(&mut self, base: Option<(&Rc<Model>, &Span)>, head: Option<(&Rc<Model>, &Span)>, marked: Option<(u32, u32)>, tinted: Option<(u32, u32)>, root: Option<u32>, body: &mut Vec<AnyElement>, reveal: &mut Option<usize>, cx: &mut Context<Self>) {
        let mut lines_of = |v: Option<(&Rc<Model>, &Span)>| -> Vec<Line> {
            let Some((m, span)) = v else { return vec![] };
            let Some(text) = m.file_text(span.file) else { return vec![] };
            let runs = self.runs(m, span.file);
            code::lines(text, &runs, span.start as usize, span.end as usize, span.line, 4000, true).0
        };
        let before = lines_of(base);
        let after = lines_of(head);
        let a: Vec<String> = before.iter().map(Line::text).collect();
        let b: Vec<String> = after.iter().map(Line::text).collect();
        let ops = diff_lines(&a, &b);
        if ops.is_empty() {
            return;
        }
        // Both versions lose the same indentation, so moved code shows its new depth.
        let cut = code::dedent(before.iter().chain(after.iter()).cloned().collect());
        let (old, new) = cut.split_at(before.len());
        let within = |r: Option<(u32, u32)>, no: u32| r.is_some_and(|(x, y)| no >= x && no <= y);
        let rows: Vec<Row> = if ops.len() > FOLD_OVER { fold(&ops, 3, |o| o.b.is_some_and(|i| within(marked, new[i].no))) } else { ops.iter().cloned().map(Row::Op).collect() };
        let mut shown = vec![];
        for r in rows {
            match r {
                Row::Gap(g) if self.unfolded.contains(&(Some(g.a), Some(g.b))) => {
                    let i = ops.iter().position(|o| o.a == Some(g.a) && o.b == Some(g.b)).unwrap_or(0);
                    shown.extend(ops[i..(i + g.count).min(ops.len())].iter().cloned().map(Row::Op));
                }
                r => shown.push(r),
            }
        }
        let last_no = shown
            .iter()
            .filter_map(|r| match r {
                Row::Op(o) if o.kind == OpKind::Del => o.a.map(|i| old[i].no),
                Row::Op(o) => o.b.map(|i| new[i].no),
                Row::Gap(_) => None,
            })
            .max()
            .unwrap_or(1);
        let digits = last_no.to_string().len();
        let ui = self.ui.clone();
        let n = shown.len();
        for (k, r) in shown.iter().enumerate() {
            let row = match r {
                Row::Gap(g) => {
                    let key = (Some(g.a), Some(g.b));
                    let (accent, faint) = (ui.accent, ui.faint);
                    div()
                        .id(("gap", k))
                        .flex()
                        .bg(ui.code_gap)
                        .text_color(faint)
                        .font_family(UI_FONT)
                        .text_size(px(11.0))
                        .cursor_pointer()
                        .hover(move |s| s.text_color(accent))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.unfolded.insert(key);
                            cx.notify();
                        }))
                        .child(div().flex_none().w(px(digits as f32 * CH + 18.0)).pl(px(8.0)).pr(px(10.0)).flex().justify_end().child("⋯"))
                        .child(div().flex_none().w(px(1.6 * CH)))
                        .child(format!("{} unchanged lines", g.count))
                        .into_any_element()
                }
                Row::Op(o) => {
                    let (line, in_head) = match o.kind {
                        OpKind::Del => (&old[o.a.unwrap()], false),
                        _ => (&new[o.b.unwrap()], true),
                    };
                    let is_marked = in_head && within(marked, line.no);
                    if reveal.is_none() && (is_marked || (marked.is_none() && o.kind != OpKind::Same)) {
                        *reveal = Some(body.len());
                    }
                    let click = if in_head { root.map(|r| (r, line.no)) } else { None };
                    self.code_line(line, digits, Some(o.kind), is_marked, in_head && within(tinted, line.no), click, k, cx)
                }
            };
            body.push(self.boxed(row, k == 0, k + 1 == n));
        }
    }

    // ----- panels -----------------------------------------------------------------------

    fn block(&mut self, s: &PanelState, id: u32, body: &mut Vec<AnyElement>, reveal: &mut Option<usize>, cx: &mut Context<Self>) {
        let m = s.m.clone();
        let ui = self.ui.clone();
        let node = m.node(id);
        let kind = kind_label(node.kind);
        body.push(self.head(kind.into(), (!node.subtitle.is_empty()).then(|| node.subtitle.clone()), if node.title.is_empty() { kind.into() } else { node.title.clone() }, node.kind == Kind::Op, cx, |v, cx| v.select(None, None, cx)));
        if !node.doc.is_empty() {
            body.push(self.doc(node.doc.clone()));
        }
        // How a change being read touched it.
        let diff = s.compare.as_ref().map(|c| c.diff.clone());
        if let Some(d) = &diff {
            let status = d.status(id);
            let inside = d.inside(id);
            let through = if inside > 0 { 0 } else { d.through(id) };
            let plural = |n: usize| if n == 1 { "" } else { "s" };
            let says = if status == Some(Status::Added) {
                "New in this change".to_string()
            } else if inside > 0 {
                format!("{inside} change{} inside", plural(inside))
            } else if through > 0 {
                format!("Leads into code with {through} change{}", plural(through))
            } else if status.is_some() {
                "Its code changed".into()
            } else {
                "Unchanged".into()
            };
            body.push(div().flex().items_center().flex_wrap().gap(px(8.0)).mb(px(6.0)).text_color(ui.muted).when_some(status, |dv, st| dv.child(self.badge(st))).child(says).into_any_element());
            if let Some(steps) = self.change_steps(s.change_index, d.changes.len(), cx) {
                body.push(steps);
            }
            let removed = d.removed_in(id).to_vec();
            if !removed.is_empty() {
                let rows = removed
                    .iter()
                    .map(|c| {
                        let info = describe_change(d, c);
                        let i = d.index_of(c.id, true).map_or(u32::MAX, |i| i as u32);
                        UseRow { id: i, name: format!("− {}", info.name), dim: Some(format!(" · {}", info.kind.to_lowercase())), at: info.line.map(|l| l.to_string()), current: false, peek: Some(c.at) }
                    })
                    .collect();
                let list = self.uses(rows, cx, |v, i, cx| v.go_to_change(i as usize, cx));
                body.push(div().my(px(10.0)).child(self.label("REMOVED FROM HERE", Some(removed.len()))).child(list).into_any_element());
            }
        }
        // Zoom in, go to where it is defined, and save it alone.
        let target = node.link.or(node.reference).filter(|&t| t != id);
        let mut actions = div().flex().flex_wrap().gap(px(6.0)).mt(px(4.0)).mb(px(12.0));
        if m.owner(node).is_some() {
            actions = actions.child(self.action("zoom", "maximize-2", "Zoom in", None).on_click(cx.listener(|this, _, _, cx| this.act(cx, |v, cx| v.open_selected(cx)))));
        }
        if let Some(t) = target {
            let def = m.node(t);
            let what = if def.kind == Kind::Class { "Go to class" } else { "Go to definition" };
            actions = actions.child(self.action("definition", "arrow-up-right", what, Some(def.title.clone())).on_click(cx.listener(move |this, _, _, cx| this.act(cx, move |v, cx| v.go_to(t, cx)))));
        }
        actions = actions.child(self.action("svg", "download", "SVG", None).on_click(cx.listener(move |this, _, _, cx| this.act(cx, move |v, cx| v.download_block(id, false, cx)))));
        body.push(actions.into_any_element());
        // Where it is used.
        let users = m.users(id).to_vec();
        if !users.is_empty() {
            let shown = if self.all_uses { &users[..] } else { &users[..users.len().min(USES_SHOWN)] };
            let rows = shown
                .iter()
                .map(|&u| {
                    let n = m.node(u);
                    let name = m.where_is(u).map_or_else(|| n.title.clone(), |w| w.0);
                    let dim = use_kind(n.kind).map(|k| if n.title != node.title { format!(" · {k} {}", n.title) } else { format!(" · {k}") });
                    let at = m.file_of(n).zip(n.span.as_ref()).map(|(f, sp)| format!("{}:{}", f.rsplit('/').next().unwrap_or(f), sp.line));
                    UseRow { id: u, name, dim, at, current: false, peek: Some(u) }
                })
                .collect();
            let list = self.uses(rows, cx, |v, u, cx| v.go_to(u, cx));
            let mut field = div().my(px(10.0)).child(self.label("USED BY", (users.len() > 1).then_some(users.len()))).child(list);
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
            body.push(field.into_any_element());
        }
        body.extend(self.chips("Inputs", &node.inputs, true));
        body.extend(self.chips("Outputs", &node.outputs, true));
        body.extend(self.chips("Tags", &node.badges, false));
        let effects: Vec<String> = node.effects.iter().filter(|e| !e.starts_with("__")).cloned().collect();
        body.extend(self.chips("Side effects", &effects, false));
        if !node.rows.is_empty() {
            let rows = node.rows.iter().take(80).map(|r| {
                if r.heading {
                    div().mt(px(6.0)).mb(px(2.0)).text_size(px(10.5)).font_weight(FontWeight::BOLD).text_color(ui.heading).child(r.key.to_uppercase())
                } else {
                    div().py(px(1.5)).flex().flex_wrap().font_family(MONO_FONT).text_size(px(11.5)).text_color(ui.muted).child(div().text_color(ui.ink).child(r.key.clone())).when(!r.value.is_empty(), |d| d.child(format!(" = {}", r.value)))
                }
            });
            body.push(
                div()
                    .my(px(10.0))
                    .child(self.label(format!("{} ENTRIES", node.rows.len()), None))
                    .child(div().id("rows").max_h(px(260.0)).overflow_y_scroll().text_size(px(12.0)).border_1().border_color(ui.line).rounded(px(8.0)).px(px(8.0)).py(px(6.0)).children(rows))
                    .into_any_element(),
            );
        }
        // Its source: a step is read in the definition around it, its own lines marked.
        let view = m.source_view(node).filter(|v| m.file_text(v.span.file).is_some());
        let Some(view) = view else {
            if !node.code.is_empty() {
                body.push(div().mt(px(14.0)).px(px(12.0)).py(px(10.0)).bg(ui.code_bg).border_1().border_color(ui.line).rounded(px(8.0)).font_family(MONO_FONT).text_size(px(11.5)).line_height(px(18.4)).child(node.code.clone()).into_any_element());
            }
            return;
        };
        let span = node.span.clone().unwrap();
        let file = m.file_of(node).unwrap_or_default().to_string();
        let marked = (view.root != id).then_some((span.line, span.end_line));
        let root = m.node(view.root);
        let place = if view.root != id && root.kind != Kind::File { format!(" · in {}", root.title) } else { String::new() };
        let lines = if span.line == span.end_line { format!("line {}", span.line) } else { format!("lines {}–{}", span.line, span.end_line) };
        let tinted = s.hover.filter(|&h| h != view.root).and_then(|h| {
            let sp = m.node(h).span.as_ref()?;
            (sp.file == view.span.file && sp.start >= view.span.start && sp.end <= view.span.end).then_some((sp.line, sp.end_line))
        });
        body.push(self.source_label(&file, format!("{place} · {lines}")));
        // In a change, code is shown as it changed: the whole definition around the block, folded, its lines kept.
        let changed = diff.as_ref().is_some_and(|d| d.status(view.root).is_some() || d.inside(view.root) > 0);
        if changed {
            let d = diff.unwrap();
            let root_span = root.span.clone().unwrap_or(view.span.clone());
            let base_span = d.base_of(view.root).and_then(|b| d.base.node(b).span.clone()).filter(|sp| d.base.file_text(sp.file).is_some());
            let base = d.base.clone();
            self.diff_view(base_span.as_ref().map(|sp| (&base, sp)), Some((&m, &root_span)), marked, tinted, Some(view.root), body, reveal, cx);
        } else {
            self.code_view(&m, &view.span, marked, tinted, view.root, body, reveal, cx);
        }
    }

    fn removed(&mut self, s: &PanelState, index: usize, body: &mut Vec<AnyElement>, reveal: &mut Option<usize>, cx: &mut Context<Self>) {
        let Some(c) = s.compare.as_ref() else { return };
        let d = c.diff.clone();
        let Some(&change) = d.changes.get(index) else { return };
        let model = d.base.clone();
        let n = model.node(change.id);
        let info = describe_change(&d, &change);
        let ui = self.ui.clone();
        body.push(self.head(info.kind.into(), (!n.subtitle.is_empty()).then(|| n.subtitle.clone()), info.name.clone(), n.kind == Kind::Op, cx, |v, cx| {
            v.removed_sel = None;
            v.camera.insets = v.insets();
            v.sync_panel(cx);
            cx.notify();
        }));
        let at = change.at;
        body.push(
            div()
                .flex()
                .items_center()
                .flex_wrap()
                .gap(px(8.0))
                .mb(px(6.0))
                .text_color(ui.muted)
                .child(self.badge(Status::Removed))
                .child(div().flex().gap(px(4.0)).child("from").child(div().id("removed-from").text_color(ui.accent).font_weight(FontWeight::MEDIUM).cursor_pointer().on_click(cx.listener(move |this, _, _, cx| this.act(cx, move |v, cx| v.fly_to_node(at, cx)))).child(d.head.qualified(at))))
                .into_any_element(),
        );
        if let Some(steps) = self.change_steps(Some(index), d.changes.len(), cx) {
            body.push(steps);
        }
        let id = change.id;
        body.push(div().flex().gap(px(6.0)).mt(px(4.0)).mb(px(12.0)).child(self.action("svg", "download", "SVG", None).on_click(cx.listener(move |this, _, _, cx| this.act(cx, move |v, cx| v.download_block(id, true, cx))))).into_any_element());
        if let Some(span) = n.span.clone().filter(|sp| model.file_text(sp.file).is_some()) {
            let lines = if span.line == span.end_line { format!("line {}", span.line) } else { format!("lines {}–{}", span.line, span.end_line) };
            body.push(self.source_label(&info.file.clone().unwrap_or_default(), format!(" · {lines} before")));
            self.diff_view(Some((&model, &span)), None, None, None, None, body, reveal, cx);
        }
    }

    fn value(&mut self, s: &PanelState, owner: u32, start: usize, edges: &[EdgeId], body: &mut Vec<AnyElement>, cx: &mut Context<Self>) {
        let m = s.m.clone();
        let Some(first) = m.node(owner).edges.get(start).cloned() else { return };
        body.push(self.head("Value".into(), None, if first.label.is_empty() { "unnamed value".into() } else { first.label.clone() }, true, cx, |v, cx| v.close_value(cx)));
        // The definition a diagram belongs to (a branch belongs to its function).
        let mut def = m.node(owner);
        while def.shape == Shape::Region {
            let Some(p) = def.parent else { break };
            def = m.node(p);
        }
        let def_id = def.id;
        body.push(self.doc(format!("In {}. Its path is highlighted in the diagram.", m.qualified(def_id))));
        body.push(div().flex().gap(px(6.0)).mt(px(4.0)).mb(px(12.0)).child(self.action("svg", "download", "SVG", None).on_click(cx.listener(move |this, _, _, cx| this.act(cx, move |v, cx| v.download_block(def_id, false, cx))))).into_any_element());
        // Ports and branches only pass the value on, and a stop is where nothing reads it;
        // the steps are what read and make it.
        let step = |id: u32| {
            let n = m.node(id);
            let in_region = n.parent.is_some_and(|p| m.node(p).shape == Shape::Region);
            n.shape != Shape::Region && n.kind != Kind::Unused && !(matches!(n.kind, Kind::Input | Kind::Output) && in_region)
        };
        let list: Vec<_> = edges.iter().map(|&(o, i)| m.node(o).edges[i].clone()).collect();
        let targets: HashSet<u32> = list.iter().map(|e| e.to).collect();
        let mut sources: Vec<u32> = vec![];
        for e in &list {
            if !targets.contains(&e.from) && step(e.from) && !sources.contains(&e.from) {
                sources.push(e.from);
            }
        }
        let mut readers: Vec<u32> = targets.iter().copied().filter(|&id| step(id)).collect();
        readers.sort_by(|&a, &b| {
            let (na, nb) = (m.node(a), m.node(b));
            let (sa, sb) = (na.span.as_ref().map_or(0, |s| s.start), nb.span.as_ref().map_or(0, |s| s.start));
            sa.cmp(&sb).then(na.y.partial_cmp(&nb.y).unwrap_or(std::cmp::Ordering::Equal))
        });
        let name = |id: u32| {
            let n = m.node(id);
            match n.kind {
                Kind::Input => format!("input {}", n.title),
                Kind::Output if n.title.is_empty() => "result".into(),
                Kind::Output => format!("result {}", n.title),
                _ if n.title.is_empty() => kind_label(n.kind).to_lowercase(),
                _ => n.title.clone(),
            }
        };
        let row = |id: u32| UseRow { id, name: name(id), dim: None, at: (!m.node(id).subtitle.is_empty()).then(|| m.node(id).subtitle.clone()), current: false, peek: Some(id) };
        let from = self.uses(sources.iter().map(|&i| row(i)).collect(), cx, |v, id, cx| {
            v.close_value(cx);
            v.pick(id, cx);
        });
        body.push(div().my(px(10.0)).child(self.label("COMES FROM", None)).child(from).into_any_element());
        let into = if readers.is_empty() {
            self.doc("Nothing reads it.")
        } else {
            self.uses(readers.iter().map(|&i| row(i)).collect(), cx, |v, id, cx| {
                v.close_value(cx);
                v.pick(id, cx);
            })
        };
        body.push(div().my(px(10.0)).child(self.label("FLOWS INTO", Some(readers.len()))).child(into).into_any_element());
    }

    fn changes(&mut self, s: &PanelState, body: &mut Vec<AnyElement>, reveal: &mut Option<usize>, cx: &mut Context<Self>) {
        let Some(c) = s.compare.clone() else { return };
        let d = c.diff.clone();
        let ui = self.ui.clone();
        body.push(self.head(c.kind.clone(), None, c.title.clone(), false, cx, |v, cx| {
            v.changes_open = false;
            v.camera.insets = v.insets();
            v.sync_panel(cx);
            cx.notify();
        }));
        let code = |t: String| div().font_family(MONO_FONT).text_size(px(11.5)).text_color(ui.muted).bg(ui.chip).px(px(5.0)).py(px(1.0)).rounded(px(4.0)).truncate().child(t);
        body.push(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(6.0))
                .mb(px(8.0))
                .text_color(ui.faint)
                .text_size(px(12.0))
                .child(code(c.label.clone()))
                .child("←")
                .child(code("working tree".into()))
                .child(div().ml_auto().id("stop").text_color(ui.accent).cursor_pointer().on_click(cx.listener(|this, _, _, cx| this.act(cx, |v, cx| v.stop_compare(cx)))).child("Stop comparing"))
                .into_any_element(),
        );
        let sm = d.summary();
        body.push(
            div()
                .flex()
                .gap(px(12.0))
                .mb(px(10.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(12.5))
                .child(div().text_color(ui.added).child(format!("{} added", sm.added)))
                .child(div().text_color(ui.changed).child(format!("{} changed", sm.changed)))
                .child(div().text_color(ui.removed).child(format!("{} removed", sm.removed)))
                .into_any_element(),
        );
        if d.changes.is_empty() {
            body.push(self.doc(if d.files.is_empty() { "No block of the analyzed code changed." } else { "No block of the analyzed code changed; the files changed are listed below." }));
        } else {
            body.push(div().mb(px(10.0)).flex().flex_wrap().items_center().gap(px(4.0)).text_color(ui.muted).line_height(px(19.5)).child("In the order the diagram reads. Press").child(kbd("N", &ui)).child("and").child(kbd("P", &ui)).child("to step through them.").into_any_element());
        }
        // Grouped by file, in the order the diagram reads.
        let mut groups: Vec<(String, Option<char>, Vec<(usize, crate::diff::Change, crate::diff::ChangeInfo)>)> = vec![];
        for (i, ch) in d.changes.iter().enumerate() {
            let info = describe_change(&d, ch);
            let file = info.file.clone().unwrap_or_default();
            let at = match groups.iter().position(|g| g.0 == file) {
                Some(at) => at,
                None => {
                    let mark = d.file_change(&file, ch.status == Status::Removed).map(|f| file_mark(f.status));
                    groups.push((file.clone(), mark, vec![]));
                    groups.len() - 1
                }
            };
            groups[at].2.push((i, *ch, info));
        }
        for (file, mark, items) in &groups {
            let rows: Vec<_> = items
                .iter()
                .map(|(i, ch, info)| {
                    let sign = match ch.status {
                        Status::Added => "+",
                        Status::Changed => "~",
                        Status::Removed => "−",
                    };
                    let name = format!("{sign}  {}{}", info.context.as_ref().map(|c| format!("{c} › ")).unwrap_or_default(), info.name);
                    UseRow { id: *i as u32, name, dim: Some(format!(" · {}", info.kind.to_lowercase())), at: info.line.map(|l| l.to_string()), current: s.change_index == Some(*i), peek: Some(ch.at) }
                })
                .collect();
            if items.iter().any(|(i, _, _)| s.change_index == Some(*i)) && reveal.is_none() {
                *reveal = Some(body.len());
            }
            let list = self.uses(rows, cx, |v, i, cx| v.go_to_change(i as usize, cx));
            body.push(
                div()
                    .my(px(10.0))
                    .child(div().flex().items_center().gap(px(6.0)).mb(px(5.0)).font_family(MONO_FONT).text_size(px(11.5)).font_weight(FontWeight::MEDIUM).text_color(ui.muted).when_some(*mark, |dv, m| dv.child(self.file_mark_el(m))).child(if file.is_empty() { "Repository".to_string() } else { file.clone() }))
                    .child(list)
                    .into_any_element(),
            );
        }
        let listed: HashSet<&str> = groups.iter().map(|g| g.0.as_str()).collect();
        let others: Vec<_> = d.files.iter().filter(|f| !listed.contains(f.path.as_str()) && !f.previous.as_deref().is_some_and(|p| listed.contains(p))).collect();
        if !others.is_empty() {
            let rows = others.iter().map(|f| div().flex().items_center().truncate().child(self.file_mark_el(file_mark(f.status))).child(format!("{}{}", f.previous.as_ref().map(|p| format!("{p} → ")).unwrap_or_default(), f.path)));
            body.push(div().my(px(10.0)).child(self.label("ALSO CHANGED", Some(others.len()))).child(div().flex().flex_col().gap(px(3.0)).font_family(MONO_FONT).text_size(px(11.5)).text_color(ui.muted).children(rows)).into_any_element());
        }
    }

    fn file_mark_el(&self, m: char) -> AnyElement {
        let ui = &self.ui;
        let bg = match m {
            'A' => ui.added,
            'M' => ui.changed_strong,
            'D' => ui.removed,
            _ => ui.renamed,
        };
        div().flex().flex_none().items_center().justify_center().size(px(15.0)).rounded(px(3.0)).bg(bg).text_color(ui.on_strong).font_family(UI_FONT).font_weight(FontWeight::BOLD).text_size(px(10.0)).mr(px(6.0)).child(m.to_string()).into_any_element()
    }
}

fn file_mark(s: crate::compare::FileStatus) -> char {
    match s {
        crate::compare::FileStatus::Added => 'A',
        crate::compare::FileStatus::Modified => 'M',
        crate::compare::FileStatus::Removed => 'D',
        crate::compare::FileStatus::Renamed => 'R',
    }
}

impl Render for Inspector {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = self.ui.clone();
        let panel = div().size_full().flex().flex_col().bg(ui.panel).border_1().border_color(ui.line).rounded(px(12.0)).shadow(ui.shadow()).font_family(UI_FONT).text_size(px(13.0)).text_color(ui.ink).occlude();
        let Some(s) = self.state.take() else { return panel };
        let mut body: Vec<AnyElement> = vec![];
        let mut reveal = None;
        match s.content.clone() {
            Content::None => {}
            Content::Block(id) => self.block(&s, id, &mut body, &mut reveal, cx),
            Content::Removed(i) => self.removed(&s, i, &mut body, &mut reveal, cx),
            Content::Value { owner, start, edges } => self.value(&s, owner, start, &edges, &mut body, cx),
            Content::Changes => self.changes(&s, &mut body, &mut reveal, cx),
        }
        let content = s.content.clone();
        self.state = Some(s);
        // Bring the marked lines (or the change being read) into view, once for each thing shown.
        if let Some(child) = reveal {
            if self.revealed.as_ref().is_none_or(|(c, k)| *c != content || *k != child) {
                self.revealed = Some((content, child));
                self.scroll.scroll_to_item(child);
            }
        }
        panel.child(div().id("inspector-body").flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.scroll).px(px(16.0)).pt(px(14.0)).pb(px(16.0)).children(body))
    }
}
