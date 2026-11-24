//! Executable-backed native RPF9 indexes. The executable is parsed as data.

use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use rayon::prelude::*;

use crate::rpf9_cipher::Cipher;
use crate::rpf9_index::{Catalog, Chunk, ENCRYPTION, NativeEntry, Storage};
use crate::rpf9_names;

pub(crate) struct Materials {
    pub cipher: Cipher,
    names: rpf9_names::Dictionary,
}

impl std::fmt::Debug for Materials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Materials").field("names", &self.names.len()).finish_non_exhaustive()
    }
}

/// Reuses runtime tables, never a checked-in or on-disk key cache.
pub(crate) fn materials(path: &Path) -> Option<Arc<Materials>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<Materials>>>> = OnceLock::new();
    let executable =
        path.ancestors().skip(1).take(5).map(|p| p.join("game_scarlett_master.exe")).find(|p| p.is_file())?;
    let mut cache = CACHE.get_or_init(Default::default).lock().ok()?;
    if let Some(materials) = cache.get(&executable) {
        return Some(Arc::clone(materials));
    }
    let bytes = std::fs::read(&executable).ok()?;
    let cipher = Cipher::from_exe(&bytes)?;
    // Names: path strings of the executable and of the metadata in the two
    // archives that list most of the game's files.
    let mut names = rpf9_names::Dictionary::default();
    for candidate in rpf9_names::candidates(&bytes) {
        names.add(&candidate);
    }
    drop(bytes);
    let nameless = Arc::new(Materials { cipher, names: Default::default() });
    let mut strings = Vec::new();
    for archive in ["common.rpf", "data.rpf"].map(|name| executable.with_file_name(name)) {
        strings.extend(NativeArchive::strings(&archive, &nameless).unwrap_or_default());
    }
    strings.iter().for_each(|text| names.add(text));
    let cipher = Arc::into_inner(nameless)?.cipher;
    let materials = Arc::new(Materials { cipher, names });
    cache.insert(executable, Arc::clone(&materials));
    Some(materials)
}

#[derive(Debug)]
pub(crate) struct NativeArchive {
    pub catalog: Catalog,
    materials: Arc<Materials>,
    /// The entries' paths, by slot.
    pub paths: Vec<String>,
    /// Lower-case path to slot.
    by_path: HashMap<String, usize>,
}

fn u16_at(b: &[u8], at: usize) -> Option<usize> {
    Some(u16::from_le_bytes(b.get(at..at.checked_add(2)?)?.try_into().ok()?) as usize)
}
fn u32_at(b: &[u8], at: usize) -> Option<usize> {
    Some(u32::from_le_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?) as usize)
}

pub(crate) fn decode_header(cipher: &Cipher, h: &mut [u8]) -> Option<()> {
    if h.len() < 64 || h.get(..4)? != b"9FPR" {
        return None;
    }
    if h[0x2e] == 0 {
        return Some(());
    }
    // The supported Scarlett build uses selector 9: two metadata spans
    // share one CBC chain, leaving the secondary transform prefix intact.
    if h[0x2e] != 9 {
        return None;
    }
    let count = u16_at(h, 0x1a)?;
    let directory_len = count.checked_mul(8)?.checked_add(15)? & !15;
    let blob = 64usize.checked_add(directory_len)?;
    let metadata_end = blob.checked_add(u32_at(h, 0x1c)?)?;
    let tail = u32_at(h, 0x30)?;
    let prefix = metadata_end.checked_sub(tail)?;
    let split = u32_at(h, 8)?;
    if prefix < blob || split < metadata_end || split > h.len() {
        return None;
    }
    let mut iv = cipher.header_iv();
    cipher.decrypt_cbc(0x10000001, h.get_mut(64..blob)?, &mut iv)?;
    cipher.decrypt_cbc(0x10000001, h.get_mut(blob..prefix)?, &mut iv)?;
    let descriptor = (0..count).find_map(|i| {
        let at = 64 + i * 8;
        (u16_at(h, at)? == 1).then(|| blob.checked_add(u32_at(h, at + 4)?)).flatten()
    })?;
    let key = u32_at(h, descriptor)? as u32;
    if u16_at(h, descriptor + 4)? != 16 {
        return None;
    }
    let initial: [u8; 16] = h.get(descriptor + 6..descriptor + 22)?.try_into().ok()?;
    let mut iv = initial;
    cipher.decrypt_cbc(key, h.get_mut(prefix..split)?, &mut iv)?;
    let mut iv = initial;
    cipher.decrypt_cbc(key, h.get_mut(split..)?, &mut iv)?;
    Some(())
}

