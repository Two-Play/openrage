//! Probe: which skeletons (fragments, drawable dictionaries) contain given bone tags.
fn main() {
    let game = std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    let dir = std::env::args().nth(2).unwrap();
    let tags: Vec<u16> = std::env::args().skip(3).map(|s| s.parse().unwrap()).collect();
    let store =
        rage_rpf::rpf8_keys::KeyStore::load(&rage_rpf::rpf8_keys::KeyStore::default_path().unwrap()).unwrap().unwrap();
    let names = rage_rpf::rpf8_names::NameIndex::load(&rage_rpf::rpf8_names::NameIndex::default_path(&game).unwrap())
        .unwrap()
        .map(std::sync::Arc::new);
    rage_rpf::rpf8::set_defaults(Some(std::sync::Arc::new(store.to_keys().unwrap())), names);
    let (node, _) = rage_mods::resolve(&game, &dir, None).unwrap();
    let rage_rpf::vfs::Node::Rpf8Dir(rpf, _) = node else { panic!() };
    for (i, e) in rpf.entries().unwrap().iter().enumerate() {
        let name = rpf.entry_name(i);
        let Ok(res) = rpf.read_resource(e) else { continue };
        let sks: Vec<(String, rage_formats::drawable::Skeleton)> = if name.ends_with(".yft") {
            rage_formats::drawable::parse_fragment_rdr2(&res)
                .ok()
                .and_then(|f| f.drawable.skeleton)
                .map(|s| vec![(name.clone(), s)])
                .unwrap_or_default()
        } else if name.ends_with(".ydd") {
            rage_formats::drawable::parse_ydd_rdr2(&res)
                .map(|v| {
                    v.into_iter().filter_map(|(h, d)| d.skeleton.map(|s| (format!("{name}#{h:08x}"), s))).collect()
                })
                .unwrap_or_default()
        } else {
            vec![]
        };
        for (n, s) in sks {
            let hit = tags.iter().filter(|t| s.find_tag(**t).is_some()).count();
            if hit > 0 {
                println!("{n}: {} bones, {hit}/{} tags", s.bones.len(), tags.len());
            }
        }
    }
}
