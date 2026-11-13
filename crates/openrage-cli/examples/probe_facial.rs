//! Probe: resources with many "FACIAL_" strings (where the face rig is defined).
use rayon::prelude::*;
fn main() {
    let game = std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    let store =
        rage_rpf::rpf8_keys::KeyStore::load(&rage_rpf::rpf8_keys::KeyStore::default_path().unwrap()).unwrap().unwrap();
    let keys = std::sync::Arc::new(store.to_keys().unwrap());
    let names = rage_rpf::rpf8_names::NameIndex::load(&rage_rpf::rpf8_names::NameIndex::default_path(&game).unwrap())
        .unwrap()
        .map(std::sync::Arc::new);
    rage_rpf::rpf8::set_defaults(Some(keys.clone()), names);
    let tops: Vec<_> = rage_rpf::rpf8_keys::archives_under(&game)
        .into_iter()
        .filter(|p| {
            let s = p.to_string_lossy();
            std::env::var_os("ALL").is_some() || s.contains("packs_") || s.contains("update_") || s.contains("anim_0")
        })
        .collect();
    let hits: Vec<(usize, String)> = tops
        .par_iter()
        .flat_map_iter(|f| {
            let mut out = Vec::new();
            fn go(r: &rage_rpf::Rpf8, path: &str, out: &mut Vec<(usize, String)>) {
                let Some(es) = r.entries() else { return };
                for (i, e) in es.iter().enumerate() {
                    if e.ext_id == 0 {
                        if let Ok(c) = r.open_child(e) {
                            go(&c, &format!("{path}/{}", r.entry_name(i)), out);
                        }
                        continue;
                    }
                    if !e.is_resource || e.size_on_disk > 32 << 20 {
                        continue;
                    }
                    let n = r.entry_name(i);
                    if n.ends_with(".ytd") || n.ends_with(".ycd") || n.ends_with(".awc") {
                        continue;
                    }
                    let Ok(res) = r.read_resource(e) else { continue };
                    if std::env::var_os("TAGS").is_some() {
                        let wanted: Vec<u16> =
                            std::env::var("TAGS").unwrap().split(",").map(|t| t.parse().unwrap()).collect();
                        let sks: Vec<rage_formats::drawable::Skeleton> = if n.ends_with(".ydd") {
                            rage_formats::drawable::parse_ydd_rdr2(&res)
                                .map(|v| v.into_iter().filter_map(|(_, d)| d.skeleton).collect())
                                .unwrap_or_default()
                        } else if n.ends_with(".yft") {
                            rage_formats::drawable::parse_fragment_rdr2(&res)
                                .ok()
                                .and_then(|f| f.drawable.skeleton)
                                .into_iter()
                                .collect()
                        } else {
                            vec![]
                        };
                        for sk in sks {
                            let c = wanted.iter().filter(|t| sk.find_tag(**t).is_some()).count();
                            if c >= wanted.len().min(3) {
                                out.push((c * 1000 + sk.bones.len(), format!("{path}/{n}")));
                            }
                        }
                        continue;
                    }
                    let c = memchr_count(&res.system, b"FACIAL_");
                    if c >= 20 {
                        out.push((c, format!("{path}/{n}")));
                    }
                }
            }
            fn memchr_count(h: &[u8], n: &[u8]) -> usize {
                h.windows(n.len()).filter(|w| *w == n).count()
            }
            if let Ok(r) = rage_rpf::Rpf8::open_default(f) {
                go(&r, &f.strip_prefix(&game).unwrap().to_string_lossy(), &mut out);
            }
            out
        })
        .collect();
    let mut hits = hits;
    hits.sort();
    hits.reverse();
    for (c, p) in hits.iter().filter(|(_, p)| std::env::var_os("TAGS").is_some() || !p.contains("/cs_")).take(40) {
        println!("{c:5} {p}");
    }
    println!("{} resources", hits.len());
}
