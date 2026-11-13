//! OpenRAGE's own licence, the credits from `NOTICE.md` and the licences of
//! every third-party crate in the programs, for the About dialog and
//! `openrage licenses`.
//!
//! The crate list is generated from `Cargo.lock` by
//! `dev/third_party_licenses.py` (CI checks it is current) and embedded.

use std::sync::OnceLock;

/// OpenRAGE's version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Where the source code is.
pub const REPOSITORY: &str = "https://github.com/Two-Play/openrage";
/// OpenRAGE's own licence.
pub const LICENSE: &str = "GPL-3.0-only";

/// Copyright and licence notice of OpenRAGE.
pub const NOTICE_SHORT: &str = "OpenRAGE is free software: you can redistribute it and/or modify it under the terms of \
the GNU General Public License, version 3, as published by the Free Software Foundation. It comes with \
ABSOLUTELY NO WARRANTY. The TFIT2 cipher (crates/rage-tfit2) is licensed under the GNU Affero General Public \
License, version 3. OpenRAGE is not affiliated with or endorsed by Rockstar Games or Take-Two Interactive; \
it contains no game files and no encryption keys.";

const GPL_TEXT: &str = include_str!("../../../LICENSE");
const AGPL_TEXT: &str = include_str!("../../rage-tfit2/LICENSE");
const NOTICE_MD: &str = include_str!("../../../NOTICE.md");
const DATA: &str = include_str!("third_party_licenses.json");

/// A third-party crate in the programs.
#[derive(Debug, Clone)]
pub struct Dependency {
    pub name: String,
    pub version: String,
    /// The licence expression the crate declares (e.g. `MIT OR Apache-2.0`).
    pub license: String,
    pub repository: String,
    /// Indices into [`texts`].
    pub texts: Vec<usize>,
}

/// One licence text as a crate ships it (with its copyright lines).
#[derive(Debug, Clone)]
pub struct LicenseText {
    /// SPDX identifier.
    pub id: String,
    pub name: String,
    pub text: String,
}

/// A section of `NOTICE.md`: whose work OpenRAGE uses, and under which terms.
#[derive(Debug, Clone)]
pub struct Credit {
    pub title: String,
    pub text: String,
}

struct Data {
    dependencies: Vec<Dependency>,
    texts: Vec<LicenseText>,
}

fn data() -> &'static Data {
    static DATA_CELL: OnceLock<Data> = OnceLock::new();
    DATA_CELL.get_or_init(|| {
        let json: serde_json::Value = serde_json::from_str(DATA).expect("embedded licence data is valid JSON");
        let s = |v: &serde_json::Value, k: &str| v[k].as_str().unwrap_or_default().to_owned();
        let texts = json["licenses"]
            .as_array()
            .map(|a| a.iter().map(|t| LicenseText { id: s(t, "id"), name: s(t, "name"), text: s(t, "text") }).collect())
            .unwrap_or_default();
        let dependencies = json["crates"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|c| Dependency {
                        name: s(c, "name"),
                        version: s(c, "version"),
                        license: s(c, "license"),
                        repository: s(c, "repository"),
                        texts: c["texts"]
                            .as_array()
                            .map(|t| t.iter().filter_map(|i| i.as_u64()).map(|i| i as usize).collect())
                            .unwrap_or_default(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Data { dependencies, texts }
    })
}

/// Every third-party crate, sorted by name.
pub fn dependencies() -> &'static [Dependency] {
    &data().dependencies
}

/// The licence texts the crates refer to.
pub fn texts() -> &'static [LicenseText] {
    &data().texts
}

/// The full licence text(s) of a crate, one after another.
pub fn dependency_text(dep: &Dependency) -> String {
    dep.texts
        .iter()
        .filter_map(|&i| texts().get(i))
        .map(|t| format!("{} ({})\n\n{}", t.name, t.id, t.text.trim_end()))
        .collect::<Vec<_>>()
        .join("\n\n────────\n\n")
}

/// The GNU GPL v3 (OpenRAGE's licence).
pub fn gpl_text() -> &'static str {
    GPL_TEXT
}

/// The GNU AGPL v3 (the TFIT2 cipher crate).
pub fn agpl_text() -> &'static str {
    AGPL_TEXT
}

/// `NOTICE.md` as it is.
pub fn notice() -> &'static str {
    NOTICE_MD
}

/// The sections of `NOTICE.md` (`## Title` and its text, Markdown kept).
pub fn credits() -> Vec<Credit> {
    let mut out: Vec<Credit> = Vec::new();
    for line in NOTICE_MD.lines() {
        if let Some(title) = line.strip_prefix("## ") {
            out.push(Credit { title: title.trim().to_owned(), text: String::new() });
        } else if line.trim_start().starts_with("```") {
            // Code fences around quoted licence texts.
            continue;
        } else if let Some(c) = out.last_mut() {
            c.text.push_str(line);
            c.text.push('\n');
        }
    }
    for c in &mut out {
        c.text = c.text.trim().to_owned();
    }
    out
}

/// Number of crates per licence expression, most used first.
pub fn summary() -> Vec<(String, usize)> {
    let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for d in dependencies() {
        *counts.entry(d.license.as_str()).or_default() += 1;
    }
    let mut out: Vec<(String, usize)> = counts.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

/// Opens a web address in the system's browser.
pub fn open_url(url: &str) -> std::io::Result<()> {
    let mut cmd = if cfg!(target_os = "windows") {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else {
        std::process::Command::new("xdg-open")
    };
    cmd.arg(url).spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_data_is_complete() {
        let deps = dependencies();
        assert!(deps.len() > 100, "{} crates", deps.len());
        for d in deps {
            assert!(!d.license.is_empty(), "{} has no licence", d.name);
            assert!(!d.texts.is_empty(), "{} has no licence text", d.name);
            assert!(d.texts.iter().all(|&i| i < texts().len()));
        }
        assert!(deps.iter().any(|d| d.name == "slint"));
        assert!(!deps.iter().any(|d| d.name.starts_with("rage-") || d.name.starts_with("openrage")));
    }

    #[test]
    fn notice_sections_are_found() {
        let credits = credits();
        assert!(credits.iter().any(|c| c.title == "CodeWalker" && c.text.contains("MIT")));
        assert!(gpl_text().contains("GNU GENERAL PUBLIC LICENSE"));
        assert!(agpl_text().contains("GNU AFFERO GENERAL PUBLIC LICENSE"));
    }
}
