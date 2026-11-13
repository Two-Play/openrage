//! `openrage plugin`: list, install, enable and run plugins.

use crate::*;

pub(crate) type GameContext<'a> = (&'a Path, Option<Arc<GtaVKeys>>, Arc<dyn openrage_plugins::GameFiles>);

pub(crate) fn cmd_plugin(cfg: &mut Config, game: Option<GameContext>, cmd: &PluginCommand) -> Result<()> {
    use openrage_plugins::{Host, Limits};
    let registry = openrage_app::plugins::registry(cfg);
    let find = |id: &str| {
        registry.get(id).with_context(|| format!("no plugin {id:?} is installed (see `openrage plugin list`)"))
    };
    let files: Arc<dyn openrage_plugins::GameFiles> =
        game.as_ref().map_or_else(|| Arc::new(openrage_plugins::NoGame) as _, |g| Arc::clone(&g.2));
    let game_id = files.game();
    // Reads a picked file, from the game or from disk.
    let read = |path: &str, from_disk: bool| -> Result<(String, Vec<u8>)> {
        if from_disk {
            let name = Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            return Ok((name, fs::read(path).with_context(|| format!("reading {path}"))?));
        }
        if game.is_none() {
            bail!("no game directory configured; pass --game <DIR>, or --file to read a file on disk")
        }
        Ok((path.replace('\\', "/"), files.read(path)?))
    };
    let print_log = |log: &[openrage_plugins::LogLine]| {
        for line in log {
            eprintln!("[plugin] {}: {}", openrage_plugins::level_name(line.level), line.message);
        }
    };

    match cmd {
        PluginCommand::List => {
            outln!("Plugin folders:");
            for d in openrage_plugins::search_dirs() {
                outln!("  {}", d.display());
            }
            if registry.plugins.is_empty() && registry.broken.is_empty() {
                outln!("No plugins installed.");
            }
            for p in &registry.plugins {
                let m = &p.manifest;
                outln!("{:<20} {:<10} {:<28} {}", m.id, m.version, status_text(&p.status), m.name);
            }
            for b in &registry.broken {
                outln!("broken: {}: {}", b.dir.display(), b.error);
            }
            Ok(())
        }
        PluginCommand::Info { id } => {
            let p = find(id)?;
            let m = &p.manifest;
            outln!("{} {} ({})", m.name, m.version, m.id);
            if !m.description.is_empty() {
                outln!("{}", m.description);
            }
            outln!("Folder:      {}", p.dir.display());
            outln!("Status:      {}", status_text(&p.status));
            if !m.authors.is_empty() {
                outln!("Authors:     {}", m.authors.join(", "));
            }
            if !m.license.is_empty() {
                outln!("Licence:     {}", m.license);
            }
            if !m.homepage.is_empty() {
                outln!("Homepage:    {}", m.homepage);
            }
            let games: Vec<_> = m.games.iter().map(|g| g.as_str()).collect();
            outln!("Games:       {}", if games.is_empty() { "all".to_owned() } else { games.join(", ") });
            if m.permissions.is_empty() {
                outln!("Permissions: none");
            }
            for perm in &m.permissions {
                outln!("Permission:  {perm} — {}", perm.description());
            }
            for t in &m.file_types {
                outln!("File type:   {} ({}) — {}", t.extensions.join(", "), t.category, t.name);
            }
            for v in &m.preview {
                outln!("Preview:     {}", v.extensions.join(", "));
            }
            for a in &m.actions {
                let mut notes = vec![format!("{:?}", a.kind).to_ascii_lowercase()];
                if a.batch {
                    notes.push("also on folders".into());
                }
                if !a.pick.is_empty() {
                    notes.push(format!("asks for {}", a.pick.join("/")));
                }
                outln!("Action:      {} — {} [{}] ({})", a.id, a.title, a.extensions.join(", "), notes.join(", "));
            }
            Ok(())
        }
        PluginCommand::Enable { id, yes } => {
            let p = find(id)?;
            let perms = &p.manifest.permissions;
            if !perms.is_empty() {
                outln!("{} asks for:", p.manifest.name);
                for perm in perms {
                    outln!("  {perm}: {}", perm.description());
                }
                confirm("Grant and enable it?", *yes)?;
            }
            openrage_app::plugins::enable(cfg, id, perms);
            cfg.save()?;
            outln!("Enabled {}.", p.manifest.name);
            Ok(())
        }
        PluginCommand::Install { file, yes } => {
            let pkg = openrage_plugins::package::Package::open(file)?;
            let m = &pkg.manifest;
            Host::new(Limits::default())?
                .validate(&pkg.files.iter().find(|(n, _)| n.eq_ignore_ascii_case(&m.wasm)).expect("checked").1)
                .with_context(|| format!("{} cannot run in this OpenRAGE", m.name))?;
            outln!("{} {} ({})", m.name, m.version, m.id);
            if !m.description.is_empty() {
                outln!("{}", m.description);
            }
            if !m.authors.is_empty() {
                outln!("Authors:     {}", m.authors.join(", "));
            }
            outln!("Licence:     {}", if m.license.is_empty() { "not stated" } else { &m.license });
            if !m.homepage.is_empty() {
                outln!("Homepage:    {}", m.homepage);
            }
            if m.permissions.is_empty() {
                outln!("Permissions: none (it sees only the files you open with it)");
            }
            for perm in &m.permissions {
                outln!("Permission:  {perm} — {}", perm.description());
            }
            outln!("Package:     {} files, {}", pkg.files.len(), human(pkg.size));
            outln!("SHA-256:     {}", pkg.sha256);
            if let Some(old) = registry.get(&m.id) {
                outln!("Replaces:    {} {} in {}", old.manifest.name, old.manifest.version, old.dir.display());
            }
            confirm("Install?", *yes)?;
            let root = openrage_plugins::user_dir().context("no per-user config folder")?;
            let dir = pkg.install(&root)?;
            outln!("Installed into {}.", dir.display());
            let reg = openrage_app::plugins::registry(cfg);
            match reg.get(&m.id).map(|p| &p.status) {
                Some(openrage_plugins::Status::Enabled) => outln!("It is on."),
                Some(openrage_plugins::Status::NeedsApproval(_)) => {
                    outln!("It asks for more permissions than before: `openrage plugin enable {}` turns it on.", m.id)
                }
                _ => outln!("It is off until you turn it on: `openrage plugin enable {}`.", m.id),
            }
            Ok(())
        }
        PluginCommand::Uninstall { id, yes } => {
            confirm(&format!("Remove the plugin {id}?"), *yes)?;
            let root = openrage_plugins::user_dir().context("no per-user config folder")?;
            if let Some(p) = registry.get(id)
                && !p.dir.starts_with(&root)
            {
                bail!("{} is not in your plugin folder but in {}; remove that folder yourself", id, p.dir.display());
            }
            let removed = openrage_plugins::package::uninstall(&root, id)?;
            if openrage_app::plugins::disable(cfg, id) {
                cfg.save()?;
            }
            outln!("Removed {}.", removed.display());
            Ok(())
        }
        PluginCommand::Pack { dir, out, wasm } => {
            let data = openrage_plugins::package::pack(dir, wasm.as_deref())?;
            let pkg = openrage_plugins::package::Package::from_bytes(&data)?;
            let m = &pkg.manifest;
            let out = out.clone().unwrap_or_else(|| {
                PathBuf::from(format!("{}-{}.{}", m.id, m.version, openrage_plugins::package::EXTENSION))
            });
            fs::write(&out, &data).with_context(|| format!("writing {}", out.display()))?;
            outln!(
                "Packed {} {} into {} ({} files, {})",
                m.name,
                m.version,
                out.display(),
                pkg.files.len(),
                human(pkg.size)
            );
            outln!("SHA-256: {}", pkg.sha256);
            Ok(())
        }
        PluginCommand::Disable { id } => {
            if openrage_app::plugins::disable(cfg, id) {
                cfg.save()?;
                outln!("Disabled {id}.");
            } else {
                outln!("{id} was not enabled.");
            }
            Ok(())
        }
        PluginCommand::Preview { path, plugin, file, png, glb } => {
            let (vpath, data) = read(path, *file)?;
            let p = match plugin {
                Some(id) => find(id)?,
                None => registry
                    .preview_for(&vpath, game_id)
                    .with_context(|| format!("no enabled plugin previews {vpath}"))?,
            };
            if !p.enabled() {
                bail!("{} is not enabled (`openrage plugin enable {}`)", p.manifest.name, p.manifest.id);
            }
            let host = Host::new(Limits::default())?;
            let component = host.compile(p)?;
            let outcome = host.preview(p, &component, files, &vpath, &data);
            print_log(&outcome.log);
            let preview = outcome.value.with_context(|| format!("{} could not preview {vpath}", p.manifest.name))?;
            for i in &preview.info {
                outln!("{}: {}", i.key, i.value);
            }
            match preview.view {
                None => {}
                Some(openrage_plugins::View::Text(text)) => outln!("\n{text}"),
                Some(openrage_plugins::View::Tree(nodes)) => {
                    let mut depth = vec![0usize; nodes.len()];
                    outln!();
                    for (i, n) in nodes.iter().enumerate() {
                        depth[i] = n.parent.map_or(0, |p| depth[p as usize] + 1);
                        let value = if n.value.is_empty() { String::new() } else { format!(": {}", n.value) };
                        outln!("{}{}{value}", "  ".repeat(depth[i]), n.label);
                    }
                }
                Some(openrage_plugins::View::Table(t)) => {
                    outln!("\n{}", t.columns.join("\t"));
                    for row in &t.rows {
                        outln!("{}", row.join("\t"));
                    }
                }
                Some(openrage_plugins::View::Model(model)) => {
                    let vertices: usize = model.meshes.iter().map(|m| m.positions.len() / 3).sum();
                    let triangles: usize = model.meshes.iter().map(|m| m.indices.len() / 3).sum();
                    outln!(
                        "Model: {} meshes, {vertices} vertices, {triangles} triangles, {} materials",
                        model.meshes.len(),
                        model.materials.len()
                    );
                    if let Some(out) = glb {
                        let stem = vpath.rsplit('/').next().unwrap_or(&vpath).split('.').next().unwrap_or("model");
                        let loaded = openrage_app::plugins::from_plugin_model(&model, stem);
                        let data = openrage_app::models::to_glb(&loaded, stem, &loaded.default_look())
                            .context("the model has nothing to export")?;
                        fs::write(out, data)?;
                        outln!("Saved {}", out.display());
                    }
                }
                Some(openrage_plugins::View::Image(img)) => {
                    outln!("Image: {}×{}", img.width, img.height);
                    if let Some(out) = png {
                        fs::write(out, rage_formats::png(img.width, img.height, &img.rgba)?)?;
                        outln!("Saved {}", out.display());
                    }
                }
            }
            Ok(())
        }
        PluginCommand::Run { id, action, paths, file, pick, out, yes } => {
            let p = find(id)?;
            if !p.enabled() {
                bail!("{} is not enabled (`openrage plugin enable {id}`)", p.manifest.name);
            }
            let Some(spec) = p.manifest.action(action) else {
                let ids: Vec<_> = p.manifest.actions.iter().map(|a| a.id.as_str()).collect();
                bail!("{} has no action {action:?} (it has: {})", p.manifest.name, ids.join(", "));
            };
            let picked = pick
                .iter()
                .map(|f| -> Result<openrage_app::plugins::Picked> {
                    let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    Ok(openrage_app::plugins::Picked {
                        name,
                        data: fs::read(f).with_context(|| format!("reading {}", f.display()))?,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let host = Host::new(Limits::default())?;
            let component = host.compile(p)?;
            let result = if *file {
                // Files from disk: one call, no folders.
                let mut inputs = Vec::new();
                for path in paths {
                    let (vpath, data) = read(path, true)?;
                    inputs.push(openrage_plugins::Input { vpath, data, picked: false });
                }
                inputs.extend(picked.iter().map(|f| openrage_plugins::Input {
                    vpath: f.name.clone(),
                    data: f.data.clone(),
                    picked: true,
                }));
                let outcome = host.run_action(p, &component, Arc::clone(&files), action, &inputs);
                let outputs = outcome.value;
                openrage_app::plugins::RunResult {
                    outputs: match outputs {
                        Ok(o) => o,
                        Err(e) => {
                            print_log(&outcome.log);
                            return Err(e.context(format!("{} failed", p.manifest.name)));
                        }
                    },
                    writes: outcome.writes,
                    log: outcome.log,
                    files: paths.len(),
                    ..Default::default()
                }
            } else {
                let mut last = Instant::now();
                openrage_app::plugins::run_action(
                    &host,
                    p,
                    &component,
                    Arc::clone(&files),
                    action,
                    paths,
                    &picked,
                    &mut |done, total, path| {
                        if last.elapsed().as_millis() > 200 || done == total {
                            eprint!("\r[{done}/{total}] {:<60.60}", path);
                            last = Instant::now();
                        }
                        true
                    },
                )
                .with_context(|| format!("{} failed", p.manifest.name))?
            };
            if result.files > 1 {
                eprintln!();
            }
            print_log(&result.log);
            for (path, error) in &result.failures {
                eprintln!("failed: {path}: {error}");
            }
            if !result.outputs.is_empty() {
                let out = out.as_ref().context("the action produced files; pass --out <DIR> to save them")?;
                for o in &result.outputs {
                    // The host has checked that names stay inside `out`.
                    let target = out.join(rage_rpf::vfs::safe_relative_path(&o.name).context("bad output name")?);
                    if let Some(parent) = target.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::write(&target, &o.data).with_context(|| format!("writing {}", target.display()))?;
                    outln!("Saved {} ({})", target.display(), human(o.data.len() as u64));
                }
            }
            if !result.writes.is_empty() {
                let Some((game_dir, keys, _)) = &game else {
                    bail!("the action changes game files, but no game is set up")
                };
                outln!("{} wants to change {} file(s) (into the mods folder):", p.manifest.name, result.writes.len());
                for w in &result.writes {
                    outln!("  {} ({})", w.vpath, human(w.data.len() as u64));
                }
                confirm("Apply?", *yes)?;
                let description = format!("{}: {}", p.manifest.name, spec.title.trim_end_matches('…'));
                let group = openrage_app::plugins::group_name(&description);
                for line in
                    openrage_app::plugins::apply_writes(game_dir, keys.clone(), &result.writes, &description, &group)?
                {
                    outln!("Changed {line}");
                }
                outln!("Undo with `openrage undo` or in History (\"{group}\").");
            }
            if result.outputs.is_empty() && result.writes.is_empty() {
                outln!("The plugin produced no files and changed nothing.");
            }
            if !result.failures.is_empty() {
                bail!("{} of {} files failed", result.failures.len(), result.files + result.failures.len());
            }
            Ok(())
        }
    }
}

pub(crate) fn status_text(status: &openrage_plugins::Status) -> String {
    match status {
        openrage_plugins::Status::Enabled => "enabled".to_owned(),
        openrage_plugins::Status::Disabled => "disabled".to_owned(),
        openrage_plugins::Status::NeedsApproval(missing) => {
            let names: Vec<_> = missing.iter().map(|p| p.as_str()).collect();
            format!("needs approval ({})", names.join(", "))
        }
    }
}
