//! Plugin packages (`.orp`): a ZIP file with `plugin.toml`, the component
//! and any other files of the plugin (README, licence), at the top level or
//! in one folder.
//!
//! Packages are untrusted: paths must stay inside the plugin folder, sizes
//! are limited, and installing never runs anything. A new plugin is off
//! until the user enables it.

use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use crate::manifest::{self, Manifest};

/// The extension of plugin packages.
pub const EXTENSION: &str = "orp";

/// Most files in a package.
const MAX_FILES: usize = 1000;
/// Most bytes all files of a package may take once unpacked.
const MAX_UNPACKED: u64 = 256 << 20;

/// A package, read and checked, ready to install.
#[derive(Debug, Clone)]
pub struct Package {
    pub manifest: Manifest,
    /// SHA-256 of the package file, in hex, to compare with what the author
    /// publishes.
    pub sha256: String,
    /// Size of the package file.
    pub size: u64,
    /// The plugin's files: path inside the plugin folder, content.
    pub files: Vec<(String, Vec<u8>)>,
}

impl Package {
    /// Reads and checks a package.
    pub fn open(path: &Path) -> Result<Self> {
        let data = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        Self::from_bytes(&data).with_context(|| format!("{} is not a valid plugin package", path.display()))
    }

    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        let sha256 = hex(&Sha256::digest(data));
        let mut zip = zip::ZipArchive::new(Cursor::new(data)).context("not a ZIP file")?;
        if zip.len() > MAX_FILES {
            bail!("it holds {} files, more than {MAX_FILES}", zip.len());
        }
        let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
        let mut total = 0u64;
        for i in 0..zip.len() {
            let mut file = zip.by_index(i)?;
            if file.is_dir() {
                continue;
            }
            let name = file.name()?.replace('\\', "/");
            if manifest::safe_relative(&name).is_none() {
                bail!("it contains a file named {name:?}, which is not allowed");
            }
            // The declared size cannot be trusted: read at most what is left.
            let left = MAX_UNPACKED - total;
            let mut content = Vec::new();
            (&mut file).take(left + 1).read_to_end(&mut content)?;
            total += content.len() as u64;
            if total > MAX_UNPACKED {
                bail!("it unpacks to more than {} MB", MAX_UNPACKED >> 20);
            }
            entries.push((name, content));
        }
        // `plugin.toml` at the top, or inside a single folder.
        let prefix = match entries.iter().find(|(n, _)| n == manifest::FILE_NAME) {
            Some(_) => String::new(),
            None => {
                let tops: Vec<&str> = entries.iter().filter_map(|(n, _)| n.split_once('/').map(|(t, _)| t)).collect();
                let top = tops.first().copied().context("it has no plugin.toml")?;
                if tops.iter().any(|t| *t != top) || entries.iter().any(|(n, _)| !n.contains('/')) {
                    bail!("it has no plugin.toml at the top or in a single folder");
                }
                format!("{top}/")
            }
        };
        let files: Vec<(String, Vec<u8>)> = entries
            .into_iter()
            .filter_map(|(n, c)| n.strip_prefix(&prefix).map(|rest| (rest.to_owned(), c)))
            .filter(|(n, _)| !n.is_empty())
            .collect();
        let toml = files
            .iter()
            .find(|(n, _)| n == manifest::FILE_NAME)
            .map(|(_, c)| String::from_utf8_lossy(c).into_owned())
            .context("it has no plugin.toml")?;
        let manifest = Manifest::parse(&toml).context("in plugin.toml")?;
        let wasm = files.iter().find(|(n, _)| n.eq_ignore_ascii_case(&manifest.wasm));
        match wasm {
            None => bail!("it does not contain {}, which plugin.toml names", manifest.wasm),
            Some((_, c)) if !c.starts_with(b"\0asm") => bail!("{} is not WebAssembly", manifest.wasm),
            Some(_) => {}
        }
        Ok(Package { manifest, sha256, size: data.len() as u64, files })
    }

    /// Installs into `<root>/<id>`, replacing an installed version. The
    /// plugin folder is written in full first and swapped in at the end, so
    /// a failure leaves the old version as it was.
    pub fn install(&self, root: &Path) -> Result<PathBuf> {
        fs::create_dir_all(root).with_context(|| format!("creating {}", root.display()))?;
        let id = &self.manifest.id;
        let target = root.join(id);
        let staging = root.join(format!(".{id}.installing"));
        let old = root.join(format!(".{id}.old"));
        let _ = fs::remove_dir_all(&staging);
        let _ = fs::remove_dir_all(&old);
        for (name, content) in &self.files {
            let rel = manifest::safe_relative(name).context("bad file name")?;
            let path = staging.join(rel);
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir)?;
            }
            fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;
        }
        if target.exists() {
            fs::rename(&target, &old).with_context(|| format!("moving the old {} aside", target.display()))?;
        }
        if let Err(e) = fs::rename(&staging, &target) {
            // Put the old version back.
            let _ = fs::rename(&old, &target);
            return Err(e).with_context(|| format!("installing into {}", target.display()));
        }
        let _ = fs::remove_dir_all(&old);
        Ok(target)
    }
}

