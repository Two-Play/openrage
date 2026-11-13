//! `openrage oiv`: OpenIV packages.

use crate::*;

pub(crate) fn print_report(report: &openrage_app::oiv::install::Report) {
    for s in &report.steps {
        let detail = if s.detail.is_empty() { String::new() } else { format!("  ({})", s.detail) };
        outln!("  {:<8} {}{detail}", s.action, s.target);
    }
    for w in &report.warnings {
        outln!("  warning: {w}");
    }
    for b in &report.blockers {
        outln!("  BLOCKED: {b}");
    }
}

pub(crate) fn cmd_oiv(game_dir: &Path, cmd: OivCommand, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    use openrage_app::oiv::{Package, install};
    match cmd {
        OivCommand::Info { file } => {
            let pkg = Package::open(&file)?;
            let m = &pkg.metadata;
            outln!("{} {} by {} (format {})", m.name, m.version, m.author, pkg.format_version);
            if !m.description.is_empty() {
                outln!("{}", m.description);
            }
            outln!("\nChanges:");
            print_report(&install::preview(game_dir, keys, &pkg)?);
        }
        OivCommand::Install { file, yes } => {
            let pkg = Package::open(&file)?;
            let report = install::preview(game_dir, keys.clone(), &pkg)?;
            outln!("{} {}:", pkg.metadata.name, pkg.metadata.version);
            print_report(&report);
            if !report.blockers.is_empty() {
                bail!("the package cannot be installed");
            }
            confirm("Install into the mods folder?", yes)?;
            let started = Instant::now();
            install::install(game_dir, keys, &pkg)?;
            outln!(
                "Installed \"{}\" in {:.1}s. Remove it with `openrage oiv uninstall \"{}\"`.",
                install::group_name(&pkg),
                started.elapsed().as_secs_f32(),
                install::group_name(&pkg)
            );
        }
        OivCommand::List => {
            let groups = rage_mods::mods_dir(game_dir)
                .map(|m| openrage_app::journal::Journal::new(&m).groups())
                .transpose()?
                .unwrap_or_default();
            if groups.is_empty() {
                outln!("No packages installed with OpenRAGE.");
            }
            for (name, n) in groups {
                outln!("{name}  ({n} recorded changes)");
            }
        }
        OivCommand::Uninstall { name, yes } => {
            confirm(&format!("Undo everything \"{name}\" installed?"), yes)?;
            let undone = install::uninstall(game_dir, &name)?;
            outln!("Uninstalled \"{name}\" ({} changes undone).", undone.len());
        }
    }
    Ok(())
}
