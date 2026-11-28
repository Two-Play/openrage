//! `openrage plugin run`: run a plugin's action on files.

use super::{PluginEnv, ensure_enabled, print_log};
use crate::*;
use openrage_plugins::Plugin;

/// The arguments of `plugin run`.
pub(super) struct RunArgs<'a> {
    pub(super) id: &'a str,
    pub(super) action: &'a str,
    pub(super) paths: &'a [String],
    pub(super) file: bool,
    pub(super) pick: &'a [PathBuf],
    pub(super) out: Option<&'a Path>,
    pub(super) yes: bool,
}

/// A plugin ready to run an action.
struct RunJob<'a> {
    env: &'a PluginEnv<'a>,
    plugin: &'a Plugin,
    host: &'a openrage_plugins::Host,
    component: &'a openrage_plugins::Component,
    args: &'a RunArgs<'a>,
}

pub(super) fn plugin_run(env: &PluginEnv, args: &RunArgs) -> Result<()> {
    let p = env.find(args.id)?;
    ensure_enabled(p, args.id)?;
    let Some(spec) = p.manifest.action(args.action) else {
        let ids: Vec<_> = p.manifest.actions.iter().map(|a| a.id.as_str()).collect();
        bail!("{} has no action {:?} (it has: {})", p.manifest.name, args.action, ids.join(", "));
    };
    let result = run_action(env, p, args)?;
    report_run(env, p, spec, &result, args)
}

/// Loads the files to run on and the plugin, and runs the action.
fn run_action(env: &PluginEnv, p: &Plugin, args: &RunArgs) -> Result<openrage_app::plugins::RunResult> {
    use openrage_plugins::{Host, Limits};
    let picked = picked_files(args.pick)?;
    let host = Host::new(Limits::default())?;
    let component = host.compile(p)?;
    let job = RunJob { env, plugin: p, host: &host, component: &component, args };
    if args.file { job.run_from_disk(&picked) } else { job.run_in_game(&picked) }
}

fn report_run(
    env: &PluginEnv,
    p: &Plugin,
    spec: &openrage_plugins::Action,
    result: &openrage_app::plugins::RunResult,
    args: &RunArgs,
) -> Result<()> {
    if result.files > 1 {
        eprintln!();
    }
    print_log(&result.log);
    for (path, error) in &result.failures {
        eprintln!("failed: {path}: {error}");
    }
    save_outputs(&result.outputs, args.out)?;
    apply_run_writes(env, p, spec, result, args.yes)?;
    if result.outputs.is_empty() && result.writes.is_empty() {
        outln!("The plugin produced no files and changed nothing.");
    }
    if !result.failures.is_empty() {
        bail!("{} of {} files failed", result.failures.len(), result.files + result.failures.len());
    }
    Ok(())
}

fn picked_files(pick: &[PathBuf]) -> Result<Vec<openrage_app::plugins::Picked>> {
    pick.iter()
        .map(|f| -> Result<openrage_app::plugins::Picked> {
            let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            Ok(openrage_app::plugins::Picked {
                name,
                data: fs::read(f).with_context(|| format!("reading {}", f.display()))?,
            })
        })
        .collect()
}

impl RunJob<'_> {
    /// Files from disk: one call, no folders.
    fn run_from_disk(&self, picked: &[openrage_app::plugins::Picked]) -> Result<openrage_app::plugins::RunResult> {
        let paths = self.args.paths;
        let mut inputs = Vec::new();
        for path in paths {
            let (vpath, data) = self.env.read(path, true)?;
            inputs.push(openrage_plugins::Input { vpath, data, picked: false });
        }
        inputs.extend(picked.iter().map(|f| openrage_plugins::Input {
            vpath: f.name.clone(),
            data: f.data.clone(),
            picked: true,
        }));
        let outcome =
            self.host.run_action(self.plugin, self.component, Arc::clone(&self.env.files), self.args.action, &inputs);
        let outputs = outcome.value;
        Ok(openrage_app::plugins::RunResult {
            outputs: match outputs {
                Ok(o) => o,
                Err(e) => {
                    print_log(&outcome.log);
                    return Err(e.context(format!("{} failed", self.plugin.manifest.name)));
                }
            },
            writes: outcome.writes,
            log: outcome.log,
            files: paths.len(),
            ..Default::default()
        })
    }

    fn run_in_game(&self, picked: &[openrage_app::plugins::Picked]) -> Result<openrage_app::plugins::RunResult> {
        let mut last = Instant::now();
        openrage_app::plugins::run_action(
            self.host,
            self.plugin,
            self.component,
            Arc::clone(&self.env.files),
            self.args.action,
            self.args.paths,
            picked,
            &mut |done, total, path| {
                if last.elapsed().as_millis() > 200 || done == total {
                    eprint!("\r[{done}/{total}] {:<60.60}", path);
                    last = Instant::now();
                }
                true
            },
        )
        .with_context(|| format!("{} failed", self.plugin.manifest.name))
    }
}

fn save_outputs(outputs: &[openrage_plugins::OutputFile], out: Option<&Path>) -> Result<()> {
    if outputs.is_empty() {
        return Ok(());
    }
    let out = out.context("the action produced files; pass --out <DIR> to save them")?;
    for o in outputs {
        // The host has checked that names stay inside `out`.
        let target = out.join(rage_rpf::vfs::safe_relative_path(&o.name).context("bad output name")?);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&target, &o.data).with_context(|| format!("writing {}", target.display()))?;
        outln!("Saved {} ({})", target.display(), human(o.data.len() as u64));
    }
    Ok(())
}

fn apply_run_writes(
    env: &PluginEnv,
    p: &Plugin,
    spec: &openrage_plugins::Action,
    result: &openrage_app::plugins::RunResult,
    yes: bool,
) -> Result<()> {
    if result.writes.is_empty() {
        return Ok(());
    }
    let Some((game_dir, keys, _)) = env.game else { bail!("the action changes game files, but no game is set up") };
    outln!("{} wants to change {} file(s) (into the mods folder):", p.manifest.name, result.writes.len());
    for w in &result.writes {
        outln!("  {} ({})", w.vpath, human(w.data.len() as u64));
    }
    confirm("Apply?", yes)?;
    let description = format!("{}: {}", p.manifest.name, spec.title.trim_end_matches('…'));
    let group = openrage_app::plugins::group_name(&description);
    for line in openrage_app::plugins::apply_writes(game_dir, keys.clone(), &result.writes, &description, &group)? {
        outln!("Changed {line}");
    }
    outln!("Undo with `openrage undo` or in History (\"{group}\").");
    Ok(())
}
