//! Facts about Scaleform movies (`.gfx`) and Bink videos (`.bik`, `.bk2`) for display.

use anyhow::Result;

pub use rage_formats::media::is_media;

/// The first few names and how many there are in all.
fn short_list(names: &[String]) -> String {
    const SHOWN: usize = 8;
    let mut s = names.iter().take(SHOWN).cloned().collect::<Vec<_>>().join(", ");
    if names.len() > SHOWN {
        s.push_str(&format!(", … ({} in all)", names.len()));
    }
    s
}

/// Facts about a media file, as (label, value) rows.
pub fn rows(data: &[u8]) -> Result<Vec<(String, String)>> {
    use rage_formats::media;
    Ok(if rage_formats::bink::is_bink(data) {
        let v = rage_formats::bink::read(data)?;
        let mut rows = vec![
            ("Format".into(), format!("Bink {} video, revision {}", v.version, v.revision as char)),
            (
                "Resolution".into(),
                format!("{} × {}{}", v.width, v.height, if v.has_alpha() { ", with alpha" } else { "" }),
            ),
            ("Frames".into(), format!("{} at {:.2} fps", v.frames(), v.frames_per_second())),
            ("Length".into(), format!("{:.1} s", v.duration())),
        ];
        let tracks: Vec<String> = v
            .tracks
            .iter()
            .map(|t| format!("{} Hz, {}", t.sample_rate, if t.channels == 2 { "stereo" } else { "mono" }))
            .collect();
        rows.push(("Sound".into(), if tracks.is_empty() { "none".into() } else { tracks.join("; ") }));
        rows
    } else {
        let g = media::gfx(data)?;
        vec![
            (
                "Format".into(),
                format!("Scaleform movie, version {}{}", g.version, if g.compressed { ", compressed" } else { "" }),
            ),
            ("Stage".into(), format!("{} × {}", g.width, g.height)),
            ("Frames".into(), format!("{} at {} fps", g.frames, g.fps)),
            ("Exports".into(), short_list(&g.exports)),
            ("Images".into(), short_list(&g.images)),
        ]
    })
}
