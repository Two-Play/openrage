//! `openrage asi`: GTA V's ASI mods, their loader and the Proton setting.

use openrage_app::asi;

use crate::*;

pub(crate) fn cmd_asi(game_dir: &Path, cmd: Option<AsiCommand>) -> Result<()> {
    match cmd {
        None => print_status(game_dir),
        Some(AsiCommand::Enable { name }) => {
            asi::set_enabled(game_dir, &name, true)?;
            outln!("Enabled {name}.");
            Ok(())
        }
        Some(AsiCommand::Disable { name }) => {
            asi::set_enabled(game_dir, &name, false)?;
            outln!("Disabled {name} (put aside, not deleted).");
            Ok(())
        }
        Some(AsiCommand::Install { file }) => {
            let path = asi::install(game_dir, &file)?;
            outln!("Installed {} (undo with `openrage undo`).", path.display());
            Ok(())
        }
        Some(AsiCommand::Proton { state }) => {
            let status = asi::status(game_dir)?;
            let proton = status.proton.as_ref().context("this game does not run through Proton here")?;
            let loader = status.loader().context("no ASI loader for this edition of the game in its folder")?;
            let on = state == "on";
            asi::set_proton_override(proton, &loader.dll(), on)?;
            outln!(
                "{} {} in {} ({}).",
                if on { "Proton now loads" } else { "Proton uses its own" },
                loader.file,
                proton.prefix.display(),
                if on { "native, then builtin" } else { "override removed" }
            );
            Ok(())
        }
    }
}

fn print_status(game_dir: &Path) -> Result<()> {
    let s = asi::status(game_dir)?;
    let proton = s.proton.as_ref().map(|p| {
        let dll = s.loader().map(|l| l.dll());
        (p, dll.as_deref().and_then(|d| p.override_of(d)))
    });
    if json() {
        let value = serde_json::json!({
            "edition": if s.enhanced { "enhanced" } else { "legacy" },
            "loaders": s.loaders.iter().map(|l| serde_json::json!({
                "file": l.file, "description": l.description, "for_enhanced": l.for_enhanced,
            })).collect::<Vec<_>>(),
            "loader": s.loader().map(|l| &l.file),
            "loader_runs": s.loader_runs(),
            "script_hook": s.script_hook,
            "mods": s.mods.iter().map(|m| serde_json::json!({
                "name": m.name, "enabled": m.enabled, "size": m.size,
            })).collect::<Vec<_>>(),
            "proton": proton.as_ref().map(|(p, value)| serde_json::json!({
                "app_id": p.app_id, "prefix": p.prefix, "override": value,
            })),
            "battleye": s.battleye,
            "problems": s.problems(),
        });
        outln!("{value:#}");
        return Ok(());
    }
    outln!("Game:        GTA V {}", if s.enhanced { "Enhanced" } else { "Legacy" });
    if s.loaders.is_empty() {
        outln!("Loader:      none");
    }
    for l in &s.loaders {
        let fits = s.loader() == Some(l);
        outln!("Loader:      {} — {}{}", l.file, l.description, if fits { "" } else { " (not for this edition)" });
    }
    outln!("Script Hook: {}", if s.script_hook { "ScriptHookV.dll" } else { "missing" });
    if let Some((p, value)) = &proton {
        outln!("Proton:      app {}, override: {}", p.app_id, value.as_deref().unwrap_or("none"));
    }
    if s.mods.is_empty() {
        outln!("ASI mods:    none");
    }
    for m in &s.mods {
        outln!("  {} {:<40} {:>8}", if m.enabled { "on " } else { "off" }, m.name, human(m.size));
    }
    for p in s.problems() {
        outln!("\nProblem: {p}");
    }
    if s.battleye {
        outln!(
            "\nASI mods are for story mode only: GTA Online bans modified games, and the anti-cheat BattlEye that GTA V Enhanced starts with may stop ASI loaders."
        );
    }
    Ok(())
}
