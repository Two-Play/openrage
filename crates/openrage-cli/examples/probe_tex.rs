//! Probe: export RDR2 textures by name (through the texture index) as PNG.
fn main() {
    let mut args = std::env::args().skip(1);
    let game = std::path::PathBuf::from(args.next().unwrap());
    let out = std::path::PathBuf::from(args.next().unwrap());
    let store =
        rage_rpf::rpf8_keys::KeyStore::load(&rage_rpf::rpf8_keys::KeyStore::default_path().unwrap()).unwrap().unwrap();
    let names = rage_rpf::rpf8_names::NameIndex::load(&rage_rpf::rpf8_names::NameIndex::default_path(&game).unwrap())
        .unwrap()
        .map(std::sync::Arc::new);
    rage_rpf::rpf8::set_defaults(Some(std::sync::Arc::new(store.to_keys().unwrap())), names);
    let index = openrage_app::rdr2::texture_index(&game).unwrap();
    for name in args {
        let dict = if name.starts_with("metaped_tint") {
            let (node, _) = rage_mods::resolve(&game, "textures_0.rpf/textures/graphics.ytd", None).unwrap();
            let rage_rpf::vfs::Node::Rpf8File(rpf, i) = node else { panic!() };
            rage_formats::ytd::parse_rdr2(&rpf.read_resource(&rpf.entries().unwrap()[i]).unwrap()).unwrap()
        } else {
            let Some(loc) = index.find(&name) else {
                println!("{name}: not found");
                continue;
            };
            openrage_app::rdr2::open_dictionary(&game, loc).unwrap()
        };
        let Some(t) = dict.textures.iter().find(|t| t.name.eq_ignore_ascii_case(&name)) else {
            println!("{name}: not in dict");
            continue;
        };
        let rgba = t.to_rgba().unwrap();
        std::fs::write(out.join(format!("{name}.png")), rage_formats::png(t.width, t.height, &rgba).unwrap()).unwrap();
        println!("{name}: {}x{} {}", t.width, t.height, t.format.name());
    }
}
