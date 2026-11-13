//! Model previews, the viewer window, animations and exporting models.

use crate::*;

/// A loaded model ready to be rendered.
pub(crate) struct ModelPreview {
    pub(crate) loaded: openrage_app::models::LoadedModel,
    /// Shown parts and material colours (shared by the preview and viewer windows).
    pub(crate) look: std::sync::RwLock<openrage_app::models::Look>,
    pub(crate) materials: openrage_app::render::Materials,
    pub(crate) name: String,
}

pub(crate) fn start_model_preview(ui: &AppWindow, vpath: String, info: Meta, generation: u64) {
    let Some((game_dir, keys, counter)) =
        with_app(|a| a.game_dir.clone().map(|d| (d, a.keys.clone(), Arc::clone(&a.preview_generation))))
    else {
        return;
    };
    ui.set_preview_loading(true);
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let extra =
            if browser::is_particles(&vpath) { particle_rows(&game_dir, keys.clone(), &vpath) } else { Meta::new() };
        let result = load_model_preview(&game_dir, keys, &vpath);
        if counter.load(Ordering::SeqCst) != generation {
            return;
        }
        let _ = weak.upgrade_in_event_loop(move |ui| {
            if counter.load(Ordering::SeqCst) != generation {
                return;
            }
            ui.set_preview_loading(false);
            match result {
                Ok(m) => {
                    let mut meta = info.clone();
                    meta.extend(extra.iter().cloned());
                    show_model_preview(&ui, m, meta);
                }
                Err(e) => {
                    let mut meta = info.clone();
                    // Particle dictionaries without models still have effects to show.
                    if extra.is_empty() {
                        meta.push(kv("Error", format!("Cannot read the model: {e:#}")));
                    } else {
                        meta.extend(extra.iter().cloned());
                    }
                    set_meta(&ui, &meta);
                }
            }
        });
    });
}

/// Shows a loaded model in the preview pane, with its counts in the info panel.
pub(crate) fn show_model_preview(ui: &AppWindow, m: ModelPreview, mut meta: Meta) {
    let l = &m.loaded;
    // A model with textures of its own (GTA IV .wdr, GTA V .ydr/.yft/.ydd): offer to open them.
    let selected_wdr = with_app(|a| {
        let l = a.listing.as_ref()?;
        let item = &l.items[a.selected?];
        openrage_app::textures::is_model(&item.name).then(|| browser::join(&l.vpath, &item.name))
    });
    // Textures of all its drawables (a .ydd has a list per drawable), each name once.
    let mut names: Vec<String> = l
        .drawables
        .iter()
        .filter_map(|(_, d)| d.textures.as_ref())
        .flat_map(|t| t.textures.iter().map(|x| x.name.to_ascii_lowercase()))
        .collect();
    names.sort();
    names.dedup();
    let own = if selected_wdr.is_some() { names.len() } else { 0 };
    ui.set_model_textures(own as i32);
    with_app(|a| a.model_textures_path = selected_wdr.filter(|_| own > 0));
    let (mut meshes, mut shaders, mut textured) = (0, 0, 0);
    let vis = l.default_visibility();
    for (_, d) in l.shown(&vis) {
        meshes += d.meshes.len();
        shaders += d.shaders.len();
        textured += d.shaders.iter().filter(|s| s.base_texture().is_some_and(|t| l.texture(d, t).is_some())).count();
    }
    let shown = vis.iter().filter(|v| **v).count();
    if l.parts.len() > 1 {
        meta.push(kv("Parts", format!("{shown} shown of {}", l.parts.len())));
    }
    meta.push(kv("Meshes", meshes));
    meta.push(kv("Vertices", grouped(l.vertex_count(&vis))));
    meta.push(kv("Triangles", grouped(l.triangle_count(&vis))));
    meta.push(kv("Textured", format!("{textured} of {shaders} shaders")));
    set_meta(ui, &meta);
    with_app(|a| {
        a.model = Some(Arc::new(m));
        a.model_view = openrage_app::render::View::default();
    });
    ui.set_model_mode(true);
    render_model(ui);
    if with_app(|a| std::mem::take(&mut a.pending_viewer)) {
        open_viewer(ui);
    }
}

