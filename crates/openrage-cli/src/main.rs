//! `openrage` — command-line access to RAGE game archives.

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
use openrage_app::extract::{extract_entry, human_size as human};
use openrage_app::install::{self, Op};
use openrage_app::setup::{self, KeySource};
use rage_crypto::GtaVKeys;
use rage_rpf::vfs::{self, Node};
use rage_rpf::{EntryId, Rpf7};
use rayon::prelude::*;

#[derive(Parser)]
#[command(name = "openrage", version, about = "Open-source explorer for RAGE engine game files")]
struct Cli {
    /// Game installation directory (remembered after `openrage keys`).
    #[arg(long, global = true, env = "OPENRAGE_GAME_DIR")]
    game: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Derive the archive keys from your own game installation and cache them locally.
    Keys {
        /// Game executable to scan (default: GTA5_Enhanced.exe or GTA5.exe in the game directory).
        #[arg(long)]
        exe: Option<PathBuf>,
        /// CodeWalker key blob (magic.dat); required for GTA V Enhanced, whose
        /// executable no longer contains the NG keys in plain form.
        #[arg(long)]
        magic: Option<PathBuf>,
    },
    /// List a directory, an archive, or a directory inside an archive.
    Ls {
        /// Virtual path relative to the game directory, e.g. `x64a.rpf/data`.
        #[arg(default_value = "")]
        path: String,
        /// Recurse into subdirectories and nested archives.
        #[arg(short, long)]
        recursive: bool,
    },
    /// Show details about an archive or an entry.
    Info { path: String },
    /// Extract a file or a whole directory/archive.
    Extract {
        path: String,
        /// Output directory.
        #[arg(short, long, default_value = ".")]
        out: PathBuf,
        /// Unpack nested archives into directories instead of writing them as .rpf files.
        #[arg(long)]
        expand_archives: bool,
    },
    /// Read and decode every entry to check that all archives can be processed.
    Verify {
        /// Limit the check to this virtual path.
        #[arg(default_value = "")]
        path: String,
        /// Instead of archives, check every texture dictionary: parse it and decode all textures.
        #[arg(long)]
        textures: bool,
        /// Instead of archives, parse every model (.ydr, .yft, .ydd).
        #[arg(long, conflicts_with = "textures")]
        models: bool,
        /// Instead of archives, decode every sound of every audio container (.awc).
        #[arg(long, conflicts_with_all = ["textures", "models"])]
        audio: bool,
        /// Instead of archives, parse every clip dictionary (.ycd) and evaluate every frame.
        #[arg(long, conflicts_with_all = ["textures", "models", "audio"])]
        animations: bool,
        /// Instead of archives, convert every metadata file (.ymap, .ytyp, .ymt, .ymf, .cut, .pso) to XML.
        #[arg(long, conflicts_with_all = ["textures", "models", "audio", "animations"])]
        meta: bool,
        /// Instead of archives, convert every text table (.gxt2, .nametable) to text and back,
        /// checking that the result is identical.
        #[arg(long, conflicts_with_all = ["textures", "models", "audio", "animations", "meta"])]
        text: bool,
        /// Instead of archives, parse every particle dictionary (.ypt): effects, textures and models.
        #[arg(long, conflicts_with_all = ["textures", "models", "audio", "animations", "meta", "text"])]
        particles: bool,
        /// Instead of archives, read every collision file (.ybn) and build its meshes.
        #[arg(long, conflicts_with_all = ["textures", "models", "audio", "animations", "meta", "text", "particles"])]
        collisions: bool,
        /// Instead of archives, read every path file (.ynd nodes, .yvr and .ywr recordings).
        #[arg(long, conflicts_with_all = ["textures", "models", "audio", "animations", "meta", "text", "particles", "collisions"])]
        paths: bool,
        /// Instead of archives, disassemble every script (.ysc) and check every jump.
        #[arg(long, conflicts_with_all = ["textures", "models", "audio", "animations", "meta", "text", "particles", "collisions", "paths"])]
        scripts: bool,
        /// Instead of archives, read the header of every Scaleform movie (.gfx) and Bink video (.bik).
        #[arg(long, conflicts_with_all = ["textures", "models", "audio", "animations", "meta", "text", "particles", "collisions", "paths", "scripts"])]
        media: bool,
        /// With --textures: also rebuild every dictionary and check that it reads back identically.
        #[arg(long, requires = "textures")]
        rebuild: bool,
    },
    /// Convert a metadata file (.ymap, .ytyp, .ymt, .ymf, .cut, .pso) to XML.
    Xml {
        /// Virtual path of the file.
        path: String,
        /// Write the XML to this file instead of printing it.
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Print the readable structure (as in the GUI) instead of the XML.
        #[arg(long, conflicts_with = "out")]
        tree: bool,
    },
    /// Put a map file (.ymap) together: its objects with their models and textures
    /// (builds the world cache on first use).
    Map {
        /// Virtual path of the .ymap (GTA IV: .wpl; none with --check).
        path: Option<String>,
        /// Render a picture of the whole map file (PNG).
        #[arg(long)]
        render: Option<PathBuf>,
        /// Camera angle around the map for --render, in degrees.
        #[arg(long, default_value_t = -130.0, allow_hyphen_values = true)]
        yaw: f32,
        /// Write the map file's objects as one glTF scene (.glb), centred on the origin.
        #[arg(long)]
        export: Option<PathBuf>,
        /// Check every map file of the game: how many of its objects have a model.
        #[arg(long, conflicts_with_all = ["path", "render", "export"])]
        check: bool,
    },
    /// List the map objects near a point of the world (builds the world cache on first use).
    World {
        x: f32,
        y: f32,
        z: f32,
        /// Radius in metres.
        #[arg(long, default_value_t = 30.0)]
        radius: f32,
    },
    /// Import native names for script disassembly from a NativeDB-style natives.json.
    Natives { file: PathBuf },
    /// Show OpenRAGE's licence, its credits and the licences of all third-party crates it contains.
    Licenses {
        /// Show the full licence text of this crate (or of `openrage` for the GPL, `notice` for NOTICE.md).
        #[arg(long, value_name = "CRATE")]
        text: Option<String>,
        /// Print every licence text (as in THIRD_PARTY_LICENSES.md).
        #[arg(long, conflicts_with = "text")]
        all: bool,
    },
    /// Set up the Red Dead Redemption 2 archive keys from your own game and store them locally.
    ///
    /// The cipher tables come from RDR2.exe; the keys from a memory dump of your running
    /// game or a key file, recognised by decrypting your archives. OpenRAGE ships no keys.
    Rdr2Keys {
        /// The RDR2 game folder (with RDR2.exe and the archives).
        game: PathBuf,
        /// A memory dump of the running game (e.g. from Task Manager: "Create dump file").
        #[arg(long)]
        dump: Option<PathBuf>,
        /// A key file of length-prefixed blocks, such as Swage's secrets.bin.
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Find names for Red Dead Redemption 2's archive entries in the game's own files.
    ///
    /// Entries only carry a hash of their name; this searches every file for
    /// strings that match (once, a few minutes) and caches the hits. A list of
    /// names (one per line) can be added with --import.
    Rdr2Names {
        /// The RDR2 game folder.
        game: PathBuf,
        /// Add names from a text file (one path or name per line) instead of searching.
        #[arg(long)]
        import: Option<PathBuf>,
    },
    /// Show a Red Dead Redemption 2 ped (people and animals): its outfits and the files of their parts.
    Rdr2Ped {
        /// The ped, e.g. `cs_johnmarston` (none: list all peds).
        name: Option<String>,
        /// Show only this outfit (counted from 0).
        #[arg(long)]
        outfit: Option<usize>,
        /// Check every outfit of every ped: all parts must be found in the game.
        #[arg(long, conflicts_with_all = ["name", "outfit"])]
        check: bool,
        /// With --check: also put every ped together (its first outfit) as the viewer does.
        #[arg(long, requires = "check")]
        load: bool,
    },
    /// Inspect Red Dead Redemption 2 archives (RPF8): a file or every .rpf in a folder.
    Rpf8 {
        path: PathBuf,
        /// List the entries of archives whose table is not encrypted.
        #[arg(long)]
        list: bool,
        /// Instead of only reading files, parse every texture dictionary and decode every texture.
        #[arg(long)]
        textures: bool,
        /// Instead of only reading files, parse every model (.ydr, .yft, .ydd).
        #[arg(long)]
        models: bool,
        /// Instead of only reading files, read every text table (.yldb) and count its strings.
        #[arg(long, conflicts_with_all = ["textures", "models"])]
        text: bool,
        /// Instead of only reading files, convert all metadata (.ymap, .ytyp, .ymt, .ymf) to XML.
        #[arg(long, conflicts_with_all = ["textures", "models", "text"])]
        meta: bool,
        /// Instead of only reading files, decode every sound of every audio container.
        #[arg(long, conflicts_with_all = ["textures", "models", "text", "meta"])]
        audio: bool,
        /// Instead of only reading files, parse every clip dictionary (.ycd) and evaluate every frame.
        #[arg(long, conflicts_with_all = ["textures", "models", "text", "meta", "audio"])]
        anims: bool,
    },
    /// Show facts about a Scaleform movie (.gfx) or a Bink video (.bik).
    Media {
        path: String,
        /// For a .gfx: write it as a Flash file (.swf) for Flash tools such as JPEXS or Ruffle.
        #[arg(long)]
        swf: Option<PathBuf>,
    },
    /// Disassemble a script (.ysc): functions, jumps, strings and native calls.
    Script {
        /// Virtual path of the .ysc.
        path: String,
        /// Write the disassembly to this file instead of printing it.
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Only list the script's strings.
        #[arg(long)]
        strings: bool,
    },
    /// Show what a particle dictionary (.ypt) contains: effects, emitters, textures, models.
    Particles {
        /// Virtual path of the .ypt.
        path: String,
    },
    /// Show a text table (.gxt2 as OpenIV .oxt text, .nametable one name per line; GTA IV .gxt) or a GTA IV placement file (.wpl).
    Text {
        /// Virtual path of the file.
        path: String,
        /// Write the text to this file instead of printing it.
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Show a model (.ydr, .yft, .ydd; GTA IV .wdr, .wft, .wdd) and optionally export it as glTF (.glb).
    Model {
        /// Virtual path of the model.
        path: String,
        /// Write the model (the first drawable of a .ydd) as .glb.
        #[arg(long)]
        export: Option<PathBuf>,
        /// Render a preview image (PNG).
        #[arg(long)]
        render: Option<PathBuf>,
        /// Camera angle around the model for --render, in degrees.
        #[arg(long, default_value_t = -130.0, allow_hyphen_values = true)]
        yaw: f32,
        /// Pose the model with a clip for --render: `<path of .ycd>:<clip name>[@seconds]`.
        #[arg(long)]
        anim: Option<String>,
        /// RDR2 ped outfit files (`…/metapeds.rpf/<ped>.ymt`): which outfit to put together (default 0).
        #[arg(long)]
        outfit: Option<usize>,
    },
    /// List the clips of a clip dictionary (.ycd).
    Clips {
        /// Virtual path of the .ycd file.
        path: String,
        /// Print the values of every track of this clip at a few frames.
        #[arg(long)]
        clip: Option<String>,
    },
    /// List the sounds of an audio container (.awc) and optionally export them as WAV.
    Audio {
        /// Virtual path of the .awc file.
        path: String,
        /// Export every sound into this folder as .wav.
        #[arg(long)]
        export: Option<PathBuf>,
    },
    /// List the textures of a texture dictionary (.ytd) and optionally export them.
    Textures {
        /// Virtual path of the .ytd file.
        path: String,
        /// Export all textures into this folder (DDS, like OpenIV).
        #[arg(long)]
        export: Option<PathBuf>,
        /// Export PNG images instead of DDS (top mip level only).
        #[arg(long)]
        png: bool,
        /// Replace or add a texture from a DDS file: `name=file.dds` (repeatable).
        /// The changed dictionary is written into the mods folder, like OpenIV.
        #[arg(long = "set", value_name = "NAME=DDS")]
        sets: Vec<String>,
        /// Remove a texture (repeatable); also written into the mods folder.
        #[arg(long = "remove", value_name = "NAME")]
        removes: Vec<String>,
    },
    /// Build or update the search index (only changed archives are re-read).
    Index,
    /// Search the index: names, hashes (`0xB779A091`, decimal) and file contents.
    Search {
        query: String,
        /// Maximum number of hits.
        #[arg(short = 'n', long, default_value_t = 50)]
        limit: usize,
        /// Only show game text in this language (`german`, `american`, …, or `all`).
        /// Defaults to the language of your system locale.
        #[arg(long)]
        lang: Option<String>,
    },
    /// Show the OpenIV `mods` folder: which files replace game files, and what changed inside them.
    Mods {
        /// List every added, removed and modified entry inside the modded archives.
        #[arg(long)]
        changes: bool,
        /// Compare the contents of every entry (slower; default compares metadata
        /// and the contents of entries up to 256 KiB).
        #[arg(long)]
        deep: bool,
    },
    /// Put a local file into an archive (written into the mods folder, like OpenIV).
    Put {
        /// Local file.
        file: PathBuf,
        /// Target virtual path inside an archive; a trailing `/` keeps the file name.
        target: String,
    },
    /// Remove a file or folder from an archive (in the mods folder).
    Rm { path: String },
    /// Rename or move a file or folder inside its archive (in the mods folder).
    Mv { from: String, to: String },
    /// Create a folder inside an archive (in the mods folder).
    Mkdir { path: String },
    /// OpenIV packages (.oiv): show, install into the mods folder, uninstall.
    #[command(subcommand)]
    Oiv(OivCommand),
    /// Show the changes made to the mods folder, newest first.
    History,
    /// Undo the newest change to the mods folder.
    Undo,
    /// Rewrite an archive with replaced/added/removed entries, OpenIV style (experimental).
    #[command(hide = true)]
    Repack {
        /// Archive to rewrite (virtual path).
        archive: String,
        /// Output file.
        #[arg(short, long)]
        out: PathBuf,
        /// `inner/path=local_file`; may reach into nested archives. RSC7 files become resources.
        #[arg(long = "set")]
        sets: Vec<String>,
        /// Inner path to remove.
        #[arg(long = "remove")]
        removes: Vec<String>,
    },
    /// Search all archives of the game for entries whose name contains PATTERN.
    Find {
        pattern: String,
        /// Limit the search to this virtual path.
        #[arg(long, default_value = "")]
        r#in: String,
    },
}

#[derive(Subcommand)]
enum OivCommand {
    /// Show a package and what installing it would change (nothing is written).
    Info { file: PathBuf },
    /// Install a package into the mods folder.
    Install {
        file: PathBuf,
        /// Install without asking.
        #[arg(short, long)]
        yes: bool,
    },
    /// List installed packages.
    List,
    /// Undo everything a package installed.
    Uninstall { name: String },
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
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
    if game_dir.join("RDR2.exe").is_file() {
        let names = rage_rpf::rpf8_names::NameIndex::default_path(&game_dir)
            .and_then(|p| rage_rpf::rpf8_names::NameIndex::load(&p).ok().flatten())
            .map(Arc::new);
        if let Some(n) = &names {
            openrage_app::rdr2::register_names(n);
        }
        rage_rpf::rpf8::set_defaults(rdr2_keys()?, names);
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
        Command::Verify { path, models: true, .. } => cmd_verify_models(&game_dir, &path, keys),
        Command::Verify { path, audio: true, .. } => cmd_verify_audio(&game_dir, &path, keys),
        Command::Verify { path, media: true, .. } => cmd_verify_media(&game_dir, &path, keys),
        Command::Verify { path, scripts: true, .. } => cmd_verify_scripts(&game_dir, &path, keys),
        Command::Verify { path, paths: true, .. } => cmd_verify_paths(&game_dir, &path, keys),
        Command::Verify { path, collisions: true, .. } => cmd_verify_collisions(&game_dir, &path, keys),
        Command::Verify { path, particles: true, .. } => cmd_verify_particles(&game_dir, &path, keys),
        Command::Verify { path, text: true, .. } if gta4 => cmd_verify_gta4_text(&game_dir, &path),
        Command::Verify { path, text: true, .. } => cmd_verify_text(&game_dir, &path, keys),
        Command::Verify { path, meta: true, .. } => cmd_verify_meta(&game_dir, &path, keys),
        Command::Rdr2Ped { name, outfit, check, load } => cmd_rdr2_ped(&game_dir, name.as_deref(), outfit, check, load),
        Command::Verify { path, animations: true, .. } => cmd_verify_animations(&game_dir, &path, keys),
        Command::Verify { path, textures: false, .. } if gta4 => cmd_verify_gta4(&game_dir, &path),
        Command::Verify { path, textures: false, .. } => cmd_verify(&game_dir, &path, keys),
        Command::Verify { path, textures: true, .. } if gta4 => cmd_verify_gta4_textures(&game_dir, &path),
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
        Command::Text { path, out } => cmd_text(&game_dir, &path, out.as_deref(), keys),
        Command::Xml { path, out, tree } => cmd_xml(&game_dir, &path, out.as_deref(), tree, keys),
        Command::Audio { path, export } => cmd_audio(&game_dir, &path, export.as_deref(), keys),
        Command::Put { file, target } => cmd_put(&game_dir, &file, &target, keys),
        Command::Rm { path } => cmd_edit(&game_dir, &path, Op::Remove, &format!("Removed {path}"), keys),
        Command::Mv { from, to } => cmd_mv(&game_dir, &from, &to, keys),
        Command::Mkdir { path } => cmd_edit(&game_dir, &path, Op::CreateDir, &format!("Created folder {path}"), keys),
        Command::History => cmd_history(&game_dir),
        Command::Oiv(cmd) => cmd_oiv(&game_dir, cmd, keys),
        Command::Undo => cmd_undo(&game_dir),
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

fn cmd_gta4_keys(game_dir: &Path) -> Result<()> {
    let (_, report) = openrage_app::gta4::derive_key(game_dir)?;
    outln!("GTA IV archive key found in {} ({:.1}s).", report.exe.display(), report.scan_seconds);
    if let Some(n) = report.verified_entries {
        outln!("Verified: vehicles.img decrypts ({n} entries).");
    }
    outln!("Key cached at {} (local only, never distribute this file).", report.cache_path.display());
    Ok(())
}

fn print_gta4_entry(archive: &rage_rpf::Gta4Archive, id: rage_rpf::Gta4EntryId, path: &str) {
    let e = archive.entry(id);
    let kind = openrage_app::gta4::kind_label(archive, id);
    let ty = e.resource_type().map(|t| format!("t{t}")).unwrap_or_default();
    let slash = if e.is_dir() { "/" } else { "" };
    outln!("{kind:<8} {:>8} {ty:>4}  {path}{slash}", human(e.size()));
}

fn cmd_licenses(text: Option<&str>, all: bool) -> Result<()> {
    use openrage_app::licenses;
    match text {
        Some(name) if name.eq_ignore_ascii_case("openrage") => outln!("{}", licenses::gpl_text()),
        Some(name) if name.eq_ignore_ascii_case("notice") => outln!("{}", licenses::notice()),
        Some(name) => {
            let found: Vec<_> = licenses::dependencies().iter().filter(|d| d.name.eq_ignore_ascii_case(name)).collect();
            if found.is_empty() {
                bail!("no crate named {name}; `openrage licenses` lists them");
            }
            for d in found {
                outln!(
                    "{} {} — {}
",
                    d.name,
                    d.version,
                    d.license
                );
                outln!(
                    "{}
",
                    licenses::dependency_text(d)
                );
            }
        }
        None if all => {
            for d in licenses::dependencies() {
                outln!(
                    "==== {} {} — {}
",
                    d.name,
                    d.version,
                    d.license
                );
                outln!(
                    "{}
",
                    licenses::dependency_text(d)
                );
            }
        }
        None => {
            outln!("OpenRAGE {} — {} ({})", licenses::VERSION, licenses::LICENSE, licenses::REPOSITORY);
            outln!(
                "{}
",
                licenses::NOTICE_SHORT
            );
            outln!("Credits (NOTICE.md):");
            for c in licenses::credits() {
                outln!("  {}", c.title);
            }
            let deps = licenses::dependencies();
            outln!(
                "
{} third-party crates:",
                deps.len()
            );
            for d in deps {
                outln!("  {:<36} {:<12} {}", d.name, d.version, d.license);
            }
            outln!(
                "
Full texts: `openrage licenses --text <crate>` (`openrage` for the GPL, `notice` for NOTICE.md) or `--all`."
            );
        }
    }
    Ok(())
}

fn cmd_keys(game_dir: &Path, exe: Option<&Path>, magic: Option<&Path>) -> Result<()> {
    let (_, report) = setup::derive_keys(game_dir, exe, magic, &mut |msg| outln!("{msg}"))?;
    outln!(
        "  scanned in {:.1}s; NG keys in executable: {}/101, NG tables: {}/272",
        report.scan_seconds,
        report.ng_keys_in_exe,
        report.ng_tables_in_exe
    );
    match report.source {
        KeySource::Executable => outln!("Complete key set found in the executable."),
        KeySource::KeyBlob => outln!("NG keys decoded from the key blob."),
    }
    if let Some(n) = report.verified_entries {
        outln!("Verified: common.rpf decrypts ({n} entries).");
    }
    outln!("Keys cached at {} (local only, never distribute this file).", report.cache_path.display());
    Ok(())
}

fn print_entry(rpf: &Rpf7, id: EntryId, path: &str) {
    let e = rpf.entry(id);
    let version = e.resource_version().map(|v| format!("v{v}")).unwrap_or_default();
    let suffix = if e.is_dir() || e.is_archive() { "/" } else { "" };
    outln!("{:<8} {:>8} {:>4}  {path}{suffix}", e.kind_label(), human(e.size()), version);
}

fn cmd_ls(game_dir: &Path, path: &str, recursive: bool, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    match vfs::resolve(game_dir, path, keys)? {
        Node::Dir(dir) => {
            let mut items: Vec<_> = fs::read_dir(&dir)?.flatten().collect();
            items.sort_by_key(|e| e.file_name());
            for item in items {
                let meta = item.metadata()?;
                let name = item.file_name().to_string_lossy().into_owned();
                let lower = name.to_ascii_lowercase();
                let kind = if meta.is_dir() {
                    "dir"
                } else if lower.ends_with(".rpf") {
                    "rpf"
                } else if lower.ends_with(".img") && rage_rpf::Gta4Archive::file_is_gta4(&item.path()) {
                    "img"
                } else {
                    "file"
                };
                let slash = if kind == "file" { "" } else { "/" };
                outln!("{kind:<8} {:>8}       {name}{slash}", human(meta.len()));
            }
        }
        Node::PlainFile(p) => outln!("file     {:>8}       {}", human(fs::metadata(&p)?.len()), p.display()),
        Node::ArchiveFile(rpf, id) => print_entry(&rpf, id, &rpf.path_of(id)),
        Node::Rpf8File(rpf, i) => print_rpf8_entry(&rpf, i, &rpf.entry_name(i)),
        Node::Rpf8Dir(rpf, dir) => {
            let (dirs, files) = rpf.list_dir(&dir);
            for d in dirs {
                outln!("{:<8} {:>8}       {d}/", "dir", "");
            }
            for i in files {
                let name = rpf.entry_name(i);
                print_rpf8_entry(&rpf, i, name.rsplit('/').next().unwrap_or(&name));
            }
        }
        Node::Gta4File(a, id) => print_gta4_entry(&a, id, &a.entry_path(id)),
        Node::Gta4Dir(a, dir) => {
            if recursive {
                vfs::walk_gta4(&a, dir, "", &mut |id, p| print_gta4_entry(&a, id, p));
            } else {
                for id in a.children(dir) {
                    print_gta4_entry(&a, id, &a.entry(id).name);
                }
            }
        }
        Node::ArchiveDir(rpf, dir) => {
            if recursive {
                vfs::walk(&rpf, dir, "", true, &mut |a, id, p| print_entry(a, id, p), &mut |e| {
                    eprintln!("warning: {e}")
                });
            } else {
                for id in rpf.children(dir) {
                    print_entry(&rpf, id, &rpf.entry(id).name);
                }
            }
        }
    }
    Ok(())
}

fn cmd_info(game_dir: &Path, path: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    match vfs::resolve(game_dir, path, keys)? {
        Node::ArchiveDir(rpf, dir) if dir == Rpf7::ROOT => {
            let (mut dirs, mut files, mut resources, mut archives) = (0, 0, 0, 0);
            for e in rpf.entries() {
                match e.kind_label() {
                    "dir" => dirs += 1,
                    "rpf" => archives += 1,
                    "resource" => resources += 1,
                    _ => files += 1,
                }
            }
            outln!("Archive:     {}", rpf.display_path());
            outln!("Format:      RPF7");
            outln!("Size:        {} ({} bytes)", human(rpf.size()), rpf.size());
            outln!("Encryption:  {:?}", rpf.encryption());
            outln!(
                "Entries:     {} ({dirs} dirs, {files} files, {resources} resources, {archives} archives)",
                rpf.entries().len()
            );
        }
        Node::ArchiveFile(rpf, id) | Node::ArchiveDir(rpf, id) => {
            let e = rpf.entry(id);
            outln!("Path:        {}/{}", rpf.display_path(), rpf.path_of(id));
            outln!("Kind:        {}", e.kind_label());
            outln!("Details:     {:?}", e.kind);
            if let Some(v) = e.resource_version() {
                outln!("Resource version: {v}");
            }
        }
        Node::Rpf8Dir(rpf, dir) => {
            let h = rpf.header();
            outln!("Archive:     {}", rpf.display_name());
            outln!("Format:      RPF8 (Red Dead Redemption 2)");
            outln!("Size:        {} ({} bytes)", human(rpf.size()), rpf.size());
            outln!(
                "Table:       {}",
                if h.is_encrypted() { format!("encrypted (key {})", h.decryption_tag) } else { "plaintext".into() }
            );
            outln!("Entries:     {}", h.entry_count);
            if !dir.is_empty() {
                outln!("Folder:      {dir}");
            }
        }
        Node::Rpf8File(rpf, i) => {
            let e = &rpf.entries().expect("entries")[i];
            outln!("Path:        {}/{}", rpf.display_name(), rpf.entry_name(i));
            outln!("Kind:        {}", if e.is_resource { "resource" } else { "file" });
            outln!("Size:        {} ({} on disk)", human(e.size()), human(e.size_on_disk));
            outln!("Compression: {:?}", e.compression);
            outln!("Encrypted:   {}", if e.is_encrypted() { format!("yes (key {})", e.key_id) } else { "no".into() });
            if let Some(v) = e.resource_version() {
                outln!("Resource version: {v}");
                outln!(
                    "Pages:       {} virtual, {} physical",
                    e.size_or_flags & 0xFFFF_FFF0,
                    e.size_or_flags >> 32 & 0xFFFF_FFF0
                );
            }
        }
        Node::Gta4Dir(a, dir) if dir == rage_rpf::Gta4Archive::ROOT => {
            let size = fs::metadata(a.path())?.len();
            let ids = 1..a.len();
            let dirs = ids.clone().filter(|&i| a.entry(i).is_dir()).count();
            let resources = ids.clone().filter(|&i| a.entry(i).resource_type().is_some()).count();
            outln!("Archive:     {}", a.display_name());
            outln!("Format:      {} (GTA IV)", a.format().name());
            outln!("Size:        {} ({size} bytes)", human(size));
            outln!("Table:       {}", if a.is_encrypted() { "encrypted (AES)" } else { "plaintext" });
            outln!(
                "Entries:     {} ({dirs} dirs, {} files, {resources} resources)",
                a.len() - 1,
                a.len() - 1 - dirs - resources
            );
        }
        Node::Gta4Dir(a, id) | Node::Gta4File(a, id) => {
            let e = a.entry(id);
            outln!("Path:        {}/{}", a.display_name(), a.entry_path(id));
            outln!("Kind:        {}", openrage_app::gta4::kind_label(&a, id));
            match e.kind {
                rage_rpf::Gta4EntryKind::Dir { ref children } => outln!("Entries:     {}", children.len()),
                rage_rpf::Gta4EntryKind::File { stored, size, compressed, .. } => {
                    outln!(
                        "Size:        {} ({} stored{})",
                        human(size),
                        human(stored),
                        if compressed { ", zlib" } else { "" }
                    )
                }
                rage_rpf::Gta4EntryKind::Resource { stored, resource_type, .. } => {
                    let (sys, gfx) = e.resource_sizes().unwrap_or_default();
                    outln!("Size:        {} (RSC5, zlib)", human(stored));
                    outln!(
                        "Resource:    type {resource_type} ({})",
                        openrage_app::gta4::resource_type_name(resource_type)
                    );
                    outln!("Memory:      {} system, {} graphics", human(sys as u64), human(gfx as u64));
                }
            }
        }
        Node::Dir(p) | Node::PlainFile(p) => outln!("{} is not inside an archive", p.display()),
    }
    Ok(())
}

fn report_rpf8_extract(rpf: &rage_rpf::Rpf8, dir: &str, file: Option<usize>, out: &Path) -> Result<()> {
    let s = openrage_app::extract::extract_rpf8(rpf, dir, file, out, &AtomicBool::new(false), &mut |_, _| {})?;
    for f in &s.failures {
        eprintln!("warning: {f}");
    }
    outln!("{} file(s) ({}) extracted to {}, {} failed", s.files, human(s.bytes), out.display(), s.failures.len());
    Ok(())
}

fn print_rpf8_entry(rpf: &rage_rpf::Rpf8, i: usize, name: &str) {
    let e = &rpf.entries().expect("entries")[i];
    let (kind, slash) = if e.ext_id == 0 {
        ("rpf", "/")
    } else if e.is_resource {
        ("resource", "")
    } else {
        ("file", "")
    };
    let version = e.resource_version().map(|v| format!("v{v}")).unwrap_or_default();
    outln!("{kind:<8} {:>8} {version:>4}  {name}{slash}", human(e.size()));
}

fn cmd_extract(game_dir: &Path, path: &str, out: &Path, expand: bool, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let (rpf, id) = match vfs::resolve(game_dir, path, keys)? {
        Node::ArchiveFile(rpf, id) | Node::ArchiveDir(rpf, id) => (rpf, id),
        Node::Rpf8File(rpf, i) => return report_rpf8_extract(&rpf, "", Some(i), out),
        Node::Rpf8Dir(rpf, dir) => return report_rpf8_extract(&rpf, &dir, None, out),
        Node::Gta4Dir(a, id) | Node::Gta4File(a, id) => {
            let s = openrage_app::gta4::extract(&a, id, out, &AtomicBool::new(false), &mut |_, _| {})?;
            for f in &s.failures {
                eprintln!("warning: {f}");
            }
            outln!(
                "Extracted {} files ({}) to {}, {} failed",
                s.files,
                human(s.bytes),
                s.target.display(),
                s.failures.len()
            );
            return Ok(());
        }
        Node::Dir(_) | Node::PlainFile(_) => bail!("{path} is not inside an archive"),
    };
    let summary = extract_entry(&rpf, id, out, expand, &AtomicBool::new(false), &mut |_, _| {})?;
    for failure in &summary.failures {
        eprintln!("warning: {failure}");
    }
    outln!(
        "Extracted {} files ({}) to {}{}",
        summary.files,
        human(summary.bytes),
        summary.target.display(),
        if summary.failures.is_empty() { String::new() } else { format!(" ({} failures)", summary.failures.len()) }
    );
    Ok(())
}

fn cmd_mods(game_dir: &Path, changes: bool, deep: bool, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let Some(mods) = rage_mods::mods_dir(game_dir) else {
        outln!("No mods folder in {}.", game_dir.display());
        return Ok(());
    };
    let files = rage_mods::scan(game_dir);
    outln!("{} files in {}", files.len(), mods.display());
    for f in &files {
        let kind = match f.kind {
            rage_mods::ModFileKind::Replaces => "replaces",
            rage_mods::ModFileKind::New => "new",
        };
        outln!("{kind:<9} {:>8}  {}", human(f.size), f.rel);
    }
    if !changes {
        return Ok(());
    }
    let mode = if deep { rage_mods::CompareMode::Deep } else { rage_mods::CompareMode::Quick };
    let started = Instant::now();
    let list =
        rage_mods::list_changes(game_dir, keys, mode, &mut |i, n, p| eprintln!("  [{}/{n}] comparing {p}", i + 1));
    outln!();
    for c in &list {
        let slash = if c.is_dir { "/" } else { "" };
        outln!("{:<15} {:>8}  {}{slash}", c.kind.label(), human(c.size), c.vpath);
    }
    eprintln!("{} changes ({:.1}s)", list.len(), started.elapsed().as_secs_f32());
    Ok(())
}

fn cmd_repack(
    game_dir: &Path,
    archive: &str,
    out: &Path,
    sets: &[String],
    removes: &[String],
    keys: Option<Arc<GtaVKeys>>,
) -> Result<()> {
    use openrage_app::repack::{Edit, repack_into};
    let rpf = match vfs::resolve(game_dir, archive, keys)? {
        Node::ArchiveDir(rpf, id) if id == Rpf7::ROOT => rpf,
        _ => bail!("{archive} is not an archive"),
    };
    let mut edits = std::collections::BTreeMap::new();
    for s in sets {
        let (inner, file) = s.split_once('=').context("--set expects inner/path=local_file")?;
        edits.insert(inner.to_owned(), Edit::Set(fs::read(file).with_context(|| format!("reading {file}"))?));
    }
    for r in removes {
        edits.insert(r.clone(), Edit::Remove);
    }
    let mut file =
        std::io::BufWriter::new(fs::File::create(out).with_context(|| format!("creating {}", out.display()))?);
    let written = repack_into(&rpf, &edits, &mut file)?;
    std::io::Write::flush(&mut file)?;
    outln!("Wrote {} ({}) with {} edits.", out.display(), human(written), edits.len());
    Ok(())
}

fn load_ytd(game_dir: &Path, path: &str, keys: Option<Arc<GtaVKeys>>) -> Result<rage_formats::ytd::TextureDictionary> {
    match vfs::resolve(game_dir, path, keys)? {
        Node::ArchiveFile(rpf, id) => Ok(rage_formats::ytd::parse(&rpf.read_resource(id)?)?),
        Node::Rpf8File(rpf, i) => {
            let entry = &rpf.entries().context("no entries")?[i];
            Ok(rage_formats::ytd::parse_rdr2(&rpf.read_resource(entry)?)?)
        }
        Node::Gta4File(a, id) => Ok(rage_formats::ytd::parse_gta4(&a.read_resource(id)?)?),
        _ => bail!("{path} is not a file inside an archive"),
    }
}

fn cmd_textures(
    game_dir: &Path,
    path: &str,
    export: Option<&Path>,
    png: bool,
    keys: Option<Arc<GtaVKeys>>,
) -> Result<()> {
    let dict = load_ytd(game_dir, path, keys)?;
    for t in &dict.textures {
        outln!(
            "{:<32} {:>5}x{:<5} {:<14} mips {:>2}  {:>8}  {}",
            t.name,
            t.width,
            t.height,
            t.format.name(),
            t.levels,
            human(t.data.len() as u64),
            t.usage_name()
        );
    }
    for e in &dict.errors {
        eprintln!("warning: {e}");
    }
    let Some(dir) = export else { return Ok(()) };
    fs::create_dir_all(dir)?;
    let mut written = 0;
    for t in &dict.textures {
        let Some(name) = vfs::safe_relative_path(&t.name) else {
            eprintln!("warning: skipped unsafe texture name {:?}", t.name);
            continue;
        };
        let (target, bytes) = if png {
            let Some(rgba) = t.to_rgba() else {
                eprintln!("warning: {}: no PNG decoder for {}", t.name, t.format.name());
                continue;
            };
            (dir.join(name).with_extension("png"), rage_formats::png(t.width, t.height, &rgba)?)
        } else {
            (dir.join(name).with_extension("dds"), rage_formats::dds::to_dds(t))
        };
        fs::write(&target, bytes).with_context(|| format!("writing {}", target.display()))?;
        written += 1;
    }
    outln!("Exported {written} textures to {}", dir.display());
    Ok(())
}

fn cmd_edit_textures(
    game_dir: &Path,
    path: &str,
    sets: &[String],
    removes: &[String],
    keys: Option<Arc<GtaVKeys>>,
) -> Result<()> {
    use rage_formats::ytd_write::Edit;
    let mut edits = Vec::new();
    for s in sets {
        let (name, file) = s.split_once('=').context("--set expects name=file.dds")?;
        edits.push(Edit::Set(openrage_app::textures::texture_from_dds(name, Path::new(file))?));
    }
    edits.extend(removes.iter().map(|r| Edit::Remove(r.clone())));
    let started = Instant::now();
    let (result, written) = openrage_app::textures::edit_textures(game_dir, keys, path, &edits)?;
    outln!(
        "{}: {} replaced/added ({} new), {} removed — {}",
        path,
        result.replaced + result.added,
        result.added,
        result.removed,
        if result.in_place { "pixel data replaced in place" } else { "dictionary rebuilt" }
    );
    outln!("Written to {} ({:.1}s)", written.display(), started.elapsed().as_secs_f32());
    Ok(())
}

fn cmd_verify_textures(game_dir: &Path, within: &str, rebuild: bool, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    #[derive(Default)]
    struct Stats {
        dicts: usize,
        textures: usize,
        decoded: usize,
        undecodable: std::collections::BTreeMap<String, usize>,
        errors: Vec<String>,
    }
    let starts = archive_roots(game_dir, within, keys)?;
    let started = Instant::now();
    let all: Vec<Stats> = starts
        .par_iter()
        .map(|(prefix, rpf, dir)| {
            let mut s = Stats::default();
            let mut nested_errors = Vec::new();
            vfs::walk(
                rpf,
                *dir,
                prefix,
                true,
                &mut |a, id, p| {
                    if !a.entry(id).name.to_ascii_lowercase().ends_with(".ytd")
                        || a.entry(id).resource_version().is_none()
                    {
                        return;
                    }
                    s.dicts += 1;
                    let dict = match a
                        .read_resource(id)
                        .map_err(anyhow::Error::from)
                        .and_then(|r| Ok(rage_formats::ytd::parse(&r)?))
                    {
                        Ok(d) => d,
                        Err(e) => return s.errors.push(format!("{p}: {e:#}")),
                    };
                    s.errors.extend(dict.errors.iter().map(|e| format!("{p}: {e}")));
                    if rebuild && dict.errors.is_empty() {
                        let check = rage_formats::ytd_write::rebuild(&dict)
                            .and_then(|file| rage_resource::Resource::from_rsc7(&file))
                            .and_then(|res| rage_formats::ytd::parse(&res));
                        match check {
                            Ok(back) => {
                                let mut a: Vec<_> = dict
                                    .textures
                                    .iter()
                                    .map(|t| (&t.name, t.width, t.height, t.levels, t.format, &t.data))
                                    .collect();
                                let mut b: Vec<_> = back
                                    .textures
                                    .iter()
                                    .map(|t| (&t.name, t.width, t.height, t.levels, t.format, &t.data))
                                    .collect();
                                a.sort_by(|x, y| x.0.cmp(y.0));
                                b.sort_by(|x, y| x.0.cmp(y.0));
                                if a != b || !back.errors.is_empty() {
                                    s.errors.push(format!("{p}: rebuilt dictionary differs from the original"));
                                }
                                // Structures must match byte for byte, apart from pointers.
                                let pointers: &[(usize, usize)] = match dict.layout {
                                    rage_formats::ytd::Layout::Legacy => &[(0x28, 0x30), (0x70, 0x78)],
                                    rage_formats::ytd::Layout::Enhanced | rage_formats::ytd::Layout::Rdr2 => {
                                        &[(0x28, 0x40)]
                                    }
                                    rage_formats::ytd::Layout::Gta4 => &[(0x14, 0x18), (0x48, 0x4C)],
                                };
                                let masked = |t: &[u8]| {
                                    let mut t = t.to_vec();
                                    for &(from, to) in pointers {
                                        t[from..to].fill(0);
                                    }
                                    t
                                };
                                for (t, st) in dict.textures.iter().zip(&dict.stored) {
                                    let Some(j) = back.textures.iter().position(|x| x.name == t.name) else { continue };
                                    let (x, y) = (masked(&st.template), masked(&back.stored[j].template));
                                    if x != y {
                                        let diffs: Vec<String> = (0..x.len())
                                            .filter(|&i| x[i] != y[i])
                                            .map(|i| format!("{i:#x}: {:02x}→{:02x}", x[i], y[i]))
                                            .collect();
                                        s.errors.push(format!(
                                            "{p}/{}: rebuilt texture structure differs ({}x{} {} mips {}; {})",
                                            t.name,
                                            t.width,
                                            t.height,
                                            t.format.name(),
                                            t.levels,
                                            diffs.join(", ")
                                        ));
                                        break;
                                    }
                                }
                            }
                            Err(e) => s.errors.push(format!("{p}: rebuild failed: {e}")),
                        }
                    }
                    for t in &dict.textures {
                        s.textures += 1;
                        match t.to_rgba() {
                            Some(_) => s.decoded += 1,
                            None => *s.undecodable.entry(t.format.name()).or_default() += 1,
                        }
                    }
                },
                &mut |e| nested_errors.push(e.to_string()),
            );
            s.errors.extend(nested_errors);
            s
        })
        .collect();
    let mut total = Stats::default();
    for s in all {
        total.dicts += s.dicts;
        total.textures += s.textures;
        total.decoded += s.decoded;
        total.errors.extend(s.errors);
        for (k, v) in s.undecodable {
            *total.undecodable.entry(k).or_default() += v;
        }
    }
    for e in total.errors.iter().take(50) {
        outln!("FAIL {e}");
    }
    for (format, n) in &total.undecodable {
        outln!("no preview decoder: {n} textures in {format}");
    }
    outln!(
        "{} texture dictionaries, {} textures, {} decoded, {} errors ({:.1}s)",
        total.dicts,
        total.textures,
        total.decoded,
        total.errors.len(),
        started.elapsed().as_secs_f32()
    );
    if !total.errors.is_empty() {
        bail!("{} errors", total.errors.len());
    }
    Ok(())
}

fn report(applied: &install::Applied, started: Instant) {
    if applied.created {
        outln!("Copied the archive into the mods folder first.");
    }
    outln!(
        "Written to {} ({} appended{}, {:.2}s). Undo with `openrage undo`.",
        applied.path.display(),
        human(applied.stats.appended),
        if applied.stats.relocated > 0 {
            format!(", {} entries relocated", applied.stats.relocated)
        } else {
            String::new()
        },
        started.elapsed().as_secs_f32()
    );
}

fn cmd_edit(game_dir: &Path, path: &str, op: Op, description: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let started = Instant::now();
    let applied = install::apply_at(game_dir, keys, path, op, description)?;
    report(&applied, started);
    Ok(())
}

fn cmd_put(game_dir: &Path, file: &Path, target: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let data = fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    let name = file.file_name().context("file has no name")?.to_string_lossy();
    let target = if target.ends_with('/') { format!("{target}{name}") } else { target.to_owned() };
    cmd_edit(game_dir, &target, Op::Set(data), &format!("Imported {name} into {target}"), keys)
}

fn cmd_mv(game_dir: &Path, from: &str, to: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let a = install::split(game_dir, from).context("source is not inside an archive")?;
    // `to` may be a full virtual path or a path inside the same archive.
    let to_inner = match install::split(game_dir, to) {
        Some(b) if b.archive.eq_ignore_ascii_case(&a.archive) => b.inner,
        Some(_) => bail!("cannot move between archives"),
        None => to.trim_start_matches('/').to_owned(),
    };
    let started = Instant::now();
    let applied = install::apply(
        game_dir,
        keys,
        &a.archive,
        &[(a.inner.clone(), Op::Rename(to_inner.clone()))],
        &format!("Moved {} to {to_inner}", a.inner),
    )?;
    report(&applied, started);
    Ok(())
}

fn print_report(report: &openrage_app::oiv::install::Report) {
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

fn cmd_oiv(game_dir: &Path, cmd: OivCommand, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
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
            if !yes {
                eprint!("Install into the mods folder? [y/N] ");
                let mut answer = String::new();
                std::io::stdin().read_line(&mut answer)?;
                if !answer.trim().eq_ignore_ascii_case("y") {
                    outln!("Cancelled.");
                    return Ok(());
                }
            }
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
        OivCommand::Uninstall { name } => {
            let undone = install::uninstall(game_dir, &name)?;
            outln!("Uninstalled \"{name}\" ({} changes undone).", undone.len());
        }
    }
    Ok(())
}

fn cmd_history(game_dir: &Path) -> Result<()> {
    let Some(mods) = rage_mods::mods_dir(game_dir) else {
        outln!("No mods folder.");
        return Ok(());
    };
    let entries = openrage_app::journal::Journal::new(&mods).entries()?;
    if entries.is_empty() {
        outln!("No recorded changes.");
    }
    for e in entries {
        outln!("#{:<4} {}  {}", e.id, e.archive, e.description);
    }
    Ok(())
}

fn cmd_undo(game_dir: &Path) -> Result<()> {
    match install::undo_last(game_dir)? {
        Some(e) => outln!("Undone: {} ({})", e.description, e.archive),
        None => outln!("Nothing to undo."),
    }
    Ok(())
}

/// What `openrage model` writes besides the listing.
struct ModelOutput<'a> {
    export: Option<&'a Path>,
    render: Option<&'a Path>,
    yaw: f32,
    anim: Option<&'a str>,
}

fn cmd_model(
    game_dir: &Path,
    path: &str,
    outfit: Option<usize>,
    output: ModelOutput,
    keys: Option<Arc<GtaVKeys>>,
) -> Result<()> {
    let ModelOutput { export, render, yaw, anim } = output;
    let model = match outfit {
        Some(n) => {
            let name = path.rsplit('/').next().unwrap_or(path);
            let stem = name.split('.').next().unwrap_or(name);
            openrage_app::models::load_rdr2_ped(game_dir, stem, n)?
        }
        None => openrage_app::models::load(game_dir, keys.clone(), path)?,
    };
    if let Some(p) = &model.ped {
        outln!("{}: outfit {} of {}", p.name, p.outfit, p.outfits);
    }
    if let Some(s) = &model.skeleton {
        outln!("skeleton: {} bones", s.bones.len());
    }
    for (name, d) in &model.drawables {
        outln!(
            "{name}: {} meshes, {} vertices, {} triangles, {} shaders",
            d.meshes.len(),
            d.vertex_count(),
            d.triangle_count(),
            d.shaders.len()
        );
        for (i, s) in d.shaders.iter().enumerate() {
            let base = s.base_texture().unwrap_or("-");
            let found = if base == "-" || model.texture(d, base).is_some() { "" } else { " (not found)" };
            outln!("  shader {i:>2} {:08x}  base {base}{found}  [{}]", s.name_hash, s.textures.join(", "));
        }
    }
    for (name, dict) in &model.external {
        outln!("textures from {name}: {}", dict.textures.len());
    }
    let shown = model.parts.iter().filter(|p| p.visible).count();
    outln!("{} parts ({shown} shown):", model.parts.len());
    for p in &model.parts {
        let [x, y, z] = [p.transform[12], p.transform[13], p.transform[14]];
        let mark = if p.visible { "*" } else { " " };
        outln!(
            "  {mark} {:<20} {:<12} at ({x:.2}, {y:.2}, {z:.2})  {}",
            p.name,
            p.group,
            model.drawables[p.drawable].0
        );
    }
    outln!("{} materials:", model.materials.len());
    for m in &model.materials {
        let kind = if m.paint {
            "paint"
        } else if m.textured {
            "textured"
        } else {
            "plain"
        };
        outln!("  {:<24} {kind}", m.label);
    }
    let stem = path.rsplit('/').next().unwrap_or(path).split('.').next().unwrap_or("model");
    if let Some(out) = export {
        let glb =
            openrage_app::models::to_glb(&model, stem, &model.default_look()).context("model has nothing to export")?;
        fs::write(out, &glb).with_context(|| format!("writing {}", out.display()))?;
        outln!("Wrote {} ({})", out.display(), human(glb.len() as u64));
    }
    if let Some(out) = render {
        use openrage_app::render;
        let materials = render::Materials::new(&model);
        let view = render::View { yaw: yaw.to_radians(), ..Default::default() };
        let started = Instant::now();
        let look = model.default_look();
        let skin = match anim {
            Some(spec) => {
                let (ycd, rest) = spec.rsplit_once(':').context("--anim wants <ycd>:<clip>[@seconds]")?;
                let (clip, secs) = match rest.split_once('@') {
                    Some((c, t)) => (c, t.parse::<f32>().context("bad time")?),
                    None => (rest, 0.0),
                };
                // A cutscene (.cut) plays its sections joined into one clip per actor.
                let dict = openrage_app::anim::load_clips(game_dir, keys, ycd)?;
                let c = dict.clips.iter().find(|c| c.name.eq_ignore_ascii_case(clip)).context("no such clip")?;
                let (_, part, at) = c.part_at(secs).context("clip without animation")?;
                let a = &dict.animations[part.animation];
                let skel = model.skeleton.as_ref().context("the model has no skeleton")?;
                let binding = openrage_app::anim::Binding::new(skel, a).with_expressions(skel, a, &model.expressions);
                outln!(
                    "animating {} of {} bones ({:.1} s clip)",
                    binding.animated_bones(),
                    skel.bones.len(),
                    c.duration()
                );
                Some(binding.skin(skel, a, a.frame_at(at)))
            }
            None => None,
        };
        let (lo, hi) = render::bounds(&model, &look.parts).context("nothing to render")?;
        let camera = render::orbit_camera(lo, hi, view);
        let rgba = render::render_camera(&model, &look, &materials, &camera, skin.as_deref(), 960, 600);
        let ms = started.elapsed().as_millis();
        fs::write(out, rage_formats::png(960, 600, &rgba)?)?;
        outln!("Rendered {} in {ms} ms", out.display());
    }
    Ok(())
}

fn cmd_map(
    game_dir: &Path,
    path: Option<&str>,
    render: Option<&Path>,
    yaw: f32,
    export: Option<&Path>,
    check: bool,
    keys: Option<Arc<GtaVKeys>>,
) -> Result<()> {
    use openrage_app::world;
    let started = Instant::now();
    let w = world::load(game_dir, keys.clone())?;
    let files = world::files(game_dir)?;
    if check {
        return check_maps(game_dir, &w, &files, keys, started);
    }
    let path = path.context("which map file? (or --check for all)")?;
    let scene = world::load_map(&w, &files, game_dir, keys, path, &|done, total| {
        if done % 25 == 0 || done == total {
            eprint!("\rloading models: {done}/{total}");
        }
    })?;
    eprintln!();
    let placed: usize = scene.models.iter().map(|(_, _, t)| t.len()).sum();
    outln!(
        "{}: {} objects, {} models loaded ({placed} placed), {} archetypes without a model ({:.1}s)",
        path,
        scene.objects,
        scene.models.len(),
        scene.missing.len(),
        started.elapsed().as_secs_f32()
    );
    for (name, n) in scene.missing.iter().take(10) {
        outln!("  no model: {n:>3} × {name}");
    }
    let looks: Vec<openrage_app::models::Look> = scene.models.iter().map(|(_, m, _)| m.default_look()).collect();
    let items: Vec<(&openrage_app::models::LoadedModel, &openrage_app::models::Look, &[rage_formats::math::Mat4])> =
        scene.models.iter().zip(&looks).map(|((_, m, t), l)| (m, l, t.as_slice())).collect();
    let (lo, hi) = world::scene_bounds(&items).context("nothing to show")?;
    let center = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, lo[2]];
    outln!(
        "extent {:.0} × {:.0} × {:.0} m around ({:.0}, {:.0}, {:.0})",
        hi[0] - lo[0],
        hi[1] - lo[1],
        hi[2] - lo[2],
        center[0],
        center[1],
        center[2]
    );
    let stem = path.rsplit('/').next().unwrap_or(path).split('.').next().unwrap_or("map");
    if let Some(out) = export {
        let glb = openrage_app::models::scene_to_glb(stem, &items, center).context("nothing to export")?;
        fs::write(out, &glb)?;
        outln!("Wrote {} ({})", out.display(), human(glb.len() as u64));
    }
    if let Some(out) = render {
        use openrage_app::render;
        let materials: Vec<render::Materials> =
            scene.models.iter().map(|(_, m, _)| render::Materials::new(m)).collect();
        let mut soft = Vec::new();
        for (((_, m, ts), l), mat) in scene.models.iter().zip(&looks).zip(&materials) {
            for t in ts {
                soft.push(render::SceneItem { model: m, look: l, materials: mat, transform: *t, skin: None });
            }
        }
        let camera = render::orbit_camera(lo, hi, render::View { yaw: yaw.to_radians(), ..Default::default() });
        let rgba = render::render_scene(&soft, &camera, 1280, 800);
        fs::write(out, rage_formats::png(1280, 800, &rgba)?)?;
        outln!("Rendered {}", out.display());
    }
    Ok(())
}

/// A GTA IV placement file (`.wpl`) as a scene: summary, glTF export, picture.
/// With `check`, every placement file of the game: how many objects have a model.
fn cmd_map_gta4(
    game_dir: &Path,
    path: Option<&str>,
    render: Option<&Path>,
    yaw: f32,
    export: Option<&Path>,
    check: bool,
) -> Result<()> {
    use openrage_app::gta4;
    let started = Instant::now();
    if check {
        return check_maps_gta4(game_dir, started);
    }
    let path = path.context("which placement file (.wpl)? (or --check for all)")?;
    let scene = gta4::load_map(game_dir, path, &|done, total| {
        if done % 25 == 0 || done == total {
            eprint!("\rloading models: {done}/{total}");
        }
    })?;
    eprintln!();
    let placed: usize = scene.models.iter().map(|(_, _, t)| t.len()).sum();
    outln!(
        "{path}: {} objects, {} models loaded ({placed} placed), {} models not found ({:.1}s)",
        scene.objects,
        scene.models.len(),
        scene.missing.len(),
        started.elapsed().as_secs_f32()
    );
    for (name, n) in scene.missing.iter().take(10) {
        outln!("  no model: {n:>3} × {name}");
    }
    let looks: Vec<openrage_app::models::Look> = scene.models.iter().map(|(_, m, _)| m.default_look()).collect();
    let items: Vec<(&openrage_app::models::LoadedModel, &openrage_app::models::Look, &[rage_formats::math::Mat4])> =
        scene.models.iter().zip(&looks).map(|((_, m, t), l)| (m, l, t.as_slice())).collect();
    let (lo, hi) = openrage_app::world::scene_bounds(&items).context("nothing to show")?;
    let center = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, lo[2]];
    outln!(
        "extent {:.0} × {:.0} × {:.0} m around ({:.0}, {:.0}, {:.0})",
        hi[0] - lo[0],
        hi[1] - lo[1],
        hi[2] - lo[2],
        center[0],
        center[1],
        center[2]
    );
    let stem = path.rsplit('/').next().unwrap_or(path).split('.').next().unwrap_or("map");
    if let Some(out) = export {
        let glb = openrage_app::models::scene_to_glb(stem, &items, center).context("nothing to export")?;
        fs::write(out, &glb)?;
        outln!("Wrote {} ({})", out.display(), human(glb.len() as u64));
    }
    if let Some(out) = render {
        use openrage_app::render;
        let materials: Vec<render::Materials> =
            scene.models.iter().map(|(_, m, _)| render::Materials::new(m)).collect();
        let mut soft = Vec::new();
        for (((_, m, ts), l), mat) in scene.models.iter().zip(&looks).zip(&materials) {
            for t in ts {
                soft.push(render::SceneItem { model: m, look: l, materials: mat, transform: *t, skin: None });
            }
        }
        let camera = render::orbit_camera(lo, hi, render::View { yaw: yaw.to_radians(), ..Default::default() });
        let rgba = render::render_scene(&soft, &camera, 1280, 800);
        fs::write(out, rage_formats::png(1280, 800, &rgba)?)?;
        outln!("Rendered {}", out.display());
    }
    Ok(())
}

/// Every GTA IV placement file: how many of its objects name a model the
/// game's `.ide` files define and an archive holds.
fn check_maps_gta4(game_dir: &Path, started: Instant) -> Result<()> {
    use openrage_app::gta4;
    let index = gta4::texture_index(game_dir);
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut disk = Vec::new();
    collect_archives(game_dir, &["wpl"], &mut disk);
    for p in disk {
        let rel = p.strip_prefix(game_dir).unwrap_or(&p).to_string_lossy().replace('\\', "/");
        files.push((rel, fs::read(&p)?));
    }
    let (starts, _) = gta4_starts(game_dir, "")?;
    for (prefix, a, root) in &starts {
        vfs::walk_gta4(a, *root, prefix, &mut |id, p| {
            if a.entry(id).name.to_ascii_lowercase().ends_with(".wpl")
                && let Ok(d) = a.read(id)
            {
                files.push((p.to_owned(), d));
            }
        });
    }
    let (mut objects, mut unnamed, mut no_file) = (0usize, 0usize, 0usize);
    let mut missing: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (_, data) in &files {
        let Ok(wpl) = rage_formats::wpl::parse(data) else { continue };
        for i in &wpl.instances {
            objects += 1;
            match index.model_name(i.model_hash) {
                None => unnamed += 1,
                Some(n) if index.model_source(&n, game_dir, game_dir).is_none() => {
                    no_file += 1;
                    *missing.entry(n).or_default() += 1;
                }
                Some(_) => {}
            }
        }
    }
    let mut missing: Vec<(String, usize)> = missing.into_iter().collect();
    missing.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    for (name, n) in missing.iter().take(15) {
        outln!("  no model file: {n:>4} × {name}");
    }
    outln!(
        "{} placement files, {objects} objects: {} with a model, {unnamed} with an unknown model name, {no_file} whose model was not found ({:.1}s)",
        files.len(),
        objects - unnamed - no_file,
        started.elapsed().as_secs_f32()
    );
    Ok(())
}

/// Every map file: how many of its objects have a model.
fn check_maps(
    game_dir: &Path,
    w: &openrage_app::world::World,
    files: &openrage_app::world::Files,
    keys: Option<Arc<GtaVKeys>>,
    started: Instant,
) -> Result<()> {
    use rayon::prelude::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let done = AtomicUsize::new(0);
    let total = w.maps.len();
    let results: Vec<Option<(usize, Vec<String>)>> = w
        .maps
        .par_iter()
        .map(|(vpath, _, _)| {
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            if n.is_multiple_of(500) || n == total {
                eprint!("\r{n}/{total} map files");
            }
            let xml = openrage_app::world::read_map(game_dir, keys.clone(), vpath)?;
            let placed = openrage_app::world::scene_entities(w, &xml);
            let missing = placed
                .iter()
                .filter(|p| !files.models.contains_key(&p.archetype))
                .map(|p| p.archetype.clone())
                .collect();
            Some((placed.len(), missing))
        })
        .collect();
    eprintln!();
    let unreadable = results.iter().filter(|r| r.is_none()).count();
    let (mut objects, mut missing_n) = (0usize, 0usize);
    let mut missing: HashMap<String, usize> = HashMap::new();
    let mut empty = 0;
    for (n, m) in results.into_iter().flatten() {
        objects += n;
        if n == 0 {
            empty += 1;
        }
        missing_n += m.len();
        for a in m {
            *missing.entry(a).or_default() += 1;
        }
    }
    outln!(
        "{total} map files ({unreadable} unreadable, {empty} without objects to show), {objects} objects; \
         {missing_n} objects ({} archetypes) without a model",
        missing.len()
    );
    let mut top: Vec<(String, usize)> = missing.into_iter().collect();
    top.sort_by_key(|t| std::cmp::Reverse(t.1));
    for (name, n) in top.iter().take(15) {
        outln!("  {n:>5} × {name}");
    }
    eprintln!("{:.1} s", started.elapsed().as_secs_f32());
    Ok(())
}

fn cmd_clips(game_dir: &Path, path: &str, clip: Option<&str>, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let dict = openrage_app::anim::load_clips(game_dir, keys, path)?;
    outln!("{} clips, {} animations", dict.clips.len(), dict.animations.len());
    for c in &dict.clips {
        let parts: Vec<String> = c
            .parts
            .iter()
            .map(|p| {
                let a = &dict.animations[p.animation];
                format!("anim {} ({} frames, {} tracks)", p.animation, a.frames, a.tracks.len())
            })
            .collect();
        outln!("  {:<40} {:>6.2}s  {}", c.name, c.duration(), parts.join(", "));
    }
    if let Some(name) = clip {
        let c = dict.clips.iter().find(|c| c.name.eq_ignore_ascii_case(name)).context("no such clip")?;
        let a = &dict.animations[c.parts.first().context("clip without animation")?.animation];
        let mut worst_norm = 0f32;
        let mut worst_jump = 0f32;
        for (t, tr) in a.tracks.iter().enumerate() {
            let vals: Vec<[f32; 4]> = (0..a.frames).filter_map(|f| a.value(t, f as f32)).collect();
            if tr.format == 1 {
                for v in &vals {
                    let n = (v.iter().map(|x| x * x).sum::<f32>()).sqrt();
                    worst_norm = worst_norm.max((n - 1.0).abs());
                }
            }
            for w in vals.windows(2) {
                let d = (0..3).map(|i| (w[1][i] - w[0][i]).abs()).fold(0.0, f32::max);
                worst_jump = worst_jump.max(d);
            }
            if t < 12 {
                let first = vals.first().copied().unwrap_or_default();
                let mid = vals.get(vals.len() / 2).copied().unwrap_or_default();
                outln!(
                    "  bone {:04x} track {} fmt {}: f0 [{:.3} {:.3} {:.3} {:.3}]  mid [{:.3} {:.3} {:.3} {:.3}]",
                    tr.bone,
                    tr.track,
                    tr.format,
                    first[0],
                    first[1],
                    first[2],
                    first[3],
                    mid[0],
                    mid[1],
                    mid[2],
                    mid[3]
                );
            }
        }
        outln!("worst |q|-1: {worst_norm:.4}, largest frame-to-frame change: {worst_jump:.4}");
    }
    Ok(())
}

fn cmd_audio(game_dir: &Path, path: &str, export: Option<&Path>, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let awc = openrage_app::audio::load(game_dir, keys, path)?;
    outln!("flags {:#06x}, {} streams{}", awc.flags, awc.streams.len(), if awc.encrypted { ", encrypted" } else { "" });
    if let Some(f) = &awc.stream_format {
        outln!(
            "multi-channel stream: {} channels, {} blocks of {}",
            f.channels,
            f.block_count,
            human(f.block_size as u64)
        );
    }
    for s in &awc.sounds {
        outln!(
            "  {:08x}  {:>8.2}s  {:>6} Hz  {}{}",
            s.id,
            s.duration_secs(),
            s.sample_rate,
            s.codec.name(),
            s.channel.map(|c| format!("  channel {c}")).unwrap_or_default()
        );
    }
    for (i, (id, chunks)) in awc.streams.iter().enumerate().take(4) {
        for c in chunks {
            let head = &awc.data[c.offset..(c.offset + 32).min(c.offset + c.size)];
            outln!("  stream {i} {id:08x} chunk {:02x} {:>8} @ {:>8}: {}", c.kind, c.size, c.offset, hex(head));
        }
    }
    if let Some(dir) = export {
        fs::create_dir_all(dir)?;
        let stem = path.rsplit('/').next().unwrap_or(path).trim_end_matches(".awc");
        let started = Instant::now();
        if awc.stream_format.is_some() {
            let pcm = awc.decode_stream()?;
            let out = dir.join(format!("{stem}.wav"));
            fs::write(&out, pcm.to_wav())?;
            outln!("Wrote {} ({:.1}s, {} channels)", out.display(), pcm.duration_secs(), pcm.channels);
        } else {
            for (i, s) in awc.sounds.iter().enumerate() {
                let pcm = awc.decode_sound(i)?;
                let out = dir.join(format!("{stem}_{:08x}.wav", s.id));
                fs::write(&out, pcm.to_wav())?;
                outln!("Wrote {} ({:.2}s)", out.display(), pcm.duration_secs());
            }
        }
        outln!("in {} ms", started.elapsed().as_millis());
    }
    Ok(())
}

fn cmd_xml(game_dir: &Path, path: &str, out: Option<&Path>, tree: bool, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    // Names of models, textures, … from the search index, so hashes read as names.
    if let Ok(names) = open_index(game_dir).and_then(|i| Ok(i.known_names()?)) {
        rage_formats::names::register(names);
    }
    let xml = openrage_app::metadata::load(game_dir, keys, path)?;
    for e in xml.errors.iter().take(20) {
        eprintln!("warning: {e}");
    }
    if tree {
        fn print(n: &openrage_app::xmltree::Node, depth: usize) {
            outln!("{}{}", "  ".repeat(depth), n.label);
            for (k, v) in &n.fields {
                outln!("{}  · {k}: {v}", "  ".repeat(depth));
            }
            for c in &n.children {
                print(c, depth + 1);
            }
        }
        let started = Instant::now();
        let root = openrage_app::xmltree::parse(&xml.text)?;
        print(&root, 0);
        eprintln!(
            "{} structures, {} of XML, tree built in {} ms",
            root.count(),
            human(xml.text.len() as u64),
            started.elapsed().as_millis()
        );
        return Ok(());
    }
    match out {
        Some(file) => {
            fs::write(file, &xml.text)?;
            outln!(
                "Wrote {} ({}, {}{})",
                file.display(),
                xml.encoding.name(),
                xml.root.as_deref().unwrap_or("?"),
                if xml.errors.is_empty() { String::new() } else { format!(", {} problems", xml.errors.len()) }
            );
        }
        None => outln!("{}", xml.text.trim_end()),
    }
    Ok(())
}

fn cmd_media(game_dir: &Path, path: &str, swf: Option<&Path>, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let (node, _) = rage_mods::resolve(game_dir, path, keys)?;
    let data = match node {
        Node::ArchiveFile(rpf, id) => rpf.read(id)?,
        Node::PlainFile(p) => fs::read(p)?,
        _ => bail!("{path} is not a file"),
    };
    for (k, v) in openrage_app::media::rows(path, &data)? {
        outln!("{k:<14}{v}");
    }
    if let Some(out) = swf {
        fs::write(out, rage_formats::media::gfx_to_swf(&data)?)?;
        outln!("Wrote {}", out.display());
    }
    Ok(())
}

fn cmd_verify_media(game_dir: &Path, within: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    use rage_formats::media;
    let starts = archive_roots(game_dir, within, keys)?;
    let started = Instant::now();
    type Counts = (usize, usize, f32, usize);
    let all: Vec<(Counts, Vec<String>)> = starts
        .par_iter()
        .map(|(prefix, rpf, dir)| {
            let mut c: Counts = (0, 0, 0.0, 0);
            let (mut errors, mut nested) = (Vec::new(), Vec::new());
            vfs::walk(
                rpf,
                *dir,
                prefix,
                true,
                &mut |a, id, p| {
                    let name = a.entry(id).name.to_ascii_lowercase();
                    if !media::is_media(&name) {
                        return;
                    }
                    let result = a.read(id).map_err(anyhow::Error::from).and_then(|d| {
                        if name.ends_with(".bik") {
                            let b = media::bink(&d)?;
                            c.1 += 1;
                            c.2 += b.duration();
                        } else {
                            let g = media::gfx(&d)?;
                            c.0 += 1;
                            c.3 += g.exports.len();
                        }
                        Ok(())
                    });
                    if let Err(e) = result {
                        errors.push(format!("{p}: {e:#}"));
                    }
                },
                &mut |e| nested.push(e.to_string()),
            );
            errors.extend(nested);
            (c, errors)
        })
        .collect();
    let mut t: Counts = (0, 0, 0.0, 0);
    for (c, _) in &all {
        t = (t.0 + c.0, t.1 + c.1, t.2 + c.2, t.3 + c.3);
    }
    let errors: Vec<&String> = all.iter().flat_map(|x| &x.1).collect();
    for e in errors.iter().take(30) {
        outln!("FAIL {e}");
    }
    outln!(
        "{} Scaleform movies ({} exported symbols), {} Bink videos ({:.1} min); {} errors ({:.1}s)",
        t.0,
        t.3,
        t.1,
        t.2 / 60.0,
        errors.len(),
        started.elapsed().as_secs_f32()
    );
    if !errors.is_empty() {
        bail!("{} errors", errors.len());
    }
    Ok(())
}

fn cmd_script(
    game_dir: &Path,
    path: &str,
    out: Option<&Path>,
    strings: bool,
    keys: Option<Arc<GtaVKeys>>,
) -> Result<()> {
    let (node, _) = rage_mods::resolve(game_dir, path, keys)?;
    if let Node::Gta4File(a, id) = &node {
        let rows = openrage_app::gta4::sco_rows(&a.read(*id)?).context("not a GTA IV script (.sco)")?;
        for (k, v) in rows {
            outln!("{k:<17} {v}");
        }
        outln!("(GTA IV scripts cannot be disassembled yet.)");
        return Ok(());
    }
    let Node::ArchiveFile(rpf, id) = node else { bail!("{path} is not a file inside an archive") };
    let s = rage_formats::ysc::parse(&rpf.read_resource(id)?)?;
    let text = if strings {
        s.all_strings().join("\n") + "\n"
    } else {
        let header = format!(
            "// {}: {} of code, {} natives, {} statics, {} strings, opcode table {}\n",
            s.name,
            human(s.code.len() as u64),
            s.natives.len(),
            s.statics.len(),
            s.all_strings().len(),
            s.table.map_or("unknown (jumps do not line up)".to_owned(), |t| format!("{t:?}"))
        );
        header + &s.listing(openrage_app::natives::name).map_err(|e| anyhow::anyhow!(e))?
    };
    match out {
        Some(file) => {
            fs::write(file, &text)?;
            outln!("Wrote {}", file.display());
        }
        None => outln!("{}", text.trim_end()),
    }
    Ok(())
}

fn cmd_particles(game_dir: &Path, path: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let (node, _) = rage_mods::resolve(game_dir, path, keys)?;
    let Node::ArchiveFile(rpf, id) = node else { bail!("{path} is not a file inside an archive") };
    let p = rage_formats::ypt::parse(&rpf.read_resource(id)?)?;
    outln!(
        "{}: {} effects, {} emitters, {} particle rules",
        p.name,
        p.effects.len(),
        p.emitters.len(),
        p.particles.len()
    );
    match &p.textures {
        Some(t) => outln!("{} textures", t.textures.len()),
        None => outln!("no textures"),
    }
    let tris: usize = p.drawables.iter().map(|(_, d)| d.triangle_count()).sum();
    outln!("{} models ({tris} triangles)", p.drawables.len());
    for e in &p.errors {
        outln!("problem: {e}");
    }
    for e in &p.effects {
        outln!("  effect  {e}");
    }
    Ok(())
}

fn cmd_verify_scripts(game_dir: &Path, within: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    use rage_formats::ysc;
    let starts = archive_roots(game_dir, within, keys)?;
    let started = Instant::now();
    type Counts = (usize, usize, usize, usize, usize);
    let all: Vec<(Counts, Vec<String>)> = starts
        .par_iter()
        .map(|(prefix, rpf, dir)| {
            let mut c: Counts = (0, 0, 0, 0, 0);
            let (mut errors, mut nested) = (Vec::new(), Vec::new());
            vfs::walk(
                rpf,
                *dir,
                prefix,
                true,
                &mut |a, id, p| {
                    if !a.entry(id).name.to_ascii_lowercase().ends_with(".ysc") {
                        return;
                    }
                    c.0 += 1;
                    match a.read_resource(id).map_err(anyhow::Error::from).and_then(|r| Ok(ysc::parse(&r)?)) {
                        Ok(s) => match s.table {
                            Some(t) => {
                                if t == ysc::Table::Current {
                                    c.1 += 1;
                                } else {
                                    c.2 += 1;
                                }
                                c.3 += s.check().unwrap_or(0);
                                c.4 += s.code.len();
                            }
                            None => errors
                                .push(format!("{p}: no opcode table fits ({})", s.check().err().unwrap_or_default())),
                        },
                        Err(e) => errors.push(format!("{p}: {e:#}")),
                    }
                },
                &mut |e| nested.push(e.to_string()),
            );
            errors.extend(nested);
            (c, errors)
        })
        .collect();
    let mut t: Counts = (0, 0, 0, 0, 0);
    for (c, _) in &all {
        t = (t.0 + c.0, t.1 + c.1, t.2 + c.2, t.3 + c.3, t.4 + c.4);
    }
    let errors: Vec<&String> = all.iter().flat_map(|x| &x.1).collect();
    for e in errors.iter().take(30) {
        outln!("FAIL {e}");
    }
    outln!(
        "{} scripts ({} current opcode table, {} classic), {} instructions, {} of code; {} errors ({:.1}s)",
        t.0,
        t.1,
        t.2,
        t.3,
        human(t.4 as u64),
        errors.len(),
        started.elapsed().as_secs_f32()
    );
    if !errors.is_empty() {
        bail!("{} errors", errors.len());
    }
    Ok(())
}

fn cmd_verify_paths(game_dir: &Path, within: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    use rage_formats::paths;
    let starts = archive_roots(game_dir, within, keys)?;
    let started = Instant::now();
    type Counts = (usize, usize, usize, usize, f32, usize);
    let all: Vec<(Counts, Vec<String>)> = starts
        .par_iter()
        .map(|(prefix, rpf, dir)| {
            let mut c: Counts = (0, 0, 0, 0, 0.0, 0);
            let (mut errors, mut nested) = (Vec::new(), Vec::new());
            vfs::walk(
                rpf,
                *dir,
                prefix,
                true,
                &mut |a, id, p| {
                    let name = a.entry(id).name.to_ascii_lowercase();
                    if !paths::is_path_file(&name) {
                        return;
                    }
                    c.0 += 1;
                    let ext = name.rsplit('.').next().unwrap_or_default().to_owned();
                    match a.read_resource(id).map_err(anyhow::Error::from).and_then(|r| Ok(paths::parse(&r, &ext)?)) {
                        Ok((d, s)) => {
                            c.1 += s.nodes + s.polygons;
                            c.2 += s.links;
                            c.3 += s.points;
                            c.4 += s.duration;
                            c.5 += s.skipped;
                            let bad = d
                                .meshes
                                .iter()
                                .flat_map(|m| &m.positions)
                                .any(|p| p.iter().any(|v| !v.is_finite() || v.abs() > 20000.0));
                            if bad {
                                errors.push(format!("{p}: positions out of the map"));
                            }
                        }
                        Err(e) => errors.push(format!("{p}: {e:#}")),
                    }
                },
                &mut |e| nested.push(e.to_string()),
            );
            errors.extend(nested);
            (c, errors)
        })
        .collect();
    let mut t: Counts = (0, 0, 0, 0, 0.0, 0);
    for (c, _) in &all {
        t = (t.0 + c.0, t.1 + c.1, t.2 + c.2, t.3 + c.3, t.4 + c.4, t.5 + c.5);
    }
    let errors: Vec<&String> = all.iter().flat_map(|x| &x.1).collect();
    for e in errors.iter().take(30) {
        outln!("FAIL {e}");
    }
    outln!(
        "{} path and nav mesh files, {} nodes and polygons, {} links, {} recorded points ({:.1} h of vehicle recordings, {} implausible points skipped); {} errors ({:.1}s)",
        t.0,
        t.1,
        t.2,
        t.3,
        t.4 / 3600.0,
        t.5,
        errors.len(),
        started.elapsed().as_secs_f32()
    );
    if !errors.is_empty() {
        bail!("{} errors", errors.len());
    }
    Ok(())
}

fn cmd_verify_collisions(game_dir: &Path, within: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let starts = archive_roots(game_dir, within, keys)?;
    let started = Instant::now();
    type Counts = (usize, usize, usize, usize);
    let all: Vec<(Counts, Vec<String>)> = starts
        .par_iter()
        .map(|(prefix, rpf, dir)| {
            let mut c: Counts = (0, 0, 0, 0);
            let (mut errors, mut nested) = (Vec::new(), Vec::new());
            vfs::walk(
                rpf,
                *dir,
                prefix,
                true,
                &mut |a, id, p| {
                    if !a.entry(id).name.to_ascii_lowercase().ends_with(".ybn") {
                        return;
                    }
                    c.0 += 1;
                    match a
                        .read_resource(id)
                        .map_err(anyhow::Error::from)
                        .and_then(|r| Ok(rage_formats::ybn::parse(&r)?))
                    {
                        Ok((_, s)) => {
                            c.1 += s.bounds;
                            c.2 += s.triangles;
                            c.3 += s.primitives;
                        }
                        Err(e) => errors.push(format!("{p}: {e:#}")),
                    }
                },
                &mut |e| nested.push(e.to_string()),
            );
            errors.extend(nested);
            (c, errors)
        })
        .collect();
    let mut t: Counts = (0, 0, 0, 0);
    for (c, _) in &all {
        t = (t.0 + c.0, t.1 + c.1, t.2 + c.2, t.3 + c.3);
    }
    let errors: Vec<&String> = all.iter().flat_map(|x| &x.1).collect();
    for e in errors.iter().take(30) {
        outln!("FAIL {e}");
    }
    outln!(
        "{} collision files, {} bounds, {} triangles, {} primitives; {} errors ({:.1}s)",
        t.0,
        t.1,
        t.2,
        t.3,
        errors.len(),
        started.elapsed().as_secs_f32()
    );
    if !errors.is_empty() {
        bail!("{} errors", errors.len());
    }
    Ok(())
}

fn cmd_verify_particles(game_dir: &Path, within: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let starts = archive_roots(game_dir, within, keys)?;
    let started = Instant::now();
    type Counts = (usize, usize, usize, usize, usize);
    let all: Vec<(Counts, Vec<String>)> = starts
        .par_iter()
        .map(|(prefix, rpf, dir)| {
            let mut c: Counts = (0, 0, 0, 0, 0);
            let (mut errors, mut nested) = (Vec::new(), Vec::new());
            vfs::walk(
                rpf,
                *dir,
                prefix,
                true,
                &mut |a, id, p| {
                    if !a.entry(id).name.to_ascii_lowercase().ends_with(".ypt") {
                        return;
                    }
                    c.0 += 1;
                    match a
                        .read_resource(id)
                        .map_err(anyhow::Error::from)
                        .and_then(|r| Ok(rage_formats::ypt::parse(&r)?))
                    {
                        Ok(d) => {
                            c.1 += d.effects.len();
                            c.2 += d.textures.as_ref().map_or(0, |t| t.textures.len());
                            c.3 += d.drawables.len();
                            c.4 += d.drawables.iter().map(|(_, m)| m.triangle_count()).sum::<usize>();
                            let bad_tex = d.textures.as_ref().map_or(0, |t| t.errors.len());
                            if !d.errors.is_empty() || bad_tex > 0 {
                                errors.push(format!("{p}: {} {bad_tex} texture errors", d.errors.join("; ")));
                            }
                        }
                        Err(e) => errors.push(format!("{p}: {e:#}")),
                    }
                },
                &mut |e| nested.push(e.to_string()),
            );
            errors.extend(nested);
            (c, errors)
        })
        .collect();
    let mut t: Counts = (0, 0, 0, 0, 0);
    for (c, _) in &all {
        t = (t.0 + c.0, t.1 + c.1, t.2 + c.2, t.3 + c.3, t.4 + c.4);
    }
    let errors: Vec<&String> = all.iter().flat_map(|x| &x.1).collect();
    for e in errors.iter().take(30) {
        outln!("FAIL {e}");
    }
    outln!(
        "{} particle dictionaries, {} effects, {} textures, {} models ({} triangles); {} errors ({:.1}s)",
        t.0,
        t.1,
        t.2,
        t.3,
        t.4,
        errors.len(),
        started.elapsed().as_secs_f32()
    );
    if !errors.is_empty() {
        bail!("{} errors", errors.len());
    }
    Ok(())
}

fn cmd_text(game_dir: &Path, path: &str, out: Option<&Path>, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    use openrage_app::texttable::{self, Kind};
    if path.to_ascii_lowercase().ends_with(".wpl") {
        let data = match rage_mods::resolve(game_dir, path, keys)?.0 {
            Node::Gta4File(a, id) => a.read(id)?,
            Node::PlainFile(p) => fs::read(p)?,
            _ => bail!("{path} is not a file"),
        };
        let (text, count) = openrage_app::gta4::wpl_to_text(Some(game_dir), &data)?;
        return match out {
            Some(file) => {
                fs::write(file, &text)?;
                outln!("Wrote {} ({count} records)", file.display());
                Ok(())
            }
            None => {
                outln!("{}", text.trim_end());
                Ok(())
            }
        };
    }
    let kind = Kind::of(path).context("not a text table (.gxt2, .nametable, .gxt) or placement file (.wpl)")?;
    if let Ok(names) = open_index(game_dir).and_then(|i| Ok(i.known_names()?)) {
        rage_formats::names::register(names);
    }
    let (node, _) = rage_mods::resolve(game_dir, path, keys)?;
    let data = match node {
        Node::ArchiveFile(rpf, id) => rpf.read(id)?,
        Node::Gta4File(a, id) => a.read(id)?,
        Node::PlainFile(p) => fs::read(p)?,
        _ => bail!("{path} is not a file"),
    };
    let (text, count) = texttable::to_text(kind, &data)?;
    match out {
        Some(file) => {
            fs::write(file, &text)?;
            outln!("Wrote {} ({count} entries)", file.display());
        }
        None => outln!("{}", text.trim_end()),
    }
    Ok(())
}

fn cmd_verify_text(game_dir: &Path, within: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    use openrage_app::texttable::{self, Kind};
    let starts = archive_roots(game_dir, within, keys)?;
    let started = Instant::now();
    let all: Vec<(usize, usize, Vec<String>)> = starts
        .par_iter()
        .map(|(prefix, rpf, dir)| {
            let (mut files, mut entries, mut errors) = (0, 0, Vec::new());
            let mut nested = Vec::new();
            vfs::walk(
                rpf,
                *dir,
                prefix,
                true,
                &mut |a, id, p| {
                    let Some(kind) = Kind::of(&a.entry(id).name) else { return };
                    files += 1;
                    let result = a.read(id).map_err(anyhow::Error::from).and_then(|data| {
                        let (text, n) = texttable::to_text(kind, &data)?;
                        let back = texttable::from_text(kind, &text)?;
                        if back != data {
                            bail!("converting back changes the file ({} → {} bytes)", data.len(), back.len());
                        }
                        Ok(n)
                    });
                    match result {
                        Ok(n) => entries += n,
                        Err(e) => errors.push(format!("{p}: {e:#}")),
                    }
                },
                &mut |e| nested.push(e.to_string()),
            );
            errors.extend(nested);
            (files, entries, errors)
        })
        .collect();
    let (files, entries): (usize, usize) = all.iter().fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
    let errors: Vec<&String> = all.iter().flat_map(|x| &x.2).collect();
    for e in errors.iter().take(30) {
        outln!("FAIL {e}");
    }
    outln!(
        "{files} text tables, {entries} strings and names, {} errors ({:.1}s)",
        errors.len(),
        started.elapsed().as_secs_f32()
    );
    if !errors.is_empty() {
        bail!("{} errors", errors.len());
    }
    Ok(())
}

fn cmd_verify_meta(game_dir: &Path, within: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    use openrage_app::metadata;
    use std::collections::BTreeMap;
    let starts = archive_roots(game_dir, within, keys)?;
    let started = Instant::now();
    type Counts = BTreeMap<String, (usize, usize)>;
    let all: Vec<(Counts, Vec<String>, Vec<String>)> = starts
        .par_iter()
        .map(|(prefix, rpf, dir)| {
            let (mut counts, mut errors, mut problems) = (Counts::new(), Vec::new(), Vec::new());
            let mut nested = Vec::new();
            vfs::walk(
                rpf,
                *dir,
                prefix,
                true,
                &mut |a, id, p| {
                    let name = &a.entry(id).name;
                    if !metadata::is_metadata(name) {
                        return;
                    }
                    let ext = name.rsplit('.').next().unwrap_or_default().to_ascii_lowercase();
                    let result = a.read(id).map_err(anyhow::Error::from).and_then(|d| metadata::from_bytes(&d));
                    let c = counts.entry(ext).or_insert((0, 0));
                    c.0 += 1;
                    match result {
                        Ok(x) => {
                            c.1 += x.text.len();
                            if let Some(e) = x.errors.first() {
                                problems.push(format!("{p}: {} problems, first: {e}", x.errors.len()));
                            }
                        }
                        Err(e) => errors.push(format!("{p}: {e:#}")),
                    }
                },
                &mut |e| nested.push(e.to_string()),
            );
            errors.extend(nested);
            (counts, errors, problems)
        })
        .collect();
    let mut counts = BTreeMap::<String, (usize, usize)>::new();
    for (c, _, _) in &all {
        for (k, v) in c {
            let e = counts.entry(k.clone()).or_default();
            e.0 += v.0;
            e.1 += v.1;
        }
    }
    let errors: Vec<&String> = all.iter().flat_map(|x| &x.1).collect();
    let problems: Vec<&String> = all.iter().flat_map(|x| &x.2).collect();
    for e in errors.iter().take(30) {
        outln!("FAIL {e}");
    }
    for e in problems.iter().take(30) {
        outln!("PART {e}");
    }
    for (ext, (n, bytes)) in &counts {
        outln!("  .{ext:<5} {n:>6} files, {} of XML", human(*bytes as u64));
    }
    let total: usize = counts.values().map(|c| c.0).sum();
    outln!(
        "{total} metadata files, {} errors, {} with unreadable parts ({:.1}s)",
        errors.len(),
        problems.len(),
        started.elapsed().as_secs_f32()
    );
    if !errors.is_empty() || !problems.is_empty() {
        bail!("{} errors, {} partial", errors.len(), problems.len());
    }
    Ok(())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" ")
}

