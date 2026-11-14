//! Progress of the current background load (a model with its textures, …),
//! for the GUI to show as "Loading textures 5/13" with a percentage instead
//! of an endless spinner. One load at a time reports here.
//!
//! Loads that run many models at once (a map, the surroundings of a
//! cutscene) have their own progress; they run the model loads [`quietly`],
//! so they neither overwrite nor clear the progress of the explorer's preview.

use std::cell::Cell;
use std::sync::Mutex;

static STATE: Mutex<Option<(String, u64, u64)>> = Mutex::new(None);

thread_local! {
    static QUIET: Cell<bool> = const { Cell::new(false) };
}

/// Reports `done` of `total` steps of `label` (total 0: unknown).
pub fn set(label: &str, done: u64, total: u64) {
    if QUIET.get() {
        return;
    }
    if let Ok(mut s) = STATE.lock() {
        *s = Some((label.to_owned(), done, total));
    }
}

/// The load finished.
pub fn clear() {
    if QUIET.get() {
        return;
    }
    if let Ok(mut s) = STATE.lock() {
        *s = None;
    }
}

/// The current step: label, done, total.
pub fn get() -> Option<(String, u64, u64)> {
    STATE.lock().ok()?.clone()
}

/// Runs `f` without reporting progress from this thread.
pub fn quietly<R>(f: impl FnOnce() -> R) -> R {
    let before = QUIET.replace(true);
    let result = f();
    QUIET.set(before);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_loads_leave_the_progress_alone() {
        set("Preview", 1, 3);
        quietly(|| {
            set("Map model", 0, 0);
            clear();
        });
        assert_eq!(get(), Some(("Preview".to_owned(), 1, 3)));
        clear();
        assert_eq!(get(), None);
    }
}
