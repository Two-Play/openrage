//! Names behind the hashes in metadata files (field, structure and enum names),
//! plus names an application registers at run time (model and texture names, …).
//!
//! The built-in list comes from CodeWalker's `MetaNames` (MIT licence, see NOTICE).

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

use rage_core::hash::{jenkins_bytes, joaat};

/// One name per line, hashed case-sensitively; `name hash` lines give the hash.
const LIST: &str = include_str!("meta_names.txt");

fn builtin() -> &'static HashMap<u32, &'static str> {
    static TABLE: OnceLock<HashMap<u32, &'static str>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut map = HashMap::new();
        for line in LIST.lines().filter(|l| !l.is_empty() && !l.starts_with('#')) {
            match line.split_once(' ') {
                Some((name, hash)) => {
                    if let Ok(h) = u32::from_str_radix(hash, 16) {
                        map.insert(h, name);
                    }
                }
                None => {
                    map.insert(jenkins_bytes(line.as_bytes()), line);
                }
            }
        }
        map
    })
}

fn extra() -> &'static RwLock<HashMap<u32, String>> {
    static EXTRA: OnceLock<RwLock<HashMap<u32, String>>> = OnceLock::new();
    EXTRA.get_or_init(Default::default)
}

/// Makes more names known (hashed with the usual lowercase joaat), e.g. the
/// model and texture names found in the game's archives.
pub fn register<I, S>(names: I)
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut map = extra().write().unwrap_or_else(|e| e.into_inner());
    for name in names {
        let name = name.as_ref();
        map.entry(joaat(name)).or_insert_with(|| name.to_owned());
    }
}

/// How many names were registered with [`register`].
pub fn registered() -> usize {
    extra().read().map(|m| m.len()).unwrap_or(0)
}

fn lookup(hash: u32, builtin_first: bool) -> Option<String> {
    let from_builtin = || builtin().get(&hash).map(|s| (*s).to_owned());
    let from_extra = || extra().read().ok().and_then(|m| m.get(&hash).cloned());
    if builtin_first { from_builtin().or_else(from_extra) } else { from_extra().or_else(from_builtin) }
}

/// A structure, field or enum name: the known name or `hash_XXXXXXXX`; empty for 0.
pub fn field(hash: u32) -> String {
    if hash == 0 {
        return String::new();
    }
    lookup(hash, true).unwrap_or_else(|| format!("hash_{hash:08X}"))
}

/// A hash value stored in a field (usually a model, texture or other asset
/// name): like [`field`] but registered names win.
pub fn value(hash: u32) -> String {
    if hash == 0 {
        return String::new();
    }
    lookup(hash, false).unwrap_or_else(|| format!("hash_{hash:08X}"))
}

/// The hash a name in the XML stands for (`hash_XXXXXXXX` or a name).
pub fn parse(text: &str) -> u32 {
    match text.strip_prefix("hash_").and_then(|h| u32::from_str_radix(h, 16).ok()) {
        Some(h) => h,
        None if text.is_empty() => 0,
        None => {
            let exact = jenkins_bytes(text.as_bytes());
            if builtin().get(&exact).is_some_and(|n| *n == text) { exact } else { joaat(text) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_names() {
        assert_eq!(field(jenkins_bytes(b"CMapData")), "CMapData");
        assert_eq!(field(jenkins_bytes(b"AABB")), "AABB");
        assert_eq!(field(jenkins_bytes(b"aabb")), "aabb");
        assert_eq!(field(1), "hash_00000001");
        assert_eq!(field(0), "");
        assert_eq!(parse("hash_00000001"), 1);
        assert_eq!(parse("AABB"), jenkins_bytes(b"AABB"));
        assert_eq!(parse("adder"), 0xB779_A091);
    }

    #[test]
    fn registered_names() {
        register(["openrage_test_model"]);
        assert_eq!(value(joaat("openrage_test_model")), "openrage_test_model");
    }
}