/// Summary rows of a particle dictionary: counts and the first effect names.
pub(crate) fn particle_rows(game_dir: &Path, keys: Option<Arc<GtaVKeys>>, vpath: &str) -> Meta {
    let read = || -> anyhow::Result<rage_formats::ypt::ParticleDictionary> {
        let (node, _) = rage_mods::resolve(game_dir, vpath, keys)?;
        let Node::ArchiveFile(rpf, id) = node else { anyhow::bail!("not in an archive") };
        Ok(rage_formats::ypt::parse(&rpf.read_resource(id)?)?)
    };
    match read() {
        Ok(p) => {
            let mut rows = vec![
                kv("Effects", p.effects.len()),
                kv("Emitters", p.emitters.len()),
                kv("Particle rules", p.particles.len()),
                kv("Textures", p.textures.as_ref().map_or(0, |t| t.textures.len())),
                kv("Models", p.drawables.len()),
            ];
            let mut names: Vec<&str> = p.effects.iter().map(String::as_str).collect();
            names.sort_unstable();
            let more = if names.len() > 12 { format!(", … ({} in all)", names.len()) } else { String::new() };
            if !names.is_empty() {
                rows.push(kv("Effect names", format!("{}{more}", names[..names.len().min(12)].join(", "))));
            }
            rows.push(note("Double-click to browse its textures; Open in viewer shows its models."));
            rows
        }
        Err(e) => vec![kv("Error", format!("Cannot read the particle dictionary: {e:#}"))],
    }
}

pub(crate) fn load_model_preview(
    game_dir: &Path,
    keys: Option<Arc<GtaVKeys>>,
    vpath: &str,
) -> anyhow::Result<ModelPreview> {
    openrage_app::models::load(game_dir, keys, vpath).map(model_preview)
}

/// A loaded model ready to show: default look, materials, a name for exports.
pub(crate) fn model_preview(loaded: openrage_app::models::LoadedModel) -> ModelPreview {
    let materials = openrage_app::render::Materials::new(&loaded);
    let name = match &loaded.ped {
        Some(p) => format!("{}_outfit_{}", p.name.to_ascii_lowercase(), p.outfit),
        None => loaded.drawables.first().map(|(n, _)| n.clone()).unwrap_or_default(),
    };
    let look = std::sync::RwLock::new(loaded.default_look());
    ModelPreview { loaded, look, materials, name }
}

/// Loads a model in the background and opens it in a viewer playing `ycd`.
pub(crate) fn open_model_with_animation(ui: &AppWindow, model: String, ycd: String) {
    let Some((game_dir, keys)) = with_app(|a| a.game_dir.clone().map(|d| (d, a.keys.clone()))) else { return };
    ui.set_status(format!("Opening {model} …").into());
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let result = load_model_preview(&game_dir, keys, &model);
        let _ = weak.upgrade_in_event_loop(move |ui| match result {
            Ok(m) => {
                let name = model.rsplit('/').next().unwrap_or(&model).to_owned();
                let (icon, tint) = badges::icon(&name, ItemKind::Resource);
                let labels = viewer::Labels {
                    heading: name,
                    path: model.clone(),
                    icon: icon.into(),
                    tint,
                    rows: ModelRc::default(),
                };
                viewer::open_with(&ui, viewer::Content::Model(Arc::new(m)), labels, Some(ycd));
                ui.set_status(SharedString::new());
            }
            Err(e) => ui.set_status(format!("Cannot open {model}: {e:#}").into()),
        });
    });
}

