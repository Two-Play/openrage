//! Opt-in tests against the user's local corpus; no game data is bundled.
//! RPF9_EXE=/path/to/reference.exe RPF9_ROOT=/path/to/game cargo test -p rage-rpf --test rpf9_local -- --ignored --nocapture
use rage_rpf::{rpf9::Archive, rpf9_exe::extract_scarlett_1_0_2_1};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
};

#[test]
#[ignore = "requires RPF9_EXE and RPF9_ROOT pointing at the user's local files"]
fn reads_external_corpus_and_copies_bounded_sample() {
    let root = PathBuf::from(std::env::var_os("RPF9_ROOT").expect("RPF9_ROOT"));
    let exe = PathBuf::from(std::env::var_os("RPF9_EXE").expect("RPF9_EXE"));
    let keys = extract_scarlett_1_0_2_1(&exe).unwrap();
    let mut paths = Vec::new();
    let mut directories = vec![root.clone()];
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir()
                && (entry.file_name() == "pack" || entry.path().starts_with(root.join("pack")))
            {
                directories.push(entry.path());
            } else if entry.path().extension().is_some_and(|ext| ext == "rpf") {
                paths.push(entry.path());
            }
        }
    }
    paths.sort();
    assert!(!paths.is_empty());
    let mut total = 0;
    for path in paths {
        let archive = Archive::open(&path, &keys).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(archive.metadata.entries.len(), archive.header.entry_count as usize);
        total += archive.metadata.entries.len();
        eprintln!("{}: {} entries", path.display(), archive.metadata.entries.len());
    }
    eprintln!("Total logical entries: {total}");
    // Only this small entry is copied in this test. Never load whole archives.
    let path = root.join("script.rpf");
    let mut archive = Archive::open(&path, &keys).unwrap();
    let entry = archive
        .metadata
        .entries
        .iter()
        .find(|e| e.extent.as_ref().is_some_and(|x| x.stored_size <= 1 << 20))
        .unwrap()
        .clone();
    let extent = entry.extent.unwrap();
    let mut copied = Vec::new();
    assert_eq!(archive.copy_raw(entry.index, &mut copied).unwrap(), extent.stored_size);
    let mut source = File::open(path).unwrap();
    source.seek(SeekFrom::Start(extent.offset)).unwrap();
    let mut expected = vec![0; extent.stored_size as usize];
    source.read_exact(&mut expected).unwrap();
    assert_eq!(copied, expected);
    assert_eq!(&copied[..4], &rage_rpf::rpf9::MAGIC.to_le_bytes());
}

#[test]
#[ignore = "requires RPF9_EXE and RPF9_ROOT pointing at the user's local files"]
fn decrypts_small_ctr_payload_to_valid_zlib_resource() {
    let root = PathBuf::from(std::env::var_os("RPF9_ROOT").expect("RPF9_ROOT"));
    let exe = PathBuf::from(std::env::var_os("RPF9_EXE").expect("RPF9_EXE"));
    let keys = extract_scarlett_1_0_2_1(&exe).unwrap();
    let path = root.join("appdata0.rpf");
    let archive = Archive::open(&path, &keys).unwrap();
    let entry =
        archive.metadata.entries.iter().find(|e| e.extent.as_ref().is_some_and(|x| x.stored_size <= 4096)).unwrap();
    let crypto = entry.crypto.as_ref().unwrap();
    assert_eq!(crypto.algorithm, 0x11);
    let extent = entry.extent.as_ref().unwrap();
    let mut file = File::open(&path).unwrap();
    file.seek(SeekFrom::Start(extent.offset)).unwrap();
    let mut data = vec![0; extent.stored_size as usize];
    file.read_exact(&mut data).unwrap();
    let mut cipher = rage_rpf::rpf9::TfitCtr::new(keys.payload_keys[&crypto.selector].clone(), crypto.iv).unwrap();
    for chunk in data.chunks_mut(7) {
        cipher.apply(chunk);
    }
    let mut zlib = flate2::read::ZlibDecoder::new(data.as_slice());
    let mut decoded = Vec::new();
    zlib.by_ref().take(4097).read_to_end(&mut decoded).unwrap();
    assert_eq!(decoded.len() as u64, extent.unpacked_size);
    assert_eq!(&decoded[..4], b"PSIN");
    eprintln!("CTR + zlib resource: {} decoded bytes", decoded.len());
}

#[test]
#[ignore = "requires RPF9_EXE and RPF9_ROOT pointing at the user's local files"]
fn decodes_bounded_payload_samples_including_sparse_ctr() {
    let root = PathBuf::from(std::env::var_os("RPF9_ROOT").expect("RPF9_ROOT"));
    let exe = PathBuf::from(std::env::var_os("RPF9_EXE").expect("RPF9_EXE"));
    let keys = extract_scarlett_1_0_2_1(&exe).unwrap();
    let mut sparse = 0;
    for name in ["appdata0.rpf", "common.rpf", "pack/story000_bvh4/story000_bvh4.rpf"] {
        let mut archive = Archive::open(&root.join(name), &keys).unwrap();
        let samples: Vec<_> = archive
            .metadata
            .entries
            .iter()
            .filter(|e| {
                e.extent.as_ref().is_some_and(|x| x.stored_size <= 1 << 20 && x.unpacked_size <= 4 << 20)
                    && e.resource_flags.is_none()
            })
            .take(32)
            .cloned()
            .collect();
        for entry in samples {
            let expected = entry.extent.as_ref().unwrap().unpacked_size;
            let decoded = archive
                .copy_decoded(entry.index, &keys, &mut std::io::sink())
                .unwrap_or_else(|e| panic!("{name} entry {}: {e}", entry.index));
            assert_eq!(decoded, expected);
            if entry.crypto.as_ref().is_some_and(|c| c.range_fields[2] != 0) {
                sparse += 1;
            }
            eprintln!("{name} #{}: {decoded} decoded bytes", entry.index);
        }
    }
    assert!(sparse > 0, "samples must exercise selective CTR");
}

