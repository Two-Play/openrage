//! Text editor windows. A file opens read-only; once edit mode is unlocked in
//! the main window it can be changed and saved, which writes it into the mods
//! folder (with undo) like every other change.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use slint::ComponentHandle;

use crate::{AppWindow, EditorWindow};

/// How tabs are shown in the editor.
pub const TAB_MARK: char = '→';

/// A file loaded for editing.
pub struct Document {
    pub vpath: String,
    pub name: String,
    pub text: String,
    /// Original line endings and byte order mark, restored when saving.
    pub crlf: bool,
    pub bom: bool,
    /// Tabs are shown as [`TAB_MARK`] (the text field has no glyph for them).
    pub tab_mark: bool,
    /// Why the file cannot be saved (empty: it can).
    pub locked_reason: String,
}

impl Document {
    /// Decodes file contents; `None` for binary data.
    pub fn from_bytes(vpath: String, data: &[u8]) -> Option<Self> {
        let name = vpath.rsplit('/').next().unwrap_or(&vpath).to_owned();
        let (bom, body) = match data.strip_prefix(b"\xEF\xBB\xBF") {
            Some(rest) => (true, rest),
            None => (false, data),
        };
        match std::str::from_utf8(body) {
            Ok(text) if !text.contains('\0') => {
                let crlf = text.contains("\r\n");
                let mut text = if crlf { text.replace("\r\n", "\n") } else { text.to_owned() };
                // Only when it can be undone exactly on saving.
                let tab_mark = text.contains('\t') && !text.contains(TAB_MARK);
                if tab_mark {
                    text = text.replace('\t', &TAB_MARK.to_string());
                }
                Some(Self { vpath, name, text, crlf, bom, tab_mark, locked_reason: String::new() })
            }
            _ => {
                // Other encodings are shown but not saved (it would change the file's encoding).
                let text = rage_core::text::decode_text(data, usize::MAX)?;
                Some(Self {
                    vpath,
                    name,
                    text,
                    crlf: false,
                    bom: false,
                    tab_mark: false,
                    locked_reason: "This file is not UTF-8 text; it is shown read-only so its encoding stays intact."
                        .into(),
                })
            }
        }
    }

    /// File contents for `text`, with the original line endings and BOM.
    pub fn encode(&self, text: &str) -> Vec<u8> {
        let mut out = Vec::with_capacity(text.len() + 3);
        if self.bom {
            out.extend_from_slice(b"\xEF\xBB\xBF");
        }
        let text = if self.tab_mark { text.replace(TAB_MARK, "\t") } else { text.to_owned() };
        if self.crlf {
            out.extend_from_slice(text.replace("\r\n", "\n").replace('\n', "\r\n").as_bytes());
        } else {
            out.extend_from_slice(text.as_bytes());
        }
        out
    }
}

thread_local! {
    static OPEN: RefCell<Vec<(u64, EditorWindow)>> = const { RefCell::new(Vec::new()) };
    static NEXT_ID: Cell<u64> = const { Cell::new(0) };
}

