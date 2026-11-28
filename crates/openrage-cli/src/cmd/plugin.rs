//! `openrage plugin`: list, install, enable and run plugins.

use crate::*;
use openrage_plugins::{Plugin, Registry};

mod run;
use run::{RunArgs, plugin_run};

pub(crate) type GameContext<'a> = (&'a Path, Option<Arc<GtaVKeys>>, Arc<dyn openrage_plugins::GameFiles>);

/// What the plugin subcommands share: the installed plugins and the game (if any).
struct PluginEnv<'a> {
    registry: &'a Registry,
    game: Option<&'a GameContext<'a>>,
    files: Arc<dyn openrage_plugins::GameFiles>,
}

impl<'a> PluginEnv<'a> {
    fn find(&self, id: &str) -> Result<&'a Plugin> {
        self.registry.get(id).with_context(|| format!("no plugin {id:?} is installed (see `openrage plugin list`)"))
    }

    /// Reads a picked file, from the game or from disk.
    fn read(&self, path: &str, from_disk: bool) -> Result<(String, Vec<u8>)> {
        if from_disk {
            let name = Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            return Ok((name, fs::read(path).with_context(|| format!("reading {path}"))?));
        }
        if self.game.is_none() {
            bail!("no game directory configured; pass --game <DIR>, or --file to read a file on disk")
        }
        Ok((path.replace('\\', "/"), self.files.read(path)?))
    }
}

fn print_log(log: &[openrage_plugins::LogLine]) {
    for line in log {
        eprintln!("[plugin] {}: {}", openrage_plugins::level_name(line.level), line.message);
    }
}

fn ensure_enabled(p: &Plugin, id: &str) -> Result<()> {
    if !p.enabled() {
        bail!("{} is not enabled (`openrage plugin enable {id}`)", p.manifest.name);
    }
    Ok(())
}

pub(crate) fn cmd_plugin(cfg: &mut Config, game: Option<GameContext>, cmd: &PluginCommand) -> Result<()> {
    let registry = openrage_app::plugins::registry(cfg);
    let files: Arc<dyn openrage_plugins::GameFiles> =
        game.as_ref().map_or_else(|| Arc::new(openrage_plugins::NoGame) as _, |g| Arc::clone(&g.2));
    let env = PluginEnv { registry: &registry, game: game.as_ref(), files };

    match cmd {
        PluginCommand::List => plugin_list(&registry),
        PluginCommand::Info { id } => plugin_info(env.find(id)?),
        PluginCommand::Enable { id, yes } => plugin_enable(cfg, env.find(id)?, id, *yes),
        PluginCommand::Install { file, yes } => plugin_install(&env, cfg, file, *yes),
        PluginCommand::Uninstall { id, yes } => plugin_uninstall(&registry, cfg, id, *yes),
        PluginCommand::Pack { dir, out, wasm } => plugin_pack(dir, out.as_deref(), wasm.as_deref()),
        PluginCommand::Disable { id } => plugin_disable(cfg, id),
        PluginCommand::Preview { .. } | PluginCommand::Tool { .. } | PluginCommand::Run { .. } => plugin_use(&env, cmd),
    }
}

/// The subcommands that run a plugin. `cmd_plugin` names them, so a new
/// subcommand does not compile until it is routed there.
fn plugin_use(env: &PluginEnv, cmd: &PluginCommand) -> Result<()> {
    match cmd {
        PluginCommand::Preview { path, plugin, file, png, glb } => {
            let args =
                PreviewArgs { path, plugin: plugin.as_deref(), file: *file, png: png.as_deref(), glb: glb.as_deref() };
            plugin_preview(env, &args)
        }
        PluginCommand::Tool { id, tool, set } => plugin_tool(env, id, tool, set),
        PluginCommand::Run { id, action, paths, file, pick, out, yes } => {
            let args = RunArgs { id, action, paths, file: *file, pick, out: out.as_deref(), yes: *yes };
            plugin_run(env, &args)
        }
        _ => Ok(()),
    }
}

fn plugin_list(registry: &Registry) -> Result<()> {
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

fn plugin_info(p: &Plugin) -> Result<()> {
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
    info_contributions(m);
    Ok(())
}

/// What a plugin asks for and offers: permissions, file types, previews, actions and tools.
fn info_contributions(m: &openrage_plugins::Manifest) {
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
        info_action(a);
    }
    for t in &m.tools {
        info_tool(t);
    }
}

fn info_action(a: &openrage_plugins::Action) {
    let mut notes = vec![format!("{:?}", a.kind).to_ascii_lowercase()];
    if a.batch {
        notes.push("also on folders".into());
    }
    if !a.pick.is_empty() {
        notes.push(format!("asks for {}", a.pick.join("/")));
    }
    outln!("Action:      {} — {} [{}] ({})", a.id, a.title, a.extensions.join(", "), notes.join(", "));
}

