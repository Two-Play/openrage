//! Tools → …: the tools of the enabled plugins, each in a window of its own.
//! OpenRAGE draws the fields the plugin declared; the plugin computes the
//! result (its `preview` for `tool:<id>`), off the UI thread.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use openrage_plugins::manifest::FieldKind;

use crate::*;

thread_local! {
    /// The tools listed in the Tools menu, in its order.
    static TOOLS: RefCell<Vec<plugins::ToolRef>> = const { RefCell::new(Vec::new()) };
    /// The open tool windows (kept alive while shown).
    static WINDOWS: RefCell<Vec<ToolWindow>> = const { RefCell::new(Vec::new()) };
}

/// The tool window opened last (for debug snapshots).
#[cfg(debug_assertions)]
pub(crate) fn newest_tool_window() -> Option<ToolWindow> {
    WINDOWS.with(|ws| ws.borrow().last().map(ComponentHandle::clone_strong))
}

/// Fills the Tools menu from the enabled plugins (after plugins or the game changed).
pub(crate) fn refresh_tool_menu(ui: &AppWindow) {
    let tools = plugins::tools();
    let rows: Vec<ToolMenuRow> = tools
        .iter()
        .map(|t| ToolMenuRow { title: t.tool.title.clone().into(), plugin: t.plugin_name.clone().into() })
        .collect();
    ui.set_plugin_tools(ModelRc::new(VecModel::from(rows)));
    TOOLS.with(|t| *t.borrow_mut() = tools);
}

/// Opens the window of the menu's `index`-th tool.
pub(crate) fn open_tool(ui: &AppWindow, index: usize) {
    let Some(tool) = TOOLS.with(|t| t.borrow().get(index).cloned()) else { return };
    let Ok(win) = ToolWindow::new() else { return };
    win.set_theme_mode(ui.get_theme_mode());
    win.set_mono_font(ui.get_mono_font());
    crate::accent::apply(&win);
    win.set_tool_title(tool.tool.title.clone().into());
    win.set_plugin(tool.plugin_name.clone().into());
    win.set_description(tool.tool.description.clone().into());
    let fields: Vec<ToolFieldRow> = tool
        .tool
        .fields
        .iter()
        .map(|f| {
            let value = f.initial();
            ToolFieldRow {
                label: f.label.clone().into(),
                kind: match f.kind {
                    FieldKind::Text => 0,
                    FieldKind::Choice => 1,
                    FieldKind::Check => 2,
                },
                choice_index: f.choices.iter().position(|c| *c == value).unwrap_or(0) as i32,
                choices: ModelRc::new(VecModel::from(
                    f.choices.iter().map(|c| SharedString::from(c.as_str())).collect::<Vec<_>>(),
                )),
                value: value.into(),
            }
        })
        .collect();
    win.set_fields(ModelRc::new(VecModel::from(fields)));
    let values: Rc<RefCell<Vec<(String, String)>>> =
        Rc::new(RefCell::new(tool.tool.fields.iter().map(|f| (f.id.clone(), f.initial())).collect()));
    let generation = Arc::new(AtomicU64::new(0));
    let tool = Arc::new(tool);

    let (w, v, g, t) = (win.as_weak(), Rc::clone(&values), Arc::clone(&generation), Arc::clone(&tool));
    win.on_changed(move |i, value| {
        if let Some(slot) = v.borrow_mut().get_mut(i.max(0) as usize) {
            slot.1 = value.to_string();
        }
        compute(&w, &t, &g, v.borrow().clone(), 150);
    });
    let w = win.as_weak();
    win.on_copy_result(move || {
        let Some(win) = w.upgrade() else { return };
        let text = win.get_result().to_string();
        if let Err(e) = copy_to_clipboard(&text) {
            win.set_error(format!("Cannot copy to the clipboard: {e}").into());
        }
    });
    compute(&win.as_weak(), &tool, &generation, values.borrow().clone(), 0);
    let _ = win.show();
    WINDOWS.with(|ws| {
        let mut ws = ws.borrow_mut();
        // Forget windows that were closed.
        ws.retain(|w| w.window().is_visible());
        ws.push(win);
    });
}

/// Asks the plugin for the result after `delay_ms` (typing on restarts the
/// wait), in the background; only the newest answer is shown.
fn compute(
    win: &slint::Weak<ToolWindow>,
    tool: &Arc<plugins::ToolRef>,
    generation: &Arc<AtomicU64>,
    values: Vec<(String, String)>,
    delay_ms: u64,
) {
    let mine = generation.fetch_add(1, Ordering::Relaxed) + 1;
    let (win, tool, generation) = (win.clone(), Arc::clone(tool), Arc::clone(generation));
    slint::Timer::single_shot(Duration::from_millis(delay_ms), move || {
        if generation.load(Ordering::Relaxed) != mine {
            return;
        }
        let Some(w) = win.upgrade() else { return };
        w.set_running(true);
        let game = with_app(|a| plugins::GameContext { dir: a.game_dir.clone(), keys: a.keys.clone() });
        std::thread::spawn(move || {
            let result = plugins::run_tool(&tool, &game, &values);
            let _ = win.upgrade_in_event_loop(move |w| {
                if generation.load(Ordering::Relaxed) != mine {
                    return;
                }
                w.set_running(false);
                w.set_error(result.error.into());
                w.set_info(result.info.into());
                w.set_result(result.text.into());
                let rows: Vec<ToolResultRow> = result
                    .rows
                    .into_iter()
                    .map(|(key, value)| ToolResultRow { key: key.into(), value: value.into() })
                    .collect();
                w.set_rows(ModelRc::new(VecModel::from(rows)));
            });
        });
    });
}

pub(crate) fn wire_tools(ui: &AppWindow) {
    refresh_tool_menu(ui);
    let w = ui.as_weak();
    ui.on_open_tool(move |i| with_ui(&w, |ui| open_tool(ui, i.max(0) as usize)));
}
