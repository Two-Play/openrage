//! Metadata in a readable form: the structure as a tree with the selected
//! node's fields, and the XML with syntax colors (lines built on demand).

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use openrage_app::metadata::Xml;
use openrage_app::xmltree::{self, Node, SpanKind};
use slint::{ComponentHandle, Model, ModelRc, ModelTracker, SharedString, VecModel};

use crate::{AppWindow, CastRow, InfoRow, XmlLine, XmlNodeRow, XmlSpan, XmlWindow};

/// A node of the tree in a flat arena (`id` = index).
struct Entry {
    label: String,
    depth: usize,
    fields: Vec<(String, String)>,
    children: Vec<usize>,
    /// Where the element is in the XML text (bytes).
    range: std::ops::Range<usize>,
    /// Lowercased label and fields, for the filter.
    haystack: String,
}

struct State {
    entries: Vec<Entry>,
    expanded: HashSet<usize>,
    /// Ids of the rows currently shown.
    shown: Vec<usize>,
    filter: String,
}

fn flatten(node: &Node, depth: usize, out: &mut Vec<Entry>) -> usize {
    let id = out.len();
    let mut haystack = node.label.to_lowercase();
    for (k, v) in &node.fields {
        haystack.push('\n');
        haystack.push_str(&k.to_lowercase());
        haystack.push('\t');
        haystack.push_str(&v.to_lowercase());
    }
    out.push(Entry {
        label: node.label.clone(),
        depth,
        fields: node.fields.clone(),
        children: Vec::new(),
        range: node.range.clone(),
        haystack,
    });
    let children: Vec<usize> = node.children.iter().map(|c| flatten(c, depth + 1, out)).collect();
    out[id].children = children;
    id
}

impl State {
    /// Whether a node or one of its descendants matches the filter.
    fn matches(&self, id: usize, memo: &mut Vec<Option<bool>>) -> bool {
        if let Some(m) = memo[id] {
            return m;
        }
        let own = self.entries[id].haystack.contains(&self.filter);
        let any = own || self.entries[id].children.clone().into_iter().any(|c| self.matches(c, memo));
        memo[id] = Some(any);
        any
    }

    fn rows(&mut self) -> Vec<XmlNodeRow> {
        let mut shown = Vec::new();
        let mut rows = Vec::new();
        let filtering = !self.filter.is_empty();
        let mut memo = vec![None; self.entries.len()];
        let mut stack = vec![0usize];
        while let Some(id) = stack.pop() {
            if filtering && !self.matches(id, &mut memo) {
                continue;
            }
            let e = &self.entries[id];
            // While filtering, every matching branch is open.
            let open = if filtering { true } else { self.expanded.contains(&id) };
            rows.push(XmlNodeRow {
                id: id as i32,
                label: e.label.clone().into(),
                depth: e.depth as i32,
                expandable: !e.children.is_empty(),
                expanded: open && !e.children.is_empty(),
                hit: filtering && e.haystack.contains(&self.filter),
            });
            shown.push(id);
            if open {
                stack.extend(e.children.iter().rev());
            }
        }
        self.shown = shown;
        rows
    }
}

/// The XML lines as a model that highlights a line only when it is shown.
struct Lines {
    lines: Vec<String>,
}

impl Model for Lines {
    type Data = XmlLine;

    fn row_count(&self) -> usize {
        self.lines.len()
    }

    fn row_data(&self, row: usize) -> Option<XmlLine> {
        let line = self.lines.get(row)?;
        let spans: Vec<XmlSpan> = xmltree::highlight(line)
            .into_iter()
            .map(|(kind, text)| XmlSpan {
                text: text.into(),
                kind: match kind {
                    SpanKind::Punctuation => 0,
                    SpanKind::Tag => 1,
                    SpanKind::Attribute => 2,
                    SpanKind::Value => 3,
                    SpanKind::Text => 4,
                },
            })
            .collect();
        Some(XmlLine { number: row as i32 + 1, spans: ModelRc::new(VecModel::from(spans)) })
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &()
    }
}