#[test]
#[ignore = "requires RPF9_EXE and RPF9_ROOT pointing at the user's local files"]
fn decodes_resource_and_opens_multi_column_nested_archive() {
    use sha2::{Digest, Sha256};
    use std::io::Write;
    struct ResourceSink {
        header: Vec<u8>,
        hash: Sha256,
    }
    impl Write for ResourceSink {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let header_bytes = (16 - self.header.len()).min(bytes.len());
            self.header.extend_from_slice(&bytes[..header_bytes]);
            self.hash.update(&bytes[header_bytes..]);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let root = PathBuf::from(std::env::var_os("RPF9_ROOT").expect("RPF9_ROOT"));
    let exe = PathBuf::from(std::env::var_os("RPF9_EXE").expect("RPF9_EXE"));
    let keys = extract_scarlett_1_0_2_1(&exe).unwrap();
    let mut data = Archive::open(&root.join("data.rpf"), &keys).unwrap();
    let mut sink = ResourceSink { header: Vec::new(), hash: Sha256::new() };
    let length = data.copy_decoded(476, &keys, &mut sink).unwrap();
    assert_eq!(length, 0xe30010);
    assert_eq!(&sink.header[..4], b"RSC8");
    assert_eq!(
        format!("{:x}", sink.hash.finalize()),
        "fc00b7674d3418f1ebab8576c20646c5e24954a930103cffdbb67d9b6ceea2c9"
    );
    let story = Archive::open(&root.join("pack/story000/story000.rpf"), &keys).unwrap();
    let mut child = story.open_child(419, &keys).unwrap();
    assert_eq!(child.base_offset, 0x37b965000);
    assert_eq!(child.header.region_b_mode, 2);
    assert_eq!(child.header.alignment, 0x350);
    assert_eq!(child.metadata.entries.len(), 111);
    let small = child
        .metadata
        .entries
        .iter()
        .find(|e| e.extent.as_ref().is_some_and(|x| x.stored_size < 1 << 20 && x.unpacked_size < 4 << 20))
        .unwrap()
        .clone();
    let expected = small.extent.unwrap().unpacked_size + 16;
    assert_eq!(child.copy_decoded(small.index, &keys, &mut std::io::sink()).unwrap(), expected);
}

#[test]
#[ignore = "requires RPF9_EXE and RPF9_ROOT pointing at the user's local files"]
fn samples_distinct_nested_metadata_layouts_with_bounded_reads() {
    use std::collections::BTreeMap;
    let root = PathBuf::from(std::env::var_os("RPF9_ROOT").expect("RPF9_ROOT"));
    let exe = PathBuf::from(std::env::var_os("RPF9_EXE").expect("RPF9_EXE"));
    let keys = extract_scarlett_1_0_2_1(&exe).unwrap();
    let mut dirs = vec![root.clone()];
    let mut samples = BTreeMap::new();
    while let Some(dir) = dirs.pop() {
        for item in std::fs::read_dir(dir).unwrap() {
            let item = item.unwrap();
            let path = item.path();
            if item.file_type().unwrap().is_dir() && (item.file_name() == "pack" || path.starts_with(root.join("pack")))
            {
                dirs.push(path);
                continue;
            }
            if !path.extension().is_some_and(|ext| ext == "rpf") {
                continue;
            }
            let archive = Archive::open(&path, &keys).unwrap();
            let mut file = File::open(&path).unwrap();
            for entry in &archive.metadata.entries {
                if entry.crypto.is_some() || entry.blocks.is_some() || entry.resource_flags.is_some() {
                    continue;
                }
                let Some(extent) = &entry.extent else {
                    continue;
                };
                if extent.stored_size < 64 || extent.stream_codec != 0 {
                    continue;
                }
                file.seek(SeekFrom::Start(extent.offset)).unwrap();
                let mut h = [0; 64];
                file.read_exact(&mut h).unwrap();
                let Ok(h) = rage_rpf::rpf9::Header::parse(&h) else {
                    continue;
                };
                if h.metadata_end > 512 << 10 {
                    continue;
                }
                let profile = (h.region_b_mode, h.index_width, h.format_constant >> 16);
                let sample = samples.entry(profile).or_insert((path.clone(), entry.index, h.metadata_end));
                if h.metadata_end < sample.2 {
                    *sample = (path.clone(), entry.index, h.metadata_end);
                }
            }
        }
    }
    assert!(samples.len() >= 20);
    for (profile, (path, index, _)) in &samples {
        let archive = Archive::open(path, &keys).unwrap();
        let child = archive
            .open_child(*index, &keys)
            .unwrap_or_else(|e| panic!("{} #{index} {profile:?}: {e}", path.display()));
        assert_eq!(child.metadata.entries.len(), child.header.entry_count as usize);
        assert!(child.metadata.entries.iter().all(|e| e.extent.is_some()));
    }
    eprintln!("{} distinct nested metadata layouts verified", samples.len());
}
