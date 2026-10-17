//! Everything drawn around the diagram, ported from src/App.tsx with the
//! values of src/styles.css: the top bar (brand, history, breadcrumbs, the
//! change being read), the toolbar and its menus, the zoom bar, the footer and
//! legend, the details panel's frame, and the dialogs over the diagram.

use crate::crumbs;
use crate::menus;
use crate::icons::icon;
use crate::pen::Pen;
use crate::render::drawn_in;
use crate::theme::{Color, Ui, LEGEND, MONO_FONT, UI_FONT};
use crate::viewer::{Dialog, Menu, Viewer, PANEL_MIN, PANEL_W};
use blockviz_core::ir::EdgeKind;
use gpui::{
    canvas, div, percentage, prelude::*, px, svg, Animation, AnimationExt, AnyElement, App, Bounds, Context, CursorStyle, ExternalPaths,
    FontWeight, Hsla, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, SharedString, Stateful, Transformation,
    Window,
};
use std::time::Duration;

/// A hint shown under the pointer, as a `title` attribute is.
struct Tip(SharedString, Ui);

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let ui = &self.1;
        div().px(px(8.0)).py(px(4.0)).rounded(px(6.0)).bg(ui.panel).border_1().border_color(ui.line).shadow(ui.shadow()).text_size(px(12.0)).text_color(ui.ink).font_family(UI_FONT).child(self.0.clone())
    }
}

fn tip(text: impl Into<SharedString>, ui: &Ui) -> impl Fn(&mut Window, &mut App) -> gpui::AnyView + 'static {
    let (text, ui) = (text.into(), ui.clone());
    move |_, cx| cx.new(|_| Tip(text.clone(), ui.clone())).into()
}

pub fn ico(name: &str, size: f32, color: Hsla) -> gpui::Svg {
    svg().path(icon(name)).size(px(size)).flex_none().text_color(color)
}

/// `.icon`: a square button holding an icon.
pub fn icon_button(id: impl Into<gpui::ElementId>, name: &str, size: f32, side: f32, title: &str, ui: &Ui, enabled: bool) -> Stateful<gpui::Div> {
    let (muted, ink, hover) = (ui.muted, ui.ink, ui.hover);
    div()
        .id(id.into())
        .size(px(side))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(px(7.0))
        .text_color(muted)
        .when(enabled, |d| d.cursor_pointer().hover(move |s| s.bg(hover).text_color(ink)))
        .when(!enabled, |d| d.opacity(0.45))
        .tooltip(tip(title.to_string(), ui))
        .child(svg().path(icon(name)).size(px(size)).text_color(muted))
}

pub fn kbd(text: &str, ui: &Ui) -> gpui::Div {
    div().font_family(MONO_FONT).text_size(px(11.0)).text_color(ui.faint).border_1().border_color(ui.line).rounded(px(4.0)).px(px(5.0)).py(px(1.0)).bg(ui.code_bg).child(text.to_string())
}

/// A small drawing made with the diagram's own pen (the legend's samples).
fn sample(w: f32, h: f32, draw: impl Fn(&mut dyn Pen) + 'static) -> AnyElement {
    canvas(
        |_, _, _| (),
        move |bounds: Bounds<Pixels>, (), window, cx| {
            let mut pen = crate::gpuipen::GpuiPen::new(window, cx, (f64::from(bounds.origin.x), f64::from(bounds.origin.y)));
            draw(&mut pen);
        },
    )
    .w(px(w))
    .h(px(h))
    .flex_none()
    .into_any_element()
}

/// An arrow as the legend draws one: a line and its head, in `color`, dashed by `dash`.
fn arrow_sample(color: Color, width: f64, dash: &'static [f64]) -> AnyElement {
    sample(34.0, 10.0, move |p| {
        p.set_stroke(color);
        p.set_fill(color);
        p.set_line_width(width);
        p.set_dash(dash);
        p.begin_path();
        p.move_to(0.0, 5.0);
        p.line_to(26.0, 5.0);
        p.stroke();
        p.set_dash(&[]);
        p.begin_path();
        p.move_to(26.0, 1.5);
        p.line_to(33.0, 5.0);
        p.line_to(26.0, 8.5);
        p.close_path();
        p.fill();
    })
}

