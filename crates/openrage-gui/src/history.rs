//! The change history in its own window.

use std::cell::RefCell;
use std::time::{SystemTime, UNIX_EPOCH};

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::{AppWindow, HistoryRow, HistoryWindow};

thread_local! {
    static WINDOW: RefCell<Option<HistoryWindow>> = const { RefCell::new(None) };
}

/// Shows the history window (creating it on first use).
pub fn open(ui: &AppWindow) {
    let existing = WINDOW.with(|w| w.borrow().as_ref().map(ComponentHandle::clone_strong));
    let win = match existing {
        Some(w) => w,
        None => {
            let Ok(win) = HistoryWindow::new() else { return };
            let main = ui.as_weak();
            win.on_undo(move || {
                if let Some(ui) = main.upgrade() {
                    ui.invoke_undo();
                }
            });
            let main = ui.as_weak();
            win.on_uninstall(move |group| {
                if let Some(ui) = main.upgrade() {
                    ui.set_history_group(group);
                    ui.invoke_uninstall_group();
                }
            });
            let main = ui.as_weak();
            win.on_show_in_main(move || {
                if let Some(ui) = main.upgrade() {
                    crate::show_history_in_list(&ui);
                    let _ = ui.show();
                }
            });
            let main = ui.as_weak();
            win.on_set_edit_mode(move |on| {
                if let Some(ui) = main.upgrade() {
                    ui.set_edit_mode(on);
                }
            });
            WINDOW.with(|w| *w.borrow_mut() = Some(win.clone_strong()));
            win
        }
    };
    win.set_theme_mode(ui.get_theme_mode());
    crate::accent::apply(&win);
    refresh(ui);
    let _ = win.show();
}

/// Reloads the entries and the main window's state (after every change).
pub fn refresh(ui: &AppWindow) {
    let Some(win) = WINDOW.with(|w| w.borrow().as_ref().map(ComponentHandle::clone_strong)) else { return };
    win.set_edit_mode(ui.get_edit_mode());
    win.set_busy(ui.get_busy());
    win.set_status(ui.get_status());
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let rows: Vec<HistoryRow> = crate::journal_entries()
        .into_iter()
        .map(|e| {
            use openrage_app::journal::Kind;
            let place = match e.kind {
                Kind::FileWritten { in_game: true, .. } => format!("game folder: {}", e.archive),
                _ => format!("mods/{}", e.archive),
            };
            HistoryRow {
                when: ago(now.saturating_sub(e.time)).into(),
                description: e.description.into(),
                place: place.into(),
                group: e.group.unwrap_or_default().into(),
            }
        })
        .collect();
    if win.get_selected() >= rows.len() as i32 {
        win.set_selected(-1);
    }
    win.set_rows(ModelRc::new(VecModel::from(rows)));
}

/// The history window, if open (for debug snapshots).
#[cfg(debug_assertions)]
pub fn window() -> Option<HistoryWindow> {
    WINDOW.with(|w| w.borrow().as_ref().map(ComponentHandle::clone_strong))
}

pub fn set_theme(mode: i32) {
    WINDOW.with(|w| {
        if let Some(win) = w.borrow().as_ref() {
            win.set_theme_mode(mode);
        }
    });
}

/// `90` → "1 min ago".
fn ago(secs: u64) -> String {
    match secs {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", secs / 60),
        3600..86_400 => format!("{} h ago", secs / 3600),
        _ => format!("{} days ago", secs / 86_400),
    }
}

/// Applies a changed accent color to the history window.
pub fn set_accent() {
    WINDOW.with(|w| {
        if let Some(win) = w.borrow().as_ref() {
            crate::accent::apply(win);
        }
    });
}
