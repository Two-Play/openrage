//! Red Dead Redemption 2 text tables (`.yldb`, "language database"): the
//! localized strings, RDR2's counterpart of GTA V's [`gxt2`](crate::gxt2).
//!
//! They live in `data/lang/<language>_rel.rpf` (one table per mission,
//! character or menu area, `global.yldb` for the shared strings). Each is a
//! resource whose root is a hash map from label hash to string
//! (`rage::atMap`):
//!
//! | offset | root                                                    |
//! |--------|---------------------------------------------------------|
//! | `0x08` | pointer to the page map (as in every resource)          |
//! | `0x10` | pointer to the buckets: `slots × pointer to first node` |
//! | `0x18` | `u32` slot count                                        |
//! | `0x20` | `u32` entry count                                       |
//!
//! A node is `label hash (u32, padded to 8)`, pointer to the value, pointer to
//! the next node in the same bucket. A value is 16 reserved bytes (always
//! zero in the game's files), pointer to the UTF-8 string and its length
//! including the terminating NUL (`u32`).

use crate::gxt2::Entry;
use rage_resource::{Resource, ResourceError};

/// The resource extension of a text table.
pub const EXTENSION: &str = "yldb";

fn bad(msg: String) -> ResourceError {
    ResourceError::Format(format!("invalid text table: {msg}"))
}

/// Reads all strings of a text table, sorted by label hash.
pub fn parse(res: &Resource) -> Result<Vec<Entry>, ResourceError> {
    let root = Resource::ROOT;
    let buckets = res.u64(root + 0x10)?;
    let slots = res.u32(root + 0x18)? as u64;
    let used = res.u32(root + 0x20)? as usize;
    // Every node takes 24 bytes and every slot 8: anything larger cannot fit.
    let size = (res.system.len() + res.graphics.len()) as u64;
    if slots * 8 > size || used as u64 * 24 > size {
        return Err(bad(format!("{slots} slots / {used} entries do not fit")));
    }
    let mut out = Vec::with_capacity(used);
    for slot in 0..slots {
        let mut node = res.u64(buckets + slot * 8)?;
        while node != 0 {
            if out.len() == used {
                return Err(bad(format!("more than the {used} entries the header names (or a loop)")));
            }
            let hash = res.u32(node)?;
            let value = res.u64(node + 8)?;
            let text = res.u64(value + 0x10)?;
            let len = res.u32(value + 0x18)? as usize;
            let bytes = if len == 0 { &[][..] } else { res.bytes(text, len)? };
            let bytes = bytes.strip_suffix(&[0]).unwrap_or(bytes);
            out.push(Entry { hash, text: String::from_utf8_lossy(bytes).into_owned() });
            node = res.u64(node + 16)?;
        }
    }
    if out.len() != used {
        return Err(bad(format!("{} entries found, the header names {used}", out.len())));
    }
    out.sort_by_key(|e| e.hash);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: u64 = 0x5000_0000;

    /// A table with the given strings in `slots` buckets (by hash modulo slots).
    fn table(strings: &[(u32, &str)], slots: usize) -> Resource {
        let mut sys = vec![0u8; 0x30];
        let put = |sys: &mut Vec<u8>, at: usize, v: u64| sys[at..at + 8].copy_from_slice(&v.to_le_bytes());
        let buckets = sys.len();
        sys.resize(buckets + slots * 8, 0);
        put(&mut sys, 0x10, BASE + buckets as u64);
        put(&mut sys, 0x18, slots as u64);
        put(&mut sys, 0x20, strings.len() as u64);
        for &(hash, s) in strings {
            let text = sys.len();
            sys.extend(s.as_bytes());
            sys.push(0);
            sys.resize(sys.len().next_multiple_of(16), 0);
            let value = sys.len();
            sys.resize(value + 0x20, 0);
            put(&mut sys, value + 0x10, BASE + text as u64);
            put(&mut sys, value + 0x18, s.len() as u64 + 1);
            let node = sys.len();
            sys.resize(node + 0x20, 0);
            put(&mut sys, node, u64::from(hash));
            put(&mut sys, node + 8, BASE + value as u64);
            // Push onto the front of the bucket's chain.
            let slot = buckets + (hash as usize % slots) * 8;
            let first = u64::from_le_bytes(sys[slot..slot + 8].try_into().unwrap());
            put(&mut sys, node + 16, first);
            put(&mut sys, slot, BASE + node as u64);
        }
        Resource { version: 1, system: sys, graphics: Vec::new() }
    }

    #[test]
    fn reads_chained_buckets_sorted_by_hash() {
        let res =
            table(&[(0x30, "~z~Not now!"), (0x11, "Well, thank you kindly."), (0x22, "Grüß Gott"), (0x14, "")], 3);
        let entries = parse(&res).unwrap();
        let got: Vec<(u32, &str)> = entries.iter().map(|e| (e.hash, e.text.as_str())).collect();
        assert_eq!(got, [(0x11, "Well, thank you kindly."), (0x14, ""), (0x22, "Grüß Gott"), (0x30, "~z~Not now!")]);
    }

    #[test]
    fn rejects_a_wrong_entry_count() {
        let mut res = table(&[(1, "a"), (2, "b")], 2);
        res.system[0x20] = 3;
        assert!(parse(&res).is_err());
        res.system[0x20] = 1;
        assert!(parse(&res).is_err());
    }

    #[test]
    fn rejects_a_loop() {
        let mut res = table(&[(1, "a")], 1);
        let node = u64::from_le_bytes(res.system[0x30..0x38].try_into().unwrap());
        let at = (node - BASE) as usize + 16;
        res.system[at..at + 8].copy_from_slice(&node.to_le_bytes());
        res.system[0x20] = 5;
        assert!(parse(&res).is_err());
    }

    #[test]
    fn rejects_implausible_sizes() {
        let mut res = table(&[], 1);
        res.system[0x18..0x1C].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse(&res).is_err());
    }
}
