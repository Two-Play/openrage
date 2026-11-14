//! `openrage` — command-line access to RAGE game archives.
#![forbid(unsafe_code)]

/// `println!` that exits quietly when stdout is closed (e.g. piped into `head`).
macro_rules! outln {
    ($($t:tt)*) => {{
        use std::io::Write as _;
        if writeln!(std::io::stdout().lock(), $($t)*).is_err() {
            std::process::exit(0);
        }
    }};
}

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use openrage_app::config::Config;
use openrage_app::detect::{collect_archives, collect_rpfs};
use openrage_app::extract::{extract_entry, human_size as human};
use openrage_app::install::{self, Op};
use openrage_app::setup::{self, KeySource};
use rage_crypto::GtaVKeys;
use rage_rpf::vfs::{self, Node};
use rage_rpf::{EntryId, Rpf7};
use rayon::prelude::*;

mod cli;
mod cmd;

use cli::*;
use cmd::*;

/// `--json`: commands that support it print JSON instead of text.
static JSON: AtomicBool = AtomicBool::new(false);

pub(crate) fn json() -> bool {
    JSON.load(std::sync::atomic::Ordering::Relaxed)
}

/// The rows of a listing: lines of text, or with `--json` one JSON array that
/// is written row by row (a recursive listing of the game has millions).
pub(crate) struct Rows {
    json: bool,
    first: bool,
}

impl Rows {
    pub(crate) fn start() -> Self {
        let json = json();
        if json {
            outln!("[");
        }
        Self { json, first: true }
    }

    /// One row: `text` for the text output, `value` for JSON.
    pub(crate) fn row(&mut self, text: impl FnOnce() -> String, value: serde_json::Value) {
        if self.json {
            let sep = if self.first { "" } else { "," };
            outln!("{sep}{value}");
        } else {
            outln!("{}", text());
        }
        self.first = false;
    }

