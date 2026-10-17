//! The containers the view is in, root first; each goes back out to it
//! (src/ui/Breadcrumbs.tsx).

use crate::theme::{Ui, UI_FONT};
use gpui::{div, prelude::*, px, AnyElement, FontWeight, SharedString};

/// Long paths keep their first and last steps and fold the middle into a menu,
/// folding more (keeping fewer last steps) while the path is wider than its room.
const FOLDS: [(usize, usize); 3] = [(5, 3), (3, 2), (2, 1)];
const MAX_CRUMB: f64 = 220.0;

/// About how wide a crumb is drawn, chevron included: GPUI lays out after
/// this is decided, so the folding is chosen from an estimate.
fn width(title: &str) -> f64 {
    (title.chars().count() as f64 * 7.4 + 14.0).min(MAX_CRUMB) + 14.0
}

/// What is shown: crumb indices, with `None` where the middle is folded, and the folded ones.
pub fn fold(titles: &[SharedString], room: f64) -> (Vec<Option<usize>>, Vec<usize>) {
    let n = titles.len();
    for (level, &(over, keep)) in FOLDS.iter().enumerate() {
        let (shown, hidden): (Vec<Option<usize>>, Vec<usize>) = if n > over {
            (std::iter::once(Some(0)).chain(std::iter::once(None)).chain((n - keep..n).map(Some)).collect(), (1..n - keep).collect())
        } else {
            ((0..n).map(Some).collect(), vec![])
        };
        let wide: f64 = shown.iter().map(|c| c.map_or(30.0, |i| width(&titles[i]))).sum();
        if wide <= room || level == FOLDS.len() - 1 {
            return (shown, hidden);
        }
    }
    unreachable!()
}

/// A crumb as a button: `on_go` is called with its index.
pub fn crumb(ui: &Ui, i: usize, title: SharedString, last: bool, on_go: impl Fn(usize, &mut gpui::Window, &mut gpui::App) + 'static) -> AnyElement {
    div()
        .id(("crumb", i))
        .px(px(7.0))
        .py(px(4.0))
        .rounded(px(6.0))
        .max_w(px(MAX_CRUMB as f32))
        .truncate()
        .cursor_pointer()
        .text_color(if last { ui.ink } else { ui.muted })
        .when(last, |d| d.font_weight(FontWeight::SEMIBOLD))
        .hover(|s| s.bg(ui.hover).text_color(ui.ink))
        .when(i == 0, |d| d.debug_selector(|| "crumb-root".into()))
        .on_click(move |_, window, cx| on_go(i, window, cx))
        .child(title)
        .into_any_element()
}

pub fn chevron(ui: &Ui) -> AnyElement {
    div().text_color(ui.faint).px(px(1.0)).child("›").into_any_element()
}

/// The bar the crumbs sit on.
pub fn bar(ui: &Ui) -> gpui::Div {
    div().flex().items_center().bg(ui.glass).rounded(px(8.0)).p(px(2.0)).font_family(UI_FONT).text_size(px(13.0)).occlude()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_paths_fold_their_middle_and_fold_more_when_narrow() {
        let titles: Vec<SharedString> = ["repo", "src", "canvas", "renderer.ts", "Renderer", "draw", "interior"].map(SharedString::from).to_vec();
        let (shown, hidden) = fold(&titles, 2000.0);
        assert_eq!(shown, [Some(0), None, Some(4), Some(5), Some(6)]);
        assert_eq!(hidden, [1, 2, 3]);
        let (shown, _) = fold(&titles, 200.0);
        assert_eq!(shown, [Some(0), None, Some(6)]);
        let short: Vec<SharedString> = ["repo", "src"].map(SharedString::from).to_vec();
        assert_eq!(fold(&short, 100.0).0, [Some(0), Some(1)]);
    }
}