/// Models a clip dictionary can be played on, best first (slow: runs off the
/// UI thread). Candidates come from the names the dictionary and its clips
/// hint at, all peds for human animations and all vehicles for vehicle
/// animations without a named vehicle; they are ranked by how many of the
/// animated bones their skeletons have.
pub(crate) fn suggest_models(game_dir: &Path, keys: Option<Arc<GtaVKeys>>, profile: &suggest::Profile) -> Vec<String> {
    if is_gta4_dir(game_dir) {
        let tags: std::collections::HashSet<u16> =
            profile.animations.iter().flat_map(|(t, _)| t.iter().copied()).collect();
        return openrage_app::gta4::animation_models(game_dir, &tags);
    }
    let Some(index) = index_path(game_dir).filter(|p| p.exists()).and_then(|p| Index::open(&p).ok()) else {
        return Vec::new();
    };
    let search = |query: &str, limit: usize| -> Vec<String> {
        index.search(query, limit, None).map(|r| r.hits.into_iter().map(|h| h.vpath).collect()).unwrap_or_default()
    };
    let is_model = |v: &str| {
        let n = v.to_ascii_lowercase();
        (n.ends_with(".ydd") || n.ends_with(".yft") || n.ends_with(".ydr")) && !n.ends_with("_hi.yft")
    };
    // Models named exactly like a hint; peds as their drawable dictionary.
    let mut named: Vec<String> = Vec::new();
    for hint in profile.hints.iter().take(80) {
        let hits: Vec<String> =
            search(hint, 100).into_iter().filter(|v| is_model(v) && suggest::model_stem(v) == *hint).collect();
        let has_ydd = hits.iter().any(|v| v.ends_with(".ydd"));
        named.extend(hits.into_iter().filter(|v| !has_ydd || v.ends_with(".ydd")));
    }
    let mut candidates: Vec<suggest::Candidate> = Vec::new();
    let mut load: Vec<String> = named.clone();
    if let Some(animal) = &profile.animal {
        load.extend(search(&format!("a_c_{animal}"), 160).into_iter().filter(|v| v.ends_with(".ydd")));
    } else if profile.kinds.contains(&suggest::Kind::Ped) {
        // All peds share one skeleton: no need to load each.
        const PREFIXES: [&str; 13] =
            ["a_m_", "a_f_", "s_m_", "s_f_", "g_m_", "g_f_", "u_m_", "u_f_", "ig_", "cs_", "csb_", "mp_", "player_"];
        let mut peds: Vec<String> = search(".ydd", 24000)
            .into_iter()
            .filter(|v| {
                let n = v.rsplit('/').next().unwrap_or(v).to_ascii_lowercase();
                n.ends_with(".ydd")
                    && PREFIXES.iter().any(|p| n.starts_with(p))
                    && !n.contains("_p.ydd")
                    && v.contains("peds")
            })
            .collect();
        // Female clips suggest female peds first.
        peds.sort_by_key(|v| {
            let n = suggest::model_stem(v);
            let is_female = n.contains("_f_") || n.starts_with("a_f") || n.starts_with("s_f");
            (is_female != profile.female, v.starts_with("update"), n)
        });
        peds.dedup_by(|a, b| suggest::model_stem(a) == suggest::model_stem(b));
        peds.truncate(150);
        let skeleton = suggest::ped_skeleton();
        candidates.extend(peds.into_iter().map(|vpath| suggest::Candidate { vpath, tags: Some(skeleton.clone()) }));
    }
    if profile.kinds.contains(&suggest::Kind::Vehicle) && !named.iter().any(|v| v.contains("/vehicles")) {
        // No vehicle named: try them all (their skeletons are cached).
        let mut vehicles: Vec<String> = search(".yft", 200_000)
            .into_iter()
            .filter(|v| is_model(v) && v.contains("/vehicles") && !v.contains("vehiclemods"))
            .collect();
        vehicles.sort_by_key(|v| (suggest::model_stem(v), v.starts_with("update"), v.len()));
        vehicles.dedup_by(|a, b| suggest::model_stem(a) == suggest::model_stem(b));
        load.extend(vehicles);
    }
    if profile.kinds.contains(&suggest::Kind::Prop) && named.is_empty() {
        // Objects sharing a word of the dictionary name.
        if let Some(word) = profile.hints.iter().filter(|h| !h.contains('@')).max_by_key(|h| h.len()) {
            load.extend(search(word, 400).into_iter().filter(|v| is_model(v)).take(80));
        }
    }
    load.sort();
    load.dedup();
    // Read the skeletons in parallel.
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    let chunk = load.len().div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        let jobs: Vec<_> = load
            .chunks(chunk)
            .map(|paths| {
                let keys = keys.clone();
                scope.spawn(move || {
                    paths
                        .iter()
                        .map(|v| suggest::Candidate {
                            vpath: v.clone(),
                            tags: suggest::skeleton_tags(game_dir, keys.clone(), v),
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for job in jobs {
            candidates.extend(job.join().unwrap_or_default());
        }
    });
    let mut ranked = suggest::rank(profile, candidates);
    ranked.truncate(200);
    ranked
}

/// Renders a model into an image of `w × h` pixels: on the GPU if there is
/// one, else with the software renderer.
pub(crate) fn render_image(
    model: &Arc<ModelPreview>,
    camera: impl FnOnce([f32; 3], [f32; 3]) -> openrage_app::render::Camera,
    skin: Option<&[rage_formats::math::Mat4]>,
    w: usize,
    h: usize,
) -> Option<(slint::Image, ModelRc<GizmoAxis>)> {
    let look = model.look.read().ok()?.clone();
    let Some((lo, hi)) = openrage_app::render::bounds(&model.loaded, &look.parts) else {
        // Nothing shown: an empty (transparent) image.
        let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(w as u32, h as u32);
        return Some((slint::Image::from_rgba8_premultiplied(buffer), ModelRc::default()));
    };
    let camera = camera(lo, hi);
    let gizmo = gizmo_axes(&camera);
    if std::env::var_os("OPENRAGE_SOFTWARE_RENDERER").is_none()
        && let Some(img) = gpu::render(model, &look, &camera, skin, w as u32, h as u32)
    {
        return Some((img, gizmo));
    }
    let rgba = openrage_app::render::render_camera(&model.loaded, &look, &model.materials, &camera, skin, w, h);
    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, w as u32, h as u32);
    Some((slint::Image::from_rgba8(buffer), gizmo))
}

/// The world axes as the camera sees them, for the orientation compass.
pub(crate) fn gizmo_axes(camera: &openrage_app::render::Camera) -> ModelRc<GizmoAxis> {
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let axes: Vec<GizmoAxis> = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
        .iter()
        .map(|a| GizmoAxis { dx: dot(*a, camera.right), dy: -dot(*a, camera.up), away: dot(*a, camera.forward) > 0.0 })
        .collect();
    ModelRc::new(VecModel::from(axes))
}

/// Orbit angles that look along an axis (0 = X, 1 = Y, 2 = Z from above).
pub(crate) fn axis_view(axis: i32, current: openrage_app::render::View) -> openrage_app::render::View {
    let mut v = current;
    match axis {
        0 => {
            v.yaw = 0.0;
            v.pitch = 0.0;
        }
        1 => {
            v.yaw = std::f32::consts::FRAC_PI_2;
            v.pitch = 0.0;
        }
        _ => v.pitch = 1.5,
    }
    v
}

/// Renders the current model at the size of the preview area.
pub(crate) fn render_model(ui: &AppWindow) {
    let Some((model, view)) = with_app(|a| a.model.clone().map(|m| (m, a.model_view))) else { return };
    let w = (ui.get_preview_area_width() - 16.0).max(64.0) as usize;
    let h = (ui.get_preview_area_height() - 16.0).max(64.0) as usize;
    if let Some((img, gizmo)) =
        render_image(&model, |lo, hi| openrage_app::render::orbit_camera(lo, hi, view), None, w, h)
    {
        ui.set_gizmo(gizmo);
        ui.set_preview_image(img);
        ui.set_has_image(true);
    }
}

/// Opens the selected model or image in its own window (once it has loaded).
pub(crate) fn open_viewer(ui: &AppWindow) {
    if ui.get_model_mode() {
        if let Some(m) = with_app(|a| a.model.clone()) {
            viewer::open(ui, viewer::Content::Model(m));
            return;
        }
    } else if ui.get_has_image() {
        viewer::open(ui, viewer::Content::Image(ui.get_preview_image()));
        return;
    }
    if ui.get_preview_loading() {
        with_app(|a| a.pending_viewer = true);
        ui.set_status("The viewer opens when the preview has loaded…".into());
    }
}

pub(crate) fn export_gltf(ui: &AppWindow) {
    if let Some(model) = with_app(|a| a.model.clone()) {
        save_glb(model, ui.as_weak());
    }
}

/// Asks for a file name and writes a scene's shown objects as one `.glb`
/// (centred on `center`); reports in the main window.
pub(crate) fn save_scene_glb(
    name: String,
    items: Vec<(Arc<ModelPreview>, openrage_app::models::Look, Vec<rage_formats::math::Mat4>)>,
    center: [f32; 3],
    main: Weak<AppWindow>,
) {
    std::thread::spawn(move || {
        let stem = name.split('.').next().unwrap_or(&name).to_owned();
        let Some(path) = rfd::FileDialog::new()
            .set_title("Export glTF")
            .set_file_name(format!("{stem}.glb"))
            .add_filter("glTF binary", &["glb"])
            .save_file()
        else {
            return;
        };
        let refs: Vec<(&openrage_app::models::LoadedModel, &openrage_app::models::Look, &[rage_formats::math::Mat4])> =
            items.iter().map(|(p, l, t)| (&p.loaded, l, t.as_slice())).collect();
        let result = openrage_app::models::scene_to_glb(&stem, &refs, center)
            .ok_or_else(|| "nothing visible to export".to_owned())
            .and_then(|glb| std::fs::write(&path, glb).map_err(|e| e.to_string()));
        let message = match result {
            Ok(()) => format!("Exported {}", path.display()),
            Err(e) => format!("Cannot export {stem}: {e}"),
        };
        let _ = main.upgrade_in_event_loop(move |ui| ui.set_status(message.into()));
    });
}

/// Asks for a file name and writes the model as `.glb`; reports in the main window.
pub(crate) fn save_glb(model: Arc<ModelPreview>, main: Weak<AppWindow>) {
    std::thread::spawn(move || {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Export glTF")
            .set_file_name(format!("{}.glb", model.name))
            .add_filter("glTF binary", &["glb"])
            .save_file()
        else {
            return;
        };
        let look = model.look.read().map(|l| l.clone()).unwrap_or_else(|_| model.loaded.default_look());
        let result = openrage_app::models::to_glb(&model.loaded, &model.name, &look)
            .ok_or_else(|| "nothing visible to export".to_owned())
            .and_then(|glb| std::fs::write(&path, glb).map_err(|e| e.to_string()));
        let msg = match result {
            Ok(()) => format!("Exported {}", path.display()),
            Err(e) => format!("Export failed: {e}"),
        };
        let _ = main.upgrade_in_event_loop(move |ui| ui.set_status(msg.into()));
    });
}

/// Asks for a file name and saves an image shown in a viewer as PNG.
pub(crate) fn save_png_image(img: &slint::Image, name: String, main: Weak<AppWindow>) {
    let Some(pixels) = img.to_rgba8() else { return };
    let (w, h) = (pixels.width(), pixels.height());
    let rgba = pixels.as_bytes().to_vec();
    std::thread::spawn(move || {
        let stem = name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s).to_owned();
        let Some(path) = rfd::FileDialog::new()
            .set_title("Save PNG")
            .set_file_name(format!("{stem}.png"))
            .add_filter("PNG image", &["png"])
            .save_file()
        else {
            return;
        };
        let result = rage_formats::png(w, h, &rgba)
            .map_err(|e| e.to_string())
            .and_then(|png| std::fs::write(&path, png).map_err(|e| e.to_string()));
        let msg = match result {
            Ok(()) => format!("Saved {}", path.display()),
            Err(e) => format!("Saving failed: {e}"),
        };
        let _ = main.upgrade_in_event_loop(move |ui| ui.set_status(msg.into()));
    });
}

// ---------------------------------------------------------------------------
// Animations

/// Whether an item is a clip dictionary (`.ycd`; GTA IV `.wad`).
pub(crate) fn is_clip_dictionary(item: &Item) -> bool {
    let n = item.name.to_ascii_lowercase();
    n.ends_with(".ycd") || (n.ends_with(".wad") && item.kind == ItemKind::Resource)
}

/// Selecting a `.ycd` (or `.wad`): its clips and (found in the background) the models it fits.
pub(crate) fn show_clip_dictionary(ui: &AppWindow, vpath: String, mut info: Meta) {
    match load_clip_dictionary(&vpath) {
        Ok(dict) => {
            let profile = suggest::Profile::new(&vpath, &dict);
            info.push(kv("Clips", dict.clips.len()));
            info.push(kv("Animations", dict.animations.len()));
            // The kinds are told apart by GTA V's bone tags.
            let gta4 = with_app(|a| a.game_dir.as_deref().is_some_and(is_gta4_dir));
            info.push(kv("Kind", if gta4 { "GTA IV animation".to_owned() } else { profile.describe() }));
            let names: Vec<&str> = dict.clips.iter().take(12).map(|c| c.name.as_str()).collect();
            let more = if dict.clips.len() > 12 { ", …" } else { "" };
            info.push(kv("Clips", format!("{}{more}", names.join(", "))));
            info.push(note("Pick a model to play the animation on it; double-click the file opens the first."));
            set_meta(ui, &info);
            ui.set_suggest_title("Looking for fitting models…".into());
            ui.set_suggest_icon("model".into());
            ui.set_suggestions(ModelRc::default());
            ui.set_suggest_pending(true);
            let Some((game_dir, keys)) = with_app(|a| {
                a.suggestions.clear();
                a.suggest_ycd = vpath.clone();
                a.suggest_pending = true;
                a.game_dir.clone().map(|d| (d, a.keys.clone()))
            }) else {
                return;
            };
            let weak = ui.as_weak();
            std::thread::spawn(move || {
                let models = suggest_models(&game_dir, keys, &profile);
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    // Another file may have been selected meanwhile.
                    if !with_app(|a| a.suggest_ycd == vpath) {
                        return;
                    }
                    let rows: Vec<ClipRow> = models
                        .iter()
                        .map(|m| {
                            let (dir, name) = m.rsplit_once('/').unwrap_or(("", m));
                            ClipRow { name: name.into(), info: dir.into() }
                        })
                        .collect();
                    ui.set_suggest_title(
                        if rows.is_empty() {
                            "No fitting models found (the search index may still be building).".to_owned()
                        } else {
                            format!("{} fitting models, best first — click one to play the animation", rows.len())
                        }
                        .into(),
                    );
                    ui.set_suggestions(ModelRc::new(VecModel::from(rows)));
                    ui.set_suggest_pending(false);
                    with_app(|a| {
                        a.suggestions = models;
                        a.suggest_pending = false;
                    });
                });
            });
        }
        Err(e) => {
            info.push(kv("Error", format!("{e:#}")));
            set_meta(ui, &info);
        }
    }
}

