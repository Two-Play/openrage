//! Taking RDR2's cipher tables and IV from the user's `RDR2.exe`, and key
//! candidates from a memory dump of the user's running game.
//!
//! The tables are stored in the executable as 565 separate pieces. They are
//! found by SHA-256 fingerprints (`rpf8_fingerprints.txt`: checksums only, no
//! game data); the 4-byte XOR values carry no fingerprint and are read next
//! to their masks. All archive keys start with the same 32 bytes, whose
//! fingerprint is in the list too: every place in a memory dump that starts
//! that way is a key candidate, which [`crate::rpf8_keys::identify`] then
//! confirms by decrypting the user's archives.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;

use rage_tfit2::{CONTEXT_LEN, IV_LEN, KEY_LEN};
use sha2::{Digest, Sha256};

const FINGERPRINTS: &str = include_str!("rpf8_fingerprints.txt");
/// Pieces of the tables (565), then the IV, then the shared key start.
const TABLE_PIECES: usize = 565;
const KEY_START_LEN: usize = 32;

#[derive(Clone, Copy)]
enum Piece {
    Data { len: usize, pre: u64, sha: [u8; 32] },
    Xor,
}

fn pieces() -> Vec<Piece> {
    FINGERPRINTS
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            if l.trim() == "xor" {
                return Piece::Xor;
            }
            let mut f = l.split_whitespace();
            let len = f.next().and_then(|v| v.parse().ok()).expect("length");
            let pre = f.next().and_then(|v| u64::from_str_radix(v, 16).ok()).expect("prefilter");
            let hex = f.next().expect("sha256");
            let sha = std::array::from_fn(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).expect("hex"));
            Piece::Data { len, pre, sha }
        })
        .collect()
}

/// FNV-1a 64 of the first 16 bytes.
fn prefilter(b: &[u8]) -> u64 {
    b[..16].iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &x| (h ^ u64::from(x)).wrapping_mul(0x0100_0000_01b3))
}

/// What the executable provides.
pub struct ExeTables {
    /// The cipher tables in their in-memory layout ([`CONTEXT_LEN`] bytes).
    pub context: Vec<u8>,
    pub iv: [u8; IV_LEN],
    /// The 32 bytes every archive key starts with.
    pub key_start: [u8; KEY_START_LEN],
    /// Keys stored in the executable itself (usually one).
    pub keys: Vec<Vec<u8>>,
}

/// Errors from [`tables_from_exe`].
#[derive(Debug, thiserror::Error)]
pub enum ExeError {
    #[error(
        "{0} of the {total} cipher-table pieces are not in this RDR2.exe; this game version is not supported yet",
        total = TABLE_PIECES + 2
    )]
    Missing(usize),
    #[error("the cipher tables in this RDR2.exe are not in the expected layout")]
    Layout,
}

/// Finds the cipher tables, IV and key start in the bytes of `RDR2.exe`.
pub fn tables_from_exe(exe: &[u8]) -> Result<ExeTables, ExeError> {
    let pieces = pieces();
    let mut by_pre: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, p) in pieces.iter().enumerate() {
        if let Piece::Data { pre, .. } = p {
            by_pre.entry(*pre).or_default().push(i);
        }
    }
    let mut found: Vec<Option<usize>> = vec![None; pieces.len()];
    for off in (0..exe.len().saturating_sub(16)).step_by(16) {
        let Some(list) = by_pre.get(&prefilter(&exe[off..])) else { continue };
        for &i in list {
            if let Piece::Data { len, sha, .. } = pieces[i]
                && found[i].is_none()
                && off + len <= exe.len()
                && Sha256::digest(&exe[off..off + len]).as_slice() == sha
            {
                found[i] = Some(off);
            }
        }
    }
    let missing = pieces.iter().zip(&found).filter(|(p, f)| matches!(p, Piece::Data { .. }) && f.is_none()).count();
    if missing > 0 {
        return Err(ExeError::Missing(missing));
    }
    let data = |i: usize| -> &[u8] {
        let Piece::Data { len, .. } = pieces[i] else { unreachable!("data piece") };
        let off = found[i].expect("found");
        &exe[off..off + len]
    };

    // Assemble in the in-memory layout: init tables; 17 rounds of lookup +
    // 16 × (masks, xor, padding); end masks; end tables; end xor.
    let mut ctx = Vec::with_capacity(CONTEXT_LEN);
    ctx.extend(data(0));
    let mut i = 1;
    for _ in 0..17 {
        ctx.extend(data(i));
        i += 1;
        for block in 0..16 {
            let masks_at = found[i].expect("masks");
            ctx.extend(data(i));
            let xor_at =
                if block % 2 == 0 { masks_at + 128 } else { masks_at.checked_sub(8).ok_or(ExeError::Layout)? };
            let xor = exe.get(xor_at..xor_at + 4).ok_or(ExeError::Layout)?;
            if u32::from_le_bytes(xor.try_into().expect("4 bytes")) >= 4096 {
                return Err(ExeError::Layout);
            }
            ctx.extend(xor);
            ctx.extend([0u8; 4]);
            i += 2;
        }
    }
    for _ in 0..3 {
        ctx.extend(data(i));
        i += 1;
    }
    if ctx.len() != CONTEXT_LEN || i != TABLE_PIECES || rage_tfit2::Context::from_bytes(&ctx).is_none() {
        return Err(ExeError::Layout);
    }
    let iv = data(TABLE_PIECES).try_into().expect("16 bytes");
    let key_start: [u8; KEY_START_LEN] = data(TABLE_PIECES + 1).try_into().expect("32 bytes");
    let keys = keys_starting_with(exe, &key_start);
    Ok(ExeTables { context: ctx, iv, key_start, keys })
}