impl Viewer {
    fn brand(&self, cx: &mut Context<Self>) -> AnyElement {
        let ui = &self.ui;
        let cell = |c: Hsla, w: f32| div().w(px(w)).h(px(7.0)).rounded(px(2.0)).border(px(1.2)).border_color(ui.ink).bg(c);
        let hover = ui.hover;
        div()
            .id("brand")
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(9.0))
            .py(px(6.0))
            .rounded(px(8.0))
            .bg(ui.glass)
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .font_weight(FontWeight(650.0))
            .text_size(px(14.0))
            .tooltip(tip("Whole project (H)", ui))
            .on_click(cx.listener(|v, _, _, cx| v.home(cx)))
            .child(div().flex().flex_col().gap(px(2.0)).child(div().flex().gap(px(2.0)).child(cell(ui.logo[0], 7.0)).child(cell(ui.logo[1], 7.0))).child(cell(ui.logo[2], 16.0)))
            .child("blockviz")
            .into_any_element()
    }

    fn topbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let ui = &self.ui;
        let history = div()
            .flex()
            .gap(px(1.0))
            .bg(ui.glass)
            .rounded(px(8.0))
            .mr(px(2.0))
            .child(icon_button("back", "arrow-left", 15.0, 28.0, "Back (⌥←)", ui, self.history.can_back()).on_click(cx.listener(|v, _, _, cx| v.back(cx))))
            .child(icon_button("forward", "arrow-right", 15.0, 28.0, "Forward (⌥→)", ui, self.history.can_forward()).on_click(cx.listener(|v, _, _, cx| v.forward(cx))));
        let mut where_ = div().flex().items_center().gap(px(4.0)).min_w_0();
        if let Some(m) = self.m.clone() {
            let titled: Vec<usize> = (0..self.crumbs.len()).filter(|&i| !m.node(self.crumbs[i]).title.is_empty()).collect();
            if titled.len() <= 1 {
                where_ = where_.child(div().flex_none().font_weight(FontWeight::SEMIBOLD).px(px(7.0)).py(px(4.0)).bg(ui.glass).rounded(px(6.0)).whitespace_nowrap().child(m.scene.title.clone()));
                if self.crumbs.len() <= 1 && self.compare.is_none() && !m.scene.description.is_empty() {
                    where_ = where_.child(div().min_w_0().max_w(px(520.0)).truncate().text_size(px(12.0)).text_color(ui.faint).bg(ui.glass).px(px(6.0)).py(px(4.0)).rounded(px(6.0)).child(m.scene.description.clone()));
                }
            } else {
                where_ = where_.child(self.breadcrumbs(&titled, cx));
            }
            if let Some(c) = &self.compare {
                let s = c.diff.summary();
                let hover = (ui.line_strong, ui.ink);
                where_ = where_.child(
                    div()
                        .id("change-chip")
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .min_w_0()
                        .max_w(px(460.0))
                        .px(px(9.0))
                        .py(px(4.0))
                        .rounded(px(8.0))
                        .bg(ui.glass)
                        .border_1()
                        .border_color(ui.line)
                        .text_color(ui.muted)
                        .cursor_pointer()
                        .hover(move |st| st.border_color(hover.0).text_color(hover.1))
                        .tooltip(tip(format!("{}: show its changes", c.title), ui))
                        .on_click(cx.listener(|v, _, _, cx| v.show_changes(cx)))
                        .child(ico("git-compare", 13.0, ui.muted))
                        .child(div().font_weight(FontWeight(650.0)).text_color(ui.ink).whitespace_nowrap().child(format!("vs {}", c.label)))
                        .when(self.crumbs.len() <= 1, |d| d.child(div().min_w_0().truncate().child(c.title.clone())))
                        .child(
                            div()
                                .flex()
                                .gap(px(5.0))
                                .font_family(MONO_FONT)
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(px(11.5))
                                .whitespace_nowrap()
                                .child(div().text_color(ui.added).child(format!("+{}", s.added)))
                                .child(div().text_color(ui.changed).child(format!("~{}", s.changed)))
                                .child(div().text_color(ui.removed).child(format!("−{}", s.removed))),
                        ),
                );
            }
        }
        div()
            .absolute()
            .top(px(12.0))
            .left(px(12.0))
            .right(px(330.0))
            .flex()
            .items_center()
            .gap(px(4.0))
            .child(self.brand(cx))
            .child(history)
            .child(where_)
            .into_any_element()
    }

    fn breadcrumbs(&self, titled: &[usize], cx: &mut Context<Self>) -> AnyElement {
        let m = self.m.as_ref().unwrap();
        let ui = &self.ui;
        let titles: Vec<SharedString> = titled.iter().map(|&i| m.titles[self.crumbs[i] as usize].clone()).collect();
        let room = self.width - 330.0 - 200.0;
        let (shown, hidden) = crumbs::fold(&titles, room);
        let go = |k: usize| {
            let i = titled[k];
            cx.listener(move |v: &mut Viewer, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Viewer>| {
                v.menu = None;
                let path = if i >= 1 { v.crumbs[..=i].to_vec() } else { vec![v.crumbs[0]] };
                v.fly_to(&path, crate::camera::Framing::Fit, 0.06, cx);
            })
        };
        let mut bar = div().flex().items_center().min_w_0().bg(ui.glass).rounded(px(8.0)).p(px(2.0)).debug_selector(|| "crumbs".into());
        for (j, c) in shown.iter().enumerate() {
            if j > 0 {
                bar = bar.child(ico("chevron-right", 13.0, ui.faint));
            }
            match *c {
                Some(k) => bar = bar.child(crumbs::crumb(ui, k, titles[k].clone(), k == titles.len() - 1).on_click(go(k))),
                None => {
                    let mut fold = div().relative().child(
                        div()
                            .id("crumb-fold")
                            .px(px(7.0))
                            .py(px(4.0))
                            .rounded(px(6.0))
                            .cursor_pointer()
                            .text_color(ui.muted)
                            .hover({
                                let h = ui.hover;
                                move |s| s.bg(h)
                            })
                            .tooltip(tip(hidden.iter().map(|&k| titles[k].to_string()).collect::<Vec<_>>().join(" › "), ui))
                            .on_click(cx.listener(|v, _, _, cx| {
                                v.menu = if v.menu == Some(Menu::Crumbs) { None } else { Some(Menu::Crumbs) };
                                cx.notify();
                            }))
                            .child("…"),
                    );
                    if self.menu == Some(Menu::Crumbs) {
                        let mut menu = div()
                            .absolute()
                            .top(px(30.0))
                            .left_0()
                            .flex()
                            .flex_col()
                            .min_w(px(180.0))
                            .max_w(px(360.0))
                            .p(px(4.0))
                            .bg(ui.panel)
                            .border_1()
                            .border_color(ui.line)
                            .rounded(px(10.0))
                            .shadow(ui.shadow())
                            .occlude();
                        for &k in &hidden {
                            menu = menu.child(crumbs::crumb(ui, k, titles[k].clone(), false).text_color(ui.ink).on_click(go(k)));
                        }
                        fold = fold.child(menu);
                    }
                    bar = bar.child(fold);
                }
            }
        }
        bar.into_any_element()
    }

    /// Inside code reached through a call: where that code is defined.
    fn defined_in(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let m = self.m.as_ref()?;
        let def = m.borrowed_definition(&self.crumbs)?;
        let ui = &self.ui;
        let n = m.node(def);
        let at = m.file_of(n).zip(n.span.as_ref()).map(|(f, s)| format!(" · {}:{}", f.rsplit('/').next().unwrap_or(f), s.line));
        let hover = ui.accent_bg_strong;
        Some(
            div()
                .id("defined-in")
                .absolute()
                .top(px(50.0))
                .left(px(12.0))
                .max_w(px((self.width - 440.0).max(200.0) as f32))
                .flex()
                .px(px(10.0))
                .py(px(4.0))
                .rounded(px(8.0))
                .bg(ui.accent_bg)
                .text_color(ui.muted)
                .text_size(px(12.0))
                .shadow(ui.shadow())
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .truncate()
                .tooltip(tip("Go to where this code is defined", ui))
                .on_click(cx.listener(move |v, _, _, cx| v.go_to(def, cx)))
                .child("defined in ")
                .child(div().text_color(ui.accent).font_weight(FontWeight::SEMIBOLD).child(m.qualified(def)))
                .when_some(at, |d, at| d.child(div().text_color(ui.faint).child(at)))
                .into_any_element(),
        )
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let ui = &self.ui;
        let (line_strong, muted) = (ui.line_strong, ui.muted);
        let search = div()
            .id("search-button")
            .flex()
            .items_center()
            .gap(px(8.0))
            .h(px(32.0))
            .pl(px(10.0))
            .pr(px(8.0))
            .border_1()
            .border_color(ui.line)
            .rounded(px(8.0))
            .text_color(ui.faint)
            .min_w(px(190.0))
            .bg(ui.panel)
            .cursor_pointer()
            .hover(move |s| s.border_color(line_strong).text_color(muted))
            .on_click(cx.listener(|v, _, window, cx| v.open_search(window, cx)))
            .child(ico("search", 15.0, ui.faint))
            .child(div().flex_1().child("Search"))
            .child(kbd("⌘K", ui));
        let repo = self.project.as_ref().is_some_and(|p| p.repo.is_some());
        div()
            .absolute()
            .top(px(12.0))
            .right(px(12.0))
            .flex()
            .items_center()
            .gap(px(4.0))
            .bg(ui.glass)
            .rounded(px(10.0))
            .p(px(3.0))
            .when(self.m.is_some(), |d| d.child(search))
            .when(repo, |d| d.child(icon_button("compare", "git-compare", 17.0, 30.0, "Compare with an earlier version", ui, true).on_click(cx.listener(|v, _, _, cx| v.open_compare_menu(cx)))))
            .child(icon_button("open", "folder-open", 17.0, 30.0, "Open a codebase (⌘O)", ui, true).on_click(cx.listener(|v, _, _, cx| {
                v.dialog = Some(Dialog::Open);
                cx.notify();
            })))
            .when(self.m.is_some(), |d| {
                d.child(icon_button("download", "download", 17.0, 30.0, "Download this view", ui, true).on_click(cx.listener(|v, _, _, cx| {
                    v.menu = if v.menu == Some(Menu::Download) { None } else { Some(Menu::Download) };
                    cx.notify();
                })))
            })
            .child(
                icon_button("scheme", if self.dark() { "sun" } else { "moon" }, 17.0, 30.0, if self.dark() { "Light mode" } else { "Dark mode" }, ui, true)
                    .on_click(cx.listener(|v, _, _, cx| v.toggle_scheme(cx))),
            )
            .into_any_element()
    }

    fn menu_item(&self, id: impl Into<gpui::ElementId>, label: impl IntoElement) -> Stateful<gpui::Div> {
        let hover = self.ui.hover;
        div().id(id.into()).w_full().px(px(10.0)).py(px(7.0)).rounded(px(7.0)).font_weight(FontWeight::SEMIBOLD).cursor_pointer().hover(move |s| s.bg(hover)).child(label)
    }

    fn menu_frame(&self, min_w: f32) -> gpui::Div {
        let ui = &self.ui;
        div()
            .absolute()
            .top(px(56.0))
            .right(px(12.0))
            .flex()
            .flex_col()
            .p(px(4.0))
            .min_w(px(min_w))
            .border_1()
            .border_color(ui.line)
            .rounded(px(10.0))
            .bg(ui.panel)
            .text_color(ui.ink)
            .shadow(ui.shadow())
            .occlude()
    }

    fn menus(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ui = &self.ui;
        match self.menu? {
            Menu::Download => Some(
                self.menu_frame(120.0)
                    .child(self.menu_item("svg", "SVG").on_click(cx.listener(|v, _, window, cx| v.download(false, window, cx))))
                    .child(self.menu_item("png", "PNG").on_click(cx.listener(|v, _, window, cx| v.download(true, window, cx))))
                    .into_any_element(),
            ),
            Menu::Compare => {
                let line = |title: String, detail: String| div().flex().flex_col().child(title).child(div().text_size(px(11.5)).font_weight(FontWeight::NORMAL).text_color(ui.faint).child(detail));
                let mut menu = self.menu_frame(320.0).max_w(px(420.0)).child(
                    self.menu_item("uncommitted", line("Uncommitted changes".into(), "The folder as it is, against the last commit (HEAD)".into()))
                        .on_click(cx.listener(|v, _, _, cx| v.start_compare("HEAD".into(), "Uncommitted changes".into(), "HEAD".into(), "Working tree vs HEAD".into(), cx))),
                );
                if let Some(base) = self.default_branch.clone() {
                    let short = base.trim_start_matches("origin/").to_string();
                    if self.branch.as_deref() != Some(short.as_str()) {
                        menu = menu.child(self.menu_item("branch", line("This branch".into(), format!("Everything since it left {short}"))).on_click(cx.listener(|v, _, _, cx| v.compare_branch(cx))));
                    }
                }
                if !self.log.is_empty() {
                    menu = menu.child(div().px(px(10.0)).pt(px(8.0)).pb(px(4.0)).text_size(px(11.0)).font_weight(FontWeight::SEMIBOLD).text_color(ui.faint).child("SINCE A COMMIT"));
                    for (i, c) in self.log.iter().enumerate() {
                        let (sha, short, subject) = (c.sha.clone(), c.short.clone(), c.subject.clone());
                        menu = menu.child(
                            self.menu_item(("commit", i), div().flex().gap(px(8.0)).min_w_0().child(div().flex_none().font_family(MONO_FONT).text_size(px(11.5)).text_color(ui.accent).child(c.short.clone())).child(div().min_w_0().truncate().font_weight(FontWeight::MEDIUM).child(c.subject.clone())))
                                .on_click(cx.listener(move |v, _, _, cx| v.start_compare(sha.clone(), "Since a commit".into(), short.clone(), subject.clone(), cx))),
                        );
                    }
                }
                if self.compare.is_some() {
                    menu = menu.child(div().h(px(1.0)).my(px(4.0)).bg(ui.line)).child(self.menu_item("stop", "Stop comparing").text_color(ui.removed).on_click(cx.listener(|v, _, _, cx| v.stop_compare(cx))));
                }
                Some(menu.into_any_element())
            }
            Menu::Crumbs => None,
        }
    }

    fn zoombar(&self, cx: &mut Context<Self>) -> AnyElement {
        let ui = &self.ui;
        let on = self.trunks;
        let (hover, ink) = (ui.hover, ui.ink);
        let switch = div()
            .id("trunks")
            .flex()
            .items_center()
            .gap(px(6.0))
            .h(px(30.0))
            .pl(px(7.0))
            .pr(px(6.0))
            .rounded(px(7.0))
            .text_color(if on { ui.accent } else { ui.muted })
            .cursor_pointer()
            .hover(move |s| if on { s.bg(hover) } else { s.bg(hover).text_color(ink) })
            .tooltip(tip("Tidy arrows: draw one branching line out of each block, not a separate line for every value", ui))
            .on_click(cx.listener(|v, _, _, cx| {
                let on = !v.trunks;
                v.set_trunks(on, cx);
            }))
            .child(ico("split", 15.0, if on { ui.accent } else { ui.muted }))
            .child(
                div()
                    .relative()
                    .w(px(26.0))
                    .h(px(15.0))
                    .rounded(px(8.0))
                    .bg(if on { ui.accent } else { ui.line_strong })
                    .child(div().absolute().top(px(2.0)).left(px(if on { 13.0 } else { 2.0 })).size(px(11.0)).rounded_full().bg(gpui::white()).shadow(vec![gpui::BoxShadow { color: gpui::hsla(0.0, 0.0, 0.0, 0.2), offset: gpui::point(px(0.0), px(1.0)), blur_radius: px(2.0), spread_radius: px(0.0) }])),
            );
        div()
            .absolute()
            .right(px(12.0))
            .bottom(px(12.0))
            .flex()
            .items_center()
            .gap(px(2.0))
            .bg(ui.glass)
            .border_1()
            .border_color(ui.line)
            .rounded(px(10.0))
            .p(px(3.0))
            .shadow(ui.shadow())
            .child(icon_button("zoom-out", "minus", 15.0, 30.0, "Zoom out (−)", ui, true).on_click(cx.listener(|v, _, _, cx| v.zoom_by(1.0 / 1.5, cx))))
            .child(icon_button("zoom-in", "plus", 15.0, 30.0, "Zoom in (+)", ui, true).on_click(cx.listener(|v, _, _, cx| v.zoom_by(1.5, cx))))
            .child(icon_button("fit", "maximize", 15.0, 30.0, "Fit (F)", ui, true).on_click(cx.listener(|v, _, _, cx| v.fit(cx))))
            .child(icon_button("home", "house", 15.0, 30.0, "Whole project (H)", ui, true).on_click(cx.listener(|v, _, _, cx| v.home(cx))))
            .child(div().w(px(1.0)).h(px(18.0)).bg(ui.line).mx(px(3.0)))
            .child(switch)
            .into_any_element()
    }

    fn footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let ui = &self.ui;
        let on = self.legend;
        let (ink, line_strong) = (ui.ink, ui.line_strong);
        let toggle = div()
            .id("legend-toggle")
            .flex()
            .items_center()
            .gap(px(6.0))
            .px(px(10.0))
            .py(px(6.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(if on { ui.line_strong } else { ui.line })
            .bg(ui.glass)
            .text_color(if on { ui.ink } else { ui.muted })
            .cursor_pointer()
            .hover(move |s| s.text_color(ink).border_color(line_strong))
            .on_click(cx.listener(|v, _, _, cx| {
                v.legend = !v.legend;
                cx.notify();
            }))
            .child(ico("info", 13.0, if on { ui.ink } else { ui.muted }))
            .child("Legend");
        let stats = self.m.as_ref().map(|m| {
            let s = &m.scene.stats;
            let ms = self.project.as_ref().map_or(0, |p| p.elapsed.as_millis());
            let n = |v: u64| {
                let t = v.to_string();
                let mut out = String::new();
                for (i, c) in t.chars().enumerate() {
                    if i > 0 && (t.len() - i) % 3 == 0 {
                        out.push(',');
                    }
                    out.push(c);
                }
                out
            };
            div().bg(ui.glass).px(px(6.0)).py(px(3.0)).rounded(px(6.0)).child(format!("{} files · {} lines · {} blocks · {} ms", s.files, n(s.lines as u64), n(s.nodes as u64), ms))
        });
        div()
            .absolute()
            .left(px(12.0))
            .bottom(px(12.0))
            .flex()
            .items_center()
            .gap(px(12.0))
            .text_color(ui.faint)
            .text_size(px(11.5))
            .child(toggle)
            .when_some(stats, |d, s| d.child(s))
            .into_any_element()
    }

    fn legend_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.legend {
            return None;
        }
        let m = self.m.as_ref()?;
        let ui = &self.ui;
        let theme = self.theme.clone();
        let drawn = drawn_in(m);
        let item = |swatch: AnyElement, label: &str| div().flex().items_center().gap(px(7.0)).text_color(ui.muted).text_size(px(12.0)).child(swatch).child(label.to_string());
        let mut grid = div().grid().grid_cols(2).gap_x(px(10.0)).gap_y(px(6.0));
        let mut roles = 0;
        for (cat, label) in LEGEND {
            if drawn.roles.contains(cat) {
                roles += 1;
                let sw = div().w(px(18.0)).h(px(12.0)).flex_none().border(px(1.1)).rounded(px(3.0)).bg(theme.swatch(cat).fill.hsla()).border_color(theme.outline.hsla()).into_any_element();
                grid = grid.child(item(sw, label));
            }
        }
        let mut lines = div().grid().grid_cols(2).gap_x(px(10.0)).gap_y(px(6.0));
        let mut any_lines = false;
        if drawn.edges.contains(&EdgeKind::Data) || drawn.edges.contains(&EdgeKind::Uses) {
            any_lines = true;
            lines = lines.child(item(arrow_sample(theme.edge, 1.4, &[]), "value flows into"));
        }
        if drawn.edges.contains(&EdgeKind::Control) {
            any_lines = true;
            lines = lines.child(item(arrow_sample(theme.context, 1.2, &[1.5, 2.5]), "decides whether it runs"));
        }
        if drawn.edges.contains(&EdgeKind::Artifact) {
            any_lines = true;
            lines = lines.child(item(arrow_sample(theme.artifact, 1.4, &[6.0, 3.0]), "file written → read"));
        }
        if drawn.edges.contains(&EdgeKind::Exec) {
            any_lines = true;
            lines = lines.child(item(arrow_sample(theme.faint_edge, 1.4, &[5.0, 4.0]), "code executed"));
        }
        if drawn.add {
            any_lines = true;
            let t = theme.clone();
            let add = sample(34.0, 14.0, move |p| {
                p.set_fill(t.paper);
                p.set_stroke(t.outline);
                p.set_line_width(1.0);
                p.begin_path();
                p.arc(17.0, 7.0, 6.0, 0.0, std::f64::consts::TAU);
                p.fill();
                p.stroke();
                p.begin_path();
                p.move_to(13.5, 7.0);
                p.line_to(20.5, 7.0);
                p.move_to(17.0, 3.5);
                p.line_to(17.0, 10.5);
                p.stroke();
            });
            lines = lines.child(item(add, "add"));
        }
        if drawn.effects {
            any_lines = true;
            let t = theme.clone();
            let dot = sample(34.0, 14.0, move |p| {
                p.set_fill(t.effect);
                p.begin_path();
                p.arc(17.0, 7.0, 3.5, 0.0, std::f64::consts::TAU);
                p.fill();
            });
            lines = lines.child(item(dot, "has side effects"));
        }
        let mut changes = None;
        if let Some(c) = self.compare.as_ref().filter(|c| !c.diff.changes.is_empty()) {
            let s = c.diff.summary();
            let boxed = |tint: Color, ink: Color| div().w(px(18.0)).h(px(12.0)).flex_none().border(px(1.6)).rounded(px(3.0)).bg(tint.hsla()).border_color(ink.hsla()).into_any_element();
            let mut g = div().grid().grid_cols(2).gap_x(px(10.0)).gap_y(px(6.0));
            if s.added > 0 {
                g = g.child(item(boxed(theme.added.tint, theme.added.ink), "added"));
            }
            if s.changed > 0 {
                g = g.child(item(boxed(theme.changed.tint, theme.changed.ink), "changed"));
            }
            if !c.diff.added.is_empty() {
                g = g.child(item(arrow_sample(theme.added.ink, 2.0, &[]), "new connection"));
            }
            if !c.diff.removed.is_empty() {
                g = g.child(item(arrow_sample(theme.removed.ink, 1.6, &[5.0, 4.0]), "removed connection"));
            }
            let count = div().flex().items_center().justify_center().min_w(px(18.0)).h(px(13.0)).rounded(px(7.0)).bg(ui.changed_strong).text_color(ui.on_strong).text_size(px(9.5)).font_weight(FontWeight::SEMIBOLD).child("3").into_any_element();
            g = g.child(item(count, "changes inside"));
            let dashed = div().w(px(18.0)).h(px(12.0)).flex_none().border(px(1.6)).border_dashed().rounded(px(3.0)).bg(ui.bg).border_color(theme.changed.ink.hsla()).into_any_element();
            g = g.child(item(dashed, "leads into changes"));
            changes = Some(g);
        }
        let section = |d: gpui::Div| d.mt(px(12.0)).pt(px(10.0)).border_t_1().border_color(ui.line);
        Some(
            div()
                .id("legend")
                .absolute()
                .left(px(12.0))
                .bottom(px(52.0))
                .w(px(300.0))
                .bg(ui.panel)
                .border_1()
                .border_color(ui.line)
                .rounded(px(12.0))
                .shadow(ui.shadow())
                .px(px(14.0))
                .py(px(12.0))
                .occlude()
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .items_center()
                        .mb(px(8.0))
                        .child(div().font_weight(FontWeight::BOLD).child("Legend"))
                        .child(icon_button("legend-close", "x", 14.0, 26.0, "Close legend", ui, true).on_click(cx.listener(|v, _, _, cx| {
                            v.legend = false;
                            cx.notify();
                        }))),
                )
                .when(roles > 0, |d| d.child(grid))
                .when(any_lines, |d| d.child(if roles > 0 { section(lines) } else { lines }))
                .when_some(changes, |d, g| d.child(section(g)))
                .child(div().mt(px(10.0)).text_color(ui.faint).text_size(px(11.5)).line_height(px(16.7)).child(format!(
                    "Bigger blocks contain more. Zoom into any block with a corner mark to see inside.{}",
                    if self.compare.is_some() { " What the change did not touch is faded." } else { "" }
                )))
                .into_any_element(),
        )
    }

    fn panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.panel_open() || self.m.is_none() {
            return None;
        }
        let ui = &self.ui;
        let grip_color = if self.resizing.is_some() { ui.accent } else { ui.line_strong };
        let handle = div()
            .id("panel-resize")
            .absolute()
            .left(px(-5.0))
            .top_0()
            .bottom_0()
            .w(px(10.0))
            .cursor(CursorStyle::ResizeColumn)
            .tooltip(tip("Drag to resize · double-click to reset", ui))
            .on_mouse_down(MouseButton::Left, cx.listener(|v, e: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                if e.click_count == 2 {
                    v.panel_w = PANEL_W;
                    v.settings.panel_w = None;
                    v.settings.save();
                    v.resizing = None;
                    v.sync_panel(cx);
                } else {
                    v.resizing = Some((f32::from(e.position.x), v.panel_w));
                }
                cx.notify();
            }))
            .child(div().absolute().left(px(3.0)).top(gpui::relative(0.5)).mt(px(-18.0)).w(px(4.0)).h(px(36.0)).rounded(px(2.0)).bg(grip_color).opacity(if self.resizing.is_some() { 1.0 } else { 0.0 }));
        Some(
            div()
                .absolute()
                .top(px(60.0))
                .right(px(12.0))
                .bottom(px(64.0))
                .w(px(self.panel_w as f32))
                .debug_selector(|| "inspector".into())
                .child(gpui::AnyView::from(self.inspector.clone()).cached(gpui::StyleRefinement::default().size_full()))
                .child(handle)
                .into_any_element(),
        )
    }

    fn busy_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let text = self.busy.clone()?;
        let ui = &self.ui;
        Some(
            div()
                .absolute()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .bg(ui.panel)
                        .border_1()
                        .border_color(ui.line)
                        .rounded(px(12.0))
                        .px(px(16.0))
                        .py(px(12.0))
                        .shadow(ui.shadow())
                        .text_color(ui.muted)
                        .occlude()
                        .child(svg().path(icon("spinner")).size(px(14.0)).text_color(ui.ink).with_animation("spin", Animation::new(Duration::from_millis(800)).repeat(), |s, t| s.with_transformation(Transformation::rotate(percentage(t)))))
                        .child(text)
                        .child(div().id("cancel").text_color(ui.accent).font_weight(FontWeight::MEDIUM).cursor_pointer().on_click(cx.listener(|v, _, _, cx| v.cancel(cx))).child("Cancel")),
                )
                .into_any_element(),
        )
    }

    fn toast(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let error = self.error.clone()?;
        let ui = &self.ui;
        Some(
            div()
                .absolute()
                .bottom(px(20.0))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .max_w(px(560.0))
                        .bg(ui.error_bg)
                        .border_1()
                        .border_color(ui.error_line)
                        .text_color(ui.error_ink)
                        .pl(px(14.0))
                        .pr(px(8.0))
                        .py(px(8.0))
                        .rounded(px(10.0))
                        .shadow(ui.shadow())
                        .occlude()
                        .child(error)
                        .child(icon_button("dismiss", "x", 14.0, 26.0, "Dismiss", ui, true).on_click(cx.listener(|v, _, _, cx| {
                            v.error = None;
                            cx.notify();
                        }))),
                )
                .into_any_element(),
        )
    }

    fn scrim(&self) -> gpui::Div {
        div().absolute().size_full().bg(self.ui.scrim).flex().items_start().justify_center().pt(px((self.height * 0.12) as f32)).px(px(16.0)).occlude()
    }

    fn open_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        let ui = &self.ui;
        let can_close = self.m.is_some();
        let (hover, line_strong, ink, strong) = (ui.hover, ui.line_strong, ui.ink, ui.ink_strong);
        let mut recent = div().grid().grid_cols(2).gap(px(8.0)).mt(px(12.0));
        let mut any = false;
        for (i, f) in self.settings.recent.iter().enumerate() {
            let path = std::path::PathBuf::from(f);
            if !path.is_dir() {
                continue;
            }
            any = true;
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| f.clone());
            let home = std::env::var("HOME").unwrap_or_default();
            let shown = if !home.is_empty() && f.starts_with(&home) { format!("~{}", &f[home.len()..]) } else { f.clone() };
            recent = recent.child(
                div()
                    .id(("recent", i))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .px(px(12.0))
                    .py(px(10.0))
                    .border_1()
                    .border_color(ui.line)
                    .rounded(px(10.0))
                    .text_color(ui.muted)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover).border_color(line_strong).text_color(ink))
                    .on_click(cx.listener(move |v, _, _, cx| v.open_folder(path.clone(), cx)))
                    .child(ico("folder", 17.0, ui.muted))
                    .child(div().flex().flex_col().min_w_0().child(div().text_color(ui.ink).font_weight(FontWeight::SEMIBOLD).text_size(px(12.5)).truncate().child(name)).child(div().text_color(ui.faint).text_size(px(11.5)).mt(px(1.0)).truncate().child(shown))),
            );
        }
        self.scrim()
            .id("open-scrim")
            .on_mouse_down(MouseButton::Left, cx.listener(move |v, _, window, cx| {
                if can_close {
                    v.close_dialog(window, cx);
                }
            }))
            .child(
                div()
                    .id("open-dialog")
                    .w(px((self.width - 32.0).min(620.0) as f32))
                    .bg(ui.panel)
                    .border_1()
                    .border_color(ui.line)
                    .rounded(px(16.0))
                    .shadow(ui.shadow_large())
                    .pt(px(20.0))
                    .px(px(22.0))
                    .pb(px(22.0))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .items_start()
                            .gap(px(12.0))
                            .child(
                                div()
                                    .child(div().text_size(px(18.0)).font_weight(FontWeight::BOLD).mb(px(4.0)).child("Open a codebase"))
                                    .child(div().text_color(ui.muted).mb(px(16.0)).line_height(px(19.0)).child("Code is analyzed on this Mac, in Rust. Nothing is uploaded or executed. The diagram follows the folder as you edit it, and in a git repository Compare shows what a change did.")),
                            )
                            .when(can_close, |d| d.child(icon_button("open-close", "x", 16.0, 30.0, "Close", ui, true).on_click(cx.listener(|v, _, window, cx| v.close_dialog(window, cx))))),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .border_1()
                            .border_color(ui.line_strong)
                            .rounded(px(10.0))
                            .pl(px(12.0))
                            .pr(px(4.0))
                            .py(px(4.0))
                            .text_color(ui.muted)
                            .child(ico("folder-open", 17.0, ui.muted))
                            .child(div().flex_1().py(px(8.0)).text_size(px(14.0)).text_color(ui.faint).child("A folder on this Mac, or drop one on the window"))
                            .child(
                                div()
                                    .id("choose")
                                    .bg(ui.ink)
                                    .text_color(ui.bg)
                                    .px(px(14.0))
                                    .py(px(8.0))
                                    .rounded(px(8.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .cursor_pointer()
                                    .hover(move |s| s.bg(strong))
                                    .on_click(cx.listener(|v, _, _, cx| v.prompt_folder(cx)))
                                    .child("Choose…"),
                            ),
                    )
                    .when(any, |d| d.child(div().mt(px(16.0)).text_size(px(11.0)).font_weight(FontWeight::SEMIBOLD).text_color(ui.faint).child("RECENT")).child(recent)),
            )
            .into_any_element()
    }

    fn search_palette(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Some(Dialog::Search(s)) = &self.dialog else { return None };
        let m = self.m.as_ref()?;
        let ui = &self.ui;
        let rows = s.results.iter().enumerate().map(|(i, &id)| {
            let n = m.node(id);
            div()
                .id(("result", i))
                .flex()
                .flex_col()
                .gap(px(1.0))
                .w_full()
                .px(px(10.0))
                .py(px(7.0))
                .rounded(px(8.0))
                .cursor_pointer()
                .when(i == s.active, |d| d.bg(ui.accent_bg))
                .on_hover(cx.listener(move |v, hovered: &bool, _, cx| {
                    if let Some(Dialog::Search(s)) = v.dialog.as_mut().filter(|_| *hovered) {
                        s.active = i;
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(move |v, _, window, cx| v.pick_result(i, window, cx)))
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(8.0))
                        .font_weight(FontWeight(550.0))
                        .child(n.title.clone())
                        .when(!n.subtitle.is_empty(), |d| d.child(div().font_weight(FontWeight::NORMAL).text_size(px(12.0)).text_color(ui.faint).child(n.subtitle.clone()))),
                )
                .child(div().text_size(px(11.5)).text_color(ui.faint).truncate().child(m.trail(id)))
        });
        let list = div()
            .id("search-results")
            .max_h(px((self.height * 0.52) as f32))
            .overflow_y_scroll()
            .track_scroll(&s.scroll)
            .p(px(6.0))
            .when(s.results.is_empty(), |d| d.child(div().p(px(18.0)).text_color(ui.faint).flex().justify_center().child("No matches")))
            .children(rows);
        Some(
            self.scrim()
                .id("search-scrim")
                .on_mouse_down(MouseButton::Left, cx.listener(|v, _, window, cx| v.close_dialog(window, cx)))
                .child(
                    div()
                        .w(px((self.width - 32.0).min(640.0) as f32))
                        .bg(ui.panel)
                        .border_1()
                        .border_color(ui.line)
                        .rounded(px(14.0))
                        .shadow(ui.shadow_large())
                        .overflow_hidden()
                        .debug_selector(|| "search".into())
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_key_down(cx.listener(Self::search_key))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(10.0))
                                .px(px(14.0))
                                .py(px(12.0))
                                .border_b_1()
                                .border_color(ui.line)
                                .text_color(ui.ink)
                                .text_size(px(15.0))
                                .child(ico("search", 16.0, ui.faint))
                                .child(s.input.clone())
                                .child(kbd("esc", ui)),
                        )
                        .child(list),
                )
                .into_any_element(),
        )
    }

    fn search_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Dialog::Search(s)) = self.dialog.as_mut() else { return };
        match e.keystroke.key.as_str() {
            "down" => s.active = (s.active + 1).min(s.results.len().saturating_sub(1)),
            "up" => s.active = s.active.saturating_sub(1),
            "enter" => {
                let i = s.active;
                cx.stop_propagation();
                return self.pick_result(i, window, cx);
            }
            "escape" => {
                cx.stop_propagation();
                return self.close_dialog(window, cx);
            }
            _ => return,
        }
        s.scroll.scroll_to_item(s.active);
        cx.stop_propagation();
        cx.notify();
    }

    fn open_recent(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Some(f) = self.settings.recent.get(i).cloned() {
            self.open_folder(std::path::PathBuf::from(f), cx);
        }
    }

    fn resize_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some((x0, w0)) = self.resizing else { return };
        if !e.dragging() {
            return self.end_resize(cx);
        }
        let max = (self.width - 160.0).max(PANEL_MIN);
        self.panel_w = (w0 - (f32::from(e.position.x) - x0) as f64).clamp(PANEL_MIN, max);
        self.camera.insets = self.insets();
        self.sync_panel(cx);
        cx.notify();
    }

    fn end_resize(&mut self, cx: &mut Context<Self>) {
        if self.resizing.take().is_some() {
            self.settings.panel_w = Some(self.panel_w);
            self.settings.save();
            cx.notify();
        }
    }
}


