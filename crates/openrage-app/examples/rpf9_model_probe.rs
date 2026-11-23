//! Bounded local-corpus probe, with no recovery scan or archive extraction.
//! Run with the game directory as the sole argument. The reference executable
//! is discovered by Rpf9 in that directory's ancestors.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use rage_formats::drawable6;
use rage_rpf::{Rpf9, rpf9::Kind};

const MAX_RESOURCE: u64 = 32 << 20;
const MAX_SAMPLES: usize = 16;

fn main() -> Result<()> {
    let root = PathBuf::from(std::env::args_os().nth(1).context("usage: rpf9_model_probe <game-directory>")?);
    let rpf = Rpf9::open(&root.join("data.rpf"))?;
    ensure!(rpf.has_toc(), "no usable TOC: refusing to scan archive payloads");
    let mut totals = (0, 0);
    probe_models(&rpf, "data.rpf", &mut totals)?;
    let entries = rpf.entries()?;
    for (index, _) in entries
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e.kind, Kind::Toc { file_type: 0, resource: false, .. }))
        .take(10)
    {
        if totals.0 + totals.1 >= MAX_SAMPLES {
            break;
        }
        match rpf.open_child(index) {
            Ok(child) => probe_models(&child, &format!("data.rpf/{index}"), &mut totals)?,
            Err(error) => println!("CHILD_FAIL index={index} error={error}"),
        }
    }
    println!("MODEL_SUMMARY parsed={} failed={}", totals.0, totals.1);
    let textures = probe_textures(&root)?;
    ensure!(totals.0 > 0 || textures > 0, "neither sampled models nor textures decoded successfully");
    Ok(())
}

fn probe_models(rpf: &Rpf9, archive: &str, totals: &mut (usize, usize)) -> Result<()> {
    ensure!(rpf.has_toc(), "refusing recovery scan");
    let entries = rpf.entries()?;
    let candidates: Vec<_> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e.kind, Kind::Toc { resource: true, file_type: 2..=4, .. }) && e.size <= MAX_RESOURCE)
        .take(MAX_SAMPLES.saturating_sub(totals.0 + totals.1))
        .collect();
    println!(
        "archive={archive} entries={} samples={} max_resource_bytes={MAX_RESOURCE}",
        entries.len(),
        candidates.len()
    );
    for (index, entry) in candidates {
        let name = rpf.entry_name(entry);
        let result = (|| -> Result<(usize, usize, usize, usize, usize)> {
            let (system, graphics) = rpf.resource_parts(index)?.context("TOC entry has no resource parts")?;
            ensure!(system.len() as u64 + graphics.len() as u64 <= MAX_RESOURCE, "decoded resource exceeds limit");
            let drawables = drawable6::parse(&system, &graphics)?;
            let meshes: usize = drawables.iter().map(|d| d.meshes.len()).sum();
            let vertices: usize = drawables.iter().flat_map(|d| &d.meshes).map(|m| m.positions.len()).sum();
            ensure!(meshes > 0 && vertices > 0, "parser returned no renderable geometry");
            Ok((system.len(), graphics.len(), drawables.len(), meshes, vertices))
        })();
        match result {
            Ok((sys, gfx, drawables, meshes, vertices)) => {
                totals.0 += 1;
                println!(
                    "OK archive={archive} index={index} name={name:?} sys={sys} gfx={gfx} drawables={drawables} meshes={meshes} vertices={vertices}"
                );
            }
            Err(error) => {
                totals.1 += 1;
                println!(
                    "FAIL archive={archive} index={index} name={name:?} logical_bytes={} error={error:#}",
                    entry.size
                );
            }
        }
    }
    Ok(())
}

fn probe_textures(root: &Path) -> Result<usize> {
    let mut files = Vec::new();
    openrage_app::detect::collect_archives(&root.join("data/ttdb"), &["ttpf"], &mut files);
    files.sort_by_key(|p| std::fs::metadata(p).map_or(u64::MAX, |m| m.len()));
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
    let output = std::env::temp_dir().join(format!("openrage-rpf9-textures-{}-{stamp}", std::process::id()));
    let (mut attempted, mut decoded, mut pools) = (0, 0, 0);
    for file in files.into_iter().take(64) {
        if attempted >= MAX_SAMPLES {
            break;
        }
        pools += 1;
        let pool = match rage_formats::ttpf::Pool::open(&file) {
            Ok(pool) => pool,
            Err(error) => {
                println!("POOL_FAIL path={file:?} error={error}");
                continue;
            }
        };
        for index in 0..pool.len() {
            if attempted >= MAX_SAMPLES {
                break;
            }
            let Some(d) = pool.descriptor(index) else {
                continue;
            };
            if d.base.is_none() || d.dimension != 1 || d.depth != 1 || d.width > 512 || d.height > 512 {
                continue;
            }
            attempted += 1;
            let result = (|| -> Result<_> {
                let texture = pool.texture_with_limit(index, Some(512), MAX_RESOURCE as usize)?;
                ensure!(texture.width <= 512 && texture.height <= 512, "mip fallback exceeds preview dimensions");
                let rgba = texture.to_rgba().context("pixel decoder does not support this format")?;
                let png = rage_formats::png(texture.width, texture.height, &rgba)?;
                std::fs::create_dir_all(&output)?;
                let path = output.join(format!("pool{pools}-texture{index}.png"));
                std::fs::File::create_new(&path)?.write_all(&png)?;
                Ok((texture.width, texture.height, texture.format.name(), path))
            })();
            match result {
                Ok((width, height, format, path)) => {
                    decoded += 1;
                    println!(
                        "TEXTURE_OK pool={file:?} index={index} key={:016x} width={width} height={height} format={format} png={path:?}",
                        pool.keys()[index]
                    );
                }
                Err(error) => println!("TEXTURE_FAIL pool={file:?} index={index} error={error:#}"),
            }
        }
    }
    println!("TEXTURE_SUMMARY pools={pools} attempted={attempted} decoded={decoded} failed={}", attempted - decoded);
    Ok(decoded)
}