/// Every [`KEY_LEN`]-byte block in `data` that starts with `start`.
fn keys_starting_with(data: &[u8], start: &[u8]) -> Vec<Vec<u8>> {
    memchr::memmem::find_iter(data, start)
        .filter(|&at| at + KEY_LEN <= data.len())
        .map(|at| data[at..at + KEY_LEN].to_vec())
        .collect()
}

/// Key candidates in a memory dump (any format: the file is searched as is),
/// read in pieces so dumps of many gigabytes work. Duplicates are dropped.
pub fn keys_in_dump(
    path: &Path,
    start: &[u8; KEY_START_LEN],
    mut progress: impl FnMut(u64, u64),
) -> std::io::Result<Vec<Vec<u8>>> {
    const CHUNK: usize = 256 << 20;
    let mut f = std::fs::File::open(path)?;
    let total = f.metadata()?.len();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut buf = vec![0u8; CHUNK + KEY_LEN];
    let mut carry = 0; // bytes kept from the previous chunk
    let mut done = 0u64;
    loop {
        let mut n = carry;
        while n < buf.len() {
            let r = f.read(&mut buf[n..])?;
            if r == 0 {
                break;
            }
            n += r;
        }
        if n == carry {
            break;
        }
        for key in keys_starting_with(&buf[..n], start) {
            if seen.insert(key.clone()) {
                out.push(key);
            }
        }
        done += (n - carry) as u64;
        progress(done, total);
        // Keep the last KEY_LEN - 1 bytes: a key may straddle the boundary.
        let keep = (KEY_LEN - 1).min(n);
        buf.copy_within(n - keep..n, 0);
        carry = keep;
        if n < buf.len() {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_list_is_complete() {
        let p = pieces();
        assert_eq!(p.len(), TABLE_PIECES + 2);
        assert_eq!(p.iter().filter(|p| matches!(p, Piece::Xor)).count(), 17 * 16);
        let len = |i: usize| match p[i] {
            Piece::Data { len, .. } => len,
            Piece::Xor => 4,
        };
        assert_eq!(len(0), 16 * 256 * 8);
        assert_eq!(len(TABLE_PIECES), IV_LEN);
        assert_eq!(len(TABLE_PIECES + 1), KEY_START_LEN);
        // The pieces add up to the table layout (minus the 4 padding bytes per block).
        let total: usize = (0..TABLE_PIECES).map(len).sum();
        assert_eq!(total + 17 * 16 * 4, CONTEXT_LEN);
    }

    #[test]
    fn finds_keys_in_a_dump() {
        let start = [0xA5u8; KEY_START_LEN];
        let mut data = vec![0u8; 5000];
        for at in [10usize, 3000] {
            data[at..at + KEY_START_LEN].copy_from_slice(&start);
            data[at + KEY_START_LEN] = at as u8;
        }
        let dir = std::env::temp_dir().join(format!("openrage-dump-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("t.dmp");
        std::fs::write(&file, &data).unwrap();
        let keys = keys_in_dump(&file, &start, |_, _| {}).unwrap();
        assert_eq!(keys.len(), 2);
        assert!(keys.iter().all(|k| k.len() == KEY_LEN && k.starts_with(&start)));
        std::fs::remove_dir_all(dir).ok();
    }
}
