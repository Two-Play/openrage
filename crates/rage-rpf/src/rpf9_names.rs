//! Name-hash primitives observed in the Scarlett executable.
//!
//! These identify *candidate* paths; hashes do not recover the original text.
//! The 32-bit leaf is normalized Jenkins one-at-a-time. The 64-bit leaves
//! return the first half of MurmurHash3 x64-128, seed zero, with an unusual
//! final length XOR: only the low byte of the length is used. This matters
//! for strings longer than 255 bytes. These are format algorithms, not keys.
//!
//! Native routines treat inputs as C strings. A NUL ends each input; a leading
//! double quote makes the Jenkins leaf stop at the matching quote. The Murmur
//! leaf does not have that quote rule. See `docs/rpf9-name-hashes.md` for the
//! observation and isolated native comparison boundaries.

/// ASCII path normalization shared by the observed native hash leaves.
pub fn normalize_byte(byte: u8) -> u8 {
    match byte {
        b'A'..=b'Z' => byte + (b'a' - b'A'),
        b'\\' => b'/',
        _ => byte,
    }
}

fn c_string(bytes: &[u8]) -> &[u8] {
    &bytes[..bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len())]
}

/// Native normalized Jenkins name hash, with the leading-quote C-string rule.
pub fn hash32(bytes: &[u8]) -> u32 {
    let bytes = c_string(bytes);
    let bytes = if let Some(quoted) = bytes.strip_prefix(b"\"") {
        &quoted[..quoted.iter().position(|b| *b == b'"').unwrap_or(quoted.len())]
    } else {
        bytes
    };
    let mut hash = 0u32;
    for byte in bytes {
        hash = hash.wrapping_add(u32::from(normalize_byte(*byte)));
        hash = hash.wrapping_add(hash << 10);
        hash ^= hash >> 6;
    }
    hash = hash.wrapping_add(hash << 3);
    hash ^= hash >> 11;
    hash.wrapping_add(hash << 15)
}

/// Normalized 64-bit native name-hash leaf (ASCII case and slash folding).
pub fn hash64(bytes: &[u8]) -> u64 {
    murmur64(c_string(bytes), normalize_byte)
}

/// Raw 64-bit native name-hash leaf, without case or slash folding.
pub fn hash64_raw(bytes: &[u8]) -> u64 {
    murmur64(c_string(bytes), |byte| byte)
}

fn murmur64(bytes: &[u8], normalize: impl Fn(u8) -> u8) -> u64 {
    const C1: u64 = 0x87c3_7b91_1142_53d5;
    const C2: u64 = 0x4cf5_ad43_2745_937f;
    let mix1 = |value: u64| value.wrapping_mul(C1).rotate_left(31).wrapping_mul(C2);
    let mix2 = |value: u64| value.wrapping_mul(C2).rotate_left(33).wrapping_mul(C1);
    let word = |slice: &[u8]| {
        slice.iter().enumerate().fold(0u64, |value, (i, byte)| value | (u64::from(normalize(*byte)) << (i * 8)))
    };
    let (mut h1, mut h2) = (0u64, 0u64);
    let (blocks, tail) = bytes.as_chunks::<16>();
    for chunk in blocks {
        h1 ^= mix1(word(&chunk[..8]));
        h1 = h1.rotate_left(27).wrapping_add(h2).wrapping_mul(5).wrapping_add(0x52dc_e729);
        h2 ^= mix2(word(&chunk[8..]));
        h2 = h2.rotate_left(31).wrapping_add(h1).wrapping_mul(5).wrapping_add(0x3849_5ab5);
    }
    if tail.len() > 8 {
        h2 ^= mix2(word(&tail[8..]));
    }
    if !tail.is_empty() {
        h1 ^= mix1(word(&tail[..tail.len().min(8)]));
    }
    // The observed native leaves explicitly zero-extend AL, not EAX.
    let length = u64::from(bytes.len() as u8);
    h1 ^= length;
    h2 ^= length;
    h1 = h1.wrapping_add(h2);
    h2 = h2.wrapping_add(h1);
    fmix(h1).wrapping_add(fmix(h2))
}

fn fmix(mut value: u64) -> u64 {
    value ^= value >> 33;
    value = value.wrapping_mul(0xff51_afd7_ed55_8ccd);
    value ^= value >> 33;
    value = value.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    value ^ (value >> 33)
}

