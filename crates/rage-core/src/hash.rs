//! Jenkins one-at-a-time hashing ("joaat"), used throughout RAGE for names.

/// Jenkins one-at-a-time hash over raw bytes (no case folding).
pub fn jenkins_bytes(data: &[u8]) -> u32 {
    let mut h: u32 = 0;
    for &b in data {
        h = h.wrapping_add(b as u32);
        h = h.wrapping_add(h << 10);
        h ^= h >> 6;
    }
    h = h.wrapping_add(h << 3);
    h ^= h >> 11;
    h.wrapping_add(h << 15)
}

/// RAGE "joaat" hash of a name: ASCII-lowercased Jenkins hash.
pub fn joaat(name: &str) -> u32 {
    let lower: Vec<u8> = name.bytes().map(|b| b.to_ascii_lowercase()).collect();
    jenkins_bytes(&lower)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_joaat_values() {
        // Well-known GTA V model name hashes.
        assert_eq!(joaat("adder"), 0xB779_A091);
        assert_eq!(joaat("ADDER"), 0xB779_A091);
        assert_eq!(joaat(""), 0);
    }
}