/// Copies text to the clipboard and says what happened, for the status line.
fn copy(text: &str) -> String {
    match crate::copy_to_clipboard(text) {
        Ok(()) => format!("Copied {} characters", text.chars().count()),
        Err(e) => format!("Cannot copy to the clipboard: {e}"),
    }
}

/// Opens a window for a metadata file already converted to XML.
pub fn open(ui: &AppWindow, vpath: String, xml: Xml) {
    let tree = match xmltree::parse(&xml.text) {
        Ok(t) => t,
        Err(e) => {
            crate::show_error(ui, format!("Cannot read the XML of {vpath}: {e}"));
            return;
        }
    };
    let Ok(win) = XmlWindow::new() else { return };
    let name = vpath.rsplit('/').next().unwrap_or(&vpath).to_owned();
    win.set_theme_mode(ui.get_theme_mode());
    crate::accent::apply(&win);
    win.set_mono_font(ui.get_mono_font());
    win.set_heading(name.clone().into());
    win.set_path(vpath.clone().into());
    let mut summary = format!("{} · {}", xml.encoding.name(), vpath);
    if !xml.errors.is_empty() {
        summary = format!("{} unreadable parts · {summary}", xml.errors.len());
    }
    win.set_summary(summary.into());
    let line_count = xml.text.lines().count();
    let nodes = tree.count();
    win.set_status(format!("{nodes} structures · {line_count} lines of XML").into());
    win.set_cutscene_summary(
        format!(
            "{} · stored as {} · {nodes} structures · {line_count} lines of XML{}",
            xml.root.as_deref().unwrap_or("Metadata"),
            xml.encoding.name(),
            if xml.errors.is_empty() { String::new() } else { format!(" · {} unreadable parts", xml.errors.len()) }
        )
        .into(),
    );

    let mut entries = Vec::with_capacity(nodes);
    flatten(&tree, 0, &mut entries);
    // The root and its direct children start open.
    let mut expanded: HashSet<usize> = HashSet::from([0]);
    if entries[0].children.len() <= 50 {
        expanded.extend(entries[0].children.iter().copied());
    }
    let state = Rc::new(RefCell::new(State { entries, expanded, shown: Vec::new(), filter: String::new() }));

    // Map files of GTA V: their objects in a 3D scene.
    if vpath.to_ascii_lowercase().ends_with(".ymap")
        && !crate::with_app(|a| a.game_dir.as_deref().is_some_and(crate::is_rdr2_dir))
    {
        win.set_is_map(true);
        let (main, path) = (ui.as_weak(), vpath.clone());
        win.on_show_map(move || {
            if let Some(ui) = main.upgrade() {
                crate::scene::open_map(&ui, path.clone());
            }
        });
    }
    if vpath.to_ascii_lowercase().ends_with(".cut")
        && let Ok(cut) = openrage_app::cutscene::from_xml(&xml.text)
    {
        show_cast(&win, ui, &vpath, cut);
    }
    let lines: Vec<String> = xml.text.lines().map(|l| l.replace('\t', "    ")).collect();
    win.set_lines(ModelRc::new(Lines { lines }));

    let refresh = {
        let (w, state) = (win.as_weak(), Rc::clone(&state));
        move || {
            let Some(win) = w.upgrade() else { return };
            let selected_id = {
                let s = state.borrow();
                usize::try_from(win.get_selected_node()).ok().and_then(|r| s.shown.get(r).copied())
            };
            let rows = state.borrow_mut().rows();
            win.set_nodes(ModelRc::new(VecModel::from(rows)));
            // Keep the same node selected if it is still shown.
            let row = selected_id.and_then(|id| state.borrow().shown.iter().position(|&s| s == id));
            win.set_selected_node(row.map_or(-1, |r| r as i32));
        }
    };
    let show_fields = {
        let (w, state) = (win.as_weak(), Rc::clone(&state));
        move |row: i32| {
            let Some(win) = w.upgrade() else { return };
            let s = state.borrow();
            let Some(&id) = usize::try_from(row).ok().and_then(|r| s.shown.get(r)) else { return };
            let e = &s.entries[id];
            win.set_selected_node(row);
            win.set_node_title(e.label.clone().into());
            let fields: Vec<InfoRow> =
                e.fields.iter().map(|(k, v)| InfoRow { key: k.clone().into(), value: v.clone().into() }).collect();
            win.set_fields(ModelRc::new(VecModel::from(fields)));
        }
    };
    refresh();
    show_fields(0);

    let (st, r) = (Rc::clone(&state), refresh.clone());
    win.on_toggle(move |row| {
        let id = { st.borrow().shown.get(row as usize).copied() };
        if let Some(id) = id {
            let mut s = st.borrow_mut();
            if !s.expanded.remove(&id) {
                s.expanded.insert(id);
            }
        }
        r();
    });
    let f = show_fields.clone();
    win.on_select(f);
    let (st, r, f) = (Rc::clone(&state), refresh.clone(), show_fields.clone());
    win.on_filter_edited(move |text: SharedString| {
        st.borrow_mut().filter = text.trim().to_lowercase();
        r();
        f(0);
    });
    let (st, r) = (Rc::clone(&state), refresh.clone());
    win.on_expand_all(move |open| {
        {
            let mut s = st.borrow_mut();
            s.expanded.clear();
            if open {
                let all: Vec<usize> = (0..s.entries.len()).filter(|&i| !s.entries[i].children.is_empty()).collect();
                s.expanded.extend(all);
            } else {
                s.expanded.insert(0);
            }
        }
        r();
    });
    let (main, p) = (ui.as_weak(), vpath.clone());
    win.on_export_xml(move || {
        if let Some(ui) = main.upgrade() {
            crate::export_xml(&ui, p.clone());
        }
    });
    // XML stored as text is edited as the file itself; converted XML is read-only.
    let plain = xml.encoding == openrage_app::metadata::Encoding::Text;
    win.set_plain_text(plain);
    let text = Rc::new(xml.text.clone());
    let w = win.as_weak();
    win.on_copy_text(move |t: SharedString| {
        let msg = copy(t.as_str());
        if let Some(win) = w.upgrade() {
            win.set_status(msg.into());
        }
    });
    let (w, st, t) = (win.as_weak(), Rc::clone(&state), Rc::clone(&text));
    win.on_node_action(move |row, action: SharedString| {
        let Some(win) = w.upgrade() else { return };
        let s = st.borrow();
        let Some(e) = usize::try_from(row).ok().and_then(|r| s.shown.get(r)).map(|&id| &s.entries[id]) else {
            return;
        };
        match action.as_str() {
            "copy-values" => {
                let values: Vec<String> = e.fields.iter().map(|(k, v)| format!("{k}: {v}")).collect();
                win.set_status(copy(&values.join("\n")).into());
            }
            "copy-xml" => {
                // From the start of the element's line, so the indentation stays even.
                let start = t[..e.range.start].rfind('\n').map_or(0, |i| i + 1);
                let snippet = if t[start..e.range.start].trim().is_empty() { start } else { e.range.start };
                win.set_status(copy(&t[snippet..e.range.end]).into());
            }
            "show-xml" => win.invoke_show_line(t[..e.range.start].matches('\n').count() as i32),
            _ => {}
        }
    });
    let (w, t) = (win.as_weak(), Rc::clone(&text));
    win.on_line_action(move |row, action: SharedString| {
        let Some(win) = w.upgrade() else { return };
        let msg = match action.as_str() {
            "copy" => copy(t.lines().nth(row as usize).unwrap_or_default()),
            "copy-all" => copy(&t),
            _ => return,
        };
        win.set_status(msg.into());
    });
    let main = ui.as_weak();
    let format = xml.encoding.name();
    win.on_open_text(move || {
        if let Some(ui) = main.upgrade() {
            if plain {
                crate::open_editor(&ui, vpath.clone());
            } else {
                crate::editor::open(&ui, crate::editor::Document::converted(vpath.clone(), text.to_string(), format));
            }
        }
    });
    let id = NEXT_ID.with(|n| {
        n.set(n.get() + 1);
        n.get()
    });
    win.window().on_close_requested(move || {
        // Dropped after the event has been handled.
        slint::Timer::single_shot(std::time::Duration::ZERO, move || {
            OPEN.with(|o| o.borrow_mut().retain(|(i, _)| *i != id));
        });
        slint::CloseRequestResponse::HideWindow
    });
    if win.show().is_ok() {
        OPEN.with(|o| o.borrow_mut().push((id, win)));
    }
}