fn info_tool(t: &openrage_plugins::manifest::Tool) {
    outln!("Tool:        {} — {}", t.id, t.title);
    for f in &t.fields {
        let kind = match f.kind {
            openrage_plugins::manifest::FieldKind::Text => "text".to_owned(),
            openrage_plugins::manifest::FieldKind::Check => "true/false".to_owned(),
            openrage_plugins::manifest::FieldKind::Choice => f.choices.join("/"),
        };
        outln!("               --set {}=…  {} ({kind}, starts as {:?})", f.id, f.label, f.initial());
    }
}

fn plugin_enable(cfg: &mut Config, p: &Plugin, id: &str, yes: bool) -> Result<()> {
    let perms = &p.manifest.permissions;
    if !perms.is_empty() {
        outln!("{} asks for:", p.manifest.name);
        for perm in perms {
            outln!("  {perm}: {}", perm.description());
        }
        confirm("Grant and enable it?", yes)?;
    }
    openrage_app::plugins::enable(cfg, id, perms);
    cfg.save()?;
    outln!("Enabled {}.", p.manifest.name);
    Ok(())
}

fn plugin_install(env: &PluginEnv, cfg: &Config, file: &Path, yes: bool) -> Result<()> {
    use openrage_plugins::{Host, Limits};
    let pkg = openrage_plugins::package::Package::open(file)?;
    let m = &pkg.manifest;
    Host::new(Limits::default())?
        .validate(&pkg.files.iter().find(|(n, _)| n.eq_ignore_ascii_case(&m.wasm)).expect("checked").1)
        .with_context(|| format!("{} cannot run in this OpenRAGE", m.name))?;
    print_package(&pkg);
    if let Some(old) = env.registry.get(&m.id) {
        outln!("Replaces:    {} {} in {}", old.manifest.name, old.manifest.version, old.dir.display());
    }
    confirm("Install?", yes)?;
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

fn print_package(pkg: &openrage_plugins::package::Package) {
    let m = &pkg.manifest;
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
}

fn plugin_uninstall(registry: &Registry, cfg: &mut Config, id: &str, yes: bool) -> Result<()> {
    confirm(&format!("Remove the plugin {id}?"), yes)?;
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

fn plugin_pack(dir: &Path, out: Option<&Path>, wasm: Option<&Path>) -> Result<()> {
    let data = openrage_plugins::package::pack(dir, wasm)?;
    let pkg = openrage_plugins::package::Package::from_bytes(&data)?;
    let m = &pkg.manifest;
    let out = out
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(format!("{}-{}.{}", m.id, m.version, openrage_plugins::package::EXTENSION)));
    fs::write(&out, &data).with_context(|| format!("writing {}", out.display()))?;
    outln!("Packed {} {} into {} ({} files, {})", m.name, m.version, out.display(), pkg.files.len(), human(pkg.size));
    outln!("SHA-256: {}", pkg.sha256);
    Ok(())
}

fn plugin_disable(cfg: &mut Config, id: &str) -> Result<()> {
    if openrage_app::plugins::disable(cfg, id) {
        cfg.save()?;
        outln!("Disabled {id}.");
    } else {
        outln!("{id} was not enabled.");
    }
    Ok(())
}

/// The arguments of `plugin preview`.
struct PreviewArgs<'a> {
    path: &'a str,
    plugin: Option<&'a str>,
    file: bool,
    png: Option<&'a Path>,
    glb: Option<&'a Path>,
}

fn plugin_preview(env: &PluginEnv, args: &PreviewArgs) -> Result<()> {
    use openrage_plugins::{Host, Limits};
    let (vpath, data) = env.read(args.path, args.file)?;
    let p = match args.plugin {
        Some(id) => env.find(id)?,
        None => env
            .registry
            .preview_for(&vpath, env.files.game())
            .with_context(|| format!("no enabled plugin previews {vpath}"))?,
    };
    ensure_enabled(p, &p.manifest.id)?;
    let host = Host::new(Limits::default())?;
    let component = host.compile(p)?;
    let outcome = host.preview(p, &component, Arc::clone(&env.files), &vpath, &data);
    print_log(&outcome.log);
    let preview = outcome.value.with_context(|| format!("{} could not preview {vpath}", p.manifest.name))?;
    for i in &preview.info {
        outln!("{}: {}", i.key, i.value);
    }
    print_view(preview.view, &vpath, args.png, args.glb)
}