/// The built-in native extension type for a lower-case extension without `.`.
///
/// Only the observed static mapping is represented. Additional types can be
/// registered by the game; this function returns `None` for unknown types.
/// The z/i/# families are aliases for each numeric type. In particular the
/// GTA V `y` family is not assumed to be an alias.
pub fn extension_type(extension: &str) -> Option<u8> {
    if extension == "rpf" {
        return Some(0);
    }
    let rest = extension.strip_prefix(['z', 'i', '#'])?;
    Some(match rest {
        "mf" => 1,
        "dr" => 2,
        "ft" => 3,
        "dd" => 4,
        "td" => 5,
        "bn" => 6,
        "bd" => 7,
        "pd" => 8,
        "bs" => 9,
        "sd" => 10,
        "mt" => 11,
        "sc" => 12,
        "cs" => 13,
        "datb" => 14,
        "bdr" => 15,
        "bdd" => 16,
        "bft" => 17,
        "vert" => 18,
        _ => return None,
    })
}

/// The canonical extension for a built-in native type, the inverse of
/// [`extension_type`] over its `#` family. `None` for an unknown type.
/// Used to name an entry whose path is not recovered.
pub fn extension_for_type(type_id: u8) -> Option<&'static str> {
    Some(match type_id {
        0 => "rpf",
        1 => "#mf",
        2 => "#dr",
        3 => "#ft",
        4 => "#dd",
        5 => "#td",
        6 => "#bn",
        7 => "#bd",
        8 => "#pd",
        9 => "#bs",
        10 => "#sd",
        11 => "#mt",
        12 => "#sc",
        13 => "#cs",
        14 => "#datb",
        15 => "#bdr",
        16 => "#bdd",
        17 => "#bft",
        18 => "#vert",
        _ => return None,
    })
}

/// Candidate path hashes and the recognized static extension type, if any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PathHashes {
    pub hash32: u32,
    pub hash64: u64,
    pub file_type: Option<u8>,
}

/// Hashes a relative path candidate. Recognized extensions are represented by
/// their type and omitted from the hash; unrecognized extensions stay in it.
///
/// The extension is ASCII-lowercased for candidate matching. This is a helper
/// convention: the observed native extension dispatcher itself uses `strcmp`.
/// No mount prefix is removed by this helper.
pub fn path_hashes(path: &str) -> PathHashes {
    let (stem, file_type) = path.rsplit_once('.').map_or((path, None), |(stem, extension)| {
        let file_type = extension_type(&extension.to_ascii_lowercase());
        (if file_type.is_some() { stem } else { path }, file_type)
    });
    PathHashes { hash32: hash32(stem.as_bytes()), hash64: hash64(stem.as_bytes()), file_type }
}

/// Collects bounded NUL-terminated ASCII path candidates from an executable.
/// A candidate contains a slash or a static resource/media extension. Literal
/// `mount:/relative/path` strings contribute `relative/path`, matching native
/// mount-prefix removal. No directory or extension guesses are synthesized.
/// Candidates need catalog hash/type checks before being used as names.
pub fn candidates(exe: &[u8]) -> Vec<String> {
    let mut found = std::collections::BTreeSet::new();
    let mut start = None;
    for (at, byte) in exe.iter().enumerate() {
        if byte.is_ascii_graphic() || *byte == b' ' {
            start.get_or_insert(at);
        } else {
            if *byte == 0
                && let Some(from) = start
                && (3..=1024).contains(&(at - from))
                && let Ok(text) = std::str::from_utf8(&exe[from..at])
                && let Some(path) = candidate_path(text)
            {
                found.insert(path);
            }
            start = None;
        }
    }
    found.into_iter().collect()
}