/// Opens an editor window for `doc` (or brings an open one for the same file forward).
pub fn open(ui: &AppWindow, doc: Document) {
    let existing = OPEN.with(|o| {
        o.borrow().iter().find(|(_, w)| w.get_path().eq_ignore_ascii_case(&doc.vpath)).map(|(_, w)| w.clone_strong())
    });
    if let Some(win) = existing {
        let _ = win.show();
        return;
    }
    let Ok(win) = EditorWindow::new() else { return };
    win.set_theme_mode(ui.get_theme_mode());
    win.set_mono_font(ui.get_mono_font());
    win.set_heading(doc.name.clone().into());
    win.set_path(doc.vpath.clone().into());
    win.set_text(doc.text.clone().into());
    win.set_editable(ui.get_edit_mode());
    win.set_locked_reason(doc.locked_reason.clone().into());
    win.set_status(status_line(&doc).into());
    let id = NEXT_ID.with(|n| {
        n.set(n.get() + 1);
        n.get()
    });
    let doc = Rc::new(RefCell::new(doc));

    let (w, d, main) = (win.as_weak(), Rc::clone(&doc), ui.as_weak());
    win.on_save(move || {
        let (Some(win), Some(ui)) = (w.upgrade(), main.upgrade()) else { return };
        let text = win.get_text().to_string();
        let doc = d.borrow();
        let bytes = doc.encode(&text);
        win.set_saving(true);
        win.set_status("Saving into the mods folder…".into());
        let w2 = w.clone();
        crate::save_file(&ui, doc.vpath.clone(), bytes, format!("Edited {}", doc.name), move |result| {
            let Some(win) = w2.upgrade() else { return };
            win.set_saving(false);
            match result {
                Ok(msg) => {
                    win.set_modified(false);
                    win.set_status(msg.into());
                    if win.get_confirm_close() {
                        win.set_confirm_close(false);
                    }
                }
                Err(e) => win.set_status(format!("Saving failed: {e}").into()),
            }
        });
    });
    let main = ui.as_weak();
    win.on_unlock(move || {
        if let Some(ui) = main.upgrade() {
            ui.set_unlock_visible(true);
            let _ = ui.show();
        }
    });
    let (w, d, main) = (win.as_weak(), Rc::clone(&doc), ui.as_weak());
    win.on_reload(move || {
        let (Some(win), Some(ui)) = (w.upgrade(), main.upgrade()) else { return };
        let vpath = d.borrow().vpath.clone();
        match crate::read_file(&ui, &vpath).map(|data| Document::from_bytes(vpath, &data)) {
            Ok(Some(new)) => {
                win.set_text(new.text.clone().into());
                win.set_locked_reason(new.locked_reason.clone().into());
                win.set_modified(false);
                win.set_status(status_line(&new).into());
                *d.borrow_mut() = new;
            }
            Ok(None) => win.set_status("The file is no longer text.".into()),
            Err(e) => win.set_status(format!("Cannot read the file: {e:#}").into()),
        }
    });
    let w = win.as_weak();
    win.on_discard_and_close(move || {
        if let Some(win) = w.upgrade() {
            let _ = win.hide();
        }
        forget(id);
    });
    let w = win.as_weak();
    win.window().on_close_requested(move || {
        let Some(win) = w.upgrade() else { return slint::CloseRequestResponse::HideWindow };
        if win.get_modified() {
            win.set_confirm_close(true);
            return slint::CloseRequestResponse::KeepWindowShown;
        }
        forget(id);
        slint::CloseRequestResponse::HideWindow
    });

    if win.show().is_ok() {
        OPEN.with(|o| o.borrow_mut().push((id, win)));
    }
}

fn status_line(doc: &Document) -> String {
    let lines = doc.text.lines().count();
    let endings = if doc.crlf { "CRLF" } else { "LF" };
    let tabs = if doc.tab_mark { " · tabs shown as →" } else { "" };
    let encoding = if !doc.locked_reason.is_empty() {
        "other encoding"
    } else if doc.bom {
        "UTF-8 with BOM"
    } else {
        "UTF-8"
    };
    format!("{lines} lines · {encoding} · {endings}{tabs}")
}

fn forget(id: u64) {
    slint::Timer::single_shot(std::time::Duration::ZERO, move || {
        OPEN.with(|o| o.borrow_mut().retain(|(i, _)| *i != id));
    });
}

/// Follows the main window's edit mode.
pub fn set_editable(editable: bool) {
    OPEN.with(|o| o.borrow().iter().for_each(|(_, w)| w.set_editable(editable)));
}

pub fn set_theme(mode: i32) {
    OPEN.with(|o| o.borrow().iter().for_each(|(_, w)| w.set_theme_mode(mode)));
}

/// The newest editor window (for debug snapshots).
#[cfg(debug_assertions)]
pub fn newest() -> Option<EditorWindow> {
    OPEN.with(|o| o.borrow().last().map(|(_, w)| w.clone_strong()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_line_endings_and_bom() {
        let data = b"\xEF\xBB\xBF<a>\r\n  <b/>\t\r\n</a>\r\n";
        let doc = Document::from_bytes("x/test.xml".into(), data).unwrap();
        assert!(doc.crlf && doc.bom && doc.tab_mark && doc.locked_reason.is_empty());
        assert_eq!(doc.text, "<a>\n  <b/>→\n</a>\n");
        assert_eq!(doc.encode(&doc.text), data);
        assert!(Document::from_bytes("x/bin".into(), &[0, 1, 2, 0xFF, 0]).is_none());
    }
}