impl NativeArchive {
    pub fn open(path: &Path, base: u64, extent: u64, materials: Arc<Materials>) -> Result<Self, String> {
        let mut file = File::open(path).map_err(|e| e.to_string())?;
        file.seek(SeekFrom::Start(base)).map_err(|e| e.to_string())?;
        let mut head = [0u8; 64];
        file.read_exact(&mut head).map_err(|e| e.to_string())?;
        let len = u32_at(&head, 4).ok_or("short header")?;
        if !(64..=64 << 20).contains(&len) || len as u64 > extent {
            return Err("native header extent is outside the archive".into());
        }
        let mut bytes = vec![0u8; len];
        bytes[..64].copy_from_slice(&head);
        file.read_exact(&mut bytes[64..]).map_err(|e| e.to_string())?;
        decode_header(&materials.cipher, &mut bytes).ok_or("unsupported or damaged native header transform")?;
        let catalog = Catalog::parse(bytes, extent)?;
        let mut archive = Self { catalog, materials, paths: Vec::new(), by_path: HashMap::new() };
        archive.name_entries(&HashMap::new());
        Ok(archive)
    }

    /// The archive at `base` in a file, with the names its own files tell.
    /// The last few opened ones are kept.
    pub fn cached(path: &Path, base: u64, extent: u64) -> Option<Arc<Self>> {
        type Key = (PathBuf, u64, u64, Option<std::time::SystemTime>);
        static OPENED: Mutex<Vec<(Key, Arc<NativeArchive>)>> = Mutex::new(Vec::new());
        let key = (path.to_path_buf(), base, extent, std::fs::metadata(path).and_then(|m| m.modified()).ok());
        if let Some((_, found)) = OPENED.lock().ok()?.iter().find(|(k, _)| *k == key) {
            return Some(Arc::clone(found));
        }
        let mut archive = Self::open(path, base, extent, materials(path)?).ok()?;
        let learned = archive.learn_names(path, base);
        archive.name_entries(&learned);
        let mut guessed = archive.guess_names();
        if !guessed.is_empty() {
            guessed.extend(learned);
            archive.name_entries(&guessed);
        }
        let archive = Arc::new(archive);
        let mut opened = OPENED.lock().ok()?;
        if opened.len() >= KEPT {
            opened.remove(0);
        }
        opened.push((key, Arc::clone(&archive)));
        Some(archive)
    }

    /// Gives the entries their paths: names by slot, then the known ones.
    fn name_entries(&mut self, learned: &HashMap<usize, String>) {
        self.paths = Vec::with_capacity(self.catalog.entries.len());
        self.by_path = HashMap::with_capacity(self.catalog.entries.len());
        for entry in &self.catalog.entries {
            let name = learned
                .get(&entry.slot)
                .cloned()
                .or_else(|| {
                    let hash = entry.name_hash64?;
                    self.materials.names.find(hash, entry.name_hash32, entry.type_id)
                })
                .filter(|name| !self.by_path.contains_key(&name.to_ascii_lowercase()));
            let path = name.unwrap_or_else(|| {
                let ext = rpf9_names::type_extension(entry.type_id);
                format!("{UNNAMED}/{:06}_{:016x}.{ext}", entry.slot, entry.name_hash64.unwrap_or(0))
            });
            self.by_path.insert(path.to_ascii_lowercase(), entry.slot);
            self.paths.push(path);
        }
    }

