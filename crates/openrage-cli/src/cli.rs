//! Command-line arguments (clap).

use crate::*;

#[derive(Parser)]
#[command(name = "openrage", version, about = "Open-source explorer for RAGE engine game files")]
pub(crate) struct Cli {
    /// Game installation directory (remembered after `openrage keys`).
    #[arg(long, global = true, env = "OPENRAGE_GAME_DIR")]
    pub(crate) game: Option<PathBuf>,

    /// Print results as JSON instead of text (ls, search, history).
    #[arg(long, global = true)]
    pub(crate) json: bool,

    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Subcommand)]
pub(crate) enum Command {
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
        /// Instead of archives, read every GTA IV cutscene (.cut): its sections, the clips of
        /// its actors and camera in the animation dictionaries next to it, and its models.
        #[arg(long, conflicts_with_all = ["textures", "models", "audio", "animations", "meta", "text", "particles", "collisions", "paths", "scripts", "media"])]
        cutscenes: bool,
        /// With --textures: also rebuild every dictionary and check that it reads back identically.
        /// With --models: write every GTA V Enhanced drawable (.ydr, .yft, .ydd) anew with its own
        /// meshes and check that it reads back identically.
        /// With --collisions: write every GTA V Enhanced collision (.ybn, and those of .ydr) anew
        /// from its own triangles and check that it reads back the same.
        /// With --meta: convert every PSO file (.ymt, .ymf, .pso, …) to XML and back, and check
        /// that it comes out byte for byte the same or at least reads back to the same XML.
        /// GTA IV without --textures: rebuild the table of every .rpf package in memory and
        /// check that it matches the stored one byte for byte (nothing is written).
        /// RDR2 without --textures: edit a temporary copy of every archive (top-level ones up to
        /// 512 MB, and every nested one) with the RPF8 writer, check that every entry reads back
        /// as it should, undo the edit and check that the copy is the original again.
        #[arg(long)]
        rebuild: bool,
        /// With --models: write every model (.ydr, .yft, .ydd) and texture dictionary (.ytd) as
        /// OpenIV openFormats text and read it back, checking that nothing changes; GTA V
        /// Enhanced drawables and fragments are also imported back onto themselves (meshes
        /// written anew) and must read back as the original.
        #[arg(long, requires = "models")]
        openformats: bool,
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
        #[arg(short = 'o', long, visible_alias = "out")]
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
        /// With --meta: convert every PSO file to XML and back, and check that it comes out the
        /// same bytes or at least reads back to the same XML.
        #[arg(long, requires = "meta")]
        rebuild: bool,
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
    /// Show what a particle dictionary (.ypt; GTA IV .wpfl effect list) contains: effects, emitters, textures, models.
    Particles {
        /// Virtual path of the .ypt or .wpfl.
        path: String,
    },
    /// Show a text table (.gxt2 as OpenIV .oxt text, .nametable one name per line; GTA IV .gxt) or a GTA IV placement file (.wpl).
    Text {
        /// Virtual path of the file.
        path: String,
        /// Write the text to this file instead of printing it.
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// GTA IV placement file (.wpl): read this edited text and write the file back
        /// (update folder, or the game files when chosen; undo with `openrage undo`).
        #[arg(long, value_name = "TEXT", conflicts_with = "out")]
        import: Option<PathBuf>,
    },
    /// Replace the meshes of a GTA V Enhanced drawable (.ydr), fragment (.yft: vehicles, their
    /// mod parts) or drawable dictionary (.ydd: peds, clothing) with a Wavefront OBJ model. The
    /// model's shaders and textures stay; OBJ materials go to its shaders in order. Vehicles: OBJ
    /// parts named like a bone (`door_dside_f`, `bonnet`) move with it, parts named after the
    /// wheel (`wheel_lf`) replace its model, other parts move with the body. Dictionaries: parts
    /// named after a drawable (`uppr_000_u`) replace it. Models blended between bones take the
    /// weights of the original's nearest vertex. A collision file (.ybn) gets the OBJ as its
    /// collision.
    ReplaceModel {
        /// The model in the game, e.g. `x64i.rpf/levels/gta5/props/…/prop_x.ydr` or
        /// `x64e.rpf/levels/gta5/vehicles.rpf/adder.yft`.
        path: String,
        /// The OBJ file.
        obj: PathBuf,
        /// Write the new drawable to this file.
        #[arg(short, long, required_unless_present = "mods")]
        out: Option<PathBuf>,
        /// Put the new drawable into the mods folder in place of the original (can be undone).
        #[arg(long, conflicts_with = "out")]
        mods: bool,
        /// Put every OBJ material on this shader of the drawable.
        #[arg(long)]
        shader: Option<u16>,
        /// Further detail levels, from closest to furthest (up to three OBJ files).
        #[arg(long, num_args = 1)]
        lod: Vec<PathBuf>,
        /// Keep the drawable's textures instead of embedding the ones the OBJ's materials name.
        #[arg(long)]
        no_textures: bool,
        /// Drawable dictionaries (.ydd): the drawable (name or index) for the OBJ's parts that
        /// are named after none.
        #[arg(long)]
        drawable: Option<String>,
        /// Drawables (.ydr): build the collision from this OBJ model instead of keeping the
        /// original (OBJ materials named like a collision material, e.g. CONCRETE, get it).
        #[arg(long, value_name = "OBJ")]
        collision: Option<PathBuf>,
    },
    /// Export a model or texture dictionary as OpenIV openFormats text files: .ydr → .odr,
    /// .yft → .oft, .ydd → .odd, .ytd → .otd, with meshes (.mesh), skeletons (.skel) and
    /// textures (.otx + .dds) in a folder named after the file.
    ExportOpenformats {
        /// The file in the game, e.g. `x64i.rpf/levels/gta5/props/…/prop_x.ydr`.
        path: String,
        /// Folder to write into.
        #[arg(short, long, default_value = ".")]
        out: PathBuf,
    },
    /// Import openFormats files (.odr, .oft, .otd) onto the game file they belong to: its
    /// meshes, detail levels and embedded textures (dictionaries: textures) are replaced, its
    /// shaders, skeleton and physics stay. GTA V Enhanced models; texture dictionaries of both.
    ImportOpenformats {
        /// The file in the game, e.g. `x64i.rpf/levels/gta5/props/…/prop_x.ydr`.
        path: String,
        /// The main openFormats file (`prop_x.odr`).
        file: PathBuf,
        /// Write the new game file to this file.
        #[arg(short, long, required_unless_present = "mods")]
        out: Option<PathBuf>,
        /// Put the new file into the mods folder in place of the original (can be undone).
        #[arg(long, conflicts_with = "out")]
        mods: bool,
    },
    /// Show a model (.ydr, .yft, .ydd; GTA IV .wdr, .wft, .wdd) and optionally export it as glTF (.glb).
    Model {
        /// Virtual path of the model.
        path: String,
        /// Write the model (the first drawable of a .ydd) as .glb.
        #[arg(short = 'o', long, visible_alias = "out")]
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
    /// List the clips of a clip dictionary (.ycd; GTA IV .wad).
    Clips {
        /// Virtual path of the .ycd file.
        path: String,
        /// Print the values of every track of this clip at a few frames.
        #[arg(long)]
        clip: Option<String>,
    },
    /// Show a GTA IV cutscene (.cut): its sections, cast with their models and clips,
    /// and subtitles.
    Cutscene {
        /// Virtual path of the .cut file.
        path: String,
        /// Also load every actor's model with its textures, as the scene window does, and time it.
        #[arg(long)]
        models: bool,
        /// Also load the map objects around the cutscene (within this many metres), as the
        /// scene window does, and time it.
        #[arg(long)]
        map: Option<f32>,
    },
    /// List the sounds of an audio container (.awc) and optionally export them as WAV.
    Audio {
        /// Virtual path of the .awc file.
        path: String,
        /// Export every sound into this folder as .wav.
        #[arg(short = 'o', long, visible_alias = "out")]
        export: Option<PathBuf>,
    },
    /// List the textures of a texture dictionary (.ytd) and optionally export them.
    Textures {
        /// Virtual path of the .ytd file.
        path: String,
        /// Export all textures into this folder (DDS, like OpenIV).
        #[arg(short = 'o', long, visible_alias = "out")]
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
    /// GTA V: ASI mods (story mode only), their loader, and on Linux the Proton
    /// setting that lets the loader run. Without a command: what is installed
    /// and why mods would not load.
    Asi {
        #[command(subcommand)]
        cmd: Option<AsiCommand>,
    },
    /// Plugins: list, enable or disable them, preview files and run their actions.
    #[command(subcommand)]
    Plugin(PluginCommand),
    /// Show the changes made to the mods folder, newest first.
    History,
    /// Undo the newest change to the mods folder.
    Undo,
    /// GTA IV: where changes go, `update` (the update folder, default) or
    /// `game` (the game's own files, originals kept). Without a value: show it.
    WriteTarget { target: Option<String> },
    /// GTA IV: game files changed in place whose originals are kept.
    Originals {
        /// Put the original of this file back (path relative to the game folder).
        #[arg(long, value_name = "PATH")]
        restore: Option<String>,
        /// Put every kept original back.
        #[arg(long, conflicts_with = "restore")]
        restore_all: bool,
        /// Restore without asking (the changes leave the history).
        #[arg(short, long)]
        yes: bool,
    },
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
pub(crate) enum PluginCommand {
    /// List the installed plugins and where they are looked for.
    List,
    /// Show what a plugin adds and which permissions it asks for.
    Info { id: String },
    /// Enable a plugin, granting the permissions it asks for.
    Enable {
        id: String,
        /// Grant without asking.
        #[arg(short, long)]
        yes: bool,
    },
    /// Disable a plugin.
    Disable { id: String },
    /// Install a plugin package (.orp) into your plugin folder (it stays off
    /// until you enable it; an update keeps your choice).
    Install {
        file: PathBuf,
        /// Install without asking.
        #[arg(short, long)]
        yes: bool,
    },
    /// Remove a plugin installed into your plugin folder.
    Uninstall {
        id: String,
        /// Remove without asking.
        #[arg(short, long)]
        yes: bool,
    },
    /// Pack a plugin folder (plugin.toml, the .wasm, README/LICENSE) into a package (.orp).
    Pack {
        dir: PathBuf,
        /// Where to write the package (default: <id>-<version>.orp).
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// The built component, if it is not in the folder (e.g. target/wasm32-wasip2/release/x.wasm).
        #[arg(long)]
        wasm: Option<PathBuf>,
    },
    /// Show a file the way a plugin previews it.
    Preview {
        /// Virtual path in the game (or a file on disk with --file).
        path: String,
        /// The plugin to use (default: the one that previews this file type).
        #[arg(long)]
        plugin: Option<String>,
        /// Read PATH from disk instead of from the game.
        #[arg(long)]
        file: bool,
        /// Save an image preview as PNG.
        #[arg(long)]
        png: Option<PathBuf>,
        /// Save a 3D model preview as glTF (.glb).
        #[arg(long)]
        glb: Option<PathBuf>,
    },
    /// Run one of a plugin's tools (the windows of the Tools menu) and print
    /// its result; without --set, its fields keep their starting values.
    Tool {
        /// The plugin.
        id: String,
        /// The tool (as listed by `openrage plugin info`).
        tool: String,
        /// A field's value, e.g. --set value=0x1F; repeatable.
        #[arg(long = "set", value_name = "FIELD=VALUE")]
        set: Vec<String>,
    },
    /// Run a plugin's action on files (or, for batch actions, folders and
    /// archives), save what it produces and apply its changes to the mods folder.
    Run {
        /// The plugin.
        id: String,
        /// The action (as listed by `openrage plugin info`).
        action: String,
        /// Virtual paths in the game (or files on disk with --file).
        #[arg(required = true)]
        paths: Vec<String>,
        /// Read the PATHS from disk instead of from the game.
        #[arg(long)]
        file: bool,
        /// Files from disk for actions that ask for them (e.g. a patch); repeatable.
        #[arg(long)]
        pick: Vec<PathBuf>,
        /// Folder to save the results in (needed when the action produces files).
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Apply changes to the mods folder without asking.
        #[arg(short, long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum AsiCommand {
    /// Put a disabled ASI mod back into the game folder.
    Enable { name: String },
    /// Put an ASI mod aside so the loader skips it (nothing is deleted).
    Disable { name: String },
    /// Copy an .asi file into the game folder (recorded in the history: `openrage undo` removes it).
    Install { file: PathBuf },
    /// Linux: let Proton load the game's ASI loader instead of Wine's own DLL
    /// (`on`), or take that back (`off`). The game must not be running.
    Proton {
        #[arg(value_parser = ["on", "off"])]
        state: String,
    },
}

#[derive(Subcommand)]
pub(crate) enum OivCommand {
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
    Uninstall {
        name: String,
        /// Uninstall without asking.
        #[arg(short, long)]
        yes: bool,
    },
}
