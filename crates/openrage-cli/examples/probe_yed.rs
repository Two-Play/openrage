//! Probe: parse an RDR2 expression dictionary and look for given track ids.
fn main() {
    let game = std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    let path = std::env::args().nth(2).unwrap();
    let ids: Vec<u16> = std::env::args().skip(3).map(|s| s.parse().unwrap()).collect();
    let store =
        rage_rpf::rpf8_keys::KeyStore::load(&rage_rpf::rpf8_keys::KeyStore::default_path().unwrap()).unwrap().unwrap();
    let names = rage_rpf::rpf8_names::NameIndex::load(&rage_rpf::rpf8_names::NameIndex::default_path(&game).unwrap())
        .unwrap()
        .map(std::sync::Arc::new);
    rage_rpf::rpf8::set_defaults(Some(std::sync::Arc::new(store.to_keys().unwrap())), names);
    let (node, _) = rage_mods::resolve(&game, &path, None).unwrap();
    let rage_rpf::vfs::Node::Rpf8File(rpf, i) = node else { panic!() };
    let res = rpf.read_resource(&rpf.entries().unwrap()[i]).unwrap();
    println!("resource version {}, {} + {} bytes", res.version, res.system.len(), res.graphics.len());
    if let Ok(dump) = std::env::var("DUMP") {
        for a in dump.split(',') {
            let a = u64::from_str_radix(a, 16).unwrap();
            let w: Vec<String> = (0..16).map(|k| format!("{:x}", res.u64(a + k * 8).unwrap_or(0))).collect();
            println!("dump {a:x}: {}", w.join(" "));
        }
    }
    match rage_formats::expression::parse_rdr2(&res) {
        Ok(d) => {
            for e in &d.expressions {
                let types: std::collections::BTreeMap<u8, usize> =
                    e.tracks.iter().fold(Default::default(), |mut m, t| {
                        *m.entry(t.track).or_insert(0) += 1;
                        m
                    });
                let hits: Vec<String> = e
                    .tracks
                    .iter()
                    .filter(|t| ids.contains(&t.bone))
                    .map(|t| format!("{}/{}", t.bone, t.track))
                    .collect();
                println!(
                    "{}: {} tracks {:?}, {} streams, {} vars; hits {}",
                    e.name,
                    e.tracks.len(),
                    types,
                    e.streams.len(),
                    e.variables,
                    hits.len()
                );
                if !hits.is_empty() {
                    println!("   {}", hits.join(" "));
                }
            }
        }
        Err(e) => println!("parse error: {e}"),
    }
}