    fn unnamed(&self) -> impl Iterator<Item = &NativeEntry> {
        self.catalog
            .entries
            .iter()
            .filter(|e| self.paths[e.slot].strip_prefix(UNNAMED).is_some_and(|rest| rest.starts_with('/')))
    }

    /// Names of unnamed entries that follow from a pattern or from the names
    /// of others in the archive; checked against the hashes in the table.
    fn guess_names(&self) -> HashMap<usize, String> {
        let mut wanted: HashMap<u64, Vec<&NativeEntry>> = HashMap::new();
        let mut types = BTreeSet::new();
        for e in self.unnamed() {
            if let Some(hash) = e.name_hash64 {
                wanted.entry(hash).or_default().push(e);
                types.insert(e.type_id);
            }
        }
        let matching = |name: &str, found: &mut Vec<(usize, String)>| {
            for e in wanted.get(&rpf9_names::hash64(name.as_bytes())).into_iter().flatten() {
                if e.name_hash32.is_none_or(|hash| hash == rpf9_names::hash32(name.as_bytes())) {
                    found.push((e.slot, rpf9_names::with_type(name, e.type_id)));
                }
            }
        };
        let mut found: Vec<(usize, String)> = types
            .into_par_iter()
            .flat_map(|type_id| (0..rpf9_names::GRID_PARTS).into_par_iter().map(move |part| (type_id, part)))
            .flat_map_iter(|(type_id, part)| {
                let mut found = Vec::new();
                rpf9_names::numbered(type_id, part, &mut |name| matching(name, &mut found));
                found
            })
            .collect();
        if !wanted.is_empty() {
            found.par_extend(self.paths.par_iter().flat_map_iter(|path| {
                let mut found = Vec::new();
                let stem = path.rsplit_once('.').map_or(path.as_str(), |(stem, _)| stem);
                rpf9_names::related(stem, &mut |name| matching(name, &mut found));
                found
            }));
        }
        found.into_iter().collect()
    }

    /// Names of unnamed entries, found in their own data: models, clip
    /// dictionaries and some others hold their name. What is found is
    /// checked against the hash in the table.
    fn learn_names(&self, path: &Path, base: u64) -> HashMap<usize, String> {
        let candidates: Vec<&NativeEntry> =
            self.unnamed().filter(|e| SELF_NAMED.contains(&e.type_id) && e.size <= LEARN_MAX).collect();
        candidates
            .into_par_iter()
            .filter_map(|e| {
                let wanted = e.name_hash64?;
                let mut data = Vec::new();
                self.copy_to(path, base, e, &mut data).ok()?;
                let mut names = rpf9_names::Dictionary::default();
                rpf9_names::tokens(&data).iter().for_each(|text| names.add(text));
                Some((e.slot, names.find(wanted, e.name_hash32, e.type_id)?))
            })
            .collect()
    }

    /// The slot with this path (case-insensitive).
    pub fn find(&self, path: &str) -> Option<usize> {
        self.by_path.get(&path.to_ascii_lowercase()).copied()
    }

    /// The path-like strings in an archive's own files.
    fn strings(path: &Path, materials: &Arc<Materials>) -> Result<Vec<String>, String> {
        let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
        let archive = Self::open(path, 0, size, Arc::clone(materials))?;
        let mut found: Vec<String> = archive
            .catalog
            .entries
            .par_iter()
            .filter(|e| e.type_id != 0 && e.size <= STRINGS_MAX)
            .flat_map_iter(|e| {
                let mut data = Vec::new();
                let read = archive.copy_to(path, 0, e, &mut data);
                if read.is_ok() { rpf9_names::tokens(&data) } else { Vec::new() }
            })
            .collect();
        found.par_sort_unstable();
        found.dedup();
        Ok(found)
    }