fn plugin_tool(env: &PluginEnv, id: &str, tool: &str, set: &[String]) -> Result<()> {
    use openrage_plugins::{Host, Limits};
    let p = env.find(id)?;
    ensure_enabled(p, id)?;
    let Some(spec) = p.manifest.tool(tool) else {
        let ids: Vec<_> = p.manifest.tools.iter().map(|t| t.id.as_str()).collect();
        bail!("{} has no tool {tool:?} (it has: {})", p.manifest.name, ids.join(", "));
    };
    let values = tool_values(spec, set)?;
    let host = Host::new(Limits::default())?;
    let component = host.compile(p)?;
    let outcome = host.run_tool(p, &component, Arc::clone(&env.files), tool, &values);
    print_log(&outcome.log);
    let result = outcome.value.with_context(|| format!("{}: {}", p.manifest.name, spec.title))?;
    for i in &result.info {
        outln!("{}: {}", i.key, i.value);
    }
    print_view(result.view, &openrage_plugins::manifest::tool_vpath(tool), None, None)
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

/// Prints a plugin's view (text, tree, table, model or image); `png`/`glb`
/// save images and models.
fn print_view(view: Option<openrage_plugins::View>, vpath: &str, png: Option<&Path>, glb: Option<&Path>) -> Result<()> {
    match view {
        None => {}
        Some(openrage_plugins::View::Text(text)) => outln!("\n{text}"),
        Some(openrage_plugins::View::Tree(nodes)) => print_tree(&nodes),
        Some(openrage_plugins::View::Table(t)) => print_table(&t),
        Some(openrage_plugins::View::Model(model)) => print_model(&model, vpath, glb)?,
        Some(openrage_plugins::View::Image(img)) => print_image(&img, png)?,
    }
    Ok(())
}

fn print_table(t: &openrage_plugins::Table) {
    outln!("\n{}", t.columns.join("\t"));
    for row in &t.rows {
        outln!("{}", row.join("\t"));
    }
}

fn print_image(img: &openrage_plugins::ImageView, png: Option<&Path>) -> Result<()> {
    outln!("Image: {}×{}", img.width, img.height);
    if let Some(out) = png {
        fs::write(out, rage_formats::png(img.width, img.height, &img.rgba)?)?;
        outln!("Saved {}", out.display());
    }
    Ok(())
}

fn print_tree(nodes: &[openrage_plugins::TreeNode]) {
    let mut depth = vec![0usize; nodes.len()];
    outln!();
    for (i, n) in nodes.iter().enumerate() {
        depth[i] = n.parent.map_or(0, |p| depth[p as usize] + 1);
        let value = if n.value.is_empty() { String::new() } else { format!(": {}", n.value) };
        outln!("{}{}{value}", "  ".repeat(depth[i]), n.label);
    }
}

fn print_model(model: &openrage_plugins::Model, vpath: &str, glb: Option<&Path>) -> Result<()> {
    let vertices: usize = model.meshes.iter().map(|m| m.positions.len() / 3).sum();
    let triangles: usize = model.meshes.iter().map(|m| m.indices.len() / 3).sum();
    outln!(
        "Model: {} meshes, {vertices} vertices, {triangles} triangles, {} materials",
        model.meshes.len(),
        model.materials.len()
    );
    if let Some(out) = glb {
        let stem = vpath.rsplit('/').next().unwrap_or(vpath).split('.').next().unwrap_or("model");
        let loaded = openrage_app::plugins::from_plugin_model(model, stem);
        let data = openrage_app::models::to_glb(&loaded, stem, &loaded.default_look())
            .context("the model has nothing to export")?;
        fs::write(out, data)?;
        outln!("Saved {}", out.display());
    }
    Ok(())
}

/// The values of a tool's fields: their starting values, changed by
/// `field=value` arguments (checked against the field's kind).
fn tool_values(tool: &openrage_plugins::manifest::Tool, set: &[String]) -> Result<Vec<(String, String)>> {
    use openrage_plugins::manifest::FieldKind;
    let mut values: Vec<(String, String)> = tool.fields.iter().map(|f| (f.id.clone(), f.initial())).collect();
    for arg in set {
        let (id, value) = arg.split_once('=').with_context(|| format!("--set {arg}: write it as FIELD=VALUE"))?;
        let Some(field) = tool.fields.iter().find(|f| f.id == id) else {
            let ids: Vec<_> = tool.fields.iter().map(|f| f.id.as_str()).collect();
            bail!("{} has no field {id:?} (it has: {})", tool.title, ids.join(", "));
        };
        match field.kind {
            FieldKind::Choice if !field.choices.iter().any(|c| c == value) => {
                bail!("{id}: {value:?} is not one of {}", field.choices.join(", "))
            }
            FieldKind::Check if !matches!(value, "true" | "false") => bail!("{id}: use true or false"),
            _ => {}
        }
        if let Some(slot) = values.iter_mut().find(|(k, _)| k == id) {
            slot.1 = value.to_owned();
        }
    }
    Ok(values)
}
