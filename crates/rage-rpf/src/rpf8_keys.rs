//! Finding the RDR2 archive keys among unlabelled pieces of data — the blocks
//! of a key file such as Swage's `secrets.bin`, or (later) candidates from a
//! memory dump of the user's running game — and storing them locally.
//!
//! OpenRAGE carries no reference data for this (no keys, no fingerprints).
//! Pieces are recognised by what they do with the user's own archives:
//! - a key for a table is the one under which the encrypted entry table of an
//!   archive with that decryption tag looks like a real table;
//! - the IV is the one that then also makes the first entry plausible;
//! - a key used only for files is the one under which the files that name it
//!   start like Oodle-compressed data.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use rage_tfit2::{CONTEXT_LEN, Context, IV_LEN, KEY_LEN, Key};

use crate::rpf8::{Compression, Entry, Header, Rdr2Keys, plausible_table};

const STORE_MAGIC: &[u8; 8] = b"ORRDR2K1";

/// Keys in their raw in-memory layout, as stored in the local key file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyStore {
    pub context: Vec<u8>,
    pub iv: [u8; IV_LEN],
    pub keys: BTreeMap<u8, Vec<u8>>,
}

impl KeyStore {
    /// Where OpenRAGE keeps the keys (`<config dir>/openrage/rdr2.keys`).
    pub fn default_path() -> Option<PathBuf> {
        Some(dirs::config_dir()?.join("openrage").join("rdr2.keys"))
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + CONTEXT_LEN + IV_LEN + 4 + self.keys.len() * (1 + KEY_LEN));
        out.extend(STORE_MAGIC);
        out.extend(&self.context);
        out.extend(self.iv);
        out.extend((self.keys.len() as u32).to_le_bytes());
        for (id, key) in &self.keys {
            out.push(*id);
            out.extend(key);
        }
        out
    }

    pub fn from_bytes(b: &[u8]) -> Option<KeyStore> {
        let rest = b.strip_prefix(STORE_MAGIC)?;
        if rest.len() < CONTEXT_LEN + IV_LEN + 4 {
            return None;
        }
        let (context, rest) = rest.split_at(CONTEXT_LEN);
        let (iv, rest) = rest.split_at(IV_LEN);
        let (count, mut rest) = rest.split_at(4);
        let count = u32::from_le_bytes(count.try_into().ok()?) as usize;
        let mut keys = BTreeMap::new();
        for _ in 0..count {
            if rest.len() < 1 + KEY_LEN {
                return None;
            }
            keys.insert(rest[0], rest[1..1 + KEY_LEN].to_vec());
            rest = &rest[1 + KEY_LEN..];
        }
        Some(KeyStore { context: context.to_vec(), iv: iv.try_into().ok()?, keys })
    }

    pub fn load(path: &Path) -> std::io::Result<Option<KeyStore>> {
        match std::fs::read(path) {
            Ok(b) => Ok(KeyStore::from_bytes(&b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, self.to_bytes())
    }

    /// The keys ready for decryption.
    pub fn to_keys(&self) -> Option<Rdr2Keys> {
        Some(Rdr2Keys {
            context: Context::from_bytes(&self.context)?,
            iv: self.iv,
            keys: self.keys.iter().filter_map(|(id, k)| Some((*id, Key::from_bytes(k)?))).collect(),
        })
    }
}

/// Splits a Swage-style secrets file: blocks of `u32 length` + data.
pub fn parse_secrets(b: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 4 <= b.len() {
        let len = u32::from_le_bytes(b[at..at + 4].try_into().expect("4 bytes")) as usize;
        at += 4;
        if at + len > b.len() {
            break;
        }
        out.push(b[at..at + len].to_vec());
        at += len;
    }
    out
}

/// Pieces that may be cipher tables, IVs or keys.
#[derive(Default)]
pub struct Candidates {
    pub contexts: Vec<Vec<u8>>,
    pub ivs: Vec<[u8; IV_LEN]>,
    pub keys: Vec<Vec<u8>>,
}

impl Candidates {
    /// Sorts blocks by their size.
    pub fn from_blocks(blocks: Vec<Vec<u8>>) -> Candidates {
        let mut c = Candidates::default();
        for b in blocks {
            match b.len() {
                CONTEXT_LEN => c.contexts.push(b),
                IV_LEN => c.ivs.push(b.try_into().expect("16 bytes")),
                KEY_LEN => c.keys.push(b),
                _ => {}
            }
        }
        c
    }
}

/// What [`identify`] found.
pub struct Found {
    pub store: KeyStore,
    /// Archives whose table now decrypts, of all encrypted ones.
    pub tables: (usize, usize),
    /// Key ids named by entries that are still unknown.
    pub missing: Vec<u8>,
}

/// Errors from [`identify`].
#[derive(Debug, thiserror::Error)]
pub enum IdentifyError {
    #[error("no cipher tables found (expected a valid block of {CONTEXT_LEN} bytes)")]
    NoContext,
    #[error(
        "none of the {0} key candidates decrypts an archive table; the archive keys exist only in the running game's memory, so a dump taken while RDR2 runs (in the main menu or later) is needed"
    )]
    NoTableKey(usize),
    #[error("no IV found that fits the decrypted tables")]
    NoIv,
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

