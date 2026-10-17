//! The macOS menu bar: what the toolbar and keys do, where a Mac app keeps it.

use crate::input;
use gpui::{actions, App, KeyBinding, Menu, MenuItem, OsAction, SystemMenuType};
use std::path::Path;

actions!(
    blockviz,
    [
        Quit,
        Hide,
        HideOthers,
        ShowAll,
        Open,
        CloseWindow,
        DownloadSvg,
        DownloadPng,
        Search,
        Back,
        Forward,
        ZoomIn,
        ZoomOut,
        Fit,
        Home,
        ToggleLegend,
        ToggleTrunks,
        ToggleScheme,
        FullScreen,
        CompareUncommitted,
        CompareBranch,
        StopComparing,
        Minimize,
        Zoom,
        OpenRecent0,
        OpenRecent1,
        OpenRecent2,
        OpenRecent3,
        OpenRecent4,
        OpenRecent5,
        OpenRecent6,
        OpenRecent7,
    ]
);

/// The shortcuts the menus show, bound wherever the window has focus.
pub fn bind(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("alt-cmd-h", HideOthers, None),
        KeyBinding::new("cmd-o", Open, None),
        KeyBinding::new("cmd-w", CloseWindow, None),
        KeyBinding::new("cmd-k", Search, None),
        KeyBinding::new("cmd-[", Back, None),
        KeyBinding::new("cmd-]", Forward, None),
        KeyBinding::new("cmd-=", ZoomIn, None),
        KeyBinding::new("cmd--", ZoomOut, None),
        KeyBinding::new("cmd-0", Home, None),
        KeyBinding::new("ctrl-cmd-f", FullScreen, None),
        KeyBinding::new("cmd-m", Minimize, None),
    ]);
}

/// Opening the `i`th recent folder, as an action a menu item can carry.
pub fn open_recent(i: usize) -> Box<dyn gpui::Action> {
    match i {
        0 => Box::new(OpenRecent0),
        1 => Box::new(OpenRecent1),
        2 => Box::new(OpenRecent2),
        3 => Box::new(OpenRecent3),
        4 => Box::new(OpenRecent4),
        5 => Box::new(OpenRecent5),
        6 => Box::new(OpenRecent6),
        _ => Box::new(OpenRecent7),
    }
}

/// The menus, with the folders opened lately under File › Open Recent.
pub fn menus(recent: &[String]) -> Vec<Menu> {
    let mut recent_items: Vec<MenuItem> = recent
        .iter()
        .take(8)
        .enumerate()
        .map(|(i, f)| {
            let name = Path::new(f).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| f.clone());
            MenuItem::Action { name: name.into(), action: open_recent(i), os_action: None }
        })
        .collect();
    if recent_items.is_empty() {
        recent_items.push(MenuItem::Action { name: "No Recent Folders".into(), action: Box::new(Open), os_action: None });
    }
    vec![
        Menu {
            name: "blockviz".into(),
            items: vec![
                MenuItem::os_submenu("Services", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("Hide blockviz", Hide),
                MenuItem::action("Hide Others", HideOthers),
                MenuItem::action("Show All", ShowAll),
                MenuItem::separator(),
                MenuItem::action("Quit blockviz", Quit),
            ],
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("Open…", Open),
                MenuItem::submenu(Menu { name: "Open Recent".into(), items: recent_items }),
                MenuItem::separator(),
                MenuItem::action("Download View as SVG…", DownloadSvg),
                MenuItem::action("Download View as PNG…", DownloadPng),
                MenuItem::separator(),
                MenuItem::action("Close Window", CloseWindow),
            ],
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::os_action("Cut", input::Cut, OsAction::Cut),
                MenuItem::os_action("Copy", input::Copy, OsAction::Copy),
                MenuItem::os_action("Paste", input::Paste, OsAction::Paste),
                MenuItem::os_action("Select All", input::SelectAll, OsAction::SelectAll),
            ],
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Search…", Search),
                MenuItem::separator(),
                MenuItem::action("Back", Back),
                MenuItem::action("Forward", Forward),
                MenuItem::separator(),
                MenuItem::action("Zoom In", ZoomIn),
                MenuItem::action("Zoom Out", ZoomOut),
                MenuItem::action("Fit", Fit),
                MenuItem::action("Whole Project", Home),
                MenuItem::separator(),
                MenuItem::action("Legend", ToggleLegend),
                MenuItem::action("Tidy Arrows", ToggleTrunks),
                MenuItem::action("Light or Dark", ToggleScheme),
                MenuItem::separator(),
                MenuItem::action("Enter Full Screen", FullScreen),
            ],
        },
        Menu {
            name: "Compare".into(),
            items: vec![
                MenuItem::action("Uncommitted Changes", CompareUncommitted),
                MenuItem::action("This Branch", CompareBranch),
                MenuItem::separator(),
                MenuItem::action("Stop Comparing", StopComparing),
            ],
        },
        Menu { name: "Window".into(), items: vec![MenuItem::action("Minimize", Minimize), MenuItem::action("Zoom", Zoom)] },
    ]
}
