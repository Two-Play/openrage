//! `openrage verify --rebuild` for Red Dead Redemption 2: the RPF8 writer
//! checked against the game's own archives.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Instant;

use rage_rpf::rpf8::Rpf8;
use rage_rpf::rpf8_editor::Rpf8Editor;

use crate::*;

/// Archives larger than this are not copied for the round trip (the copy
/// goes to the temporary folder); their nested archives are still checked.
const COPY_LIMIT: u64 = 512 << 20;

fn digest(b: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    b.hash(&mut h);
    h.finish()
}

fn file_digest(path: &Path) -> Result<u64> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut h = DefaultHasher::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            return Ok(h.finish());
        }
        buf[..n].hash(&mut h);
    }
}

#[derive(Default)]
struct Tally {
    archives: usize,
    entries: usize,
    /// Texture dictionaries whose textures were written in place and read back.
    dictionaries: usize,
    textures: usize,
    skipped: Vec<String>,
    failed: Vec<String>,
    /// Compressor field of stored `RSC8` headers by the entry's compression (0xFFFF: no `RSC8` magic).
    rsc8_fields: std::collections::BTreeMap<(String, u32), usize>,
}

/// What every entry reads as: its decoded bytes' digest, or the read error.
fn contents(rpf: &Rpf8) -> Vec<(u32, u8, std::result::Result<u64, String>)> {
    rpf.entries()
        .unwrap_or_default()
        .iter()
        .map(|e| (e.hash, e.ext_id, rpf.read(e).map(|d| digest(&d)).map_err(|x| x.to_string())))
        .collect()
}

/// Edits a copy of an archive (replace a file, re-store a resource from its
/// exported RSC8 file, add a file, so the table grows), checks that every
/// entry reads back as it should, then undoes the edit and checks that the
/// file is byte for byte the original again.
fn round_trip(path: &Path, label: &str, tally: &mut Tally) -> Result<()> {
    let keys = rage_rpf::rpf8::default_keys();
    let before = Rpf8::open_with_keys(path, keys.clone())?;
    let Some(entries) = before.entries() else {
        tally.skipped.push(format!("{label}: entry table encrypted with a key that is not loaded"));
        return Ok(());
    };
    for e in entries.iter().filter(|e| e.is_resource) {
        if let Ok(head) = before.read_stored_head(e, 16)
            && head.len() == 16
        {
            let magic = u32::from_le_bytes(head[0..4].try_into().unwrap_or_default());
            let info = u32::from_le_bytes(head[4..8].try_into().unwrap_or_default());
            let field = if magic == rage_rpf::rpf8::RSC8_MAGIC { info >> 8 & 0x1F } else { 0xFFFF };
            let kind = format!("{:?}{}", e.compression, if e.is_encrypted() { " encrypted" } else { "" });
            *tally.rsc8_fields.entry((kind, field)).or_default() += 1;
        }
    }
    textures(&before, label, tally);
    let original = contents(&before);
    let original_file = file_digest(path)?;
    let readable = |i: usize| original[i].2.is_ok();

    // Pick what to change: the first readable plain file, the first readable resource.
    let plain = (0..entries.len()).find(|&i| readable(i) && !entries[i].is_resource && entries[i].ext_id != 0);
    let resource = (0..entries.len()).find(|&i| readable(i) && entries[i].is_resource);
    let mut ed = Rpf8Editor::open(path, keys.clone(), None)?;
    let name = |i: usize| entries[i].display_name(before.platform());
    let new_plain = plain.map(|i| {
        let mut d = before.read(&entries[i]).unwrap_or_default();
        d.extend(b"\n-- changed by openrage verify --rebuild --\n");
        d
    });
    if let (Some(i), Some(d)) = (plain, &new_plain) {
        ed.set(&name(i), d.clone())?;
    }
    if let Some(i) = resource {
        ed.set(&name(i), before.resource_file(&entries[i])?)?;
    }
    let added = b"openrage verify --rebuild".to_vec();
    let added_name = "openrage_verify/added.txt";
    // Several new entries: the table grows over the first entries' data.
    for k in 0..8 {
        ed.set(&format!("{added_name}{k}"), added.clone())?;
    }
    let mut backup = None;
    ed.commit(&mut |b| {
        backup = Some(b.clone());
        Ok(())
    })?;
    let backup = backup.context("no backup")?;

    let after = Rpf8::open_with_keys(path, keys)?;
    let problems = (|| -> std::result::Result<(), String> {
        if after.header().is_encrypted() {
            return Err("the table was not written in plain text".into());
        }
        let new_entries = after.entries().ok_or("table unreadable")?;
        if new_entries.len() != entries.len() + 8 {
            return Err(format!("{} entries, expected {}", new_entries.len(), entries.len() + 8));
        }
        if !new_entries.windows(2).all(|w| w[0].hash <= w[1].hash) {
            return Err("entries not sorted by hash".into());
        }
        for (i, (hash, ext, want)) in original.iter().enumerate() {
            let e = new_entries
                .iter()
                .find(|e| e.hash == *hash && e.ext_id == *ext)
                .ok_or_else(|| format!("{} lost", name(i)))?;
            let got = after.read(e).map(|d| digest(&d)).map_err(|x| x.to_string());
            let want = match (plain, &new_plain) {
                (Some(p), Some(d)) if p == i => Ok(digest(d)),
                _ => want.clone(),
            };
            if got != want {
                return Err(format!("{} reads differently ({got:?} vs {want:?})", name(i)));
            }
            if resource == Some(i) && (e.size_or_flags != entries[i].size_or_flags || !e.is_resource) {
                return Err(format!("{}: resource flags changed", name(i)));
            }
        }
        let (h, x) = rage_rpf::rpf8::path_hash(&format!("{added_name}0"), after.platform());
        let e = new_entries.iter().find(|e| e.hash == h && e.ext_id == x).ok_or("added entry missing")?;
        if after.read(e).map_err(|x| x.to_string())? != added {
            return Err("added entry reads differently".into());
        }
        Ok(())
    })();
    backup.restore(path)?;
    let restored = file_digest(path)? == original_file;
    tally.archives += 1;
    tally.entries += entries.len();
    match (problems, restored) {
        (Ok(()), true) => {}
        (Err(p), _) => tally.failed.push(format!("{label}: {p}")),
        (Ok(()), false) => tally.failed.push(format!("{label}: undo did not restore the original bytes")),
    }
    Ok(())
}

