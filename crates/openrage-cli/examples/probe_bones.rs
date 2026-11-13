//! Probe: own skeleton size vs. bone indices used by each drawable of a .ydd.
fn main() {
    let game = std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    let store =
        rage_rpf::rpf8_keys::KeyStore::load(&rage_rpf::rpf8_keys::KeyStore::default_path().unwrap()).unwrap().unwrap();
    let names = rage_rpf::rpf8_names::NameIndex::load(&rage_rpf::rpf8_names::NameIndex::default_path(&game).unwrap())
        .unwrap()
        .map(std::sync::Arc::new);
    rage_rpf::rpf8::set_defaults(Some(std::sync::Arc::new(store.to_keys().unwrap())), names);
    for path in std::env::args().skip(2) {
        let (node, _) = rage_mods::resolve(&game, &path, None).unwrap();
        let rage_rpf::vfs::Node::Rpf8File(rpf, i) = node else { panic!() };
        let res = rpf.read_resource(&rpf.entries().unwrap()[i]).unwrap();
        for (h, d) in rage_formats::drawable::parse_ydd_rdr2(&res).unwrap() {
            let max = d.meshes.iter().flat_map(|m| m.bone_indices.iter().flatten().copied()).max();
            let mut used: Vec<u16> = d
                .meshes
                .iter()
                .flat_map(|m| {
                    m.bone_indices
                        .iter()
                        .zip(&m.bone_weights)
                        .flat_map(|(i, w)| (0..4).filter(|k| w[*k] > 0.0).map(move |k| i[k]))
                })
                .collect();
            used.sort();
            used.dedup();
            println!("   used: {used:?}");
            let sk = d.skeleton.as_ref().map(|s| s.bones.len());
            let names: Vec<String> = d
                .skeleton
                .as_ref()
                .map(|s| s.bones.iter().take(40).map(|b| b.name.clone()).collect())
                .unwrap_or_default();
            println!("{path} {h:08X}: own skeleton {sk:?}, max bone index {max:?}\n   {}", names.join(" "));
        }
    }
}
