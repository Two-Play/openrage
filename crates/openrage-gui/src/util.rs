//! Small helpers shared by the UI modules.

use crate::*;

thread_local! {
    // Kept alive: on X11 the clipboard content disappears with its owner.
    static CLIPBOARD: RefCell<Option<arboard::Clipboard>> = const { RefCell::new(None) };
}

pub(crate) fn copy_to_clipboard(text: &str) -> Result<(), arboard::Error> {
    CLIPBOARD.with(|c| {
        let mut c = c.borrow_mut();
        if c.is_none() {
            *c = Some(arboard::Clipboard::new()?);
        }
        c.as_mut().map_or(Ok(()), |c| c.set_text(text))
    })
}

pub(crate) fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// The platform's fixed-width font family.
pub(crate) fn mono_font() -> String {
    if cfg!(windows) {
        return "Consolas".into();
    }
    if cfg!(target_os = "macos") {
        return "Menlo".into();
    }
    std::process::Command::new("fc-match")
        .args(["-f", "%{family[0]}", "monospace"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "DejaVu Sans Mono".into())
}

/// `13379` → `13,379`.
pub(crate) fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Decoded RGBA image: (width, height, pixels).
pub(crate) type Rgba = (u32, u32, Vec<u8>);

/// Metadata shown as a key/value table; an empty key is a note spanning the row.
pub(crate) type Meta = Vec<(String, String)>;

pub(crate) fn kv(key: &str, value: impl ToString) -> (String, String) {
    (key.to_owned(), value.to_string())
}

pub(crate) fn note(text: impl ToString) -> (String, String) {
    (String::new(), text.to_string())
}

/// The configured game folder.
pub(crate) fn game_dir() -> Option<std::path::PathBuf> {
    with_app(|a| a.game_dir.clone())
}

/// Shows an error in the status bar: in the error colour for as long as it is
/// the status, with the whole text in a tooltip (long messages are cut off).
pub(crate) fn show_error(ui: &AppWindow, msg: String) {
    let msg = SharedString::from(msg);
    ui.set_status_error(msg.clone());
    ui.set_status(msg);
}

/// Saves the settings; a failure shows in the status bar.
pub(crate) fn save_settings(ui: &AppWindow) {
    if let Err(e) = with_app(|a| a.cfg.save()) {
        show_error(ui, format!("Could not save the settings: {e}"));
    }
}

/// Marks a task as running (or done) that cannot stop halfway: the status bar
/// shows a spinner but no Cancel button.
pub(crate) fn set_busy(ui: &AppWindow, busy: bool, status: String) {
    with_app(|a| a.busy = busy);
    ui.set_busy(busy);
    ui.set_cancellable(false);
    ui.set_status(status.into());
}

/// Marks a task as running that watches `App::cancel`: the status bar offers Cancel.
pub(crate) fn set_busy_cancellable(ui: &AppWindow, status: String) {
    set_busy(ui, true, status);
    ui.set_cancellable(true);
}