/// Writes every texture of every dictionary in the archive in place (first
/// its own pixels, which must give the same resource back, then changed
/// pixels for the first texture, which must read back as written and leave
/// the others alone).
fn textures(rpf: &Rpf8, label: &str, tally: &mut Tally) {
    use rage_formats::ytd_write::{Edit, apply_rdr2};
    let Some(entries) = rpf.entries() else { return };
    for (i, e) in entries.iter().enumerate() {
        if !e.is_resource || rage_rpf::rpf8::extension(e.ext_id, rpf.platform()).as_deref() != Some("ytd") {
            continue;
        }
        let Ok(res) = rpf.read_resource(e) else { continue };
        let Ok(dict) = rage_formats::ytd::parse_rdr2(&res) else { continue };
        if dict.layout != rage_formats::ytd::Layout::Rdr2 || !dict.errors.is_empty() || dict.textures.is_empty() {
            continue;
        }
        let name = format!("{label}/{}", rpf.entry_name(i));
        let result = (|| -> std::result::Result<(), String> {
            let same: Vec<Edit> = dict.textures.iter().cloned().map(Edit::Set).collect();
            let back = apply_rdr2(&res, &dict, &same).map_err(|x| x.to_string())?;
            if back.system != res.system || back.graphics != res.graphics {
                return Err("writing the textures' own pixels changed the resource".into());
            }
            let mut changed = dict.textures[0].clone();
            changed.data.iter_mut().enumerate().for_each(|(k, b)| *b = !*b ^ k as u8);
            let out = apply_rdr2(&res, &dict, &[Edit::Set(changed.clone())]).map_err(|x| x.to_string())?;
            let reread = rage_formats::ytd::parse_rdr2(&out).map_err(|x| x.to_string())?;
            if reread.textures[0].data != changed.data {
                return Err(format!("{}: changed pixels read back differently", changed.name));
            }
            if reread.textures[1..].iter().zip(&dict.textures[1..]).any(|(a, b)| a.data != b.data) {
                return Err(format!("changing {} changed other textures", changed.name));
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                tally.dictionaries += 1;
                tally.textures += dict.textures.len();
            }
            Err(x) => tally.failed.push(format!("{name}: {x}")),
        }
    }
}

/// Round-trips every nested archive below `rpf`, depth first.
fn nested(rpf: &Rpf8, prefix: &str, scratch: &Path, tally: &mut Tally) {
    let Some(entries) = rpf.entries() else { return };
    for (i, e) in entries.iter().enumerate() {
        if e.ext_id != 0 {
            continue;
        }
        let label = format!("{prefix}/{}", rpf.entry_name(i));
        match rpf.open_child(e) {
            Ok(child) => {
                nested(&child, &label, scratch, tally);
                if e.size_on_disk > COPY_LIMIT {
                    tally.skipped.push(format!("{label}: {} MB", e.size_on_disk >> 20));
                    continue;
                }
                let p = scratch.join(format!("nested{}.rpf", child_depth(&label)));
                let result = rpf
                    .read(e)
                    .map_err(anyhow::Error::from)
                    .and_then(|bytes| Ok(std::fs::write(&p, bytes)?))
                    .and_then(|()| round_trip(&p, &label, tally));
                if let Err(x) = result {
                    tally.failed.push(format!("{label}: {x:#}"));
                }
                let _ = std::fs::remove_file(&p);
            }
            Err(x) => tally.skipped.push(format!("{label}: {x}")),
        }
    }
}

/// How deep a nested archive is (its file name in the scratch folder, so a
/// parent's copy is never overwritten by its child's).
fn child_depth(label: &str) -> usize {
    label.matches(".rpf/").count()
}

pub(crate) fn cmd_verify_rdr2_rebuild(game_dir: &Path, within: &str) -> Result<()> {
    let root = game_dir.join(within.trim_start_matches('/'));
    let mut found = Vec::new();
    if root.is_file() {
        found.push(root);
    } else {
        fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
            let Ok(rd) = std::fs::read_dir(dir) else { return };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    collect(&p, out);
                } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("rpf")) {
                    out.push(p);
                }
            }
        }
        collect(&root, &mut found);
    }
    found.sort();
    let scratch = std::env::temp_dir().join(format!("openrage-verify-rdr2-{}", std::process::id()));
    std::fs::create_dir_all(&scratch)?;
    let started = Instant::now();
    let mut tally = Tally::default();
    for path in found {
        let rel = path.strip_prefix(game_dir).unwrap_or(&path).to_string_lossy().replace('\\', "/");
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if size == 0 || !Rpf8::file_is_rpf8(&path) {
            continue;
        }
        let rpf = match Rpf8::open_default(&path) {
            Ok(r) => r,
            Err(e) => {
                tally.failed.push(format!("{rel}: {e}"));
                continue;
            }
        };
        nested(&rpf, &rel, &scratch, &mut tally);
        if size <= COPY_LIMIT {
            let copy = scratch.join("top.rpf");
            std::fs::copy(&path, &copy)?;
            if let Err(e) = round_trip(&copy, &rel, &mut tally) {
                tally.failed.push(format!("{rel}: {e:#}"));
            }
            let _ = std::fs::remove_file(&copy);
        } else {
            tally.skipped.push(format!("{rel}: {} MB, only its nested archives are checked", size >> 20));
        }
        outln!("{rel}: {} archives so far, {} failed", tally.archives, tally.failed.len());
    }
    let _ = std::fs::remove_dir_all(&scratch);
    for s in &tally.skipped {
        outln!("skipped {s}");
    }
    for f in &tally.failed {
        outln!("FAIL {f}");
    }
    let fields: Vec<String> =
        tally.rsc8_fields.iter().map(|((c, f), n)| format!("{c}: compressor field {f:#x} ×{n}")).collect();
    outln!("RSC8 headers of resource entries: {}", fields.join(", "));
    outln!(
        "{} archives edited and undone ({} entries), {} texture dictionaries ({} textures) written in place, \
         {} failed, {} skipped ({:.1}s)",
        tally.archives,
        tally.entries,
        tally.dictionaries,
        tally.textures,
        tally.failed.len(),
        tally.skipped.len(),
        started.elapsed().as_secs_f32()
    );
    if !tally.failed.is_empty() {
        bail!("{} archives failed", tally.failed.len());
    }
    Ok(())
}
