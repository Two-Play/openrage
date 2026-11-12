//! Progress of the current background load (a model with its textures, …),
//! for the GUI to show as "Loading textures 5/13" with a percentage instead
//! of an endless spinner. One load at a time reports here.

use std::sync::Mutex;

static STATE: Mutex<Option<(String, u64, u64)>> = Mutex::new(None);

/// Reports `done` of `total` steps of `label` (total 0: unknown).
pub fn set(label: &str, done: u64, total: u64) {
    if let Ok(mut s) = STATE.lock() {
        *s = Some((label.to_owned(), done, total));
    }
}

/// The load finished.
pub fn clear() {
    if let Ok(mut s) = STATE.lock() {
        *s = None;
    }
}

/// The current step: label, done, total.
pub fn get() -> Option<(String, u64, u64)> {
    STATE.lock().ok()?.clone()
}
