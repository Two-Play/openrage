//! Executable-backed native RPF9 indexes. The executable is parsed as data.

use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

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

/// Names of files that were learned from other archives, by their hash.
static LEARNED: RwLock<Option<HashMap<u64, Box<str>>>> = RwLock::new(None);

/// Adds names files may have, without their extension. Archives opened from
/// now on use them.
pub fn add_names(names: impl IntoIterator<Item = String>) {
    let mut learned = LEARNED.write().unwrap_or_else(|e| e.into_inner());
    let learned = learned.get_or_insert_default();
    learned.extend(names.into_iter().map(|name| (rpf9_names::hash64(name.as_bytes()), name.into_boxed_str())));
    OPENED.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

type OpenedKey = (PathBuf, u64, u64, Option<std::time::SystemTime>);
static OPENED: Mutex<Vec<(OpenedKey, Arc<NativeArchive>)>> = Mutex::new(Vec::new());

/// Reuses runtime tables, never a checked-in or on-disk key cache.
pub(crate) fn materials(path: &Path) -> Option<Arc<Materials>> {
    type Key = (PathBuf, u64, Option<std::time::SystemTime>);
    // `None`: not a supported build, which is not read again and again.
    static CACHE: OnceLock<Mutex<HashMap<Key, Option<Arc<Materials>>>>> = OnceLock::new();
    let executable =
        path.ancestors().skip(1).take(5).map(|p| p.join("game_scarlett_master.exe")).find(|p| p.is_file())?;
    let meta = std::fs::metadata(&executable).ok()?;
    let key = (executable, meta.len(), meta.modified().ok());
    let mut cache = CACHE.get_or_init(Default::default).lock().ok()?;
    if let Some(materials) = cache.get(&key) {
        return materials.clone();
    }
    let materials = load_materials(&key.0).map(Arc::new);
    cache.insert(key, materials.clone());
    materials
}

fn load_materials(executable: &Path) -> Option<Materials> {
    let bytes = std::fs::read(executable).ok()?;
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
    let read = || {
        for archive in ["common.rpf", "data.rpf"].map(|name| executable.with_file_name(name)) {
            strings.extend(NativeArchive::strings(&archive, &nameless).unwrap_or_default());
        }
    };
    crate::in_own_threads(read);
    strings.iter().for_each(|text| names.add(text));
    let cipher = Arc::into_inner(nameless)?.cipher;
    Some(Materials { cipher, names })
}

#[derive(Debug)]
pub(crate) struct NativeArchive {
    pub catalog: Catalog,
    materials: Arc<Materials>,
    /// The entries' paths, by slot.
    pub paths: Vec<String>,
    /// Lower-case path to slot.
    by_path: HashMap<String, usize>,
    /// Extensions of unnamed entries without a type, from their content.
    sniffed: HashMap<usize, String>,
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
        let mut archive =
            Self { catalog, materials, paths: Vec::new(), by_path: HashMap::new(), sniffed: HashMap::new() };
        archive.name_entries(&HashMap::new());
        Ok(archive)
    }

    /// The archive at `base` in a file, with the names its own files tell.
    /// The last few opened ones are kept.
    pub fn cached(path: &Path, base: u64, extent: u64) -> Option<Arc<Self>> {
        let key = (path.to_path_buf(), base, extent, std::fs::metadata(path).and_then(|m| m.modified()).ok());
        if let Some((_, found)) = OPENED.lock().ok()?.iter().find(|(k, _)| *k == key) {
            return Some(Arc::clone(found));
        }
        let mut archive = Self::open(path, base, extent, materials(path)?).ok()?;
        let learned = archive.learn_names(path, base);
        archive.sniffed = archive.sniff(path, base);
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
        let shared = LEARNED.read().unwrap_or_else(|e| e.into_inner());
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
                .or_else(|| {
                    // Archives keep the place they were found at.
                    let hash = entry.name_hash64.filter(|_| entry.type_id != 0)?;
                    Some(rpf9_names::with_type(shared.as_ref()?.get(&hash)?, entry.type_id))
                })
                .filter(|name| !self.by_path.contains_key(&name.to_ascii_lowercase()));
            let path = name.unwrap_or_else(|| {
                let sniffed = self.sniffed.get(&entry.slot).cloned();
                let ext = sniffed.unwrap_or_else(|| rpf9_names::type_extension(entry.type_id));
                format!("{UNNAMED}/{:06}_{:016x}.{ext}", entry.slot, entry.name_hash64.unwrap_or(0))
            });
            self.by_path.insert(path.to_ascii_lowercase(), entry.slot);
            self.paths.push(path);
        }
    }

    fn unnamed(&self) -> impl Iterator<Item = &NativeEntry> {
        self.catalog.entries.iter().filter(|e| self.is_unnamed(e))
    }

    /// Whether an entry is listed by its slot, as its name is not known.
    pub fn is_unnamed(&self, entry: &NativeEntry) -> bool {
        self.paths[entry.slot].strip_prefix(UNNAMED).is_some_and(|rest| rest.starts_with('/'))
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

    /// The extensions of unnamed entries without a type, by how they start.
    fn sniff(&self, path: &Path, base: u64) -> HashMap<usize, String> {
        /// Takes the first bytes, then ends the unpacking.
        struct Head(Vec<u8>);
        impl Write for Head {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                if self.0.len() >= 16 {
                    return Err(std::io::ErrorKind::WriteZero.into());
                }
                let n = buf.len().min(16 - self.0.len());
                self.0.extend_from_slice(&buf[..n]);
                Ok(n)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let unnamed: Vec<&NativeEntry> = self.unnamed().filter(|e| e.type_id == u8::MAX).collect();
        unnamed
            .into_par_iter()
            .filter_map(|e| {
                let mut head = Head(Vec::new());
                // Fails when the head is full, which is fine.
                let _ = self.copy_to(path, base, e, &mut head);
                Some((e.slot, crate::rpf9::extension_of(&head.0)?))
            })
            .collect()
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
        let path = path.to_ascii_lowercase();
        self.by_path.get(&path).copied().or_else(|| {
            // Its name was learned since: the stand-in has the slot and the hash.
            let (slot, rest) = path.strip_prefix(UNNAMED)?.strip_prefix('/')?.split_once('_')?;
            let hash = u64::from_str_radix(rest.get(..16)?, 16).ok()?;
            let slot: usize = slot.parse().ok()?;
            self.catalog.entries.get(slot).filter(|e| e.slot == slot && e.name_hash64 == Some(hash)).map(|e| e.slot)
        })
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
            // One raw DEFLATE stream, padded like the member.
            1 if !resource => inflate_exact(&mut member, out, false, len, ALIGN, entry.size)?,
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
const KEPT: usize = 192;
/// Larger members are not searched for names of others.
const STRINGS_MAX: u64 = 64 << 20;

/// Clear bytes in front of a resource's chunks.
const RESOURCE_PREFIX: u64 = 16;
/// Members whose data is one stream are padded to a multiple of this.
const ALIGN: u64 = 16;

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
        if chunk.zlib {
            // The table gives a chunk's stored bytes without padding.
            inflate_exact(&mut stored, out, true, chunk.stored, 1, chunk.size)?;
        } else if std::io::copy(&mut stored, out).map_err(|e| e.to_string())? != chunk.size {
            return Err("a chunk's decoded size differs from the index".into());
        }
    }
    Ok(size)
}

/// Inflates one zlib (`zlib`) or raw DEFLATE stream from `stored` bytes into
/// exactly `size` bytes. The stream has to reach its proper end, which for
/// zlib includes a matching checksum, and end where its stored bytes do,
/// padded to a multiple of `align`: more bytes left over mean that the index
/// and the data disagree.
fn inflate_exact(
    input: &mut impl Read,
    out: &mut dyn Write,
    zlib: bool,
    stored: u64,
    align: u64,
    size: u64,
) -> Result<u64, String> {
    let mut inflater = flate2::Decompress::new(zlib);
    let mut source = vec![0u8; 1 << 16];
    let mut target = vec![0u8; 1 << 16];
    let (mut at, mut filled) = (0, 0);
    loop {
        if at == filled {
            filled = input.read(&mut source).map_err(|e| e.to_string())?;
            at = 0;
        }
        let (read, written) = (inflater.total_in(), inflater.total_out());
        // Room for one byte more than is left shows a stream longer than
        // declared without decoding all of it.
        let room = (size - written).saturating_add(1).min(target.len() as u64) as usize;
        let flush = if filled == 0 { flate2::FlushDecompress::Finish } else { flate2::FlushDecompress::None };
        let status = inflater
            .decompress(&source[at..filled], &mut target[..room], flush)
            .map_err(|e| format!("broken compressed stream: {e}"))?;
        let consumed = (inflater.total_in() - read) as usize;
        let produced = (inflater.total_out() - written) as usize;
        at += consumed;
        if inflater.total_out() > size {
            return Err("decoded data is longer than the index says".into());
        }
        out.write_all(&target[..produced]).map_err(|e| e.to_string())?;
        if status == flate2::Status::StreamEnd {
            if inflater.total_in().next_multiple_of(align) != stored {
                return Err("compressed stream size differs from the index".into());
            }
            if inflater.total_out() != size {
                return Err("decoded size differs from the index".into());
            }
            return Ok(size);
        }
        if consumed == 0 && produced == 0 {
            return Err("compressed stream ends early".into());
        }
    }
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

    fn inflate(data: &[u8], zlib: bool, stored: usize, size: u64) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        inflate_exact(&mut &data[..stored], &mut out, zlib, stored as u64, if zlib { 1 } else { ALIGN }, size)?;
        Ok(out)
    }

    #[test]
    fn zlib_streams_need_their_end_checksum_and_exact_sizes() {
        use flate2::{Compression, write::ZlibEncoder};
        let text = b"a small zlib stream that is long enough to compress a little, a little".repeat(50);
        let size = text.len() as u64;
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&text).unwrap();
        let data = encoder.finish().unwrap();
        assert_eq!(inflate(&data, true, data.len(), size).as_deref(), Ok(&text[..]));
        // Cut into the checksum, or into the compressed data itself.
        for cut in [1, 2, 3, 4, 5, data.len() / 2] {
            assert!(inflate(&data, true, data.len() - cut, size).is_err(), "cut {cut}");
        }
        let mut bad = data.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(inflate(&bad, true, bad.len(), size).is_err());
        // More stored bytes than the stream uses.
        let mut longer = data.clone();
        longer.push(0);
        assert!(inflate(&longer, true, longer.len(), size).is_err());
        // Decoded size larger or smaller than the index says.
        assert!(inflate(&data, true, data.len(), size - 1).is_err());
        assert!(inflate(&data, true, data.len(), size + 1).is_err());
    }

    #[test]
    fn raw_deflate_streams_need_their_end_and_padded_size() {
        use flate2::{Compression, write::DeflateEncoder};
        let text = b"raw DEFLATE without a header or checksum, raw DEFLATE".repeat(40);
        let size = text.len() as u64;
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&text).unwrap();
        let data = encoder.finish().unwrap();
        // Stored like a member: padded to a multiple of 16 bytes.
        let mut padded = data.clone();
        padded.resize(data.len().next_multiple_of(16), 0);
        assert_eq!(inflate(&padded, false, padded.len(), size).as_deref(), Ok(&text[..]));
        assert!(inflate(&data, false, data.len() - 1, size).is_err());
        // Padding beyond the next multiple of 16.
        padded.resize(padded.len() + 16, 0);
        assert!(inflate(&padded, false, padded.len(), size).is_err());
        assert!(inflate(&data, false, data.len(), size - 1).is_err());
    }
}
