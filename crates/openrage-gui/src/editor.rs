//! Text editor windows. A file opens read-only; once edit mode is unlocked in
//! the main window it can be changed and saved, which writes it into the mods
//! folder (with undo) like every other change.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use slint::ComponentHandle;

use openrage_app::texttable::{self, Kind};

use crate::{AppWindow, EditorWindow};

/// How tabs are shown in the editor (the text field has no glyph for a
/// tab): as a visible arrow on request, else as blank space.
pub const TAB_SHOWN: char = '→';
pub const TAB_HIDDEN: char = '\u{a0}';

/// `Document::converted` value of script disassemblies.
const SCRIPT: &str = "script";
/// `Document::converted` value of Red Dead Redemption 2 text tables.
const RDR2_TEXT: &str = "RDR2 text table";

/// A file loaded for editing.
pub struct Document {
    pub vpath: String,
    pub name: String,
    pub text: String,
    /// Original line endings and byte order mark, restored when saving.
    pub crlf: bool,
    pub bom: bool,
    /// Tabs are shown as [`TAB_SHOWN`] or [`TAB_HIDDEN`] (only when the file
    /// contains neither, so saving restores them exactly).
    pub tab_mark: bool,
    /// Why the file cannot be saved (empty: it can).
    pub locked_reason: String,
    /// Converted from this binary format for viewing (e.g. "PSO").
    pub converted: Option<String>,
    /// A binary text table shown as text; saving converts it back.
    pub table: Option<Kind>,
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
                let tab_mark = text.contains('\t') && !text.contains(TAB_SHOWN) && !text.contains(TAB_HIDDEN);
                if tab_mark {
                    text = text.replace('\t', &TAB_HIDDEN.to_string());
                }
                Some(Self {
                    vpath,
                    name,
                    text,
                    crlf,
                    bom,
                    tab_mark,
                    locked_reason: String::new(),
                    converted: None,
                    table: None,
                })
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
                    converted: None,
                    table: None,
                })
            }
        }
    }

    /// A read-only view of text converted from a binary file (e.g. metadata as XML).
    pub fn converted(vpath: String, text: String, format: &str) -> Self {
        let name = format!("{} (XML)", vpath.rsplit('/').next().unwrap_or(&vpath));
        let locked_reason = format!(
            "Converted from {format} for viewing; it cannot be saved back yet. Use Export XML… to keep a copy."
        );
        Self {
            vpath,
            name,
            text,
            crlf: false,
            bom: false,
            tab_mark: false,
            locked_reason,
            converted: Some(format.to_owned()),
            table: None,
        }
    }

    /// A read-only view of a script's disassembly.
    pub fn script(vpath: String, text: String) -> Self {
        let name = format!("{} (disassembly)", vpath.rsplit('/').next().unwrap_or(&vpath));
        Self {
            vpath,
            name,
            text,
            crlf: false,
            bom: false,
            tab_mark: false,
            locked_reason: "Disassembled from the compiled script for reading; scripts cannot be edited here.".into(),
            converted: Some(SCRIPT.into()),
            table: None,
        }
    }

    /// A read-only view of a Red Dead Redemption 2 text table (`.yldb`).
    pub fn rdr2_text_table(vpath: String, text: String) -> Self {
        let name = vpath.rsplit('/').next().unwrap_or(&vpath).to_owned();
        // Read-only, so the indentation need not survive a round trip.
        let text = text.replace('\t', "    ");
        Self {
            vpath,
            name,
            text,
            crlf: false,
            bom: false,
            tab_mark: false,
            locked_reason: "Red Dead Redemption 2 text tables are shown for reading; they cannot be saved back yet."
                .into(),
            converted: Some(RDR2_TEXT.into()),
            table: None,
        }
    }

    /// Opens a file: text tables (`.gxt2`, `.nametable`) as editable text that
    /// is converted back on saving, other files via [`from_bytes`](Self::from_bytes).
    pub fn from_file(vpath: String, data: &[u8]) -> Result<Self, String> {
        let Some(kind) = Kind::of(&vpath) else {
            return Self::from_bytes(vpath, data).ok_or_else(|| "not a text file".to_owned());
        };
        let (mut text, _) = texttable::to_text(kind, data).map_err(|e| format!("{e:#}"))?;
        let name = vpath.rsplit('/').next().unwrap_or(&vpath).to_owned();
        let tab_mark = text.contains('\t') && !text.contains(TAB_SHOWN) && !text.contains(TAB_HIDDEN);
        if tab_mark {
            text = text.replace('\t', &TAB_HIDDEN.to_string());
        }
        Ok(Self {
            vpath,
            name,
            text,
            crlf: false,
            bom: false,
            tab_mark,
            locked_reason: String::new(),
            converted: None,
            table: Some(kind),
        })
    }

    /// What saving writes: the text, or for text tables the converted file.
    pub fn save_bytes(&self, text: &str) -> Result<Vec<u8>, String> {
        let bytes = self.encode(text);
        match self.table {
            Some(kind) => texttable::from_text(kind, &String::from_utf8_lossy(&bytes)).map_err(|e| format!("{e:#}")),
            None => Ok(bytes),
        }
    }

    /// File contents for `text`, with the original line endings and BOM.
    pub fn encode(&self, text: &str) -> Vec<u8> {
        let mut out = Vec::with_capacity(text.len() + 3);
        if self.bom {
            out.extend_from_slice(b"\xEF\xBB\xBF");
        }
        let text = if self.tab_mark { text.replace([TAB_SHOWN, TAB_HIDDEN], "\t") } else { text.to_owned() };
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
    win.set_overlay_folder(crate::overlay_name().into());
    win.set_write_in_game(crate::writes_in_game());
    win.set_theme_mode(ui.get_theme_mode());
    crate::accent::apply(&win);
    win.set_mono_font(ui.get_mono_font());
    win.set_heading(doc.name.clone().into());
    win.set_path(doc.vpath.clone().into());
    win.set_text(doc.text.clone().into());
    win.set_editable(ui.get_edit_mode());
    win.set_locked_reason(doc.locked_reason.clone().into());
    win.set_status(status_line(&doc).into());
    win.set_has_tabs(doc.tab_mark);
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
        let bytes = match doc.save_bytes(&text) {
            Ok(b) => b,
            Err(e) => {
                win.set_status(format!("Cannot save: {e}").into());
                return;
            }
        };
        win.set_saving(true);
        win.set_status(format!("Saving into the {} folder…", crate::write_folder()).into());
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
    win.on_set_edit_mode(move |on| {
        // The main window owns the edit mode; it tells every editor.
        if let Some(ui) = main.upgrade() {
            ui.set_edit_mode(on);
        }
    });
    let w = win.as_weak();
    win.on_copy_path(move || {
        let Some(win) = w.upgrade() else { return };
        let path = win.get_path();
        let msg = match crate::copy_to_clipboard(&path) {
            Ok(()) => format!("Copied {path}"),
            Err(e) => format!("Cannot copy to the clipboard: {e}"),
        };
        win.set_status(msg.into());
    });
    let (w, d) = (win.as_weak(), Rc::clone(&doc));
    win.on_toggle_tabs(move || {
        let Some(win) = w.upgrade() else { return };
        if !d.borrow().tab_mark {
            return;
        }
        let shown = !win.get_tabs_shown();
        win.set_text(show_tabs(&win.get_text(), shown).into());
        win.set_tabs_shown(shown);
    });
    let (w, d, main) = (win.as_weak(), Rc::clone(&doc), ui.as_weak());
    win.on_reload(move || {
        let (Some(win), Some(ui)) = (w.upgrade(), main.upgrade()) else { return };
        let (vpath, converted) = {
            let doc = d.borrow();
            (doc.vpath.clone(), doc.converted.is_some())
        };
        let new = if converted && openrage_app::texttable::is_rdr2_text_table(&vpath) {
            crate::read_rdr2_text_table(&vpath).map(|text| Some(Document::rdr2_text_table(vpath, text)))
        } else if converted
            && (vpath.to_ascii_lowercase().ends_with(".ysc") || vpath.to_ascii_lowercase().ends_with(".sco"))
        {
            crate::read_script_listing(&ui, &vpath).map(|text| Some(Document::script(vpath, text)))
        } else if converted {
            crate::read_xml(&ui, &vpath).map(|x| Some(Document::converted(vpath, x.text, x.encoding.name())))
        } else {
            crate::read_file(&ui, &vpath).map(|data| Document::from_file(vpath, &data).ok())
        };
        match new {
            Ok(Some(new)) => {
                win.set_text(show_tabs(&new.text, win.get_tabs_shown()).into());
                reset_view_soon(&win);
                win.set_has_tabs(new.tab_mark);
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
        reset_view_soon(&win);
        OPEN.with(|o| o.borrow_mut().push((id, win)));
    }
}

/// Scrolls to the top left once the window has its size.
fn reset_view_soon(win: &EditorWindow) {
    let weak = win.as_weak();
    slint::Timer::single_shot(std::time::Duration::from_millis(80), move || {
        if let Some(w) = weak.upgrade() {
            w.invoke_reset_view();
        }
    });
}

/// Switches how tabs look in the editor text.
pub fn show_tabs(text: &str, shown: bool) -> String {
    if shown {
        text.replace(TAB_HIDDEN, &TAB_SHOWN.to_string())
    } else {
        text.replace(TAB_SHOWN, &TAB_HIDDEN.to_string())
    }
}

fn status_line(doc: &Document) -> String {
    let lines = doc.text.lines().count();
    let endings = if doc.crlf { "CRLF" } else { "LF" };
    let tabs = if doc.tab_mark { " · contains tabs" } else { "" };
    if let Some(format) = &doc.converted {
        if format == SCRIPT {
            return format!("{lines} lines · script disassembly");
        }
        if format == RDR2_TEXT {
            return format!("{lines} lines · {RDR2_TEXT}, read-only");
        }
        return format!("{lines} lines · XML converted from {format}");
    }
    if let Some(kind) = doc.table {
        return format!("{lines} lines · {}, saved back in its own format", kind.name());
    }
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

/// Follows the chosen write target (GTA IV: update folder or game files).
pub fn set_write_in_game(on: bool) {
    OPEN.with(|o| o.borrow().iter().for_each(|(_, w)| w.set_write_in_game(on)));
}

pub fn set_theme(mode: i32) {
    OPEN.with(|o| o.borrow().iter().for_each(|(_, w)| w.set_theme_mode(mode)));
}

/// The newest editor window (for debug snapshots).
#[cfg(debug_assertions)]
pub fn newest() -> Option<EditorWindow> {
    OPEN.with(|o| o.borrow().last().map(|(_, w)| w.clone_strong()))
}

/// Applies a changed accent color to every open window of this kind.
pub fn set_accent() {
    OPEN.with(|o| o.borrow().iter().for_each(|(_, w)| crate::accent::apply(w)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_line_endings_and_bom() {
        let data = b"\xEF\xBB\xBF<a>\r\n  <b/>\t\r\n</a>\r\n";
        let doc = Document::from_bytes("x/test.xml".into(), data).unwrap();
        assert!(doc.crlf && doc.bom && doc.tab_mark && doc.locked_reason.is_empty());
        assert_eq!(doc.text, "<a>\n  <b/>\u{a0}\n</a>\n");
        assert_eq!(doc.encode(&show_tabs(&doc.text, true)), data);
        assert_eq!(doc.encode(&doc.text), data);
        assert!(Document::from_bytes("x/bin".into(), &[0, 1, 2, 0xFF, 0]).is_none());
    }
}
