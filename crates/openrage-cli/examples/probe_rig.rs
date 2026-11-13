//! Probe: extend the male base skeleton with a face rig template and compare
//! what bcustom sets at neutral values with the template's rest pose.
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
    let tpl = rage_formats::drawable::parse_ydd_rdr2(&read(
        "packs_1.rpf/packs/base/models/metapeds_cutscene_a_f.rpf/cs_creoledoctor_ms1_merge_2.ydd",
    ))
    .unwrap()
    .into_iter()
    .filter_map(|(_, d)| d.skeleton)
    .max_by_key(|s| s.bones.len())
    .unwrap();
    println!("template {} bones; root {:?} parent", tpl.bones.len(), tpl.bones.first().map(|b| (&b.name, b.parent)));
    for (i, b) in tpl.bones.iter().enumerate().take(12) {
        let p = usize::try_from(b.parent).ok().map(|p| tpl.bones[p].name.clone()).unwrap_or("-".into());
        println!(
            "  {i} {} tag {} parent {} t {:?} in base: {}",
            b.name,
            b.tag,
            p,
            b.translation,
            base.find_tag(b.tag).is_some()
        );
    }
    let mut ident = 0;
    let mut big = Vec::new();
    for b in &tpl.bones {
        let w = b.rotation[3].abs().min(1.0);
        let angle = 2.0 * w.acos().to_degrees();
        if angle < 5.0 {
            ident += 1
        } else {
            big.push(format!("{}:{angle:.0}", b.name));
        }
    }
    println!(
        "template local rotations under 5 degrees: {ident} of {}; others: {}",
        tpl.bones.len(),
        big.iter().take(25).cloned().collect::<Vec<_>>().join(" ")
    );
    let in_base = tpl.bones.iter().filter(|b| base.find_tag(b.tag).is_some()).count();
    println!("{in_base} of the template's bones are in the base");
    let dict =
        rage_formats::expression::parse_rdr2(&read("anim_0.rpf/anim/expressions.rpf/meta_base_male.yed")).unwrap();
    let e = dict.expressions.iter().find(|e| e.name == "pack:/bcustom.expr").unwrap();
    let covered = e
        .tracks
        .iter()
        .filter(|t| t.track <= 2)
        .filter(|t| tpl.find_tag(t.bone).is_some() || base.find_tag(t.bone).is_some())
        .count();
    println!(
        "bcustom bone tracks covered by template+base: {covered} of {}",
        e.tracks.iter().filter(|t| t.track <= 2).count()
    );
    if let Ok(head) = std::env::var("HEAD") {
        let tags: Vec<u16> = head.split(',').map(|t| t.parse().unwrap()).collect();
        let driven: std::collections::BTreeSet<u16> =
            e.tracks.iter().filter(|t| t.track <= 2).map(|t| t.bone).collect();
        let (mut a, mut b, mut c, mut d) = (0, 0, 0, 0);
        for t in &tags {
            match (driven.contains(t), tpl.find_tag(*t).is_some()) {
                (true, true) => a += 1,
                (true, false) => b += 1,
                (false, true) => c += 1,
                (false, false) => d += 1,
            }
        }
        println!("head face tags: driven+template {a}, driven-only {b}, template-only {c}, neither {d}");
        let missing_driven: Vec<u16> = e
            .tracks
            .iter()
            .filter(|t| t.track <= 2 && tpl.find_tag(t.bone).is_none() && base.find_tag(t.bone).is_none())
            .map(|t| t.bone)
            .collect();
        println!("driven bones without template: {:?}", &missing_driven[..missing_driven.len().min(40)]);
    }
}
