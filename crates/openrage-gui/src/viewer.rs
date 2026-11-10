//! Separate viewer windows for models and images (opened by double-click).
//!
//! Each window keeps its own copy of what was selected and its own camera, so
//! several models can be compared side by side while browsing on.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use openrage_app::render::View;
use slint::ComponentHandle;

use crate::{AppWindow, ModelPreview, ViewerWindow};

pub enum Content {
    Model(Arc<ModelPreview>),
    Image(slint::Image),
}

struct State {
    content: Content,
    view: View,
}

thread_local! {
    static OPEN: RefCell<Vec<(u64, ViewerWindow)>> = const { RefCell::new(Vec::new()) };
    static NEXT_ID: Cell<u64> = const { Cell::new(0) };
}

/// Opens a viewer window for `content`, labelled with the main window's
/// current selection (title, path, details).
pub fn open(ui: &AppWindow, content: Content) {
    let Ok(win) = ViewerWindow::new() else { return };
    win.set_theme_mode(ui.get_theme_mode());
    win.set_mono_font(ui.get_mono_font());
    win.set_heading(ui.get_info_title());
    win.set_path(ui.get_info_path());
    win.set_icon_name(ui.get_info_icon());
    win.set_tint(ui.get_info_tint());
    win.set_rows(ui.get_info_rows());
    win.set_model_mode(matches!(content, Content::Model(_)));
    if let Content::Image(img) = &content {
        win.set_image(img.clone());
    }
    let id = NEXT_ID.with(|n| {
        n.set(n.get() + 1);
        n.get()
    });
    let state = Rc::new(RefCell::new(State { content, view: View::default() }));

    let (w, s) = (win.as_weak(), Rc::clone(&state));
    win.on_orbit(move |dx, dy| {
        let mut st = s.borrow_mut();
        st.view.yaw -= dx * 0.01;
        st.view.pitch = (st.view.pitch + dy * 0.01).clamp(-1.5, 1.5);
        drop(st);
        render(&w, &s);
    });
    let (w, s) = (win.as_weak(), Rc::clone(&state));
    win.on_zoom(move |delta| {
        let mut st = s.borrow_mut();
        st.view.zoom = (st.view.zoom * 1.1f32.powf(delta / 120.0)).clamp(0.2, 20.0);
        drop(st);
        render(&w, &s);
    });
    let (w, s) = (win.as_weak(), Rc::clone(&state));
    win.on_resized(move || render(&w, &s));
    let (w, s) = (win.as_weak(), Rc::clone(&state));
    win.on_reset_view(move || {
        s.borrow_mut().view = View::default();
        render(&w, &s);
    });
    let s = Rc::clone(&state);
    let main = ui.as_weak();
    win.on_export_gltf(move || {
        if let Content::Model(m) = &s.borrow().content {
            crate::save_glb(Arc::clone(m), main.clone());
        }
    });
    let s = Rc::clone(&state);
    let (main, w) = (ui.as_weak(), win.as_weak());
    win.on_save_png(move || {
        if let (Content::Image(img), Some(win)) = (&s.borrow().content, w.upgrade()) {
            crate::save_png_image(img, win.get_heading().to_string(), main.clone());
        }
    });
    let w = win.as_weak();
    win.on_close_viewer(move || {
        if let Some(win) = w.upgrade() {
            let _ = win.hide();
        }
        forget(id);
    });
    win.window().on_close_requested(move || {
        forget(id);
        slint::CloseRequestResponse::HideWindow
    });

    if win.show().is_ok() {
        render(&win.as_weak(), &state);
        OPEN.with(|o| o.borrow_mut().push((id, win)));
    }
}

/// Drops a closed window (later, not from inside its own callback).
fn forget(id: u64) {
    slint::Timer::single_shot(std::time::Duration::ZERO, move || {
        OPEN.with(|o| o.borrow_mut().retain(|(i, _)| *i != id));
    });
}

fn render(w: &slint::Weak<ViewerWindow>, state: &Rc<RefCell<State>>) {
    let Some(win) = w.upgrade() else { return };
    let st = state.borrow();
    if let Content::Model(m) = &st.content {
        let width = (win.get_area_width() - 32.0).max(64.0) as usize;
        let height = (win.get_area_height() - 32.0).max(64.0) as usize;
        if let Some(img) = crate::render_image(m, st.view, width, height) {
            win.set_image(img);
        }
    }
}

/// Applies the color scheme to every open viewer.
pub fn set_theme(mode: i32) {
    OPEN.with(|o| o.borrow().iter().for_each(|(_, w)| w.set_theme_mode(mode)));
}

/// The newest viewer window (for debug snapshots).
#[cfg(debug_assertions)]
pub fn newest() -> Option<ViewerWindow> {
    OPEN.with(|o| o.borrow().last().map(|(_, w)| w.clone_strong()))
}