/// Per archive: files, clips, animations, frame-count mismatches, bad values, errors.
type AnimStats = (usize, usize, usize, usize, usize, Vec<String>);

fn cmd_verify_animations(game_dir: &Path, within: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let starts = archive_roots(game_dir, within, keys)?;
    let started = Instant::now();
    let all: Vec<AnimStats> = starts
        .par_iter()
        .map(|(prefix, rpf, dir)| {
            let (mut files, mut clips, mut anims, mut mismatch, mut badq) = (0, 0, 0, 0, 0);
            let mut errors = Vec::new();
            let mut nested = Vec::new();
            vfs::walk(
                rpf,
                *dir,
                prefix,
                true,
                &mut |a, id, p| {
                    if !a.entry(id).name.to_ascii_lowercase().ends_with(".ycd")
                        || a.entry(id).resource_version().is_none()
                    {
                        return;
                    }
                    files += 1;
                    let parsed = a
                        .read_resource(id)
                        .map_err(|e| e.to_string())
                        .and_then(|r| rage_formats::clip::parse_ycd(&r).map_err(|e| e.to_string()));
                    match parsed {
                        Ok(d) => {
                            clips += d.clips.len();
                            anims += d.animations.len();
                            for an in &d.animations {
                                let blocks = an.block_frames();
                                let sum: usize = blocks.iter().sum();
                                let overlapped = sum.saturating_sub(blocks.len().saturating_sub(1));
                                if sum != an.frames && overlapped != an.frames {
                                    mismatch += 1;
                                }
                                for (t, tr) in an.tracks.iter().enumerate() {
                                    for f in (0..an.frames).step_by(7) {
                                        match an.value(t, f as f32) {
                                            Some(v) if tr.format == 1 => {
                                                let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
                                                if !(0.99..=1.01).contains(&n) {
                                                    badq += 1;
                                                }
                                            }
                                            Some(v) if v.iter().any(|x| !x.is_finite()) => badq += 1,
                                            _ => {}
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => errors.push(format!("{p}: {e}")),
                    }
                },
                &mut |e| nested.push(e.to_string()),
            );
            errors.extend(nested);
            (files, clips, anims, mismatch, badq, errors)
        })
        .collect();
    let sum = |f: fn(&AnimStats) -> usize| all.iter().map(f).sum::<usize>();
    let errors: Vec<&String> = all.iter().flat_map(|x| &x.5).collect();
    for e in errors.iter().take(40) {
        outln!("FAIL {e}");
    }
    outln!(
        "{} clip dictionaries, {} clips, {} animations; {} with unexpected frame counts, {} bad values; {} errors ({:.1}s)",
        sum(|x| x.0),
        sum(|x| x.1),
        sum(|x| x.2),
        sum(|x| x.3),
        sum(|x| x.4),
        errors.len(),
        started.elapsed().as_secs_f32()
    );
    if !errors.is_empty() {
        bail!("{} errors", errors.len());
    }
    Ok(())
}

fn cmd_verify_audio(game_dir: &Path, within: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let starts = archive_roots(game_dir, within, keys.clone())?;
    let started = Instant::now();
    // (containers, sounds, seconds, encrypted, not audio, errors)
    let all: Vec<(usize, usize, f64, usize, usize, Vec<String>)> = starts
        .par_iter()
        .map(|(prefix, rpf, dir)| {
            let (mut files, mut sounds, mut secs, mut encrypted, mut errors) = (0, 0, 0f64, 0, Vec::new());
            let mut not_audio = 0usize;
            let mut nested = Vec::new();
            vfs::walk(
                rpf,
                *dir,
                prefix,
                true,
                &mut |a, id, p| {
                    if !openrage_app::audio::is_audio(&a.entry(id).name) {
                        return;
                    }
                    files += 1;
                    let result = a
                        .read(id)
                        .map_err(anyhow::Error::from)
                        .and_then(|d| openrage_app::audio::parse(d, keys.as_deref()))
                        .and_then(|awc| {
                            if awc.encrypted || awc.flags & 0xA != 0 {
                                encrypted += 1;
                            }
                            let pcms = if awc.stream_format.is_some() {
                                vec![awc.decode_stream()?]
                            } else {
                                (0..awc.sounds.len()).map(|i| awc.decode_sound(i)).collect::<Result<Vec<_>, _>>()?
                            };
                            Ok(pcms)
                        });
                    match result {
                        Ok(pcms) => {
                            sounds += pcms.len();
                            secs += pcms.iter().map(|p| p.duration_secs() as f64).sum::<f64>();
                        }
                        // A few files only share the extension (e.g. shader caches).
                        Err(e)
                            if matches!(
                                e.downcast_ref::<rage_formats::awc::AwcError>(),
                                Some(rage_formats::awc::AwcError::NotAwc)
                            ) =>
                        {
                            files -= 1;
                            not_audio += 1;
                        }
                        Err(e) => errors.push(format!("{p}: {e:#}")),
                    }
                },
                &mut |e| nested.push(e.to_string()),
            );
            errors.extend(nested);
            (files, sounds, secs, encrypted, not_audio, errors)
        })
        .collect();
    let files: usize = all.iter().map(|x| x.0).sum();
    let sounds: usize = all.iter().map(|x| x.1).sum();
    let secs: f64 = all.iter().map(|x| x.2).sum();
    let encrypted: usize = all.iter().map(|x| x.3).sum();
    let not_audio: usize = all.iter().map(|x| x.4).sum();
    let errors: Vec<&String> = all.iter().flat_map(|x| &x.5).collect();
    for e in errors.iter().take(40) {
        outln!("FAIL {e}");
    }
    outln!(
        "{files} audio containers ({encrypted} encrypted, {not_audio} other .awc files skipped), {sounds} sounds, {:.1} hours of audio, {} errors ({:.1}s)",
        secs / 3600.0,
        errors.len(),
        started.elapsed().as_secs_f32()
    );
    if !errors.is_empty() {
        bail!("{} errors", errors.len());
    }
    Ok(())
}

fn cmd_verify_models(game_dir: &Path, within: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    use rage_formats::drawable;
    let starts = archive_roots(game_dir, within, keys)?;
    let started = Instant::now();
    let all: Vec<(usize, usize, Vec<String>)> = starts
        .par_iter()
        .map(|(prefix, rpf, dir)| {
            let (mut models, mut tris, mut errors) = (0usize, 0usize, Vec::new());
            let mut nested = Vec::new();
            vfs::walk(
                rpf,
                *dir,
                prefix,
                true,
                &mut |a, id, p| {
                    let n = a.entry(id).name.to_ascii_lowercase();
                    let drawable_file = n.ends_with(".ydr") || n.ends_with(".yft") || n.ends_with(".ydd");
                    if !drawable_file || a.entry(id).resource_version().is_none() {
                        return;
                    }
                    models += 1;
                    let parsed = a.read_resource(id).map_err(|e| e.to_string()).and_then(|r| {
                        let r = if n.ends_with(".ydd") {
                            drawable::parse_ydd(&r).map(|v| v.into_iter().map(|(_, d)| d).collect::<Vec<_>>())
                        } else if n.ends_with(".yft") {
                            drawable::parse_fragment(&r).map(|f| {
                                std::iter::once(f.drawable).chain(f.children.into_iter().map(|c| c.drawable)).collect()
                            })
                        } else {
                            drawable::parse_ydr(&r).map(|d| vec![d])
                        };
                        r.map_err(|e| e.to_string())
                    });
                    match parsed {
                        Ok(ds) => tris += ds.iter().map(|d| d.triangle_count()).sum::<usize>(),
                        Err(e) => errors.push(format!("{p}: {e}")),
                    }
                },
                &mut |e| nested.push(e.to_string()),
            );
            errors.extend(nested);
            (models, tris, errors)
        })
        .collect();
    let (models, tris): (usize, usize) = all.iter().fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
    let errors: Vec<&String> = all.iter().flat_map(|x| &x.2).collect();
    for e in errors.iter().take(40) {
        outln!("FAIL {e}");
    }
    outln!("{models} models, {tris} triangles, {} errors ({:.1}s)", errors.len(), started.elapsed().as_secs_f32());
    if !errors.is_empty() {
        bail!("{} errors", errors.len());
    }
    Ok(())
}

fn open_index(game_dir: &Path) -> Result<rage_index::Index> {
    let path = openrage_app::index_path(game_dir).context("no cache directory available")?;
    Ok(rage_index::Index::open(&path)?)
}

fn cmd_index(game_dir: &Path, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let mut index = open_index(game_dir)?;
    let stats = index.update(game_dir, keys, &AtomicBool::new(false), &mut |p| {
        eprintln!("  [{}/{}] {} ({} entries)", p.archives_done, p.archives_total, p.current, p.entries);
    })?;
    let s = index.stats()?;
    outln!(
        "Indexed {} archives, removed {}, unchanged {} in {:.1}s ({} read errors).",
        stats.archives_indexed,
        stats.archives_removed,
        stats.archives_unchanged,
        stats.seconds,
        stats.read_errors
    );
    outln!(
        "Index: {} archives, {} entries, {} text documents, {} dictionary strings, {} on disk ({})",
        s.archives,
        s.entries,
        s.text_documents,
        s.dictionary,
        human(s.file_bytes),
        index.path().display()
    );
    Ok(())
}

fn cmd_search(
    game_dir: &Path,
    query: &str,
    limit: usize,
    lang: Option<&str>,
    keys: Option<Arc<GtaVKeys>>,
) -> Result<()> {
    if let Some(l) = lang.filter(|l| !rage_index::LANGUAGES.contains(l)) {
        bail!("unknown language {l:?}; use one of {} or `all`", rage_index::LANGUAGES.join(", "));
    }
    let index = open_index(game_dir)?;
    if index.stats()?.updated_at.is_none() {
        bail!("the search index has not been built yet; run `openrage index` first");
    }
    let started = Instant::now();
    let results = index.search(query, limit, lang)?;
    let elapsed = started.elapsed();
    match results.hash {
        Some(hash) => {
            outln!("Hash 0x{hash:08X} = {} (signed {})", hash, hash as i32);
            if results.dictionary.is_empty() {
                outln!("  no known string for this hash");
            }
            for s in &results.dictionary {
                outln!("  = \"{s}\"");
            }
        }
        None => outln!("joaat(\"{query}\") = 0x{:08X}", results.query_hash),
    }
    for hit in &results.hits {
        let tag = match hit.reason {
            rage_index::HitReason::Name => "name",
            rage_index::HitReason::NameHash => "hash",
            rage_index::HitReason::Content => "text",
        };
        outln!("{tag:<5} {:>8}  {}", human(hit.size), hit.vpath);
        if hit.reason == rage_index::HitReason::Content {
            let needle = results.hash.map(|h| format!("0x{h:08X}")).unwrap_or_else(|| query.to_owned());
            if let Some(line) = content_snippet(game_dir, &hit.vpath, &hit.name, &needle, keys.clone()) {
                outln!("      └ {line}");
            }
        }
    }
    eprintln!(
        "{} hits{} in {:.0} ms",
        results.hits.len(),
        if results.limited { " (limited, use -n for more)" } else { "" },
        elapsed.as_secs_f64() * 1000.0
    );
    Ok(())
}

/// Matching line of an indexed text document, read back from the archive.
fn content_snippet(
    game_dir: &Path,
    vpath: &str,
    name: &str,
    query: &str,
    keys: Option<Arc<GtaVKeys>>,
) -> Option<String> {
    let Node::ArchiveFile(rpf, id) = vfs::resolve(game_dir, vpath, keys).ok()? else { return None };
    let data = rpf.read(id).ok()?;
    let (text, _) = rage_index::content_for(name, &data)?;
    rage_index::snippet(&text, query)
}

fn cmd_find(game_dir: &Path, within: &str, pattern: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let needle = pattern.to_ascii_lowercase();
    let starts = archive_roots(game_dir, within, keys)?;

    let started = Instant::now();
    let results: Vec<(Vec<String>, usize)> = starts
        .par_iter()
        .map(|(prefix, rpf, dir)| {
            let mut hits = Vec::new();
            let mut scanned = 0usize;
            vfs::walk(
                rpf,
                *dir,
                prefix,
                true,
                &mut |a, id, p| {
                    scanned += 1;
                    if a.entry(id).name.to_ascii_lowercase().contains(&needle) {
                        hits.push(p.to_owned());
                    }
                },
                &mut |e| eprintln!("warning: {e}"),
            );
            (hits, scanned)
        })
        .collect();

    let mut total = 0;
    let mut hits = 0;
    for (h, n) in results {
        total += n;
        hits += h.len();
        h.iter().for_each(|p| outln!("{p}"));
    }
    eprintln!("{hits} matches in {total} entries ({:.1}s)", started.elapsed().as_secs_f32());
    Ok(())
}

type Start = (String, Arc<Rpf7>, EntryId);

/// Resolves `within` to the archive directories to process: either one
/// directory inside an archive, or every `.rpf` below a real directory.
fn archive_roots(game_dir: &Path, within: &str, keys: Option<Arc<GtaVKeys>>) -> Result<Vec<Start>> {
    Ok(match vfs::resolve(game_dir, within, keys.clone())? {
        Node::ArchiveDir(rpf, dir) => vec![(within.trim_end_matches('/').to_owned(), rpf, dir)],
        Node::ArchiveFile(..) | Node::PlainFile(_) | Node::Rpf8File(..) | Node::Gta4File(..) => {
            bail!("{within} is not a directory")
        }
        Node::Gta4Dir(..) => bail!("{within}: this command does not support GTA IV archives yet"),
        Node::Rpf8Dir(..) => bail!("{within}: for Red Dead Redemption 2 archives use `openrage rpf8` for now"),
        Node::Dir(dir) => {
            let mut found = Vec::new();
            collect_rpfs(&dir, &mut found);
            found
                .into_iter()
                .filter_map(|p| {
                    let rel = p.strip_prefix(game_dir).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                    match Rpf7::open(&p, keys.clone()) {
                        Ok(rpf) => Some((rel, Arc::new(rpf), Rpf7::ROOT)),
                        Err(e) => {
                            eprintln!("warning: {e}");
                            None
                        }
                    }
                })
                .collect()
        }
    })
}

#[derive(Default)]
struct VerifyStats {
    archives: usize,
    files: usize,
    resources: usize,
    bytes: u64,
    errors: Vec<String>,
}

fn cmd_verify(game_dir: &Path, within: &str, keys: Option<Arc<GtaVKeys>>) -> Result<()> {
    let starts = archive_roots(game_dir, within, keys)?;
    let started = Instant::now();
    let stats: Vec<VerifyStats> = starts
        .par_iter()
        .map(|(prefix, rpf, dir)| {
            let mut s = VerifyStats { archives: 1, ..Default::default() };
            let mut nested_errors = Vec::new();
            vfs::walk(
                rpf,
                *dir,
                prefix,
                true,
                &mut |a, id, p| {
                    let e = a.entry(id);
                    let result = if e.is_dir() {
                        return;
                    } else if e.is_archive() {
                        s.archives += 1;
                        return;
                    } else if e.resource_version().is_some() {
                        s.resources += 1;
                        a.read_resource_pages(id)
                    } else {
                        s.files += 1;
                        a.read(id)
                    };
                    match result {
                        Ok(data) => s.bytes += data.len() as u64,
                        Err(err) => s.errors.push(format!("{p}: {err}")),
                    }
                },
                &mut |e| nested_errors.push(e.to_string()),
            );
            s.errors.extend(nested_errors);
            eprintln!("  checked {prefix}");
            s
        })
        .collect();

    let mut total = VerifyStats::default();
    for s in stats {
        total.archives += s.archives;
        total.files += s.files;
        total.resources += s.resources;
        total.bytes += s.bytes;
        total.errors.extend(s.errors);
    }
    for e in &total.errors {
        outln!("FAIL {e}");
    }
    outln!(
        "{} archives, {} files, {} resources, {} decoded, {} errors ({:.1}s)",
        total.archives,
        total.files,
        total.resources,
        human(total.bytes),
        total.errors.len(),
        started.elapsed().as_secs_f32()
    );
    if !total.errors.is_empty() {
        bail!("verification found {} errors", total.errors.len());
    }
    Ok(())
}

/// Reads every entry of every GTA IV archive below `within` (or of the one
/// archive it names) and inflates every resource.
type Gta4Start = (String, Arc<rage_rpf::Gta4Archive>, rage_rpf::Gta4EntryId);

/// The GTA IV archives to process: the folder or archive `within` names, or
/// every archive below a real folder. Archives that fail to open are returned
/// as errors.
fn gta4_starts(game_dir: &Path, within: &str) -> Result<(Vec<Gta4Start>, Vec<String>)> {
    use rage_rpf::Gta4Archive;
    let mut open_errors = Vec::new();
    let starts: Vec<Gta4Start> = match vfs::resolve(game_dir, within, None)? {
        Node::Gta4Dir(a, id) => vec![(within.trim_end_matches('/').to_owned(), a, id)],
        Node::Dir(dir) => {
            let mut found = Vec::new();
            collect_archives(&dir, &["rpf", "img"], &mut found);
            found
                .into_iter()
                .filter(|p| Gta4Archive::file_is_gta4(p))
                .filter_map(|p| {
                    let rel = p.strip_prefix(game_dir).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                    match Gta4Archive::open_default(&p) {
                        Ok(a) => Some((rel, Arc::new(a), Gta4Archive::ROOT)),
                        Err(e) => {
                            open_errors.push(format!("{rel}: {e}"));
                            None
                        }
                    }
                })
                .collect()
        }
        _ => bail!("{within} is not a folder or archive"),
    };
    Ok((starts, open_errors))
}

fn cmd_verify_gta4(game_dir: &Path, within: &str) -> Result<()> {
    let (starts, open_errors) = gta4_starts(game_dir, within)?;
    let started = Instant::now();
    let stats: Vec<VerifyStats> = starts
        .par_iter()
        .map(|(prefix, a, dir)| {
            let mut s = VerifyStats { archives: 1, ..Default::default() };
            vfs::walk_gta4(a, *dir, prefix, &mut |id, p| {
                let e = a.entry(id);
                let result = if e.is_dir() {
                    return;
                } else if e.resource_type().is_some() {
                    s.resources += 1;
                    a.read_resource(id).map(|r| r.system.len() + r.graphics.len())
                } else {
                    s.files += 1;
                    a.read(id).map(|d| d.len())
                };
                match result {
                    Ok(n) => s.bytes += n as u64,
                    Err(err) => s.errors.push(format!("{p}: {err}")),
                }
            });
            eprintln!("  checked {prefix}");
            s
        })
        .collect();
    let mut total = VerifyStats { archives: open_errors.len(), errors: open_errors, ..Default::default() };
    for s in stats {
        total.archives += s.archives;
        total.files += s.files;
        total.resources += s.resources;
        total.bytes += s.bytes;
        total.errors.extend(s.errors);
    }
    for e in &total.errors {
        outln!("FAIL {e}");
    }
    outln!(
        "{} archives, {} files, {} resources, {} decoded, {} errors ({:.1}s)",
        total.archives,
        total.files,
        total.resources,
        human(total.bytes),
        total.errors.len(),
        started.elapsed().as_secs_f32()
    );
    if !total.errors.is_empty() {
        bail!("verification found {} errors", total.errors.len());
    }
    Ok(())
}

/// Parses every GTA IV model (`.wdr`, `.wdd`, `.wft`).
fn cmd_verify_gta4_models(game_dir: &Path, within: &str) -> Result<()> {
    #[derive(Default)]
    struct Stats {
        files: usize,
        drawables: usize,
        meshes: usize,
        triangles: usize,
        textured: usize,
        shaders: usize,
        errors: Vec<String>,
    }
    let (starts, open_errors) = gta4_starts(game_dir, within)?;
    let started = Instant::now();
    let all: Vec<Stats> = starts
        .par_iter()
        .map(|(prefix, a, dir)| {
            let mut s = Stats::default();
            vfs::walk_gta4(a, *dir, prefix, &mut |id, p| {
                let e = a.entry(id);
                let lower = e.name.to_ascii_lowercase();
                let kind = ["wdr", "wdd", "wft"].into_iter().find(|x| lower.ends_with(&format!(".{x}")));
                let (Some(kind), Some(_)) = (kind, e.resource_type()) else { return };
                s.files += 1;
                let parsed = a.read_resource(id).map_err(anyhow::Error::from).and_then(|r| {
                    use rage_formats::wdr;
                    Ok(match kind {
                        "wdr" => vec![wdr::parse_wdr(&r)?],
                        "wdd" => wdr::parse_wdd(&r)?.into_iter().map(|(_, d)| d).collect(),
                        _ => {
                            let f = wdr::parse_wft(&r)?;
                            std::iter::once(f.drawable).chain(f.children.into_iter().map(|c| c.drawable)).collect()
                        }
                    })
                });
                match parsed {
                    Ok(ds) => {
                        for d in ds {
                            s.drawables += 1;
                            s.meshes += d.meshes.len();
                            s.triangles += d.triangle_count();
                            s.shaders += d.shaders.len();
                            s.textured += d.shaders.iter().filter(|sh| sh.base_texture().is_some()).count();
                        }
                    }
                    Err(err) => s.errors.push(format!("{p}: {err:#}")),
                }
            });
            s
        })
        .collect();
    let mut total = Stats { errors: open_errors, ..Default::default() };
    for s in all {
        total.files += s.files;
        total.drawables += s.drawables;
        total.meshes += s.meshes;
        total.triangles += s.triangles;
        total.shaders += s.shaders;
        total.textured += s.textured;
        total.errors.extend(s.errors);
    }
    for e in total.errors.iter().take(50) {
        outln!("FAIL {e}");
    }
    outln!(
        "{} model files, {} drawables, {} meshes, {} triangles, {}/{} shaders with a colour texture, {} errors ({:.1}s)",
        total.files,
        total.drawables,
        total.meshes,
        total.triangles,
        total.textured,
        total.shaders,
        total.errors.len(),
        started.elapsed().as_secs_f32()
    );
    if !total.errors.is_empty() {
        bail!("{} errors", total.errors.len());
    }
    Ok(())
}

/// Reads every GTA IV text table (`.gxt`: to text and back, identical), every
/// placement file (`.wpl`, on disk and in archives) and every script header (`.sco`).
fn cmd_verify_gta4_text(game_dir: &Path, within: &str) -> Result<()> {
    let started = Instant::now();
    let dir = match vfs::resolve(game_dir, within, None)? {
        Node::Dir(d) => d,
        _ => bail!("{within} is not a folder"),
    };
    let mut disk = Vec::new();
    collect_archives(&dir, &["gxt", "wpl"], &mut disk);
    let mut errors = Vec::new();
    let (mut tables, mut strings, mut placements, mut records, mut scripts) = (0, 0, 0, 0, 0);
    for p in &disk {
        let rel = p.strip_prefix(game_dir).unwrap_or(p).to_string_lossy().replace('\\', "/");
        let data = fs::read(p)?;
        if rel.to_ascii_lowercase().ends_with(".gxt") {
            let check = rage_formats::gxt::parse(&data).map_err(anyhow::Error::from).and_then(|t| {
                let text = rage_formats::gxt::to_text(&t, |_| None);
                let back = rage_formats::gxt::parse(&rage_formats::gxt::write(&rage_formats::gxt::from_text(&text)?))?;
                anyhow::ensure!(back == t, "text form does not convert back identically");
                Ok(t)
            });
            match check {
                Ok(t) => {
                    tables += t.len();
                    strings += t.iter().map(|t| t.entries.len()).sum::<usize>();
                }
                Err(e) => errors.push(format!("{rel}: {e:#}")),
            }
        } else {
            placements += 1;
            match rage_formats::wpl::parse(&data) {
                Ok(w) => records += rage_formats::wpl::record_count(&w),
                Err(e) => errors.push(format!("{rel}: {e}")),
            }
        }
    }
    let (starts, open_errors) = gta4_starts(game_dir, within)?;
    errors.extend(open_errors);
    for (prefix, a, root) in &starts {
        vfs::walk_gta4(a, *root, prefix, &mut |id, p| {
            let lower = a.entry(id).name.to_ascii_lowercase();
            if lower.ends_with(".wpl") {
                placements += 1;
                match a.read(id).map_err(anyhow::Error::from).and_then(|d| Ok(rage_formats::wpl::parse(&d)?)) {
                    Ok(w) => records += rage_formats::wpl::record_count(&w),
                    Err(e) => errors.push(format!("{p}: {e:#}")),
                }
            } else if lower.ends_with(".sco") {
                scripts += 1;
                if a.read(id).ok().and_then(|d| rage_formats::sco::header(&d)).is_none() {
                    errors.push(format!("{p}: not a script"));
                }
            }
        });
    }
    for e in errors.iter().take(50) {
        outln!("FAIL {e}");
    }
    outln!(
        "{tables} text tables ({strings} strings), {placements} placement files ({records} records), {scripts} scripts, {} errors ({:.1}s)",
        errors.len(),
        started.elapsed().as_secs_f32()
    );
    if !errors.is_empty() {
        bail!("{} errors", errors.len());
    }
    Ok(())
}

/// Parses every GTA IV texture dictionary (`.wtd`) and decodes every texture.
fn cmd_verify_gta4_textures(game_dir: &Path, within: &str) -> Result<()> {
    #[derive(Default)]
    struct Stats {
        dicts: usize,
        textures: usize,
        decoded: usize,
        formats: std::collections::BTreeMap<String, usize>,
        errors: Vec<String>,
    }
    let (starts, open_errors) = gta4_starts(game_dir, within)?;
    let started = Instant::now();
    let all: Vec<Stats> = starts
        .par_iter()
        .map(|(prefix, a, dir)| {
            let mut s = Stats::default();
            vfs::walk_gta4(a, *dir, prefix, &mut |id, p| {
                let e = a.entry(id);
                if e.resource_type().is_none() || !e.name.to_ascii_lowercase().ends_with(".wtd") {
                    return;
                }
                s.dicts += 1;
                let dict = match a
                    .read_resource(id)
                    .map_err(anyhow::Error::from)
                    .and_then(|r| Ok(rage_formats::ytd::parse_gta4(&r)?))
                {
                    Ok(d) => d,
                    Err(err) => return s.errors.push(format!("{p}: {err:#}")),
                };
                s.errors.extend(dict.errors.iter().map(|err| format!("{p}: {err}")));
                for t in &dict.textures {
                    s.textures += 1;
                    *s.formats.entry(t.format.name()).or_default() += 1;
                    match t.to_rgba() {
                        Some(_) => s.decoded += 1,
                        None => s.errors.push(format!("{p}/{}: cannot decode {}", t.name, t.format.name())),
                    }
                }
            });
            s
        })
        .collect();
    let mut total = Stats { errors: open_errors, ..Default::default() };
    for s in all {
        total.dicts += s.dicts;
        total.textures += s.textures;
        total.decoded += s.decoded;
        total.errors.extend(s.errors);
        for (k, v) in s.formats {
            *total.formats.entry(k).or_default() += v;
        }
    }
    for e in total.errors.iter().take(50) {
        outln!("FAIL {e}");
    }
    let formats: Vec<String> = total.formats.iter().map(|(f, n)| format!("{f} {n}")).collect();
    outln!("formats: {}", formats.join(", "));
    outln!(
        "{} texture dictionaries, {} textures, {} decoded, {} errors ({:.1}s)",
        total.dicts,
        total.textures,
        total.decoded,
        total.errors.len(),
        started.elapsed().as_secs_f32()
    );
    if !total.errors.is_empty() {
        bail!("{} errors", total.errors.len());
    }
    Ok(())
}

fn collect_archives(dir: &Path, extensions: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut items: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    items.sort();
    for p in items {
        if p.is_dir() {
            collect_archives(&p, extensions, out);
        } else if p.extension().is_some_and(|e| extensions.iter().any(|x| e.eq_ignore_ascii_case(x))) {
            out.push(p);
        }
    }
}

fn collect_rpfs(dir: &Path, out: &mut Vec<PathBuf>) {
    collect_archives(dir, &["rpf"], out);
}

/// Which files `openrage rpf8` parses instead of only reading them.
#[derive(Clone, Copy)]
struct Checks {
    textures: bool,
    models: bool,
    texts: bool,
    meta: bool,
    audio: bool,
    anims: bool,
}

fn cmd_rpf8(path: &Path, list: bool, checks: Checks) -> Result<()> {
    let Checks { textures, models, texts: text, meta, audio, anims } = checks;
    use rage_rpf::rpf8::{Compression, Rpf8};
    let keys = rdr2_keys()?;
    let names = rdr2_game_folder(path)
        .and_then(|g| rage_rpf::rpf8_names::NameIndex::default_path(&g))
        .and_then(|p| rage_rpf::rpf8_names::NameIndex::load(&p).ok().flatten())
        .map(std::sync::Arc::new);
    fn collect(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
        for e in std::fs::read_dir(dir)? {
            let p = e?.path();
            if p.is_dir() {
                collect(&p, out)?;
            } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("rpf")) {
                out.push(p);
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    if path.is_dir() {
        collect(path, &mut files)?;
        files.sort();
    } else {
        files.push(path.to_path_buf());
    }
    /// Counts and report lines for one archive file and everything nested in it.
    #[derive(Default)]
    struct Tally {
        archives: usize,
        nested: usize,
        readable: usize,
        entries: usize,
        failed: usize,
        /// Textures decoded, models parsed or text tables read.
        textures: usize,
        /// Triangles of the models, or strings of the text tables.
        triangles: usize,
        lines: Vec<String>,
    }
    /// Parses a texture dictionary and decodes all its textures; returns how many.
    fn check_textures(rpf: &Rpf8, e: &rage_rpf::rpf8::Entry) -> Result<usize, String> {
        let res = rpf.read_resource(e).map_err(|err| err.to_string())?;
        let dict = rage_formats::ytd::parse_rdr2(&res).map_err(|err| err.to_string())?;
        if let Some(err) = dict.errors.first() {
            return Err(format!(
                "{} of {} textures unreadable, e.g. {err}",
                dict.errors.len(),
                dict.errors.len() + dict.textures.len()
            ));
        }
        for t in &dict.textures {
            if t.to_rgba().is_none() {
                return Err(format!("{}: cannot decode {}", t.name, t.format.name()));
            }
        }
        Ok(dict.textures.len())
    }
    /// Parses a model; returns its triangle count.
    fn check_model(rpf: &Rpf8, e: &rage_rpf::rpf8::Entry) -> Result<usize, String> {
        use rage_formats::drawable;
        let res = rpf.read_resource(e).map_err(|err| err.to_string())?;
        let drawables = match e.ext_id {
            3 => vec![drawable::parse_fragment_rdr2(&res).map_err(|err| err.to_string())?.drawable],
            4 => drawable::parse_ydd_rdr2(&res).map_err(|err| err.to_string())?.into_iter().map(|(_, d)| d).collect(),
            _ => vec![drawable::parse_ydr_rdr2(&res).map_err(|err| err.to_string())?],
        };
        let triangles: usize = drawables.iter().map(|d| d.triangle_count()).sum();
        if drawables.iter().all(|d| d.meshes.is_empty()) {
            return Err("no meshes".into());
        }
        for d in &drawables {
            for m in &d.meshes {
                if m.positions.iter().any(|p| p.iter().any(|v| !v.is_finite() || v.abs() > 1.0e6)) {
                    return Err("implausible vertex positions".into());
                }
            }
        }
        Ok(triangles)
    }
    /// Converts a metadata file to XML; returns how many problems it had.
    fn check_meta(rpf: &Rpf8, e: &rage_rpf::rpf8::Entry) -> Result<usize, String> {
        let xml = if e.is_resource {
            let res = rpf.read_resource(e).map_err(|err| err.to_string())?;
            if !rage_formats::meta::is_meta(res.version) {
                return Err(format!("resource version {} is not a meta file", res.version));
            }
            openrage_app::metadata::from_resource(&res).map_err(|err| err.to_string())?
        } else {
            openrage_app::metadata::from_bytes_rdr2(&rpf.read(e).map_err(|err| err.to_string())?)
                .map_err(|err| err.to_string())?
        };
        Ok(xml.errors.len())
    }
    /// Reads a text table; returns its string count.
    fn check_texts(rpf: &Rpf8, e: &rage_rpf::rpf8::Entry) -> Result<usize, String> {
        let res = rpf.read_resource(e).map_err(|err| err.to_string())?;
        Ok(rage_formats::ldb::parse(&res).map_err(|err| err.to_string())?.len())
    }
    /// Decodes every sound of an audio container; `None` when it is not one.
    fn check_audio(rpf: &Rpf8, e: &rage_rpf::rpf8::Entry) -> Result<Option<(usize, f32)>, String> {
        let data = rpf.read(e).map_err(|err| err.to_string())?;
        if !data.starts_with(b"ADAT") {
            return Ok(None);
        }
        let awc = rage_formats::awc::Awc::parse_rdr2(data).map_err(|err| err.to_string())?;
        if awc.stream_format.is_some() {
            let pcm = awc.decode_stream().map_err(|err| err.to_string())?;
            return Ok(Some((1, pcm.duration_secs())));
        }
        let mut secs = 0.0;
        for i in 0..awc.sounds.len() {
            secs +=
                awc.decode_sound(i).map_err(|err| format!("sound {:08x}: {err}", awc.sounds[i].id))?.duration_secs();
        }
        Ok(Some((awc.sounds.len(), secs)))
    }
    fn walk(rpf: &Rpf8, depth: usize, list: bool, checks: Checks, t: &mut Tally) {
        let Checks { textures, models, texts, meta, audio, anims } = checks;
        let h = rpf.header();
        let Some(entries) = rpf.entries() else { return };
        t.readable += 1;
        let indent = "  ".repeat(depth + 1);
        for e in entries {
            t.entries += 1;
            let name = e.display_name(h.platform);
            if list {
                let comp = match e.compression {
                    Compression::None => "",
                    Compression::Deflate => " deflate",
                    Compression::Oodle => " oodle",
                    Compression::Unknown(_) => " ?",
                };
                let enc = if e.is_encrypted() { " encrypted" } else { "" };
                t.lines.push(format!("{indent}{name:<60} {:>10}{comp}{enc}", e.size()));
            }
            if e.ext_id == 0 {
                match rpf.open_child(e) {
                    Ok(child) => {
                        t.nested += 1;
                        walk(&child, depth + 1, list, checks, t);
                    }
                    Err(err) => {
                        t.failed += 1;
                        t.lines.push(format!("{indent}{name}: {err}"));
                    }
                }
            } else if anims {
                // Extension id 67 is `.#cd` (`.ycd` on PC).
                if e.ext_id == 67 && e.is_resource {
                    match rpf
                        .read_resource(e)
                        .map_err(|err| err.to_string())
                        .and_then(|res| rage_formats::clip::parse_ycd(&res).map_err(|err| err.to_string()))
                    {
                        Ok(dict) => {
                            for a in &dict.animations {
                                for f in 0..a.frames {
                                    for k in 0..a.tracks.len() {
                                        let _ = a.value(k, f as f32);
                                    }
                                }
                            }
                            t.textures += dict.clips.len();
                            t.triangles += dict.animations.len();
                        }
                        Err(err) => {
                            t.failed += 1;
                            t.lines.push(format!("{indent}{name}: {err}"));
                        }
                    }
                }
            } else if audio {
                // Audio containers have no extension (their hash covers the path).
                if e.ext_id == 0xFF && !e.is_resource {
                    match check_audio(rpf, e) {
                        Ok(Some((sounds, secs))) => {
                            t.textures += sounds;
                            t.triangles += secs.round() as usize;
                        }
                        Ok(None) => {}
                        Err(err) => {
                            t.failed += 1;
                            t.lines.push(format!("{indent}{name}: {err}"));
                        }
                    }
                }
            } else if meta {
                // .ymap 73, .ytyp 74, .ymt 11, .ymf 1
                if matches!(e.ext_id, 1 | 11 | 73 | 74) {
                    match check_meta(rpf, e) {
                        Ok(problems) => {
                            t.textures += 1;
                            t.triangles += problems;
                            if problems > 0 {
                                t.lines.push(format!("{indent}{name}: {problems} parts not read"));
                            }
                        }
                        Err(err) => {
                            t.failed += 1;
                            t.lines.push(format!("{indent}{name}: {err}"));
                        }
                    }
                }
            } else if models {
                if matches!(e.ext_id, 2..=4) && e.is_resource {
                    match check_model(rpf, e) {
                        Ok(n) => {
                            t.textures += 1;
                            t.triangles += n;
                        }
                        Err(err) => {
                            t.failed += 1;
                            t.lines.push(format!("{indent}{name}: {err}"));
                        }
                    }
                }
            } else if texts {
                // Extension id 76 is `.#ldb` (`.yldb` on PC).
                if e.ext_id == 76 && e.is_resource {
                    match check_texts(rpf, e) {
                        Ok(n) => {
                            t.textures += 1;
                            t.triangles += n;
                        }
                        Err(err) => {
                            t.failed += 1;
                            t.lines.push(format!("{indent}{name}: {err}"));
                        }
                    }
                }
            } else if textures {
                if e.ext_id == 5 && e.is_resource {
                    match check_textures(rpf, e) {
                        Ok(n) => t.textures += n,
                        Err(err) => {
                            t.failed += 1;
                            t.lines.push(format!("{indent}{name}: {err}"));
                        }
                    }
                }
            } else if let Err(err) = rpf.read(e) {
                t.failed += 1;
                t.lines.push(format!("{indent}{name}: {err}"));
            }
        }
    }
    let tallies: Vec<(String, Tally)> = {
        use rayon::prelude::*;
        files
            .par_iter()
            .map(|file| {
                let name = file
                    .strip_prefix(path)
                    .ok()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(file)
                    .display()
                    .to_string();
                let mut t = Tally::default();
                if file.metadata().is_ok_and(|m| m.len() == 0) {
                    t.lines.push(format!("{name}: empty file"));
                    return (name, t);
                }
                match Rpf8::open_with_keys(file, keys.clone()) {
                    Ok(mut rpf) => {
                        if let Some(n) = &names {
                            rpf.set_names(std::sync::Arc::clone(n));
                        }
                        t.archives = 1;
                        let h = rpf.header();
                        let table = if h.is_encrypted() {
                            format!("encrypted (key {})", h.decryption_tag)
                        } else {
                            "plaintext".to_string()
                        };
                        t.lines.push(format!(
                            "{name}: {} entries, table {table}, platform '{}', {} bytes",
                            h.entry_count,
                            h.platform,
                            rpf.size()
                        ));
                        walk(&rpf, 0, list, checks, &mut t);
                    }
                    Err(e) => {
                        t.failed += 1;
                        t.lines.push(format!("{name}: {e}"));
                    }
                }
                (name, t)
            })
            .collect()
    };
    let (mut ok, mut nested, mut plain, mut entries_total, mut failed, mut empty) = (0, 0, 0, 0usize, 0, 0);
    let mut textures_total = 0;
    let mut triangles_total = 0;
    for (_, t) in &tallies {
        for l in &t.lines {
            outln!("{l}");
        }
        if t.archives == 0 && t.failed == 0 {
            empty += 1;
        }
        ok += t.archives;
        nested += t.nested;
        plain += t.readable;
        entries_total += t.entries;
        failed += t.failed;
        textures_total += t.textures;
        triangles_total += t.triangles;
    }
    if textures {
        outln!("\n{textures_total} textures decoded");
    }
    if models {
        outln!("\n{textures_total} models parsed ({triangles_total} triangles)");
    }
    if text {
        outln!("\n{textures_total} text tables read ({triangles_total} strings)");
    }
    if meta {
        outln!("\n{textures_total} metadata files converted ({triangles_total} parts not read)");
    }
    if audio {
        outln!("\n{textures_total} sounds decoded ({:.1} hours)", triangles_total as f64 / 3600.0);
    }
    if anims {
        outln!("\n{textures_total} clips, {triangles_total} animations read (every frame evaluated)");
    }
    outln!(
        "\n{ok} archives read ({nested} more nested in them), {empty} empty, {failed} failed; {plain} with a readable table ({entries_total} entries).{}",
        if keys.is_some() {
            ""
        } else {
            " Encrypted tables need keys from your game: see `openrage rdr2-keys --help`."
        }
    );
    if failed > 0 {
        bail!("{failed} archives or entries could not be read");
    }
    Ok(())
}

/// The stored RDR2 keys, if `openrage rdr2-keys` was run.
fn rdr2_keys() -> Result<Option<std::sync::Arc<rage_rpf::rpf8::Rdr2Keys>>> {
    use rage_rpf::rpf8_keys::KeyStore;
    let Some(path) = KeyStore::default_path() else { return Ok(None) };
    let Some(store) = KeyStore::load(&path)? else { return Ok(None) };
    let keys =
        store.to_keys().with_context(|| format!("{} is damaged; run `openrage rdr2-keys` again", path.display()))?;
    Ok(Some(std::sync::Arc::new(keys)))
}

fn cmd_rdr2_keys(game: &Path, dump: Option<&Path>, file: Option<&Path>) -> Result<()> {
    use rage_rpf::rpf8_exe::{keys_in_dump, tables_from_exe};
    use rage_rpf::rpf8_keys::{Candidates, KeyStore, archives_under, identify, parse_secrets};
    let started = Instant::now();
    if dump.is_none() && file.is_none() {
        bail!("pass --dump <memory dump of the running game> or --file <key file>");
    }
    let exe_path = game.join("RDR2.exe");
    let exe = std::fs::read(&exe_path).with_context(|| format!("cannot read {}", exe_path.display()))?;
    let tables = tables_from_exe(&exe)?;
    drop(exe);
    outln!("RDR2.exe: cipher tables and IV found");
    let mut candidates = Candidates { contexts: vec![tables.context], ivs: vec![tables.iv], keys: tables.keys };
    if let Some(file) = file {
        let blocks = parse_secrets(&std::fs::read(file).with_context(|| format!("cannot read {}", file.display()))?);
        let before = candidates.keys.len();
        candidates.keys.extend(Candidates::from_blocks(blocks).keys);
        outln!("{}: {} key candidates", file.display(), candidates.keys.len() - before);
    }
    if let Some(dump) = dump {
        let found = keys_in_dump(dump, &tables.key_start, |done, total| {
            eprint!("\rsearching the dump: {} %", done * 100 / total.max(1));
        })
        .with_context(|| format!("cannot read {}", dump.display()))?;
        eprintln!();
        outln!("{}: {} key candidates", dump.display(), found.len());
        candidates.keys.extend(found);
        candidates.keys.sort();
        candidates.keys.dedup();
    }
    let archives = archives_under(game);
    if archives.is_empty() {
        bail!("no .rpf archives under {}", game.display());
    }
    let found = identify(&candidates, &archives)?;
    let path = KeyStore::default_path().context("no config folder")?;
    found.store.save(&path)?;
    outln!(
        "{} keys recognised; {} of {} encrypted archive tables open; stored in {} ({:.1} s)",
        found.store.keys.len(),
        found.tables.0,
        found.tables.1,
        path.display(),
        started.elapsed().as_secs_f32()
    );
    if !found.missing.is_empty() {
        outln!("Still unknown: key ids {:?} (their files stay closed)", found.missing);
    }
    Ok(())
}

/// The RDR2 game folder a path lies in (the folder with RDR2.exe).
fn rdr2_game_folder(path: &Path) -> Option<PathBuf> {
    let start = if path.is_dir() { path } else { path.parent()? };
    start.ancestors().find(|d| d.join("RDR2.exe").is_file()).map(Path::to_path_buf)
}

fn cmd_rdr2_ped(game: &Path, name: Option<&str>, outfit: Option<usize>, check: bool, load: bool) -> Result<()> {
    use openrage_app::metaped;
    if !game.join("RDR2.exe").is_file() {
        bail!("{} is not a Red Dead Redemption 2 folder", game.display());
    }
    let started = Instant::now();
    let peds = metaped::ped_list(game)?;
    let name = match name {
        Some(n) if !check => n,
        _ if check && load => return load_rdr2_peds(game, &peds, started),
        _ if check => return check_rdr2_peds(game, &peds, started),
        _ => {
            for p in &peds {
                outln!("{:<40} {}", p.name, p.base);
            }
            outln!("{} peds", peds.len());
            return Ok(());
        }
    };
    let finder = PedFinder::new(game)?;
    match peds.iter().find(|p| p.name.eq_ignore_ascii_case(name)) {
        Some(p) => outln!("{}: base skeleton {}", p.name, metaped::base_fragment(p)),
        None => outln!("{name}: not in the ped list"),
    }
    let outfits = metaped::load_outfits(game, &finder.index, name)?;
    outln!("{} outfits", outfits.len());
    for (k, o) in outfits.iter().enumerate().filter(|(k, _)| outfit.is_none_or(|n| n == *k)) {
        outln!("outfit {k}: {} parts, {} face shape values", o.assets.len(), o.expressions.len());
        for a in &o.assets {
            outln!("  {}", finder.model(a.drawable));
            outln!(
                "    albedo {}  normal {}  material {}",
                finder.texture(a.albedo),
                finder.texture(a.normal),
                rage_formats::names::value(a.material)
            );
            if a.palette != 0 {
                outln!("    palette {}  tints {:?}", finder.palette(a.palette), a.tints);
            }
        }
    }
    eprintln!("{:.2} s", started.elapsed().as_secs_f64());
    Ok(())
}

/// Finds the files of a ped's parts, for showing and checking them.
struct PedFinder {
    index: Arc<openrage_app::metaped::AssetIndex>,
    textures: Arc<openrage_app::rdr2::TextureIndex>,
    palettes: Vec<(u32, String)>,
}

impl PedFinder {
    fn new(game: &Path) -> Result<PedFinder> {
        use openrage_app::metaped;
        let textures = openrage_app::rdr2::texture_index(game)
            .context("no texture index yet: run `openrage rdr2-names <game folder>` first")?;
        let built = std::sync::atomic::AtomicBool::new(false);
        let index = metaped::index(game, |done, total| {
            built.store(true, std::sync::atomic::Ordering::Relaxed);
            if done % 500 == 0 || done == total {
                eprint!("\rindexing the peds' models: {done}/{total} dictionaries");
            }
        })
        .context("no RDR2 keys yet: run `openrage rdr2-keys` first")?;
        if built.into_inner() {
            eprintln!();
        }
        let palettes = metaped::palettes(game)?;
        Ok(PedFinder { index, textures, palettes })
    }

    /// The name of a hash (or the hash), and whether the file was found.
    fn show(hash: u32, found: bool) -> String {
        let name = rage_formats::names::value(hash);
        if found { name } else { format!("{name} (missing)") }
    }

    fn model(&self, hash: u32) -> String {
        match self.index.drawable(hash) {
            Some(d) if hash != 0 => {
                let file = d.file.chain.last().map(|h| rage_formats::names::value(*h)).unwrap_or_default();
                format!("{} (in {file}.ydd #{})", rage_formats::names::value(hash), d.index)
            }
            _ => Self::show(hash, hash == 0),
        }
    }

    fn texture(&self, hash: u32) -> String {
        if hash == 0 {
            return "-".into();
        }
        match self.textures.name_of(hash) {
            Some(n) => n.to_owned(),
            None => Self::show(hash, false),
        }
    }

    fn palette(&self, hash: u32) -> String {
        match self.palettes.iter().find(|(h, _)| *h == hash) {
            Some((_, n)) => n.clone(),
            None => Self::show(hash, false),
        }
    }
}

/// Puts every ped together (its first outfit) as the viewer does: parts,
/// skeleton and textures; reports what fails or stays untextured.
fn load_rdr2_peds(game: &Path, peds: &[openrage_app::metaped::Ped], started: Instant) -> Result<()> {
    use rayon::prelude::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    PedFinder::new(game)?; // builds the index first, once
    let done = AtomicUsize::new(0);
    let failed: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let (parts, shaders, untextured, no_skeleton) =
        (AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0));
    // A few at a time: a ped takes a few hundred MB while it is read.
    let pool = rayon::ThreadPoolBuilder::new().num_threads(4).build()?;
    pool.install(|| {
        peds.par_iter().for_each(|p| {
            match openrage_app::models::load_rdr2_ped(game, &p.name, 0) {
                Ok(m) => {
                    parts.fetch_add(m.parts.len(), Ordering::Relaxed);
                    if m.skeleton.is_none() {
                        no_skeleton.fetch_add(1, Ordering::Relaxed);
                    }
                    for (_, d) in &m.drawables {
                        for s in &d.shaders {
                            shaders.fetch_add(1, Ordering::Relaxed);
                            if s.base_texture().is_none_or(|t| m.texture(d, t).is_none()) {
                                untextured.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                }
                Err(e) => failed.lock().unwrap().push(format!("{}: {e:#}", p.name)),
            }
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            eprint!("\r{n}/{} peds", peds.len());
        })
    });
    eprintln!();
    let failed = failed.into_inner().unwrap();
    outln!(
        "{} peds put together, {} parts; {} of {} shaders without their colour texture; {} without a skeleton",
        peds.len() - failed.len(),
        parts.into_inner(),
        untextured.into_inner(),
        shaders.into_inner(),
        no_skeleton.into_inner()
    );
    outln!("{} failed", failed.len());
    for f in failed.iter().take(20) {
        outln!("  {f}");
    }
    eprintln!("{:.1} s", started.elapsed().as_secs_f64());
    Ok(())
}

/// Checks every ped: its outfit file, every part's model and textures, its palette.
fn check_rdr2_peds(game: &Path, peds: &[openrage_app::metaped::Ped], started: Instant) -> Result<()> {
    use openrage_app::metaped;
    use rage_formats::names::value;
    use std::collections::BTreeMap;
    let finder = PedFinder::new(game)?;
    let (outfit_files, models) = finder.index.counts();
    eprintln!("{outfit_files} outfit files, {models} models indexed ({:.1} s)", started.elapsed().as_secs_f64());
    let (mut outfits, mut parts) = (0usize, 0usize);
    // Problem → (count, example).
    let mut missing: BTreeMap<&'static str, (usize, String)> = BTreeMap::new();
    let mut note = |what: &'static str, example: String| {
        missing.entry(what).or_insert((0, example)).0 += 1;
    };
    for (n, p) in peds.iter().enumerate() {
        eprint!("\r{}/{} peds", n + 1, peds.len());
        let list = match metaped::load_outfits(game, &finder.index, &p.name) {
            Ok(l) => l,
            Err(e) => {
                note("outfit file", format!("{}: {e:#}", p.name));
                continue;
            }
        };
        if list.is_empty() {
            note("ped without outfits", p.name.clone());
        }
        outfits += list.len();
        for o in &list {
            for a in &o.assets {
                parts += 1;
                if a.drawable == 0 {
                    note("part without model", p.name.clone());
                } else if finder.index.drawable(a.drawable).is_none() {
                    note("model", format!("{} ({})", value(a.drawable), p.name));
                }
                for (hash, what) in [(a.albedo, "albedo texture"), (a.normal, "normal texture")] {
                    if hash != 0 && finder.textures.find_hash(hash).is_none() {
                        note(what, format!("{} ({})", value(hash), p.name));
                    }
                }
                if a.palette != 0 && !finder.palettes.iter().any(|(h, _)| *h == a.palette) {
                    note("palette", format!("{} ({})", value(a.palette), p.name));
                }
            }
        }
    }
    eprintln!();
    outln!("{} peds, {outfits} outfits, {parts} parts", peds.len());
    if missing.is_empty() {
        outln!("all parts found");
    }
    for (what, (count, example)) in &missing {
        outln!("missing {what}: {count} (e.g. {example})");
    }
    eprintln!("{:.1} s", started.elapsed().as_secs_f64());
    Ok(())
}

fn cmd_rdr2_names(game: &Path, import: Option<&Path>) -> Result<()> {
    use rage_rpf::rpf8_keys::archives_under;
    use rage_rpf::rpf8_names::{NameIndex, build, entry_hashes};
    let started = Instant::now();
    let keys = rdr2_keys()?.context("no RDR2 keys yet: run `openrage rdr2-keys` first")?;
    let files = archives_under(game);
    if files.is_empty() {
        bail!("no .rpf archives under {}", game.display());
    }
    let wanted = entry_hashes(&files, &keys);
    let path = NameIndex::default_path(game).context("no cache folder")?;
    let mut index = NameIndex::load(&path)?.unwrap_or_default();
    match import {
        Some(list) => {
            let text = std::fs::read_to_string(list).with_context(|| format!("cannot read {}", list.display()))?;
            let added = index.add_list(&text, &wanted);
            outln!("{}: {added} new names", list.display());
        }
        None => {
            let found = build(&files, &keys, &wanted, |done, total| {
                eprint!("\rsearching the game's files: {done}/{total} archives")
            });
            eprintln!();
            for (h, n) in found.names {
                let files = found.files.get(&h).copied().unwrap_or(1);
                index.files.entry(h).or_insert(files);
                index.names.entry(h).or_insert(n);
            }
        }
    }
    index.save(&path)?;
    if import.is_none() {
        let textures = openrage_app::rdr2::build_texture_index(game, &files, &keys, |done, total| {
            eprint!("\rindexing textures: {done}/{total} archives")
        });
        eprintln!();
        let tpath = openrage_app::rdr2::TextureIndex::default_path(game).context("no cache folder")?;
        textures.save(&tpath)?;
        outln!(
            "{} textures in {} dictionaries indexed; stored in {}",
            textures.textures.len(),
            textures.dictionaries.len(),
            tpath.display()
        );
    }
    outln!(
        "{} of {} entry names known ({:.1} %); stored in {} ({:.0} s)",
        index.trusted_count(),
        wanted.len(),
        index.trusted_count() as f64 * 100.0 / wanted.len().max(1) as f64,
        path.display(),
        started.elapsed().as_secs_f32()
    );
    Ok(())
}