/// Fills the cutscene tab: the cast, and playing an actor with its part of
/// the cutscene's animation.
fn show_cast(win: &XmlWindow, ui: &AppWindow, vpath: &str, cut: openrage_app::cutscene::Cutscene) {
    use openrage_app::cutscene::Kind;
    let people = cut.actors.iter().filter(|a| a.kind == Kind::Ped).count();
    let vehicles = cut.actors.iter().filter(|a| a.kind == Kind::Vehicle).count();
    let props = cut.actors.iter().filter(|a| a.kind == Kind::Prop).count();
    win.set_is_cutscene(true);
    win.set_cutscene_summary(
        format!(
            "{:.1} seconds · {people} people, {vehicles} vehicles, {props} props. \
             Play animation opens the model in the viewer with its animation over the whole cutscene.",
            cut.duration
        )
        .into(),
    );
    let rows: Vec<CastRow> = cut
        .actors
        .iter()
        .map(|a| CastRow {
            kind: a.kind.label().into(),
            name: a.name.clone().into(),
            playable: !a.kind.model_extensions().is_empty(),
        })
        .collect();
    win.set_cast(ModelRc::new(VecModel::from(rows)));
    win.set_tab(2);
    {
        let (main, cut_path, cut) = (ui.as_weak(), vpath.to_owned(), cut.clone());
        let w = win.as_weak();
        win.on_play_scene(move || {
            let (Some(ui), Some(win)) = (main.upgrade(), w.upgrade()) else { return };
            win.set_status("Finding the actors' models …".into());
            let models: Vec<Option<String>> = cut.actors.iter().map(|a| find_model(&a.name, a.kind)).collect();
            crate::scene::open(&ui, cut_path.clone(), cut.clone(), models);
            win.set_status(SharedString::new());
        });
    }
    let (main, cut_path, actors) = (ui.as_weak(), vpath.to_owned(), cut.actors);
    let w = win.as_weak();
    win.on_play_actor(move |i| {
        let (Some(ui), Some(win)) = (main.upgrade(), w.upgrade()) else { return };
        let Some(actor) = actors.get(i as usize) else { return };
        let name = actor.name.to_ascii_lowercase();
        match find_model(&name, actor.kind) {
            Some(model) => {
                win.set_status(format!("Opening {model} with the cutscene's animation …").into());
                crate::open_model_with_animation(&ui, model, cut_path.clone());
            }
            None => {
                win.set_status(format!("No model named {name} found (the search index may still be building).").into())
            }
        }
    });
}

