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

use std::collections::HashMap;

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

/// The extensions of the built-in types, by number, after their family
/// prefix: `z`, `i` and `#` are aliases for each. 0 is `rpf`, without one.
const BUILT_IN: [&str; 19] = [
    "rpf", "mf", "dr", "ft", "dd", "td", "bn", "bd", "pd", "bs", "sd", "mt", "sc", "cs", "datb", "bdr", "bdd", "bft",
    "vert",
];
/// The number of the first type the game registers.
const REGISTERED_FROM: u8 = 64;
/// The extensions of the registered types, in the order of the executable's
/// table (pointers at VA 0x14618dd20 for the `#` family). Those that start
/// with `.` here have no family prefix.
const REGISTERED: [&str; 52] = [
    ".mrf", ".cut", ".gfx", "cd", "ld", "pmd", "pm", "ed", "pt", "map", "typ", "ch", "ldb", "jd", "ad", "nv", "nvs",
    "pl", "nd", "vr", "wr", "nh", "fd", "as", "sm", "script", "ascript", "sma", ".iasf", "fc", "lfc", "cg", "asd",
    ".rdb", "fbs", "cps", "sp", "tsl", "mmd", "fid", "msk", "hdat", "nmd", "smm", "ascd", "rd", "smam", "ldbmt", "nmd",
    "kit", "cgc", "cov",
];

/// The type of a lower-case extension without `.`; `None` for extensions that
/// stay in a file's name. The `y` family of GTA V is not an alias. Of the two
/// types with the extension `nmd` this is the first.
pub fn extension_type(extension: &str) -> Option<u8> {
    if extension == "rpf" {
        return Some(0);
    }
    let rest = extension.strip_prefix(['z', 'i', '#']);
    let built_in = || BUILT_IN.iter().skip(1).position(|e| Some(*e) == rest).map(|i| i as u8 + 1);
    let registered = || {
        let found = REGISTERED.iter().position(|e| match e.strip_prefix('.') {
            Some(whole) => whole == extension,
            None => Some(*e) == rest,
        });
        found.map(|i| i as u8 + REGISTERED_FROM)
    };
    built_in().or_else(registered)
}

/// The extension of a type, of the `#` family; the inverse of
/// [`extension_type`]. `None` for an unknown type. Used to name entries.
pub fn extension_for_type(type_id: u8) -> Option<String> {
    let extension = match type_id.checked_sub(REGISTERED_FROM) {
        Some(index) => REGISTERED.get(usize::from(index))?,
        None => BUILT_IN.get(usize::from(type_id))?,
    };
    Some(match extension.strip_prefix('.') {
        Some(whole) => whole.to_owned(),
        None if type_id == 0 => (*extension).to_owned(),
        None => format!("#{extension}"),
    })
}

/// The extension an entry of this type is shown with when its own is not
/// known: the canonical one, or `type<number>` for an unknown type.
pub fn type_extension(type_id: u8) -> String {
    match extension_for_type(type_id) {
        Some(ext) => ext,
        None if type_id == u8::MAX => "bin".to_owned(),
        None => format!("type{type_id}"),
    }
}

/// Texts that may be paths, found by the hashes of the names they could be.
#[derive(Debug, Default)]
pub struct Dictionary {
    texts: Vec<Box<str>>,
    /// Hash to the text and the part of it that has the hash.
    by_hash: HashMap<u64, (u32, u16, u16)>,
}

impl Dictionary {
    /// Adds a path, `mount:/path` or plain name. The mount is dropped, and
    /// what follows each slash is a name as well: files name others from
    /// different folders up. Names count with and without their extension,
    /// as the index leaves out those it has a type for.
    pub fn add(&mut self, text: &str) {
        if text.len() > 1024 {
            return;
        }
        let Ok(number) = u32::try_from(self.texts.len()) else { return };
        let from = text.find(":/").map_or(0, |at| at + 2);
        let starts = std::iter::once(from).chain(text[from..].match_indices('/').map(|(at, _)| from + at + 1));
        let mut used = false;
        for start in starts {
            let name = &text[start..];
            let stem = name.rsplit_once('.').filter(|(_, ext)| !ext.contains('/')).map(|(stem, _)| stem);
            for part in std::iter::once(name).chain(stem).filter(|part| is_relative(part)) {
                self.by_hash.entry(hash64(part.as_bytes())).or_insert_with(|| {
                    used = true;
                    (number, start as u16, (start + part.len()) as u16)
                });
            }
        }
        if used {
            self.texts.push(text.into());
        }
    }

    pub fn len(&self) -> usize {
        self.by_hash.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_hash.is_empty()
    }

    /// The path of an entry with these hashes and this type.
    pub fn find(&self, hash64: u64, hash32: Option<u32>, type_id: u8) -> Option<String> {
        let &(number, start, end) = self.by_hash.get(&hash64)?;
        let name = &self.texts[number as usize][usize::from(start)..usize::from(end)];
        if hash32.is_some_and(|expected| expected != self::hash32(name.as_bytes())) {
            return None;
        }
        Some(with_type(name, type_id))
    }
}

/// The path of an entry with this name and type.
pub fn with_type(name: &str, type_id: u8) -> String {
    // Texts differ in how they write folders; hashes ignore case.
    let folder = name.rfind('/').map_or(0, |at| at + 1);
    let name = name[..folder].to_ascii_lowercase() + &name[folder..];
    if type_id == u8::MAX { name } else { format!("{name}.{}", type_extension(type_id)) }
}

