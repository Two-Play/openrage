//! Add the separate 32-bit lookup column to a previous recursive inventory.
use rage_rpf::{rpf9::Archive, rpf9_exe::extract_scarlett_1_0_2_1};
use std::{
    fs::File,
    io::{BufRead, BufReader, BufWriter, Write},
    path::PathBuf,
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 3 {
        return Err("usage: ROOT EXE EXPORT_DIRECTORY".into());
    }
    let root = PathBuf::from(&args[0]);
    let dir = PathBuf::from(&args[2]);
    let keys = extract_scarlett_1_0_2_1(&PathBuf::from(&args[1]))?;
    let mut out = BufWriter::new(File::options().write(true).create_new(true).open(dir.join("lookup32.tsv"))?);
    let mut hashes =
        BufWriter::new(File::options().write(true).create_new(true).open(dir.join("name_hashes32_all.txt"))?);
    writeln!(out, "archive\tindex\tname_hash_32")?;
    let (mut count, mut present) = (0u64, 0u64);
    for line in BufReader::new(File::open(dir.join("archives.tsv"))?).lines().skip(1) {
        let line = line?;
        let parts: Vec<_> = line.split('\t').collect();
        let path = root.join(parts[0].split('!').next().ok_or("missing path")?);
        let archive = Archive::open_range(&path, parts[1].parse()?, parts[2].parse()?, &keys)?;
        for entry in &archive.metadata.entries {
            count += 1;
            let value = entry.name_hash32.map(|h| format!("{h:08x}")).unwrap_or_default();
            if !value.is_empty() {
                writeln!(hashes, "{value}")?;
                present += 1;
            }
            writeln!(out, "{}\t{}\t{value}", parts[0], entry.index)?;
        }
    }
    out.flush()?;
    hashes.flush()?;
    println!("entries={count}, entries_with_32bit_column={present}");
    Ok(())
}
