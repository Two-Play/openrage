//! Probe: the bone tags a ped part's meshes name, and which a skeleton has.
fn main() {
    let game = std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    let ydd = std::env::args().nth(2).unwrap();
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
    let base = rage_formats::drawable::parse_fragment_rdr2(&read(&format!(
        "packs_0.rpf/packs/base/models/metaped_fragments.rpf/{}",
        std::env::var("BASE").unwrap_or("meta_base_male.yft".into())
    )))
    .unwrap()
    .drawable
    .skeleton
    .unwrap();
    let res = read(&ydd);
    let root = rage_resource::Resource::ROOT;
    let list = res.u64(root + 0x30).unwrap();
    let count = res.u16(root + 0x38).unwrap() as u64;
    for k in 0..count {
        let d = res.u64(list + k * 8).unwrap();
        let sk = res.u64(d + 0x18).unwrap();
        let w: Vec<String> = (0..16).map(|i| format!("{:x}", res.u64(sk + i * 8).unwrap_or(0))).collect();
        println!("drawable {k} skeleton @{sk:x}: {}", w.join(" "));
    }
    for (h, d) in rage_formats::drawable::parse_ydd_rdr2(&res).unwrap() {
        let mut tags: Vec<u16> = d.meshes.iter().flat_map(|m| m.bone_tags.iter().copied()).collect();
        tags.sort();
        tags.dedup();
        let missing: Vec<u16> = tags.iter().copied().filter(|t| base.find_tag(*t).is_none()).collect();
        println!(
            "{h:08x}: {} tags, {} not in base: {}",
            tags.len(),
            missing.len(),
            missing.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(",")
        );
        if let Some(s) = &d.skeleton {
            println!(
                "   own skeleton: {}",
                s.bones.iter().map(|b| format!("{}:{}", b.name, b.tag)).collect::<Vec<_>>().join(" ")
            );
        }
    }
}