/// Removes an installed plugin folder; only folders directly inside `root`
/// that hold a `plugin.toml` with this id.
pub fn uninstall(root: &Path, id: &str) -> Result<PathBuf> {
    let target = root.join(id);
    let installed = Manifest::load(&target).with_context(|| format!("{id} is not installed in {}", root.display()))?;
    if installed.id != id {
        bail!("{} holds plugin {:?}, not {id:?}", target.display(), installed.id);
    }
    fs::remove_dir_all(&target).with_context(|| format!("removing {}", target.display()))?;
    Ok(target)
}

/// Packs a plugin folder into a package: `plugin.toml`, the component and
/// the files authors usually ship (README, LICENSE, CHANGELOG, NOTICE). The
/// component is read from `wasm` if given (e.g. from Cargo's `target`
/// folder), else from the folder as `plugin.toml` names it.
pub fn pack(dir: &Path, wasm: Option<&Path>) -> Result<Vec<u8>> {
    let manifest = Manifest::load(dir)?;
    let wasm_rel = manifest::safe_relative(&manifest.wasm).context("bad wasm path")?;
    let wasm_path = wasm.map_or_else(|| dir.join(&wasm_rel), Path::to_path_buf);
    let wasm =
        fs::read(&wasm_path).with_context(|| format!("reading {} (build the plugin first)", wasm_path.display()))?;
    let mut files = vec![(manifest::FILE_NAME.to_owned(), fs::read(dir.join(manifest::FILE_NAME))?)];
    files.push((manifest.wasm.replace('\\', "/"), wasm));
    let mut extra: Vec<PathBuf> = fs::read_dir(dir)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            let name = p.file_name().map(|n| n.to_string_lossy().to_ascii_uppercase()).unwrap_or_default();
            ["README", "LICENSE", "LICENCE", "CHANGELOG", "NOTICE", "COPYING"].iter().any(|k| name.starts_with(k))
        })
        .collect();
    extra.sort();
    for p in extra {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        files.push((name, fs::read(&p)?));
    }
    let mut out = Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut out);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            // A fixed time, so the same files give the same package (and hash).
            .last_modified_time(zip::DateTime::default());
        for (name, content) in &files {
            zip.start_file(name.as_str(), options)?;
            zip.write_all(content)?;
        }
        zip.finish()?;
    }
    let data = out.into_inner();
    // What we write must be what we accept.
    Package::from_bytes(&data).context("the packed plugin does not check out")?;
    Ok(data)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOML: &str = "id = \"demo\"\nname = \"Demo\"\nversion = \"1.0.0\"\napi = \"API\"\nwasm = \"demo.wasm\"\n";

    fn toml() -> String {
        TOML.replace("API", manifest::API_VERSION)
    }

    fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        let mut z = zip::ZipWriter::new(&mut out);
        for (name, content) in files {
            z.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(content).unwrap();
        }
        z.finish().unwrap();
        out.into_inner()
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("openrage-orp-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn reads_packages_flat_or_in_a_folder() {
        let t = toml();
        let flat =
            Package::from_bytes(&zip(&[("plugin.toml", t.as_bytes()), ("demo.wasm", b"\0asm\x01\0\0\0")])).unwrap();
        assert_eq!(flat.manifest.id, "demo");
        assert_eq!(flat.sha256.len(), 64);
        let nested = Package::from_bytes(&zip(&[
            ("demo-1.0/plugin.toml", t.as_bytes()),
            ("demo-1.0/demo.wasm", b"\0asm\x01\0\0\0"),
            ("demo-1.0/README.md", b"hi"),
        ]))
        .unwrap();
        let names: Vec<_> = nested.files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["plugin.toml", "demo.wasm", "README.md"]);
    }

    #[test]
    fn rejects_bad_packages() {
        let t = toml();
        let wasm: &[u8] = b"\0asm\x01\0\0\0";
        let bad: Vec<Vec<u8>> = vec![
            b"not a zip".to_vec(),
            zip(&[("demo.wasm", wasm)]),
            zip(&[("plugin.toml", t.as_bytes())]),
            zip(&[("plugin.toml", t.as_bytes()), ("demo.wasm", b"MZ")]),
            zip(&[("plugin.toml", t.as_bytes()), ("demo.wasm", wasm), ("../evil.txt", b"x")]),
            zip(&[("a/plugin.toml", t.as_bytes()), ("b/demo.wasm", wasm)]),
            zip(&[("plugin.toml", b"id = 1"), ("demo.wasm", wasm)]),
        ];
        for (i, data) in bad.iter().enumerate() {
            assert!(Package::from_bytes(data).is_err(), "package {i} was accepted");
        }
        // A file that unpacks to more than the limit (highly compressible).
        let big = vec![0u8; (MAX_UNPACKED + 1) as usize];
        let mut out = Cursor::new(Vec::new());
        let mut z = zip::ZipWriter::new(&mut out);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        z.start_file("plugin.toml", opts).unwrap();
        z.write_all(t.as_bytes()).unwrap();
        z.start_file("demo.wasm", opts).unwrap();
        z.write_all(&big).unwrap();
        z.finish().unwrap();
        let e = Package::from_bytes(&out.into_inner()).unwrap_err();
        assert!(e.to_string().contains("unpacks to more than"), "{e}");
    }

    #[test]
    fn packs_installs_updates_and_uninstalls() {
        let src = temp("src");
        fs::write(src.join("plugin.toml"), toml()).unwrap();
        fs::write(src.join("demo.wasm"), b"\0asm\x01\0\0\0").unwrap();
        fs::write(src.join("LICENSE"), "MIT").unwrap();
        fs::write(src.join("notes.txt"), "not shipped").unwrap();
        let data = pack(&src, None).unwrap();
        assert_eq!(pack(&src, Some(&src.join("demo.wasm"))).unwrap(), data, "packing is reproducible");
        let pkg = Package::from_bytes(&data).unwrap();
        assert_eq!(pkg.files.len(), 3);

        let root = temp("root");
        let dir = pkg.install(&root).unwrap();
        assert_eq!(fs::read_to_string(dir.join("LICENSE")).unwrap(), "MIT");
        // An update replaces the folder (files of the old version go).
        fs::write(dir.join("stale.txt"), "old").unwrap();
        pkg.install(&root).unwrap();
        assert!(!dir.join("stale.txt").exists());
        assert!(fs::read_dir(&root).unwrap().count() == 1, "no staging folders left");

        assert!(uninstall(&root, "other").is_err());
        uninstall(&root, "demo").unwrap();
        assert!(!dir.exists());
        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&root);
    }
}
