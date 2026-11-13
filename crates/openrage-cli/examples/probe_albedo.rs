//! Probe: full XML of a big RDR2 PSO file.
fn main() {
    let game = std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    let path = std::env::args().nth(2).unwrap();
    let out = std::env::args().nth(3).unwrap();
    let store =
        rage_rpf::rpf8_keys::KeyStore::load(&rage_rpf::rpf8_keys::KeyStore::default_path().unwrap()).unwrap().unwrap();
    let names = rage_rpf::rpf8_names::NameIndex::load(&rage_rpf::rpf8_names::NameIndex::default_path(&game).unwrap())
        .unwrap()
        .map(std::sync::Arc::new);
    if let Some(n) = &names {
        openrage_app::rdr2::register_names(n);
    }
    rage_rpf::rpf8::set_defaults(Some(std::sync::Arc::new(store.to_keys().unwrap())), names);
    let t = std::time::Instant::now();
    let (node, _) = rage_mods::resolve(&game, &path, None).unwrap();
    let rage_rpf::vfs::Node::Rpf8File(rpf, i) = node else { panic!() };
    let data = rpf.read(&rpf.entries().unwrap()[i]).unwrap();
    let pso = rage_formats::pso::parse_rdr2(&data).unwrap();
    let (xml, errors) = pso.to_xml_with_limit(1 << 30);
    eprintln!(
        "{} bytes in, {} MB xml, {} errors, {:.1}s",
        data.len(),
        xml.len() >> 20,
        errors.len(),
        t.elapsed().as_secs_f64()
    );
    std::fs::write(out, xml).unwrap();
}