/// The model file of a cutscene actor, found through the search index.
fn find_model(name: &str, kind: openrage_app::cutscene::Kind) -> Option<String> {
    let name = name.to_ascii_lowercase();
    kind.model_extensions().iter().find_map(|ext| {
        crate::find_files(&format!("{name}{ext}"), ext, 20)
            .into_iter()
            .find(|v| v.rsplit('/').next().is_some_and(|f| f.eq_ignore_ascii_case(&format!("{name}{ext}"))))
    })
}

thread_local! {
    static OPEN: RefCell<Vec<(u64, XmlWindow)>> = const { RefCell::new(Vec::new()) };
    static NEXT_ID: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// The most recently opened XML window (for debug snapshots).
#[cfg(debug_assertions)]
pub fn newest() -> Option<XmlWindow> {
    OPEN.with(|o| o.borrow().last().map(|(_, w)| w.clone_strong()))
}

/// Applies the main window's theme to every open XML window.
pub fn set_theme(mode: i32) {
    OPEN.with(|o| {
        for (_, w) in o.borrow().iter() {
            w.set_theme_mode(mode);
        }
    });
}

/// Applies a changed accent color to every open window of this kind.
pub fn set_accent() {
    OPEN.with(|o| o.borrow().iter().for_each(|(_, w)| crate::accent::apply(w)));
}
