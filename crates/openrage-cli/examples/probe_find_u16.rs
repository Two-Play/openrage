//! Probe: where u16 values occur in a resource (system pages).
fn main() {
    let game = std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    let path = std::env::args().nth(2).unwrap();
    let vals: Vec<u16> = std::env::args().skip(3).map(|s| s.parse().unwrap()).collect();
    let store =
        rage_rpf::rpf8_keys::KeyStore::load(&rage_rpf::rpf8_keys::KeyStore::default_path().unwrap()).unwrap().unwrap();
    let names = rage_rpf::rpf8_names::NameIndex::load(&rage_rpf::rpf8_names::NameIndex::default_path(&game).unwrap())
        .unwrap()
        .map(std::sync::Arc::new);
    rage_rpf::rpf8::set_defaults(Some(std::sync::Arc::new(store.to_keys().unwrap())), names);
    let (node, _) = rage_mods::resolve(&game, &path, None).unwrap();
    let rage_rpf::vfs::Node::Rpf8File(rpf, i) = node else { panic!() };
    let res = rpf.read_resource(&rpf.entries().unwrap()[i]).unwrap();
    for v in vals {
        let hits: Vec<String> = res
            .system
            .windows(2)
            .enumerate()
            .filter(|(k, w)| k % 2 == 0 && u16::from_le_bytes([w[0], w[1]]) == v)
            .map(|(k, _)| format!("{:x}", 0x5000_0000 + k))
            .take(8)
            .collect();
        println!("{v}: {}", hits.join(" "));
    }
    let strings: Vec<&str> = std::str::from_utf8(&res.system).ok().into_iter().collect();
    let _ = strings;
    let text = String::from_utf8_lossy(&res.system);
    println!("FACIAL names: {}", text.matches("FACIAL_").count());
}
