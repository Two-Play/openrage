use super::*;

/// A member of a struct: name, offset, type, index of the element member, reference.
type M = (u32, u32, u8, u16, u32);

struct Sd {
    name: u32,
    size: u32,
    members: Vec<M>,
}

fn section(tag: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = tag.to_vec();
    out.extend_from_slice(&(payload.len() as u32 + 8).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

fn lsch(structs: &[Sd]) -> Vec<u8> {
    let mut headers = Vec::new();
    let mut members = Vec::new();
    for s in structs {
        headers.extend_from_slice(&s.name.to_le_bytes());
        headers.extend_from_slice(&[0; 5]);
        headers.push(3);
        headers.extend_from_slice(&[0; 6]);
        headers.extend_from_slice(&[0xAA; 8]);
        headers.extend_from_slice(&s.size.to_le_bytes());
        headers.extend_from_slice(&[0, 0]);
        headers.extend_from_slice(&(s.members.len() as u16).to_le_bytes());
        for (name, offset, kind, index, reference) in &s.members {
            members.extend_from_slice(&name.to_le_bytes());
            members.extend_from_slice(&offset.to_le_bytes());
            members.extend_from_slice(&[*kind, 0]);
            members.extend_from_slice(&index.to_le_bytes());
            members.extend_from_slice(&reference.to_le_bytes());
        }
    }
    [headers, members].concat()
}

fn lenm(enums: &[(u32, Vec<(i64, u32)>)]) -> Vec<u8> {
    let mut headers = Vec::new();
    let mut values = Vec::new();
    for (name, list) in enums {
        headers.extend_from_slice(&name.to_le_bytes());
        headers.extend_from_slice(&[0; 4]);
        headers.extend_from_slice(&[0xEE; 8]);
        headers.extend_from_slice(&(list.len() as u32).to_le_bytes());
        headers.extend_from_slice(&[0; 4]);
        for (value, hash) in list {
            values.extend_from_slice(&value.to_le_bytes());
            values.extend_from_slice(&hash.to_le_bytes());
            values.extend_from_slice(&[0; 4]);
        }
    }
    [headers, values].concat()
}

fn lrsc(root: u32) -> Vec<u8> {
    let mut out = b"0DRP".to_vec();
    out.extend_from_slice(&122u32.to_le_bytes());
    out.extend_from_slice(&[0; 8]);
    out.extend_from_slice(&root.to_le_bytes());
    out
}

fn file(blocks: &[(u32, Vec<u8>)], structs: &[Sd], enums: &[(u32, Vec<(i64, u32)>)], root: u32) -> Vec<u8> {
    let mut out = b"PSIN\0\0\0\0".to_vec();
    for (kind, data) in blocks {
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&[0xAA; 8]);
    }
    out.extend_from_slice(&[0x70; 8]);
    for (_, data) in blocks {
        out.extend_from_slice(data);
        out.resize(out.len().next_multiple_of(16), 0);
    }
    let first = out.len() as u32;
    out[4..8].copy_from_slice(&first.to_be_bytes());
    out.extend(section(b"LSCH", &lsch(structs)));
    if !enums.is_empty() {
        out.extend(section(b"LENM", &lenm(enums)));
    }
    out.extend(section(b"LRSC", &lrsc(root)));
    out
}

fn pointer(block: u64, offset: u64) -> [u8; 8] {
    (block | offset << 20).to_le_bytes()
}

fn n(hash: u32) -> String {
    format!("hash_{hash:08X}")
}

const ROOT: u32 = 0xA001;

/// A file whose root block holds `data` and whose root struct has these members.
fn single(size: u32, members: Vec<M>, data: Vec<u8>, more: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut blocks = vec![(ROOT, data)];
    blocks.extend_from_slice(more);
    file(&blocks, &[Sd { name: ROOT, size, members }], &[], 1)
}

fn xml_of(data: &[u8]) -> (String, Vec<String>) {
    parse(data).unwrap().to_xml()
}

fn body(lines: &[String]) -> String {
    let mut s = format!("{}\n", xml::HEADER);
    for l in lines {
        s.push_str(l);
        s.push('\n');
    }
    s
}

fn wrap(inner: &[&str]) -> String {
    let mut lines = vec![format!("<{}>", n(ROOT))];
    lines.extend(inner.iter().map(|l| l.to_string()));
    lines.push(format!("</{}>", n(ROOT)));
    body(&lines)
}

fn put(d: &mut Vec<u8>, at: usize, bytes: &[u8]) {
    if d.len() < at + bytes.len() {
        d.resize(at + bytes.len(), 0);
    }
    d[at..at + bytes.len()].copy_from_slice(bytes);
}

#[test]
fn every_scalar_type_is_written_as_a_value() {
    let cases: Vec<(u8, Vec<u8>, &str)> = vec![
        (0x01, vec![1], "true"),
        (0x01, vec![0], "false"),
        (0x10, vec![0xFE], "-2"),
        (0x11, vec![200], "200"),
        (0x12, (-300i16).to_le_bytes().to_vec(), "-300"),
        (0x13, 60000u16.to_le_bytes().to_vec(), "60000"),
        (0x20, 0x3C00u16.to_le_bytes().to_vec(), "1"),
        (0x14, (-5i32).to_le_bytes().to_vec(), "-5"),
        (0x15, 4_000_000_000u32.to_le_bytes().to_vec(), "4000000000"),
        (0x21, 2.5f32.to_le_bytes().to_vec(), "2.5"),
        (0x4A, 0xABu32.to_le_bytes().to_vec(), "0x000000AB"),
        (0x4B, 0xABu32.to_le_bytes().to_vec(), "0x000000AB"),
        (0x4D, 0xABu32.to_le_bytes().to_vec(), "0x000000AB"),
        (0x16, (-9i64).to_le_bytes().to_vec(), "-9"),
        (0x08, 7i64.to_le_bytes().to_vec(), "7"),
        (0x17, u64::MAX.to_le_bytes().to_vec(), "18446744073709551615"),
        (0x4F, 0xABu64.to_le_bytes().to_vec(), "0x00000000000000AB"),
    ];
    let mut data = Vec::new();
    let mut members = Vec::new();
    let mut expected = Vec::new();
    for (i, (kind, bytes, text)) in cases.iter().enumerate() {
        let name = 0x1000 + i as u32;
        put(&mut data, i * 16, bytes);
        members.push((name, i as u32 * 16, *kind, 0, 0));
        expected.push(format!(" <{} value=\"{text}\" />", n(name)));
    }
    let size = data.len() as u32 + 8;
    data.resize(size as usize, 0);
    let (xml, errors) = xml_of(&single(size, members, data, &[]));
    assert!(errors.is_empty(), "{errors:?}");
    let refs: Vec<&str> = expected.iter().map(String::as_str).collect();
    assert_eq!(xml, wrap(&refs));
}

#[test]
fn vectors_matrices_and_fixed_strings() {
    let floats = |v: &[f32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
    let mut data = Vec::new();
    put(&mut data, 0, &floats(&[1.0, 2.0]));
    put(&mut data, 16, &floats(&[1.0, 2.0, 3.0]));
    put(&mut data, 32, &floats(&[1.0, 2.0, 3.0, 4.0]));
    let identity: Vec<f32> = (0..16).map(|i| if i % 5 == 0 { 1.0 } else { 0.0 }).collect();
    put(&mut data, 48, &floats(&identity));
    put(&mut data, 112, b"fixed\0xyz");
    put(&mut data, 128, &[0; 16]);
    let members = vec![
        (0x2001, 0, 0x30, 0, 0),
        (0x2002, 16, 0x32, 0, 0),
        (0x2003, 32, 0x31, 0, 0),
        (0x2004, 32, 0x34, 0, 0),
        (0x2005, 48, 0x36, 0, 0),
        (0x2006, 112, 0x40, 0, 9),
        (0x2007, 128, 0x40, 0, 4),
    ];
    let (xml, errors) = xml_of(&single(144, members, data, &[]));
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(
        xml,
        wrap(&[
            " <hash_00002001 x=\"1\" y=\"2\" />",
            " <hash_00002002 x=\"1\" y=\"2\" z=\"3\" />",
            " <hash_00002003 x=\"1\" y=\"2\" z=\"3\" w=\"4\" />",
            " <hash_00002004 x=\"1\" y=\"2\" z=\"3\" w=\"4\" />",
            " <hash_00002005>",
            "  1 0 0 0",
            "  0 1 0 0",
            "  0 0 1 0",
            "  0 0 0 1",
            " </hash_00002005>",
            " <hash_00002006>fixed</hash_00002006>",
            " <hash_00002007 />",
        ])
    );
}

#[test]
fn members_outside_the_file_are_errors() {
    let kinds = [0x01u8, 0x13, 0x15, 0x17, 0x30, 0x36, 0x40, 0x60, 0x64, 0x62, 0x68];
    let members: Vec<M> = kinds.iter().enumerate().map(|(i, k)| (0x3000 + i as u32, 4000, *k, 0, 8)).collect();
    let (xml, errors) = xml_of(&single(16, members, vec![0; 16], &[]));
    let expected: Vec<String> =
        (0..kinds.len()).map(|i| format!("{}: outside the file", n(0x3000 + i as u32))).collect();
    assert_eq!(errors, expected);
    assert_eq!(xml.matches("<error>").count(), kinds.len());
}

#[test]
fn enums_flags_and_unknown_values() {
    let mut data = Vec::new();
    put(&mut data, 0, &[2]);
    put(&mut data, 16, &[7]);
    put(&mut data, 32, &9u16.to_le_bytes());
    put(&mut data, 48, &(-1i32).to_le_bytes());
    put(&mut data, 64, &0b1001u32.to_le_bytes());
    put(&mut data, 80, &(-3i64).to_le_bytes());
    put(&mut data, 96, &(-2i32).to_le_bytes());
    put(&mut data, 112, &[1]);
    let members = vec![
        (0x4001, 0, 0x60, 0, 0xE001),
        (0x4002, 16, 0x63, 0, 0xE001),
        (0x4003, 32, 0x64, 0, 0xE001),
        (0x4004, 48, 0x62, 0, 0xE001),
        (0x4005, 64, 0x65, 0, 0xE001),
        (0x4006, 80, 0x68, 0, 0xE001),
        (0x4007, 96, 0x62, 0, 0xE001),
        (0x4008, 112, 0x60, 0, 0xE777),
    ];
    let enums = vec![(0xE001, vec![(2, 0xBEEF), (9, 0xCAFE), (4_294_967_295, 0xF00D), (0, 0xB001), (-3, 0xD00D)])];
    let f = file(
        &[(ROOT, {
            let mut d = data.clone();
            d.resize(128, 0);
            d
        })],
        &[Sd { name: ROOT, size: 128, members }],
        &enums,
        1,
    );
    let (xml, errors) = xml_of(&f);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(
        xml,
        wrap(&[
            " <hash_00004001 value=\"hash_0000BEEF\" />",
            " <hash_00004002 value=\"7\" />",
            " <hash_00004003 value=\"hash_0000CAFE\" />",
            " <hash_00004004 value=\"hash_0000F00D\" />",
            " <hash_00004005 value=\"hash_0000B001 bit3\" />",
            " <hash_00004006 value=\"hash_0000D00D\" />",
            " <hash_00004007 value=\"-2\" />",
            " <hash_00004008 value=\"1\" />",
        ])
    );
}

#[test]
fn strings_inline_behind_pointers_and_unterminated() {
    let mut data = Vec::new();
    put(&mut data, 0, &pointer(2, 0));
    put(&mut data, 16, b"inline-text");
    put(&mut data, 31, &[0x80 | 11]);
    put(&mut data, 32, &pointer(2, 6));
    put(&mut data, 48, &pointer(3, 0));
    put(&mut data, 64, &[0; 16]);
    put(&mut data, 80, &pointer(2, 4000));
    let members = vec![
        (0x5001, 0, 0x42, 0, 0),
        (0x5002, 16, 0x44, 0, 0),
        (0x5003, 32, 0x44, 0, 0),
        (0x5004, 48, 0x42, 0, 0),
        (0x5005, 64, 0x42, 0, 0),
        (0x5006, 80, 0x42, 0, 0),
    ];
    let more = [(0x10, b"hello\0world\0".to_vec()), (0x10, b"no end".to_vec())];
    let (xml, errors) = xml_of(&single(96, members, data, &more));
    assert_eq!(errors, ["hash_00005004: a string without an end"]);
    assert_eq!(
        xml,
        wrap(&[
            " <hash_00005001>hello</hash_00005001>",
            " <hash_00005002>inline-text</hash_00005002>",
            " <hash_00005003>world</hash_00005003>",
            " <error>hash_00005004: a string without an end</error>",
            " <hash_00005005 />",
            " <hash_00005006 />",
        ])
    );
}

fn nested_structs() -> Vec<Sd> {
    vec![
        Sd {
            name: ROOT,
            size: 48,
            members: vec![(0x6001, 0, 0x05, 0, 0xB001), (0x6002, 16, 0x07, 0, 0), (0x6003, 32, 0x07, 0, 0)],
        },
        Sd { name: 0xB001, size: 4, members: vec![(0x6101, 0, 0x15, 0, 0)] },
        Sd { name: 0xB002, size: 0, members: vec![(ELEMENT, 0, 0x15, 0, 0)] },
    ]
}

#[test]
fn structs_inline_through_pointers_and_empty_ones() {
    let mut root = vec![0u8; 48];
    put(&mut root, 0, &5u32.to_le_bytes());
    put(&mut root, 16, &pointer(2, 0));
    put(&mut root, 32, &pointer(3, 0));
    let blocks = [(ROOT, root), (0xB001, 77u32.to_le_bytes().to_vec()), (0xB002, vec![0; 16])];
    let (xml, errors) = xml_of(&file(&blocks, &nested_structs(), &[], 1));
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(
        xml,
        wrap(&[
            " <hash_00006001>",
            "  <hash_00006101 value=\"5\" />",
            " </hash_00006001>",
            " <hash_00006002 type=\"hash_0000B001\">",
            "  <hash_00006101 value=\"77\" />",
            " </hash_00006002>",
            " <hash_00006003 type=\"hash_0000B002\" />",
        ])
    );
}

#[test]
fn a_null_pointer_to_a_struct_is_an_empty_element() {
    let blocks = [(ROOT, vec![0u8; 48])];
    let (xml, errors) = xml_of(&file(&blocks, &nested_structs(), &[], 1));
    assert!(errors.is_empty(), "{errors:?}");
    assert!(xml.contains(" <hash_00006002 />\n <hash_00006003 />\n"), "{xml}");
}

#[test]
fn struct_problems_are_errors() {
    let mut root = vec![0u8; 48];
    put(&mut root, 16, &pointer(2, 0));
    let blocks = [(ROOT, root), (0xDEAD, vec![0; 16])];
    let (_, errors) = xml_of(&file(&blocks, &nested_structs(), &[], 1));
    assert_eq!(errors, ["hash_00006002: struct hash_0000DEAD is not described in this file"]);

    // A struct whose size runs past the end of the file.
    let big = vec![Sd { name: ROOT, size: 100_000, members: vec![] }];
    let (xml, errors) = xml_of(&file(&[(ROOT, vec![0; 16])], &big, &[], 1));
    assert_eq!(errors, [format!("{}: outside the file", n(ROOT))]);
    assert_eq!(xml, body(&[format!("<error>{}: outside the file</error>", n(ROOT))]));

    // A pointer that leads back to the struct that holds it.
    let mut again = vec![0u8; 48];
    put(&mut again, 16, &pointer(1, 0));
    let (_, errors) = xml_of(&file(&[(ROOT, again)], &nested_structs(), &[], 1));
    assert_eq!(errors, ["hash_00006002: refers back to itself"]);
}

#[test]
fn a_chain_of_pointers_stops_at_the_depth_limit() {
    let links = Sd { name: ROOT, size: 16, members: vec![(0x7001, 0, 0x07, 0, 0)] };
    let blocks: Vec<(u32, Vec<u8>)> = (1..=70u64)
        .map(|i| {
            let mut d = vec![0u8; 16];
            if i < 70 {
                put(&mut d, 0, &pointer(i + 1, 0));
            }
            (ROOT, d)
        })
        .collect();
    let (xml, errors) = xml_of(&file(&blocks, &[links], &[], 1));
    assert_eq!(errors, ["hash_00007001: refers back to itself"]);
    assert_eq!(xml.matches("type=\"hash_0000A001\"").count(), 64);
}

fn array_members() -> Vec<M> {
    vec![
        (ELEMENT, 0, 0x13, 0, 0),
        (0x8001, 0, 0x50, 0, 3),
        (0x8002, 8, 0x54, 0, 0),
        (0x8003, 32, 0x52, 0, 0),
        (0x8004, 48, 0x5C, 0, 0),
        (0x8005, 64, 0x52, 0, 0),
    ]
}

fn array_data() -> Vec<u8> {
    let mut d = vec![0u8; 80];
    for (i, v) in [1u16, 2, 3].iter().enumerate() {
        put(&mut d, i * 2, &v.to_le_bytes());
    }
    put(&mut d, 8, &5u32.to_le_bytes());
    put(&mut d, 12, &2u32.to_le_bytes());
    put(&mut d, 16, &[4, 0, 5, 0, 6, 0]);
    put(&mut d, 32, &pointer(2, 0));
    put(&mut d, 40, &2u16.to_le_bytes());
    put(&mut d, 48, &1u32.to_le_bytes());
    put(&mut d, 56, &pointer(2, 4));
    d
}

#[test]
fn arrays_of_every_layout() {
    let more = [(0x13, vec![7, 0, 8, 0, 9, 0])];
    let (xml, errors) = xml_of(&single(80, array_members(), array_data(), &more));
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(
        xml,
        wrap(&[
            " <hash_00008001>",
            "  <Item value=\"1\" />",
            "  <Item value=\"2\" />",
            "  <Item value=\"3\" />",
            " </hash_00008001>",
            " <hash_00008002>",
            "  <Item value=\"4\" />",
            "  <Item value=\"5\" />",
            " </hash_00008002>",
            " <hash_00008003>",
            "  <Item value=\"7\" />",
            "  <Item value=\"8\" />",
            " </hash_00008003>",
            " <hash_00008004>",
            "  <Item value=\"9\" />",
            " </hash_00008004>",
            " <hash_00008005 />",
        ])
    );
}

#[test]
fn array_problems_are_errors() {
    let more = [(0x13, vec![7, 0, 8, 0, 9, 0])];
    let mut long = array_data();
    put(&mut long, 40, &100u16.to_le_bytes());
    let (_, errors) = xml_of(&single(80, array_members(), long, &more));
    assert_eq!(errors, ["hash_00008003: array outside its block"]);

    let mut null = array_data();
    put(&mut null, 32, &0u64.to_le_bytes());
    let (_, errors) = xml_of(&single(80, array_members(), null, &more));
    assert_eq!(errors, ["hash_00008003: array outside its block"]);

    let mut cut = array_members();
    cut[0].2 = 0x7F;
    let (_, errors) = xml_of(&single(80, cut, array_data(), &more));
    assert_eq!(errors.len(), 4);
    assert_eq!(errors[0], "hash_00008001: an array of type 0x7f, which is not understood");

    let mut orphan = array_members();
    orphan[3].3 = 9;
    let (_, errors) = xml_of(&single(80, orphan, array_data(), &more));
    assert_eq!(errors, ["hash_00008003: an array of type 0x00, which is not understood"]);

    let mut past_the_block = vec![0u8; 80];
    put(&mut past_the_block, 64, &9u32.to_le_bytes());
    put(&mut past_the_block, 68, &9u32.to_le_bytes());
    let members = vec![(ELEMENT, 0, 0x13, 0, 0), (0x8002, 64, 0x54, 0, 0)];
    let (_, errors) = xml_of(&single(80, members, past_the_block, &[]));
    assert_eq!(errors, ["hash_00008002: array outside its block"]);

    let members = vec![(ELEMENT, 0, 0x13, 0, 0), (0x8002, 78, 0x54, 0, 0)];
    let (_, errors) = xml_of(&single(2, members, vec![0; 80], &[]));
    assert_eq!(errors, ["hash_00008002: array outside its block"]);
}

#[test]
fn arrays_of_structs_and_of_struct_pointers() {
    let structs = vec![
        Sd {
            name: ROOT,
            size: 48,
            members: vec![
                (ELEMENT, 0, 0x05, 0, 0xB001),
                (0x9001, 0, 0x50, 0, 2),
                (ELEMENT, 0, 0x07, 0, 0),
                (0x9002, 16, 0x52, 2, 0),
            ],
        },
        Sd { name: 0xB001, size: 4, members: vec![(0x9101, 0, 0x15, 0, 0)] },
    ];
    let mut root = vec![0u8; 48];
    put(&mut root, 0, &[1, 0, 0, 0, 2, 0, 0, 0]);
    put(&mut root, 16, &pointer(2, 0));
    put(&mut root, 24, &1u16.to_le_bytes());
    let blocks = [(ROOT, root), (0x52, pointer(3, 0).to_vec()), (0xB001, vec![9, 0, 0, 0])];
    let (xml, errors) = xml_of(&file(&blocks, &structs, &[], 1));
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(
        xml,
        wrap(&[
            " <hash_00009001>",
            "  <Item>",
            "   <hash_00009101 value=\"1\" />",
            "  </Item>",
            "  <Item>",
            "   <hash_00009101 value=\"2\" />",
            "  </Item>",
            " </hash_00009001>",
            " <hash_00009002>",
            "  <Item type=\"hash_0000B001\">",
            "   <hash_00009101 value=\"9\" />",
            "  </Item>",
            " </hash_00009002>",
        ])
    );
}

fn map_members(kind: u8) -> Vec<M> {
    vec![(ELEMENT, 0, 0x15, 0, 0), (ELEMENT, 0, 0x15, 0, 0), (0xC001, 0, kind, 0, 1 << 16)]
}

fn pairs(pair: &[(u32, u32)]) -> Vec<u8> {
    pair.iter().flat_map(|(k, v)| [k.to_le_bytes(), v.to_le_bytes()].concat()).collect()
}

fn map_expected(items: &[(u32, u32)]) -> String {
    let mut lines = vec![" <hash_0000C001>".to_owned()];
    for (k, v) in items {
        lines.extend([
            "  <Item>".to_owned(),
            format!("   <key value=\"{k}\" />"),
            format!("   <value value=\"{v}\" />"),
            "  </Item>".to_owned(),
        ]);
    }
    lines.push(" </hash_0000C001>".to_owned());
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    wrap(&refs)
}

#[test]
fn a_sorted_map_lists_its_pairs() {
    let mut root = vec![0u8; 24];
    put(&mut root, 8, &pointer(2, 0));
    put(&mut root, 16, &2u16.to_le_bytes());
    let (xml, errors) = xml_of(&single(24, map_members(0x71), root, &[(0x100, pairs(&[(1, 10), (2, 20)]))]));
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(xml, map_expected(&[(1, 10), (2, 20)]));
}

#[test]
fn a_grouped_map_lists_its_used_places() {
    let mut root = vec![0u8; 24];
    put(&mut root, 0, &1u32.to_le_bytes());
    put(&mut root, 4, &2u32.to_le_bytes());
    put(&mut root, 8, &pointer(2, 0));
    let mut group = vec![0x80u8; 16];
    group[0] = 0;
    group[5] = 0x7F;
    group.resize(16 + 16 * 8, 0);
    put(&mut group, 16, &pairs(&[(3, 30)]));
    put(&mut group, 16 + 5 * 8, &pairs(&[(4, 40)]));
    let (xml, errors) = xml_of(&single(24, map_members(0x75), root, &[(0x100, group)]));
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(xml, map_expected(&[(3, 30), (4, 40)]));
}

#[test]
fn a_slot_map_lists_the_slots_with_a_key() {
    let mut root = vec![0u8; 24];
    put(&mut root, 0, &4u32.to_le_bytes());
    put(&mut root, 4, &2u32.to_le_bytes());
    put(&mut root, 8, &pointer(2, 0));
    let slots = pairs(&[(0, 0), (6, 60), (0, 0), (7, 70)]);
    let (xml, errors) = xml_of(&single(24, map_members(0x73), root, &[(0x100, slots)]));
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(xml, map_expected(&[(6, 60), (7, 70)]));
}

#[test]
fn map_problems_are_errors() {
    let more = [(0x100, pairs(&[(1, 10), (2, 20)]))];
    let mut root = vec![0u8; 24];
    put(&mut root, 8, &pointer(2, 0));
    put(&mut root, 16, &2u16.to_le_bytes());
    let mut no_key = map_members(0x71);
    no_key[2].4 = 9 << 16;
    assert_eq!(
        xml_of(&single(24, no_key, root.clone(), &more)).1,
        ["hash_0000C001: a map without a description of its entries"]
    );
    let mut odd = map_members(0x71);
    odd[1].2 = 0x7F;
    assert_eq!(
        xml_of(&single(24, odd, root.clone(), &more)).1,
        ["hash_0000C001: a map of types that are not understood"]
    );
    let mut many = root.clone();
    put(&mut many, 16, &500u16.to_le_bytes());
    assert_eq!(xml_of(&single(24, map_members(0x71), many, &more)).1, ["hash_0000C001: map outside its block"]);
    let mut none = vec![0u8; 24];
    let (xml, errors) = xml_of(&single(24, map_members(0x71), none.clone(), &more));
    assert!(errors.is_empty());
    assert_eq!(xml, wrap(&[" <hash_0000C001 />"]));
    put(&mut none, 4, &1u32.to_le_bytes());
    assert_eq!(xml_of(&single(24, map_members(0x73), none.clone(), &more)).1, ["hash_0000C001: map outside its block"]);
    assert_eq!(xml_of(&single(24, map_members(0x75), none, &more)).1, ["hash_0000C001: map outside its block"]);
}

#[test]
fn a_member_of_an_unknown_type_is_an_error() {
    let (xml, errors) = xml_of(&single(16, vec![(0x1234, 0, 0x7E, 0, 0)], vec![0; 16], &[]));
    assert_eq!(errors, ["hash_00001234: type 0x7e is not understood"]);
    assert_eq!(xml, wrap(&[" <error>hash_00001234: type 0x7e is not understood</error>"]));
}

#[test]
fn damaged_containers_are_errors() {
    let message = |data: &[u8]| parse(data).unwrap_err().to_string();
    assert_eq!(message(b""), "invalid PSIN file: no LSCH section");
    assert_eq!(message(b"PSIN\0\0\0\x08XXXX"), "invalid PSIN file: no LSCH section");
    let good = single(16, vec![(0x1234, 0, 0x15, 0, 0)], vec![0; 16], &[]);
    assert!(parse(&good).is_ok());

    let mut tail = good.clone();
    tail.extend_from_slice(b"TAIL");
    assert_eq!(message(&tail), "invalid PSIN file: truncated section");
    let mut small = good.clone();
    small.extend_from_slice(b"TAIL\0\0\0\x04");
    assert_eq!(message(&small), "invalid PSIN file: the file is cut off inside a section");
    let mut cut = good.clone();
    cut.extend_from_slice(b"TAIL\0\0\x01\0");
    assert_eq!(message(&cut), "invalid PSIN file: the file is cut off inside a section");

    let mut bad_count = good.clone();
    let first = u32::from_be_bytes(good[4..8].try_into().unwrap()) as usize;
    bad_count[first + 8 + 30] = 9;
    assert_eq!(message(&bad_count), "invalid PSIN file: no struct count fills LSCH");

    let blocks = [(ROOT, vec![0; 16])];
    let structs = [Sd { name: ROOT, size: 16, members: vec![] }];
    assert_eq!(message(&file(&blocks, &structs, &[], 0)), "invalid PSIN file: the root is not a struct block");
    assert_eq!(message(&file(&blocks, &structs, &[], 5)), "invalid PSIN file: the root is not a struct block");
    assert_eq!(
        message(&file(&[(0x55, vec![0; 16])], &structs, &[], 1)),
        "invalid PSIN file: the root is not a struct block"
    );

    let mut no_root = file(&blocks, &structs, &[], 1);
    let at = no_root.len() - 20;
    no_root[at..at + 4].copy_from_slice(b"XXXX");
    assert_eq!(message(&no_root), "invalid PSIN file: no root (LRSC section)");

    let mut no_end = b"PSIN\0\0\0\x18".to_vec();
    no_end.extend_from_slice(&[1, 0, 0, 0, 0, 0, 0, 0, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA]);
    no_end.extend(section(b"LSCH", &[]));
    assert_eq!(message(&no_end), "invalid PSIN file: block table without end");

    let mut big_block = file(&blocks, &structs, &[], 1);
    big_block[12..16].copy_from_slice(&0x1000u32.to_le_bytes());
    assert_eq!(message(&big_block), "invalid PSIN file: a block ends after the first section");
}

#[test]
fn the_block_table_must_be_complete() {
    let mut data = b"PSIN\0\0\0\x20".to_vec();
    data.extend_from_slice(&0x10u32.to_le_bytes());
    data.extend_from_slice(&[0; 20]);
    data.extend(section(b"LSCH", &[]));
    assert_eq!(parse(&data).unwrap_err().to_string(), "invalid PSIN file: block table cut off");
}

#[test]
fn an_enum_section_must_fill_its_headers() {
    let blocks = [(ROOT, vec![0; 16])];
    let structs = [Sd { name: ROOT, size: 16, members: vec![] }];
    let mut data = file(&blocks, &structs, &[(1, vec![(1, 2)])], 1);
    let lenm_at = data.windows(4).position(|w| w == b"LENM").unwrap();
    data[lenm_at + 8 + 16] = 7;
    assert_eq!(parse(&data).unwrap_err().to_string(), "invalid PSIN file: no enum count fills LENM");
}

#[test]
fn an_empty_struct_section_has_no_structs() {
    assert_eq!(header_count(&[], 32, 16, 30, false), Some(0));
    assert_eq!(header_count(&[0; 31], 32, 16, 30, false), None);
    let mut two = vec![0u8; 32 + 32 + 16];
    two[30] = 1;
    assert_eq!(header_count(&two, 32, 16, 30, false), Some(2));
    assert_eq!(header_count(&[0; 24], 24, 16, 16, true), Some(1));
}

#[test]
fn sizes_and_alignments_of_member_types() {
    let psin = Psin {
        data: &[],
        structs: HashMap::from([(7, Struct { size: 24, align: 4, members: vec![] })]),
        enums: HashMap::new(),
        blocks: vec![],
        root: 1,
    };
    let m = |kind, reference| Member { name: 0, offset: 0, kind, index: 0, reference };
    let sizes = [
        (0x01, 1),
        (0x63, 1),
        (0x13, 2),
        (0x64, 2),
        (0x21, 4),
        (0x65, 4),
        (0x17, 8),
        (0x42, 8),
        (0x32, 12),
        (0x44, 16),
        (0x5C, 16),
        (0x71, 24),
        (0x36, 64),
    ];
    for (kind, size) in sizes {
        assert_eq!(psin.size_of(&m(kind, 0)), Some(size), "kind {kind:#x}");
    }
    assert_eq!(psin.size_of(&m(0x05, 7)), Some(24));
    assert_eq!(psin.size_of(&m(0x05, 8)), None);
    assert_eq!(psin.size_of(&m(0x40, 9)), Some(9));
    assert_eq!(psin.size_of(&m(0x7E, 0)), None);
    assert_eq!(psin.align_of(&m(0x05, 7)), 4);
    assert_eq!(psin.align_of(&m(0x05, 8)), 8);
    assert_eq!(psin.align_of(&m(0x34, 0)), 16);
    assert_eq!(psin.align_of(&m(0x32, 0)), 4);
    assert_eq!(psin.align_of(&m(0x40, 3)), 1);
    assert_eq!(psin.align_of(&m(0x13, 0)), 2);
    assert_eq!(psin.align_of(&m(0x17, 0)), 8);
    assert_eq!(psin.map_layout(&m(0x15, 0), &m(0x17, 0), 4, 8), (8, 16));
    assert_eq!(psin.map_layout(&m(0x15, 0), &m(0x15, 0), 4, 4), (4, 8));
}

#[test]
fn pointers_blocks_and_strings_are_looked_up_in_the_block_table() {
    let data = b"abc\0defgh";
    let psin = Psin {
        data,
        structs: HashMap::new(),
        enums: HashMap::new(),
        blocks: vec![(0x10, 0, 4), (0x20, 4, 5)],
        root: 1,
    };
    assert_eq!(psin.block_end(0), Some(4));
    assert_eq!(psin.block_end(5), Some(9));
    assert_eq!(psin.block_end(9), None);
    assert_eq!(psin.string_at(0, 4), Some("abc".to_owned()));
    assert_eq!(psin.string_at(4, 9), None);
    assert_eq!(psin.string_at(0, 100), None);
    let at = |block: u64, offset: u64| {
        let mut d = pointer(block, offset).to_vec();
        d.resize(16, 0);
        Psin {
            data: Box::leak(d.into_boxed_slice()),
            ..Psin { data, structs: HashMap::new(), enums: HashMap::new(), blocks: psin.blocks.clone(), root: 1 }
        }
    };
    assert_eq!(at(2, 3).pointer(0), Some((0x20, 7, 9)));
    assert_eq!(at(1, 0).pointer(0), Some((0x10, 0, 4)));
    assert_eq!(at(0, 0).pointer(0), None);
    assert_eq!(at(3, 0).pointer(0), None);
    assert_eq!(at(2, 5).pointer(0), None);
}

#[test]
fn half_floats_of_every_class() {
    assert_eq!(half(0x0001), 2f32.powi(-24));
    assert_eq!(half(0x03FF), 1023.0 / 1024.0 * 2f32.powi(-14));
    assert_eq!(half(0x7C00), f32::INFINITY);
    assert_eq!(half(0xFC00), f32::NEG_INFINITY);
    assert!(half(0x7E00).is_nan());
    assert!(half(0x8000).is_sign_negative() && half(0x8000) == 0.0);
    assert_eq!(half(0x7BFF), 65504.0);
}
