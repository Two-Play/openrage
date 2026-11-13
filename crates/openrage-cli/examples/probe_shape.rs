//! Probe: which bones each shaping expression moves for a ped's outfit.
fn main() {
    let game = std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    let ped = std::env::args().nth(2).unwrap();
    let store =
        rage_rpf::rpf8_keys::KeyStore::load(&rage_rpf::rpf8_keys::KeyStore::default_path().unwrap()).unwrap().unwrap();
    let names = rage_rpf::rpf8_names::NameIndex::load(&rage_rpf::rpf8_names::NameIndex::default_path(&game).unwrap())
        .unwrap()
        .map(std::sync::Arc::new);
    rage_rpf::rpf8::set_defaults(Some(std::sync::Arc::new(store.to_keys().unwrap())), names);
    let index = openrage_app::metaped::index(&game, |_, _| {}).unwrap();
    let outfits = openrage_app::metaped::load_outfits(&game, &index, &ped).unwrap();
    let values = &outfits[0].expressions;
    let read = |p: &str| {
        let (node, _) = rage_mods::resolve(&game, p, None).unwrap();
        let rage_rpf::vfs::Node::Rpf8File(rpf, i) = node else { panic!() };
        rpf.read_resource(&rpf.entries().unwrap()[i]).unwrap()
    };
    let sk = rage_formats::drawable::parse_fragment_rdr2(&read(
        "packs_0.rpf/packs/base/models/metaped_fragments.rpf/meta_base_male.yft",
    ))
    .unwrap()
    .drawable
    .skeleton
    .unwrap();
    let dict =
        rage_formats::expression::parse_rdr2(&read("anim_0.rpf/anim/expressions.rpf/meta_base_male.yed")).unwrap();
    let still = rage_formats::clip::Animation::empty();
    for name in ["pack:/bcustom.expr", "pack:/meta_cust.expr", "pack:/muscles.expr"] {
        let e = dict.expressions.iter().find(|e| e.name == name).unwrap();
        let b = openrage_app::anim::Binding::new(&sk, &still).with_expressions(&sk, &still, std::slice::from_ref(e));
        let controls: openrage_app::expr::Controls =
            values.iter().map(|(id, v)| ((*id as u16, 64u8), [*v; 4])).collect();
        let skin = b.skin_with(&sk, &still, 0.0, controls);
        let moved: Vec<String> = skin
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                let id = rage_formats::math::IDENTITY;
                m.iter().zip(id.iter()).any(|(a, b)| (a - b).abs() > 1e-4 || !a.is_finite())
            })
            .map(|(i, _)| sk.bones[i].name.clone())
            .collect();
        let bone_slots = e.tracks.iter().filter(|t| t.track <= 2 && sk.find_tag(t.bone).is_some()).count();
        let missing: Vec<u16> =
            e.tracks.iter().filter(|t| t.track <= 2 && sk.find_tag(t.bone).is_none()).map(|t| t.bone).collect();
        println!(
            "{name}: {} bones moved; bone tracks found {bone_slots}, missing {}: {:?}",
            moved.len(),
            missing.len(),
            &missing[..missing.len().min(10)]
        );
        println!("   {}", moved.iter().take(30).cloned().collect::<Vec<_>>().join(" "));
    }
}
