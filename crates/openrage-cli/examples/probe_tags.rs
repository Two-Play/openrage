//! Probe: find runs of base-skeleton bone tags (u16) in a ped part's resource.
fn main() {
    let game = std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    let store =
        rage_rpf::rpf8_keys::KeyStore::load(&rage_rpf::rpf8_keys::KeyStore::default_path().unwrap()).unwrap().unwrap();
    let names = rage_rpf::rpf8_names::NameIndex::load(&rage_rpf::rpf8_names::NameIndex::default_path(&game).unwrap())
        .unwrap()
        .map(std::sync::Arc::new);
    rage_rpf::rpf8::set_defaults(Some(std::sync::Arc::new(store.to_keys().unwrap())), names);
    let read = |p: &str| {
        let (node, _) = rage_mods::resolve(&game, p, None).unwrap();
        let rage_rpf::vfs::Node::Rpf8File(rpf, i) = node else { panic!() };
        rpf.read_resource(&rpf.entries().unwrap()[i]).unwrap()
    };
    let base = rage_formats::drawable::parse_fragment_rdr2(&read(
        "packs_0.rpf/packs/base/models/metaped_fragments.rpf/meta_base_male.yft",
    ))
    .unwrap()
    .drawable
    .skeleton
    .unwrap();
    let tags: std::collections::HashMap<u16, String> = base.bones.iter().map(|b| (b.tag, b.name.clone())).collect();
    let indices: std::collections::HashMap<u16, usize> =
        base.bones.iter().enumerate().map(|(i, b)| (b.tag, i)).collect();
    for p in std::env::args().skip(2) {
        let res = read(&p);
        if let Ok(dump) = std::env::var("DUMP") {
            for a in dump.split(',') {
                let a = u64::from_str_radix(a, 16).unwrap();
                let w: Vec<String> = (0..24).map(|k| format!("{:x}", res.u64(a + k * 8).unwrap_or(0))).collect();
                println!("dump {a:x}: {}", w.join(" "));
            }
        }
        for (label, buf, basep) in [("sys", &res.system, 0x5000_0000u64), ("gfx", &res.graphics, 0x6000_0000u64)] {
            let words: Vec<u16> = buf.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            let mut i = 0;
            while i < words.len() {
                let mut j = i;
                while j < words.len() && words[j] != 0 && tags.contains_key(&words[j]) {
                    j += 1;
                }
                if j - i >= 4 {
                    let n: Vec<String> =
                        words[i..j].iter().take(20).map(|t| format!("{}({})", tags[t], indices[t])).collect();
                    let at = basep + i as u64 * 2;
                    println!("{p} {label} @{at:x} run {}: {}", j - i, n.join(" "));
                    for (l2, b2, bp2) in [("sys", &res.system, 0x5000_0000u64), ("gfx", &res.graphics, 0x6000_0000u64)]
                    {
                        for (k, c) in b2.chunks_exact(8).enumerate() {
                            let v = u64::from_le_bytes(c.try_into().unwrap());
                            if v >= at.saturating_sub(0x40) && v <= at {
                                println!("    pointer {v:x} (target-{:x}) at {l2} {:x}", at - v, bp2 + k as u64 * 8);
                            }
                        }
                    }
                }
                i = j.max(i + 1);
            }
        }
    }
}