fn is_relative(path: &str) -> bool {
    path.len() >= 2
        && !path.contains([':', '%'])
        && path.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
}

/// What follows a name in the name of a file that belongs to it.
const VARIANTS: [&str; 10] = ["", "_hi", "+hi", "_lod", "_b", "_c", "_decals", "_cloth", "_occl", "_metadata"];

/// Calls `try_name` with the names that files near the one called `name`
/// may have: its variants, and those of what it starts with.
pub fn related(name: &str, try_name: &mut dyn FnMut(&str)) {
    let mut stem = name;
    loop {
        for variant in VARIANTS {
            try_name(&format!("{stem}{variant}"));
        }
        let Some((shorter, _)) = stem.rsplit_once('_') else { break };
        stem = shorter;
    }
}

/// Calls `try_name` with the numbered names files of a type may have, in a
/// part of them: `part` is below [`GRID_PARTS`]. Navigation meshes and cover
/// are named by their cell of the world, collisions of the terrain by its
/// cell (`s09_w04_55`), height data by its tile.
pub fn numbered(type_id: u8, part: u32, try_name: &mut dyn FnMut(&str)) {
    match type_id {
        79 | 100 | 115 => {
            let name = if type_id == 115 { "coverregion" } else { "navmesh" };
            (0..GRID_PARTS).for_each(|y| try_name(&format!("{name}[{part}][{y}]")));
        }
        104 | 105 if part < 100 => {
            let name = if type_id == 104 { "moverdeepsurfacetype" } else { "minmaxheight" };
            (0..100).for_each(|y| try_name(&format!("{name}/tile_{part}_{y}")));
        }
        6 if part < 80 => {
            let row = format!("{}{:02}", if part < 40 { 'n' } else { 's' }, part % 40);
            for column in 0..80 {
                let cell = format!("{row}_{}{:02}", if column < 40 { 'e' } else { 'w' }, column % 40);
                (0..300).for_each(|i| try_name(&format!("{cell}_{i}")));
            }
        }
        _ => {}
    }
}

/// Number of parts of [`numbered`]: more than the world has cells one way.
pub const GRID_PARTS: u32 = 512;

/// The texts in a file's data that may be paths or names of files.
pub fn tokens(data: &[u8]) -> Vec<String> {
    let belongs = |b: &u8| b.is_ascii_alphanumeric() || b"_/\\.-#@+$:".contains(b);
    let mut found: Vec<String> = data
        .split(|b| !belongs(b))
        .filter(|t| (3..=255).contains(&t.len()) && t.iter().any(u8::is_ascii_alphabetic))
        .map(|t| String::from_utf8_lossy(t).replace('\\', "/"))
        .collect();
    found.sort_unstable();
    found.dedup();
    found
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
        assert_eq!((extension_type("vert"), extension_type("#vert"), extension_type("xml")), (None, Some(18), None));
        assert_eq!((extension_type("gfx"), extension_type("#gfx"), extension_type("zcd")), (Some(66), None, Some(67)));
        assert_eq!((extension_type("#script"), extension_type("#cov")), (Some(89), Some(115)));
        for id in (0..=18).chain(64..=115).filter(|id| *id != 112) {
            assert_eq!(extension_type(&extension_for_type(id).unwrap()), Some(id));
        }
        assert_eq!([19, 63, 116, 255].map(extension_for_type), [const { None }; 4]);
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
    fn dictionary_finds_names_below_mounts_and_folders() {
        let mut names = Dictionary::default();
        names.add("platform:/levels/Props/barrel.rpf");
        names.add("/movies/Intro.bk2");
        names.add("../secret/file.xml");
        let find = |name: &str, ty| names.find(hash64(name.as_bytes()), Some(hash32(name.as_bytes())), ty);
        assert_eq!(find("levels/props/barrel", 0).as_deref(), Some("levels/props/barrel.rpf"));
        assert_eq!(find("props/barrel", 2).as_deref(), Some("props/barrel.#dr"));
        assert_eq!(find("barrel", 74).as_deref(), Some("barrel.#typ"));
        assert_eq!(find("movies/intro.bk2", 255).as_deref(), Some("movies/Intro.bk2"));
        assert_eq!(find("secret/file.xml", 255).as_deref(), Some("secret/file.xml"));
        assert!(find("../secret/file.xml", 255).is_none());
        assert!(names.find(hash64(b"barrel"), Some(1), 0).is_none());
        assert_eq!(tokens(b"\x01ab\0data\\x.xml\xff123 name"), ["data/x.xml", "name"]);
    }

    #[test]
    fn generates_numbered_and_related_names() {
        let has = |type_id, part, wanted: &str| {
            let mut found = false;
            numbered(type_id, part, &mut |name| found |= name == wanted);
            found
        };
        assert!(has(79, 204, "navmesh[204][293]") && has(115, 0, "coverregion[0][511]"));
        assert!(has(104, 36, "moverdeepsurfacetype/tile_36_34") && has(105, 1, "minmaxheight/tile_1_41"));
        assert!(has(6, 49, "s09_w04_55") && has(6, 1, "n01_e00_0") && !has(2, 1, "n01_e00_0"));
        let mut names = Vec::new();
        related("sandking_4dr", &mut |name| names.push(name.to_owned()));
        assert!(["sandking_4dr_hi", "sandking", "sandking+hi"].iter().all(|n| names.iter().any(|m| m == n)));
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
