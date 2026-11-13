//! The About dialog and its list of licenses.

use crate::*;

thread_local! {
    /// Indices into `licenses::dependencies()` of the crates listed (after filtering).
    static ABOUT_SHOWN: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

pub(crate) fn register_about(ui: &AppWindow) {
    use openrage_app::licenses;
    let w = ui.as_weak();
    ui.on_about_open(move || {
        with_ui(&w, |ui| {
            ui.set_about_version(format!("Version {}", licenses::VERSION).into());
            ui.set_about_notice(licenses::NOTICE_SHORT.into());
            let credits: Vec<InfoRow> = licenses::credits()
                .into_iter()
                .map(|c| InfoRow { key: c.title.into(), value: c.text.into() })
                .collect();
            ui.set_about_credits(ModelRc::new(VecModel::from(credits)));
            let summary: Vec<String> =
                licenses::summary().into_iter().take(4).map(|(l, n)| format!("{n} × {l}")).collect();
            ui.set_about_deps_summary(
                format!("{} crates: {}, …", licenses::dependencies().len(), summary.join(", ")).into(),
            );
            ui.set_about_filter(SharedString::new());
            filter_about(ui, "");
            ui.set_about_tab(0);
            ui.set_about_visible(true);
        })
    });
    let w = ui.as_weak();
    ui.on_about_filter_edited(move |f| with_ui(&w, |ui| filter_about(ui, &f)));
    let w = ui.as_weak();
    ui.on_about_select(move |row| {
        with_ui(&w, |ui| {
            let Some(i) = ABOUT_SHOWN.with(|s| s.borrow().get(row as usize).copied()) else { return };
            let d = &licenses::dependencies()[i];
            let mut text = format!("{} {}\n{}\n", d.name, d.version, d.license);
            if !d.repository.is_empty() {
                text.push_str(&d.repository);
                text.push('\n');
            }
            text.push('\n');
            text.push_str(&licenses::dependency_text(d));
            ui.set_about_selected(row);
            ui.set_about_text(text.into());
        })
    });
    let w = ui.as_weak();
    ui.on_about_show(move |what| {
        with_ui(&w, |ui| {
            let text = match what.as_str() {
                "gpl" => licenses::gpl_text(),
                "agpl" => licenses::agpl_text(),
                _ => licenses::notice(),
            };
            ui.set_about_selected(-1);
            ui.set_about_text(text.into());
            ui.set_about_tab(2);
        })
    });
    let w = ui.as_weak();
    ui.on_about_open_source(move || {
        with_ui(&w, |ui| {
            if let Err(e) = licenses::open_url(licenses::REPOSITORY) {
                ui.set_status(
                    format!("Cannot open the browser: {e} — the source is at {}", licenses::REPOSITORY).into(),
                );
            }
        })
    });
}

/// Lists the crates whose name or licence contains `filter` and shows the first.
pub(crate) fn filter_about(ui: &AppWindow, filter: &str) {
    use openrage_app::licenses;
    let f = filter.trim().to_lowercase();
    let shown: Vec<usize> = licenses::dependencies()
        .iter()
        .enumerate()
        .filter(|(_, d)| f.is_empty() || d.name.to_lowercase().contains(&f) || d.license.to_lowercase().contains(&f))
        .map(|(i, _)| i)
        .collect();
    let rows: Vec<DepRow> = shown
        .iter()
        .map(|&i| {
            let d = &licenses::dependencies()[i];
            DepRow {
                name: d.name.as_str().into(),
                version: d.version.as_str().into(),
                license: d.license.as_str().into(),
            }
        })
        .collect();
    let any = !shown.is_empty();
    ABOUT_SHOWN.with(|s| *s.borrow_mut() = shown);
    ui.set_about_deps(ModelRc::new(VecModel::from(rows)));
    if any {
        ui.invoke_about_select(0);
    } else {
        ui.set_about_selected(-1);
        ui.set_about_text(SharedString::new());
    }
}