    pub(crate) fn finish(self) {
        if self.json {
            outln!("]");
        }
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    JSON.store(cli.json, std::sync::atomic::Ordering::Relaxed);
    let mut cfg = Config::load();
    let game_dir = cli.game.clone().or_else(|| cfg.game_dir.clone());

    if let Command::Keys { exe, magic } = &cli.command {
        let game_dir = game_dir.context("no game directory given; pass --game <DIR>")?;
        let game_dir = openrage_app::gta4::normalize_dir(&game_dir);
        if openrage_app::gta4::is_gta4_dir(&game_dir) {
            cmd_gta4_keys(&game_dir)?;
            cfg.remember_game(&game_dir);
            cfg.game_dir = Some(game_dir);
            cfg.save()?;
            return Ok(());
        }
        let magic = magic.clone().or_else(|| cfg.magic_path.clone());
        cmd_keys(&game_dir, exe.as_deref(), magic.as_deref())?;
        cfg.game_dir = Some(game_dir);
        cfg.magic_path = magic;
        cfg.save()?;
        return Ok(());
    }

    if let Command::Rpf8 { path, list, textures, models, text, meta, audio, anims } = &cli.command {
        return cmd_rpf8(
            path,
            *list,
            Checks { textures: *textures, models: *models, texts: *text, meta: *meta, audio: *audio, anims: *anims },
        );
    }
    // Managing plugins needs no game; previews and actions use it when there is one.
    if let Command::Plugin(cmd) = &cli.command
        && (game_dir.is_none()
            || matches!(
                cmd,
                PluginCommand::List
                    | PluginCommand::Info { .. }
                    | PluginCommand::Enable { .. }
                    | PluginCommand::Disable { .. }
                    | PluginCommand::Install { .. }
                    | PluginCommand::Uninstall { .. }
                    | PluginCommand::Pack { .. }
            ))
    {
        return cmd_plugin(&mut cfg, None, cmd);
    }
    if let Command::Licenses { text, all } = &cli.command {
        return cmd_licenses(text.as_deref(), *all);
    }
    if let Command::Rdr2Names { game, import } = &cli.command {
        return cmd_rdr2_names(game, import.as_deref());
    }
    if let Command::Rdr2Keys { game, dump, file } = &cli.command {
        return cmd_rdr2_keys(game, dump.as_deref(), file.as_deref());
    }

    let game_dir = game_dir.context("no game directory configured; run `openrage --game <DIR> keys` first")?;
    let game_dir = openrage_app::gta4::normalize_dir(&game_dir);
    let gta4 = openrage_app::gta4::is_gta4_dir(&game_dir);
    if gta4 && let Err(e) = openrage_app::gta4::register(&game_dir) {
        eprintln!("warning: GTA IV archive key: {e:#}");
    }
    if openrage_app::detect::is_rdr2_dir(&game_dir) {
        openrage_app::rdr2::register(&game_dir)?;
    }
    let keys = openrage_app::load_keys().unwrap_or_else(|e| {
        eprintln!("warning: {e}");
        None
    });

    match cli.command {
        Command::Keys { .. }
        | Command::Rpf8 { .. }
        | Command::Rdr2Keys { .. }
        | Command::Rdr2Names { .. }
        | Command::Licenses { .. } => {
            unreachable!()
        }
        Command::Ls { path, recursive } => cmd_ls(&game_dir, &path, recursive, keys),
        Command::Info { path } => cmd_info(&game_dir, &path, keys),
        Command::Extract { path, out, expand_archives } => cmd_extract(&game_dir, &path, &out, expand_archives, keys),
        Command::Find { pattern, r#in } => cmd_find(&game_dir, &r#in, &pattern, keys),
        Command::Verify { path, models: true, .. } if gta4 => cmd_verify_gta4_models(&game_dir, &path),
        Command::Verify { path, models: true, rebuild: true, .. } => cmd_verify_model_writer(&game_dir, &path, keys),
        Command::Verify { path, models: true, .. } => cmd_verify_models(&game_dir, &path, keys),
        Command::Verify { path, audio: true, .. } => cmd_verify_audio(&game_dir, &path, keys),
        Command::Verify { path, media: true, .. } => cmd_verify_media(&game_dir, &path, keys),
        Command::Verify { path, scripts: true, .. } if gta4 => cmd_verify_gta4_scripts(&game_dir, &path),
        Command::Verify { path, scripts: true, .. } => cmd_verify_scripts(&game_dir, &path, keys),
        Command::Verify { path, paths: true, .. } => cmd_verify_paths(&game_dir, &path, keys),
        Command::Verify { path, collisions: true, .. } if gta4 => cmd_verify_gta4_collisions(&game_dir, &path),
        Command::Verify { path, collisions: true, .. } => cmd_verify_collisions(&game_dir, &path, keys),
        Command::Verify { path, particles: true, .. } => cmd_verify_particles(&game_dir, &path, keys),
        Command::Verify { path, text: true, .. } if gta4 => cmd_verify_gta4_text(&game_dir, &path),
        Command::Verify { path, text: true, .. } => cmd_verify_text(&game_dir, &path, keys),
        Command::Verify { path, meta: true, .. } => cmd_verify_meta(&game_dir, &path, keys),
        Command::Verify { path, cutscenes: true, .. } if gta4 => cmd_verify_gta4_cutscenes(&game_dir, &path),
        Command::Verify { cutscenes: true, .. } => bail!("--cutscenes checks GTA IV cutscenes; this is not GTA IV"),
        Command::Cutscene { path, models } => cmd_gta4_cutscene(&game_dir, &path, models),
        Command::Rdr2Ped { name, outfit, check, load } => cmd_rdr2_ped(&game_dir, name.as_deref(), outfit, check, load),
        Command::Verify { path, animations: true, .. } if gta4 => cmd_verify_gta4_animations(&game_dir, &path),
        Command::Verify { path, animations: true, .. } => cmd_verify_animations(&game_dir, &path, keys),
        Command::Verify { path, textures: false, rebuild: true, .. } if gta4 => {
            cmd_verify_gta4_tables(&game_dir, &path)
        }
        Command::Verify { textures: false, rebuild: true, .. } => {
            bail!("--rebuild needs --textures or --models (or a GTA IV game)")
        }
        Command::Verify { path, textures: false, .. } if gta4 => cmd_verify_gta4(&game_dir, &path),
        Command::Verify { path, textures: false, .. } => cmd_verify(&game_dir, &path, keys),
        Command::Verify { path, textures: true, rebuild, .. } if gta4 => {
            cmd_verify_gta4_textures(&game_dir, &path, rebuild)
        }
        Command::Verify { path, textures: true, rebuild, .. } => cmd_verify_textures(&game_dir, &path, rebuild, keys),
        Command::Textures { path, sets, removes, .. } if !sets.is_empty() || !removes.is_empty() => {
            cmd_edit_textures(&game_dir, &path, &sets, &removes, keys)
        }
        Command::Textures { path, export, png, .. } => cmd_textures(&game_dir, &path, export.as_deref(), png, keys),
        Command::Index => cmd_index(&game_dir, keys),
        Command::Model { path, export, render, yaw, anim, outfit } => {
            let view = ModelOutput { export: export.as_deref(), render: render.as_deref(), yaw, anim: anim.as_deref() };
            cmd_model(&game_dir, &path, outfit, view, keys)
        }
        Command::Clips { path, clip } => cmd_clips(&game_dir, &path, clip.as_deref(), keys),
        Command::ReplaceModel { path, obj, out, mods, shader, lod, no_textures } => {
            let options = ReplaceOptions { out: out.as_deref(), mods, shader, lods: &lod, textures: !no_textures };
            cmd_replace_model(&game_dir, &path, &obj, options, keys)
        }
        Command::World { x, y, z, radius } => {
            let started = Instant::now();
            let world = openrage_app::world::load(&game_dir, keys.clone())?;
            outln!(
                "{} map files, {} archetypes ({:.1}s)",
                world.maps.len(),
                world.archetypes.len(),
                started.elapsed().as_secs_f32()
            );
            let near = openrage_app::world::objects_near(&world, &game_dir, keys, [x, y, z], radius);
            let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
            for p in &near {
                *counts.entry(p.archetype.as_str()).or_default() += 1;
            }
            outln!("{} objects within {radius} m ({:.1}s):", near.len(), started.elapsed().as_secs_f32());
            for (a, n) in counts {
                outln!("  {n:>4} × {a}");
            }
            Ok(())
        }
        Command::Map { path, render, yaw, export, check } if gta4 => {
            cmd_map_gta4(&game_dir, path.as_deref(), render.as_deref(), yaw, export.as_deref(), check)
        }
        Command::Map { path, render, yaw, export, check } => {
            cmd_map(&game_dir, path.as_deref(), render.as_deref(), yaw, export.as_deref(), check, keys)
        }
        Command::Natives { file } => {
            let n = openrage_app::natives::import(&file)?;
            outln!("Imported {n} native names; script disassemblies now show them.");
            Ok(())
        }
        Command::Media { path, swf } => cmd_media(&game_dir, &path, swf.as_deref(), keys),
        Command::Script { path, out, strings } => cmd_script(&game_dir, &path, out.as_deref(), strings, keys),
        Command::Particles { path } => cmd_particles(&game_dir, &path, keys),
        Command::Text { path, out, import: None } => cmd_text(&game_dir, &path, out.as_deref(), keys),
        Command::Text { path, import: Some(file), .. } => cmd_import_wpl(&game_dir, &path, &file, keys),
        Command::Xml { path, out, tree } => cmd_xml(&game_dir, &path, out.as_deref(), tree, keys),
        Command::Audio { path, export } => cmd_audio(&game_dir, &path, export.as_deref(), keys),
        Command::Put { file, target } => cmd_put(&game_dir, &file, &target, keys),
        Command::Rm { path } => cmd_edit(&game_dir, &path, Op::Remove, &format!("Removed {path}"), keys),
        Command::Mv { from, to } => cmd_mv(&game_dir, &from, &to, keys),
        Command::Mkdir { path } => cmd_edit(&game_dir, &path, Op::CreateDir, &format!("Created folder {path}"), keys),
        Command::History => cmd_history(&game_dir),
        Command::Oiv(cmd) => cmd_oiv(&game_dir, cmd, keys),
        Command::Plugin(cmd) => {
            let game: Arc<dyn openrage_plugins::GameFiles> =
                Arc::new(openrage_app::plugins::GameDir::new(&game_dir, keys.clone()));
            cmd_plugin(&mut cfg, Some((&game_dir, keys, game)), &cmd)
        }
        Command::Undo => cmd_undo(&game_dir),
        Command::WriteTarget { target } => cmd_write_target(&game_dir, target.as_deref()),
        Command::Originals { restore, restore_all, yes } => {
            cmd_originals(&game_dir, restore.as_deref(), restore_all, yes)
        }
        Command::Mods { changes, deep } => cmd_mods(&game_dir, changes, deep, keys),
        Command::Repack { archive, out, sets, removes } => cmd_repack(&game_dir, &archive, &out, &sets, &removes, keys),
        Command::Search { query, limit, lang } => {
            let lang = match lang.as_deref() {
                Some("all") => None,
                Some(l) => Some(l.to_ascii_lowercase()),
                None => openrage_app::game_language_from_locale().map(str::to_owned),
            };
            cmd_search(&game_dir, &query, limit, lang.as_deref(), keys)
        }
    }
}