    /// Writes a member's decoded bytes to `out` and returns their number. A
    /// resource comes out as its system part followed by its graphics part.
    pub fn copy_to(&self, path: &Path, base: u64, entry: &NativeEntry, out: &mut dyn Write) -> Result<u64, String> {
        let file = File::open(path).map_err(|e| e.to_string())?;
        // A resource starts with a clear 16-byte prefix; the encryption
        // pattern and the chunks count from its end.
        let resource = matches!(entry.storage, Storage::Resource { .. });
        let prefix = if resource { RESOURCE_PREFIX } else { 0 };
        let start = base.checked_add(entry.offset).and_then(|v| v.checked_add(prefix)).ok_or("offset overflow")?;
        let len = if entry.stored == 0 {
            0
        } else {
            entry.stored.checked_sub(prefix).ok_or("a resource is shorter than its prefix")?
        };
        let cipher = match self.catalog.record(entry.slot, ENCRYPTION) {
            Some(record) => Some((&self.materials.cipher, Encryption::parse(record, len)?)),
            None => None,
        };
        let mut member = Member { file, start, len, at: 0, cipher };
        if let Some(chunks) = self.catalog.chunks(entry.slot)? {
            return copy_chunks(&mut member, &chunks, entry.size, out);
        }
        let written = match entry.layout {
            0 if !resource && entry.size <= len => {
                std::io::copy(&mut (&mut member).take(entry.size), out).map_err(|e| e.to_string())?
            }
            // One raw DEFLATE stream.
            1 if !resource => {
                let mut inflate = flate2::bufread::DeflateDecoder::new(BufReader::with_capacity(1 << 16, member));
                // One byte more shows a stream longer than declared.
                std::io::copy(&mut (&mut inflate).take(entry.size.saturating_add(1)), out).map_err(|e| e.to_string())?
            }
            layout => return Err(format!("storage layout {layout} is not supported")),
        };
        if written == entry.size { Ok(written) } else { Err("decoded size differs from the index".into()) }
    }
}

/// Where entries without a known name are listed.
const UNNAMED: &str = "entries";
/// Types whose files hold their own name: drawables, fragments, drawable
/// dictionaries, and registered ones where this was seen in the game's data.
const SELF_NAMED: [u8; 10] = [2, 3, 4, 66, 67, 69, 70, 72, 102, 109];
/// Larger members are not searched for their own name.
const LEARN_MAX: u64 = 8 << 20;
/// Opened archives to keep.
const KEPT: usize = 16;
/// Larger members are not searched for names of others.
const STRINGS_MAX: u64 = 64 << 20;

/// Clear bytes in front of a resource's chunks.
const RESOURCE_PREFIX: u64 = 16;

/// Which bytes of a member are encrypted, and with what. Members are often
/// encrypted only in part: `head` bytes, then from there on `run` bytes of
/// every `run + gap`, and `tail` bytes at the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Encryption {
    key: u32,
    iv: [u8; 16],
    head: u64,
    run: u64,
    gap: u64,
    tail_start: u64,
}

impl Encryption {
    fn parse(record: &[u8], len: u64) -> Result<Self, String> {
        let word = |at: usize| u32_at(record, at).map(|v| v as u64).ok_or("short encryption record");
        if u16_at(record, 4) != Some(17) {
            return Err("unsupported encryption algorithm".into());
        }
        let iv = record.get(6..22).and_then(|iv| iv.try_into().ok()).ok_or("short encryption record")?;
        let (head, run, gap, tail) = (word(24)?, word(32)?, word(40)?, word(48)?);
        if [head, run, gap, tail].iter().any(|v| v % 16 != 0) {
            return Err("unaligned encryption pattern".into());
        }
        Ok(Self { key: word(0)? as u32, iv, head, run, gap, tail_start: len.saturating_sub(tail) })
    }