impl Render for Viewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if window.window_title() != self.title {
            window.set_window_title(&self.title);
        }
        let ui = self.ui.clone();
        let loaded = self.m.is_some();
        let mut root = div()
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(ui.bg)
            .font_family(UI_FONT)
            .text_size(px(13.0))
            .text_color(ui.ink)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .on_action(cx.listener(|v, _: &menus::Open, _, cx| v.prompt_folder(cx)))
            .on_action(cx.listener(|_, _: &menus::CloseWindow, window, _| window.remove_window()))
            .on_action(cx.listener(|v, _: &menus::DownloadSvg, window, cx| v.download(false, window, cx)))
            .on_action(cx.listener(|v, _: &menus::DownloadPng, window, cx| v.download(true, window, cx)))
            .on_action(cx.listener(|v, _: &menus::Search, window, cx| v.toggle_search(window, cx)))
            .on_action(cx.listener(|v, _: &menus::Back, _, cx| v.back(cx)))
            .on_action(cx.listener(|v, _: &menus::Forward, _, cx| v.forward(cx)))
            .on_action(cx.listener(|v, _: &menus::ZoomIn, _, cx| v.zoom_by(1.5, cx)))
            .on_action(cx.listener(|v, _: &menus::ZoomOut, _, cx| v.zoom_by(1.0 / 1.5, cx)))
            .on_action(cx.listener(|v, _: &menus::Fit, _, cx| v.fit(cx)))
            .on_action(cx.listener(|v, _: &menus::Home, _, cx| v.home(cx)))
            .on_action(cx.listener(|v, _: &menus::ToggleLegend, _, cx| {
                v.legend = !v.legend;
                cx.notify();
            }))
            .on_action(cx.listener(|v, _: &menus::ToggleTrunks, _, cx| {
                let on = !v.trunks;
                v.set_trunks(on, cx);
            }))
            .on_action(cx.listener(|v, _: &menus::ToggleScheme, _, cx| v.toggle_scheme(cx)))
            .on_action(cx.listener(|_, _: &menus::FullScreen, window, _| window.toggle_fullscreen()))
            .on_action(cx.listener(|v, _: &menus::CompareUncommitted, _, cx| v.compare_uncommitted(cx)))
            .on_action(cx.listener(|v, _: &menus::CompareBranch, _, cx| v.compare_branch(cx)))
            .on_action(cx.listener(|v, _: &menus::StopComparing, _, cx| v.stop_compare(cx)))
            .on_action(cx.listener(|_, _: &menus::Minimize, window, _| window.minimize_window()))
            .on_action(cx.listener(|_, _: &menus::Zoom, window, _| window.zoom_window()))
            .on_action(cx.listener(|v, _: &menus::OpenRecent0, _, cx| v.open_recent(0, cx)))
            .on_action(cx.listener(|v, _: &menus::OpenRecent1, _, cx| v.open_recent(1, cx)))
            .on_action(cx.listener(|v, _: &menus::OpenRecent2, _, cx| v.open_recent(2, cx)))
            .on_action(cx.listener(|v, _: &menus::OpenRecent3, _, cx| v.open_recent(3, cx)))
            .on_action(cx.listener(|v, _: &menus::OpenRecent4, _, cx| v.open_recent(4, cx)))
            .on_action(cx.listener(|v, _: &menus::OpenRecent5, _, cx| v.open_recent(5, cx)))
            .on_action(cx.listener(|v, _: &menus::OpenRecent6, _, cx| v.open_recent(6, cx)))
            .on_action(cx.listener(|v, _: &menus::OpenRecent7, _, cx| v.open_recent(7, cx)))
            .on_mouse_move(cx.listener(Self::resize_move))
            .on_mouse_up(MouseButton::Left, cx.listener(|v, _: &MouseUpEvent, _, cx| v.end_resize(cx)))
            .drag_over::<ExternalPaths>(|s, _, _, _| s.border_2().border_dashed().border_color(gpui::hsla(0.0, 0.0, 0.5, 0.6)))
            .on_drop(cx.listener(|v, paths: &ExternalPaths, _, cx| {
                if let Some(p) = paths.paths().first() {
                    let dir = if p.is_dir() { p.clone() } else { p.parent().map(|d| d.to_path_buf()).unwrap_or_else(|| p.clone()) };
                    v.open_folder(dir, cx);
                }
            }))
            .child(self.canvas(cx));
        if loaded {
            root = root.child(self.topbar(cx)).when_some(self.defined_in(cx), |d, e| d.child(e));
        }
        root = root.child(self.toolbar(cx)).when_some(self.menus(cx), |d, e| d.child(e));
        if loaded {
            root = root.child(self.zoombar(cx)).child(self.footer(cx)).when_some(self.legend_panel(cx), |d, e| d.child(e)).when_some(self.panel(cx), |d, e| d.child(e));
        }
        root = root.when_some(self.busy_overlay(cx), |d, e| d.child(e)).when_some(self.toast(cx), |d, e| d.child(e));
        match &self.dialog {
            Some(Dialog::Open) => root = root.child(self.open_dialog(cx)),
            Some(Dialog::Search(_)) => root = root.when_some(self.search_palette(cx), |d, e| d.child(e)),
            None if !loaded && self.busy.is_none() => root = root.child(self.open_dialog(cx)),
            None => {}
        }
        root
    }
}
