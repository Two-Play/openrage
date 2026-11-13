//! Probe: dump every RDR2 expression stream (lengths, opcodes, operand bytes) as JSON lines.
use std::io::Write;
fn main() {
    let game = std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    let out = std::env::args().nth(2).unwrap();
    let store =
        rage_rpf::rpf8_keys::KeyStore::load(&rage_rpf::rpf8_keys::KeyStore::default_path().unwrap()).unwrap().unwrap();
    let names = rage_rpf::rpf8_names::NameIndex::load(&rage_rpf::rpf8_names::NameIndex::default_path(&game).unwrap())
        .unwrap()
        .map(std::sync::Arc::new);
    rage_rpf::rpf8::set_defaults(Some(std::sync::Arc::new(store.to_keys().unwrap())), names);
    let mut f = std::io::BufWriter::new(std::fs::File::create(out).unwrap());
    let (node, _) = rage_mods::resolve(&game, "anim_0.rpf/anim/expressions.rpf", None).unwrap();
    let rage_rpf::vfs::Node::Rpf8Dir(rpf, _) = node else { panic!("not a dir") };
    let entries = rpf.entries().unwrap().to_vec();
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    let mut n = 0;
    let (mut total, mut parsed) = (0usize, 0usize);
    for (i, e) in entries.iter().enumerate() {
        if !e.is_resource {
            continue;
        }
        let Ok(res) = rpf.read_resource(e) else { continue };
        let file = rpf.entry_name(i);
        parsed += rage_formats::expression::parse_rdr2(&res).map_or(0, |d| d.expressions.len());
        let root = rage_resource::Resource::ROOT;
        let (Ok(buckets), Ok(count)) = (res.u64(root + 0x48), res.u16(root + 0x50)) else { continue };
        for b in 0..count as u64 {
            let mut entry = res.u64(buckets + b * 8).unwrap_or(0);
            while entry != 0 {
                total += 1;
                let x = res.u64(entry + 8).unwrap_or(0);
                let name = res.string(res.u64(x + 0x70).unwrap_or(0)).unwrap_or_default();
                let sp = res.u64(x + 0x30).unwrap_or(0);
                let sc = res.u16(x + 0x38).unwrap_or(0) as u64;
                let tp = res.u64(x + 0x40).unwrap_or(0);
                let tc = res.u16(x + 0x48).unwrap_or(0) as u64;
                let tracks: Vec<String> =
                    (0..tc).map(|k| format!("{:08x}", res.u32(tp + k * 4).unwrap_or(0))).collect();
                for k in 0..sc {
                    let s = res.u64(sp + k * 8).unwrap_or(0);
                    let (l1, l2, l3) = (
                        res.u32(s + 4).unwrap_or(0) as usize,
                        res.u32(s + 8).unwrap_or(0) as usize,
                        res.u16(s + 12).unwrap_or(0) as usize,
                    );
                    let d = res.bytes(s + 16, l1 + l2 + l3).unwrap_or(&[]);
                    if d.len() != l1 + l2 + l3 {
                        continue;
                    }
                    writeln!(f, "{{\"file\":\"{file}\",\"expr\":\"{name}\",\"l1\":{l1},\"l2\":{l2},\"l3\":{l3},\"tracks\":\"{}\",\"d1\":\"{}\",\"d2\":\"{}\",\"ops\":\"{}\"}}", tracks.join(" "), hex(&d[..l1]), hex(&d[l1..l1 + l2]), hex(&d[l1 + l2..])).unwrap();
                    n += 1;
                }
                entry = res.u64(entry + 0x10).unwrap_or(0);
            }
        }
    }
    eprintln!("{n} streams; {parsed} of {total} expressions decoded");
}