struct Archive {
    size: u64,
    tag: u8,
    table: Vec<u8>,
    file: PathBuf,
}

/// Every RDR2 archive under a folder.
pub fn archives_under(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("rpf")) {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, &mut out);
    out.sort();
    out
}

fn read_encrypted_tables(files: &[PathBuf]) -> std::io::Result<Vec<Archive>> {
    let mut out = Vec::new();
    for file in files {
        let mut f = File::open(file)?;
        let size = f.metadata()?.len();
        let mut head = [0u8; 16];
        if f.read_exact(&mut head).is_err() {
            continue;
        }
        let Some(h) = Header::parse(&head) else { continue };
        let len = h.entry_count as usize * 24;
        if !h.is_encrypted() || h.decryption_tag > 0xFF || len == 0 || 16 + 256 + len as u64 > size {
            continue;
        }
        f.seek(SeekFrom::Start(16 + 256))?;
        let mut table = vec![0u8; len];
        f.read_exact(&mut table)?;
        out.push(Archive { size, tag: h.decryption_tag as u8, table, file: file.clone() });
    }
    Ok(out)
}

/// Decrypts blocks 1.. of the first `blocks` blocks (block 0 needs the IV),
/// returning the plaintext from byte 16 on.
fn decrypt_without_iv(ctx: &Context, key: &Key, data: &[u8], blocks: usize) -> Vec<u8> {
    let n = blocks.min(data.len() / 16);
    let mut out = Vec::with_capacity(n * 16);
    for i in 1..n {
        let mut block: [u8; 16] = data[i * 16..i * 16 + 16].try_into().expect("16 bytes");
        rage_tfit2::decrypt_block(ctx, key, &mut block);
        out.extend(block.iter().zip(&data[(i - 1) * 16..i * 16]).map(|(a, b)| a ^ b));
    }
    out
}

/// Whether a key decrypts the start of a table plausibly, ignoring the IV:
/// the entries from byte 24 on (entry 1 onwards) must look real.
fn opens_table(ctx: &Context, key: &Key, a: &Archive) -> bool {
    let plain = decrypt_without_iv(ctx, key, &a.table, 7); // bytes 16..112
    if plain.len() < 8 + 24 {
        return false;
    }
    let whole = (plain.len() - 8) / 24 * 24;
    plausible_table(&plain[8..8 + whole], a.size)
}

/// Whether decrypted data starts like an Oodle block.
fn looks_like_oodle(b: &[u8]) -> bool {
    // Low nibble 0xC, bits 4-5 clear; then the decoder type (Kraken = 6, …).
    b.len() >= 2 && b[0] & 0x3F == 0x0C && b[1] & 0x7F <= 13
}