/// Files whose name contains `query` and ends with `extension`, from the search index.
pub(crate) fn find_files(query: &str, extension: &str, limit: usize) -> Vec<String> {
    let Some(game_dir) = with_app(|a| a.game_dir.clone()) else { return Vec::new() };
    let Some(path) = openrage_app::index_path(&game_dir).filter(|p| p.exists()) else { return Vec::new() };
    let Ok(index) = rage_index::Index::open(&path) else { return Vec::new() };
    let Ok(results) = index.search(query, limit * 4, None) else { return Vec::new() };
    let mut out: Vec<String> = results
        .hits
        .into_iter()
        .filter(|h| h.name.to_ascii_lowercase().ends_with(extension))
        .map(|h| h.vpath)
        .collect();
    // The base game first, then updates and DLC.
    out.sort_by_key(|v| (v.starts_with("update"), v.len()));
    out.truncate(limit);
    out
}

pub(crate) fn load_clip_dictionary(vpath: &str) -> anyhow::Result<rage_formats::clip::ClipDictionary> {
    let (game_dir, keys) = with_app(|a| (a.game_dir.clone(), a.keys.clone()));
    let game_dir = game_dir.ok_or_else(|| anyhow::anyhow!("no game folder"))?;
    openrage_app::anim::load_clips(&game_dir, keys, vpath)
}