    /// Whether the byte at `at` is encrypted, and where that may change next.
    fn span(&self, at: u64) -> (bool, u64) {
        if at < self.head {
            return (true, self.head);
        }
        if at >= self.tail_start {
            return (true, u64::MAX);
        }
        let period = self.run + self.gap;
        if period == 0 {
            return (false, self.tail_start);
        }
        let within = (at - self.head) % period;
        let (encrypted, left) = if within < self.run { (true, self.run - within) } else { (false, period - within) };
        (encrypted, at.saturating_add(left).min(self.tail_start))
    }
}

/// A member's stored bytes, decrypted.
struct Member<'a> {
    file: File,
    start: u64,
    len: u64,
    at: u64,
    cipher: Option<(&'a Cipher, Encryption)>,
}

impl Read for Member<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut want = (self.len - self.at).min(buf.len() as u64);
        if want == 0 {
            return Ok(0);
        }
        let span = self.cipher.as_ref().map(|(_, e)| e.span(self.at));
        if let Some((_, end)) = span {
            want = want.min(end - self.at);
        }
        self.file.seek(SeekFrom::Start(self.start + self.at))?;
        let n = self.file.read(&mut buf[..want as usize])?;
        if let (Some((cipher, e)), Some((true, _))) = (&self.cipher, span) {
            cipher
                .crypt_ctr(e.key, &mut buf[..n], &e.iv, self.at)
                .ok_or_else(|| std::io::Error::other("no key for this member"))?;
        }
        self.at += n as u64;
        Ok(n)
    }
}

fn copy_chunks(member: &mut Member, chunks: &[Chunk], size: u64, out: &mut dyn Write) -> Result<u64, String> {
    if chunks.iter().try_fold(0u64, |sum, c| sum.checked_add(c.size)) != Some(size) {
        return Err("chunk sizes do not add up to the member size".into());
    }
    for chunk in chunks {
        if chunk.offset.checked_add(chunk.stored).is_none_or(|end| end > member.len) {
            return Err("a chunk lies outside its member".into());
        }
        member.at = chunk.offset;
        let mut stored = member.by_ref().take(chunk.stored);
        let written = if chunk.zlib {
            let mut inflate = flate2::bufread::ZlibDecoder::new(BufReader::with_capacity(1 << 16, stored));
            // One byte more shows a stream longer than declared, and reading
            // to the end checks the stream's checksum.
            std::io::copy(&mut (&mut inflate).take(chunk.size + 1), out)
        } else {
            std::io::copy(&mut stored, out)
        }
        .map_err(|e| e.to_string())?;
        if written != chunk.size {
            return Err("a chunk's decoded size differs from the index".into());
        }
    }
    Ok(size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encryption_patterns_name_the_encrypted_spans() {
        let mut record = [0u8; 64];
        record[4] = 17;
        for (at, v) in [(24, 0x400u32), (32, 0x400), (40, 0x1c00)] {
            record[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        let e = Encryption::parse(&record, 0x10000).unwrap();
        assert_eq!(e.span(0), (true, 0x400));
        assert_eq!(e.span(0x400), (true, 0x800));
        assert_eq!(e.span(0x800), (false, 0x2400));
        assert_eq!(e.span(0x2410), (true, 0x2800));
        assert!(e.span(0xffff).0 == ((0xffff - 0x400) % 0x2000 < 0x400));
        record[48..52].copy_from_slice(&0x1000u32.to_le_bytes());
        let e = Encryption::parse(&record, 0x10000).unwrap();
        assert_eq!(e.span(0xf000), (true, u64::MAX));
        // Without a gap everything is encrypted.
        record[24..28].fill(0);
        record[40..44].fill(0);
        let e = Encryption::parse(&record, 0x10000).unwrap();
        assert!((0..0x10000).step_by(0x100).all(|at| e.span(at).0));
        record[4] = 16;
        assert!(Encryption::parse(&record, 0x10000).is_err());
        assert!(Encryption::parse(&record[..30], 0x10000).is_err());
    }
}
