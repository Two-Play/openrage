//! Probe: parent order and tags of an RDR2 base skeleton.
fn main() {
    let game = std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    let name = std::env::args().nth(2).unwrap();
    let store =
        rage_rpf::rpf8_keys::KeyStore::load(&rage_rpf::rpf8_keys::KeyStore::default_path().unwrap()).unwrap().unwrap();
    let names = rage_rpf::rpf8_names::NameIndex::load(&rage_rpf::rpf8_names::NameIndex::default_path(&game).unwrap())
        .unwrap()
        .map(std::sync::Arc::new);
    rage_rpf::rpf8::set_defaults(Some(std::sync::Arc::new(store.to_keys().unwrap())), names);
    let (node, _) =
        rage_mods::resolve(&game, &format!("packs_0.rpf/packs/base/models/metaped_fragments.rpf/{name}"), None)
            .unwrap();
    let rage_rpf::vfs::Node::Rpf8File(rpf, i) = node else { panic!() };
    let res = rpf.read_resource(&rpf.entries().unwrap()[i]).unwrap();
    let sk = rage_formats::drawable::parse_fragment_rdr2(&res).unwrap().drawable.skeleton.unwrap();
    let bad: Vec<_> = sk.bones.iter().enumerate().filter(|(i, b)| b.parent >= 0 && b.parent as usize >= *i).collect();
    println!("{} bones, {} with parent after them", sk.bones.len(), bad.len());
    for (i, b) in bad.iter().take(10) {
        println!("  {i} {} parent {} ({})", b.name, b.parent, sk.bones[b.parent as usize].name);
    }
    let mut tags = std::collections::HashMap::new();
    for b in &sk.bones {
        *tags.entry(b.tag).or_insert(0) += 1;
    }
    println!("duplicate tags: {}", tags.values().filter(|&&c| c > 1).count());
    for (t, _) in tags.iter().filter(|(_, c)| **c > 1).take(12) {
        let names: Vec<String> =
            sk.bones.iter().enumerate().filter(|(_, b)| b.tag == *t).map(|(i, b)| format!("{i}:{}", b.name)).collect();
        println!("  tag {t}: {}", names.join(", "));
    }
    if let Ok(ids) = std::env::var("TAGS") {
        for id in ids.split_whitespace() {
            let t: u16 = id.parse().unwrap();
            let n = sk.find_tag(t).map(|b| sk.bones[b].name.clone()).unwrap_or("-".into());
            print!("{t}:{n} ");
        }
        println!();
    }
    if std::env::var("LIST").is_ok() {
        for (i, b) in sk.bones.iter().enumerate() {
            print!("{i}:{} ", b.name);
        }
        println!();
    }
    if let Some(ycd) = std::env::args().nth(3) {
        let (path, clip) = ycd.split_once(':').unwrap();
        let dict = openrage_app::anim::load_clips(&game, None, path).unwrap();
        let c = dict.clips.iter().find(|c| c.name == clip).unwrap();
        let a = &dict.animations[c.parts[0].animation];
        let mut names = Vec::new();
        for t in &a.tracks {
            let n = sk.find_tag(t.bone).map(|b| sk.bones[b].name.clone()).unwrap_or(format!("?{}", t.bone));
            names.push(format!("{n}/{}", t.track));
        }
        println!("{}", names.join(" "));
    }
}

#[allow(dead_code)]
fn unused() {}