/// Finds the cipher tables, IV and keys among the candidates by trying them on the archives.
pub fn identify(c: &Candidates, archive_files: &[PathBuf]) -> Result<Found, IdentifyError> {
    use rayon::prelude::*;

    let archives = read_encrypted_tables(archive_files)?;
    let keys: Vec<(usize, Key)> =
        c.keys.iter().enumerate().filter_map(|(i, k)| Some((i, Key::from_bytes(k)?))).collect();
    let mut by_tag: HashMap<u8, Vec<&Archive>> = HashMap::new();
    for a in &archives {
        by_tag.entry(a.tag).or_default().push(a);
    }

    for ctx_bytes in &c.contexts {
        let Some(ctx) = Context::from_bytes(ctx_bytes) else { continue };
        // Table keys: one archive per tag is enough to find, all are checked below.
        let found: Vec<(u8, usize)> = by_tag
            .par_iter()
            .filter_map(|(tag, list)| keys.iter().find(|(_, k)| opens_table(&ctx, k, list[0])).map(|(i, _)| (*tag, *i)))
            .collect();
        if found.is_empty() {
            continue;
        }
        let mut store = KeyStore { context: ctx_bytes.clone(), iv: [0; 16], keys: BTreeMap::new() };
        for (tag, i) in &found {
            store.keys.insert(*tag, c.keys[*i].clone());
        }

        // The IV: block 0 of every table must then be plausible too.
        let iv = c.ivs.iter().find(|iv| {
            found.iter().take(8).all(|(tag, i)| {
                let a = by_tag[tag][0];
                let key = Key::from_bytes(&c.keys[*i]).expect("key");
                let mut t = a.table[..a.table.len().min(96)].to_vec();
                rage_tfit2::Cbc::new(&ctx, &key, **iv).decrypt(&mut t);
                plausible_table(&t[..t.len() / 24 * 24], a.size)
            })
        });
        let Some(iv) = iv else { return Err(IdentifyError::NoIv) };
        store.iv = *iv;

        // Tags whose archives are too short for the IV-free test (one or two entries).
        for (tag, list) in &by_tag {
            if store.keys.contains_key(tag) {
                continue;
            }
            let a = list[0];
            if let Some((i, _)) = keys.iter().find(|(_, k)| {
                let mut t = a.table.clone();
                rage_tfit2::Cbc::new(&ctx, k, *iv).decrypt(&mut t);
                plausible_table(&t, a.size)
            }) {
                store.keys.insert(*tag, c.keys[*i].clone());
            }
        }

        // Whole tables, and the keys their files name.
        let keyset = store.to_keys().expect("valid store");
        let mut opened = 0;
        let mut wanted: BTreeMap<u8, Vec<(PathBuf, Entry)>> = BTreeMap::new();
        for a in &archives {
            let Some(key) = keyset.keys.get(&a.tag) else { continue };
            let mut t = a.table.clone();
            rage_tfit2::Cbc::new(&keyset.context, key, keyset.iv).decrypt(&mut t);
            if !plausible_table(&t, a.size) {
                continue;
            }
            opened += 1;
            for e in t.as_chunks::<24>().0.iter().map(|b| Entry::parse(b)) {
                if e.is_encrypted() && !store.keys.contains_key(&e.key_id) {
                    wanted.entry(e.key_id).or_default().push((a.file.clone(), e));
                }
            }
        }

        // File-only keys: the files that name them must start like Oodle data.
        let mut missing = Vec::new();
        for (id, entries) in &wanted {
            let samples: Vec<Vec<u8>> = entries
                .iter()
                .filter(|(_, e)| e.compression == Compression::Oodle && !e.signed)
                .filter(|(_, e)| {
                    let start = u64::from(e.is_resource) * 16;
                    let chunk = rage_tfit2::chunk_size(e.is_resource, true, e.size());
                    rage_tfit2::strided_ranges(e.encryption_config, e.size_on_disk - start, chunk)
                        .first()
                        .is_some_and(|r| r.0 == 0)
                })
                .take(4)
                .filter_map(|(file, e)| {
                    let mut f = File::open(file).ok()?;
                    f.seek(SeekFrom::Start(e.offset + u64::from(e.is_resource) * 16)).ok()?;
                    let mut b = [0u8; 16];
                    f.read_exact(&mut b).ok()?;
                    Some(b.to_vec())
                })
                .collect();
            let hit = (!samples.is_empty())
                .then(|| {
                    keys.par_iter().find_any(|(_, k)| {
                        samples.iter().all(|s| {
                            let mut b = s.clone();
                            rage_tfit2::Cbc::new(&keyset.context, k, keyset.iv).decrypt(&mut b);
                            looks_like_oodle(&b)
                        })
                    })
                })
                .flatten();
            match hit {
                Some((i, _)) => {
                    store.keys.insert(*id, c.keys[*i].clone());
                }
                None => missing.push(*id),
            }
        }
        return Ok(Found { store, tables: (opened, archives.len()), missing });
    }
    if c.contexts.iter().any(|b| Context::from_bytes(b).is_some()) {
        Err(IdentifyError::NoTableKey(c.keys.len()))
    } else {
        Err(IdentifyError::NoContext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_blocks_and_store_round_trip() {
        let mut file = Vec::new();
        for len in [16usize, 288, 5, CONTEXT_LEN] {
            file.extend((len as u32).to_le_bytes());
            let fill = if len == CONTEXT_LEN { 0 } else { len as u8 }; // valid tables
            file.extend(vec![fill; len]);
        }
        file.extend([9, 0, 0, 0, 1]); // truncated last block is ignored
        let blocks = parse_secrets(&file);
        assert_eq!(blocks.len(), 4);
        let c = Candidates::from_blocks(blocks);
        assert_eq!((c.contexts.len(), c.ivs.len(), c.keys.len()), (1, 1, 1));

        let store = KeyStore {
            context: c.contexts[0].clone(),
            iv: c.ivs[0],
            keys: [(3u8, c.keys[0].clone()), (0xC0, vec![7; KEY_LEN])].into(),
        };
        let back = KeyStore::from_bytes(&store.to_bytes()).unwrap();
        assert_eq!(back, store);
        assert_eq!(back.to_keys().unwrap().keys.len(), 2);
        assert!(KeyStore::from_bytes(b"nope").is_none());
    }

    #[test]
    fn oodle_start() {
        assert!(looks_like_oodle(&[0x8C, 0x06]));
        assert!(looks_like_oodle(&[0xCC, 0x0A]));
        assert!(looks_like_oodle(&[0x8C, 0x02]));
        assert!(!looks_like_oodle(&[0x8C, 0x27]));
        assert!(!looks_like_oodle(&[0x00, 0x06]));
    }
}