/// Converts one literal executable path to a safe relative candidate.
/// Slash direction is normalized, case retained for display. Absolute paths,
/// traversal, format placeholders and invalid Windows filename characters are
/// rejected. This helper does not establish a catalog hash match.
pub fn candidate_path(text: &str) -> Option<String> {
    if !text.is_ascii() || text.trim() != text || !(3..=1024).contains(&text.len()) {
        return None;
    }
    let slashes = text.replace('\\', "/");
    let path = if let Some((mount, path)) = slashes.split_once(":/") {
        if mount.len() < 2 || !mount.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            return None;
        }
        path
    } else {
        &slashes
    };
    if path.is_empty()
        || path.starts_with('/')
        || path.bytes().any(|b| !b.is_ascii_graphic() && b != b' ' || b"<>\"|:*?%{}".contains(&b))
        || path.split('/').any(|part| part.is_empty() || part == "." || part == "..")
    {
        return None;
    }
    let supported_extension = path.rsplit_once('.').is_some_and(|(_, extension)| {
        let extension = extension.to_ascii_lowercase();
        extension_type(&extension).is_some() || matches!(extension.as_str(), "bk2" | "awc")
    });
    (path.contains('/') || supported_extension).then(|| path.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_isolated_native_leaf_results() {
        // Synthetic strings, compared with the three native leaf routines.
        for (input, short, normalized, raw) in [
            ("", 0, 0, 0),
            ("a", 0xca2e_9442, 0x8555_5565_f659_7889, 0x8555_5565_f659_7889),
            ("COMMON\\Data\\hello", 0x576c_1635, 0xccf5_dfed_5670_81fa, 0x70cd_be33_24e2_3190),
            ("\"foo\"bar", 0x2386_78dd, 0x4538_4fc0_d320_a743, 0x4538_4fc0_d320_a743),
        ] {
            assert_eq!(hash32(input.as_bytes()), short);
            assert_eq!(hash64(input.as_bytes()), normalized);
            assert_eq!(hash64_raw(input.as_bytes()), raw);
        }
        assert_eq!(hash32(b"\"foo\"bar"), hash32(b"foo"));
        assert_ne!(hash64(b"\"foo\"bar"), hash64(b"foo"));
        assert_eq!(hash32(b"a\0ignored"), hash32(b"a"));
        assert_eq!(hash64(b"a\0ignored"), hash64(b"a"));
    }

    #[test]
    fn matches_native_block_tail_and_length_byte_boundaries() {
        for (length, short, normalized, raw) in [
            (15, 0x2f4d_b830, 0x7d07_a8db_fd2e_7fbc, 0x3c58_42c4_cf7d_1737),
            (16, 0x1eac_851e, 0xf2c1_180d_62aa_a6ce, 0x37bd_7653_d0d1_9d9a),
            (17, 0x9ad8_78a9, 0x6f72_14c7_cef2_d698, 0x8256_209c_f2de_16fd),
            (255, 0x3193_730a, 0x6fd9_34f2_7dbd_cf29, 0x6d41_d82f_587f_0654),
            (256, 0x5a7c_a609, 0x42bf_fdb3_5fe4_59a8, 0x0a2f_a29d_2b04_d60d),
            (257, 0xc6e9_7364, 0xe8fb_e0e1_a76f_a966, 0x9669_0e26_7253_d702),
        ] {
            let input = vec![b'A'; length];
            assert_eq!(hash32(&input), short);
            assert_eq!(hash64(&input), normalized);
            assert_eq!(hash64_raw(&input), raw);
        }
    }

    #[test]
    fn candidate_paths_keep_unknown_extensions_and_share_static_aliases() {
        assert_eq!(extension_type("rpf"), Some(0));
        assert_eq!(extension_type("zdr"), Some(2));
        assert_eq!(extension_type("idr"), Some(2));
        assert_eq!(extension_type("#dr"), Some(2));
        assert_eq!(extension_type("ydr"), None);
        let recognized = path_hashes("Models\\Example.ZDR");
        assert_eq!(
            recognized,
            PathHashes { hash32: hash32(b"models/example"), hash64: hash64(b"models/example"), file_type: Some(2) }
        );
        let unknown = path_hashes("data/example.xml");
        assert_eq!(unknown.file_type, None);
        assert_eq!(unknown.hash32, hash32(b"data/example.xml"));
        assert_eq!(unknown.hash64, hash64(b"data/example.xml"));
    }

    #[test]
    fn executable_candidates_are_literal_bounded_and_relative() {
        let bytes = b"\xffcommon:/Data\\Example.zdr\0common:/Data/Example.zdr\0intro.bk2\0audio.awc\0pack.rpf\0plain.txt\0../bad/file.xml\0https://host/file.xml\0/absolute/file.xml\0%s/path.xml\0word with no path\0unterminated/path.xml";
        assert_eq!(candidates(bytes), ["Data/Example.zdr", "audio.awc", "intro.bk2", "pack.rpf"]);
        assert_eq!(candidate_path("data/Example.xml").as_deref(), Some("data/Example.xml"));
        for bad in [
            "..\\file.zdr",
            "a//file.zdr",
            " leading/file.zdr",
            "data/file:stream.zdr",
            "data/é.zdr",
            "C:\\Users\\file.zdr",
        ] {
            assert_eq!(candidate_path(bad), None);
        }
        let long = format!("data/{}.zdr\0", "x".repeat(1024));
        assert!(candidates(long.as_bytes()).is_empty());
    }
}
