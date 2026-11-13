//! Probe: hashes of drawables inside every .ydd of the ped archives.
use rage_rpf::Rpf8;
use rayon::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
fn main() {
    let mut args = std::env::args().skip(1);
    let game = std::path::PathBuf::from(args.next().unwrap());
    let wanted: Vec<u32> = args.map(|h| u32::from_str_radix(&h, 16).unwrap()).collect();
    let store =
        rage_rpf::rpf8_keys::KeyStore::load(&rage_rpf::rpf8_keys::KeyStore::default_path().unwrap()).unwrap().unwrap();
    let keys = Arc::new(store.to_keys().unwrap());
    let t = std::time::Instant::now();
    let files = AtomicUsize::new(0);
    let inner = AtomicUsize::new(0);
    let bytes = AtomicUsize::new(0);
    fn walk(r: &Rpf8, path: &str, out: &mut Vec<(String, usize)>) {
        let Some(es) = r.entries() else { return };
        for (i, e) in es.iter().enumerate() {
            if e.ext_id == 0 {
                if let Ok(c) = r.open_child(e) {
                    walk(&c, &format!("{path}/{}", r.entry_name(i)), out);
                }
            } else if r.entry_name(i).ends_with(".ydd") {
                out.push((path.to_string(), i));
            }
        }
    }
    let tops: Vec<_> = rage_rpf::rpf8_keys::archives_under(&game)
        .into_iter()
        .filter(|p| {
            let _s = p;
            true
        })
        .collect();
    tops.par_iter().for_each(|f| {
        let Ok(r) = Rpf8::open_with_keys(f, Some(keys.clone())) else { return };
        fn go(r: &Rpf8, path: &str, wanted: &[u32], files: &AtomicUsize, inner: &AtomicUsize, bytes: &AtomicUsize) {
            let Some(es) = r.entries() else { return };
            for (i, e) in es.iter().enumerate() {
                if e.ext_id == 0 {
                    if let Ok(c) = r.open_child(e) {
                        go(&c, &format!("{path}/{}", r.entry_name(i)), wanted, files, inner, bytes);
                    }
                } else if r.entry_name(i).ends_with(".ydd") {
                    let Ok(res) = r.read_resource(e) else { continue };
                    files.fetch_add(1, Ordering::Relaxed);
                    bytes.fetch_add(e.size_on_disk as usize, Ordering::Relaxed);
                    let root = rage_resource::Resource::ROOT;
                    let (Ok(h), Ok(c)) = (res.u64(root + 0x20), res.u16(root + 0x38)) else { continue };
                    for k in 0..c as u64 {
                        if let Ok(x) = res.u32(h + k * 4) {
                            inner.fetch_add(1, Ordering::Relaxed);
                            if wanted.contains(&x) {
                                println!("{x:08X} in {path}/{} #{k}", r.entry_name(i));
                            }
                        }
                    }
                }
            }
        }
        go(&r, &f.strip_prefix(&game).unwrap().to_string_lossy(), &wanted, &files, &inner, &bytes);
    });
    let _ = walk;
    eprintln!(
        "{} ydd, {} drawables, {} MB, {:.1}s",
        files.load(Ordering::Relaxed),
        inner.load(Ordering::Relaxed),
        bytes.load(Ordering::Relaxed) >> 20,
        t.elapsed().as_secs_f64()
    );
}
