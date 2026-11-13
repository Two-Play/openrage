//! Probe: where entries with given hashes are (all RDR2 archives, nested).
use rage_rpf::Rpf8;
use std::sync::Arc;
fn main() {
    let mut args = std::env::args().skip(1);
    let game = std::path::PathBuf::from(args.next().unwrap());
    let wanted: Vec<u32> = args.map(|h| u32::from_str_radix(&h, 16).unwrap()).collect();
    let store =
        rage_rpf::rpf8_keys::KeyStore::load(&rage_rpf::rpf8_keys::KeyStore::default_path().unwrap()).unwrap().unwrap();
    let keys = Arc::new(store.to_keys().unwrap());
    fn walk(r: &Rpf8, path: &str, wanted: &[u32]) {
        let Some(es) = r.entries() else { return };
        for (i, e) in es.iter().enumerate() {
            if wanted.contains(&e.hash) {
                println!("{path} | {} ext {} res {}", r.entry_name(i), e.ext_id, e.is_resource);
            }
            if e.ext_id == 0 {
                if let Ok(c) = r.open_child(e) {
                    walk(&c, &format!("{path}/{}", r.entry_name(i)), wanted);
                }
            }
        }
    }
    for f in rage_rpf::rpf8_keys::archives_under(&game) {
        if let Ok(r) = Rpf8::open_with_keys(&f, Some(keys.clone())) {
            walk(&r, &f.strip_prefix(&game).unwrap().to_string_lossy(), &wanted);
        }
    }
}
