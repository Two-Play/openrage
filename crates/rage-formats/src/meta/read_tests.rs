use super::*;
use rage_core::hash::joaat;

fn nm(name: &str) -> u32 {
    names::register([name]);
    joaat(name)
}

fn entry(name: &str, offset: usize, kind: u8, key: u32) -> Entry {
    Entry { name: if name.is_empty() { ARRAYINFO } else { nm(name) }, offset, kind, key }
}

fn info(entries: Vec<Entry>) -> StructInfo {
    StructInfo { size: 0x40, entries }
}

fn ptr(block: u64, offset: u64) -> u64 {
    block | offset << 12
}

fn put(d: &mut Vec<u8>, at: usize, bytes: &[u8]) {
    if d.len() < at + bytes.len() {
        d.resize(at + bytes.len(), 0);
    }
    d[at..at + bytes.len()].copy_from_slice(bytes);
}

fn meta(structs: Vec<(&str, StructInfo)>, blocks: Vec<(&str, Vec<u8>)>) -> Meta {
    Meta {
        name: String::new(),
        root_block: 1,
        structs: structs.into_iter().map(|(n, s)| (nm(n), s)).collect(),
        enums: HashMap::new(),
        blocks: blocks.into_iter().map(|(n, data)| Block { name: nm(n), data }).collect(),
        offset_shift: 12,
    }
}

fn single(entries: Vec<Entry>, data: Vec<u8>) -> Meta {
    meta(vec![("TS", info(entries))], vec![("TS", data)])
}

fn lines(body: &[&str]) -> String {
    let mut s = format!("{}\n", xml::HEADER);
    for l in body {
        s.push_str(l);
        s.push('\n');
    }
    s
}

#[test]
fn plain_numbers_are_written_as_value_elements() {
    let mut d = Vec::new();
    put(&mut d, 0, &[1, 200, 0xFE, 7]);
    put(&mut d, 4, &65535u16.to_le_bytes());
    put(&mut d, 6, &(-3i16).to_le_bytes());
    put(&mut d, 8, &0x3C00u16.to_le_bytes());
    put(&mut d, 12, &(-5i32).to_le_bytes());
    put(&mut d, 16, &4_000_000_000u32.to_le_bytes());
    put(&mut d, 20, &1.5f32.to_le_bytes());
    put(&mut d, 24, &(1u64 << 40).to_le_bytes());
    put(&mut d, 32, &0xFF00FF00u32.to_le_bytes());
    let m = single(
        vec![
            entry("tbool", 0, ty::BOOLEAN, 0),
            entry("tubyte", 1, ty::UBYTE, 0),
            entry("tsbyte", 2, ty::SBYTE, 0),
            entry("tbenum", 3, ty::BYTE_ENUM, 0),
            entry("tushort", 4, ty::USHORT, 0),
            entry("tsshort", 6, ty::SSHORT, 0),
            entry("thalf", 8, ty::HALF, 0),
            entry("tsint", 12, ty::SINT, 0),
            entry("tuint", 16, ty::UINT, 0),
            entry("tfloat", 20, ty::FLOAT, 0),
            entry("tulong", 24, ty::ULONG, 0),
            entry("color", 32, ty::UINT, 0),
        ],
        d,
    );
    let (xml, errors) = m.to_xml();
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(
        xml,
        lines(&[
            "<TS>",
            " <tbool value=\"true\" />",
            " <tubyte value=\"200\" />",
            " <tsbyte value=\"-2\" />",
            " <tbenum value=\"7\" />",
            " <tushort value=\"65535\" />",
            " <tsshort value=\"-3\" />",
            " <thalf value=\"1\" />",
            " <tsint value=\"-5\" />",
            " <tuint value=\"4000000000\" />",
            " <tfloat value=\"1.5\" />",
            " <tulong value=\"1099511627776\" />",
            " <color value=\"0xFF00FF00\" />",
            "</TS>",
        ])
    );
}

#[test]
fn the_file_name_goes_on_the_root_tag() {
    let mut m = single(vec![entry("tbool", 0, ty::BOOLEAN, 0)], vec![0]);
    m.name = "a<b".to_owned();
    assert_eq!(m.to_xml().0, lines(&["<TS name=\"a&lt;b\">", " <tbool value=\"false\" />", "</TS>"]));
    assert_eq!(m.root_type().as_deref(), Some("TS"));
}

#[test]
fn vectors_are_written_as_attributes() {
    let mut d = Vec::new();
    for (i, v) in [1.0f32, 2.0, 1.0, 2.0, 3.0, 0.0, 0.0, 0.0, 1.0, 2.0, 3.0, 4.0].iter().enumerate() {
        put(&mut d, i * 4, &v.to_le_bytes());
    }
    let m = single(
        vec![
            entry("tv2", 0, ty::VEC2, 0),
            entry("tv3", 8, ty::FLOAT_XYZ, 0),
            entry("tv3p", 8, ty::VEC3_PADDED, 0),
            entry("tv4", 32, ty::FLOAT_XYZW, 0),
        ],
        d,
    );
    assert_eq!(
        m.to_xml().0,
        lines(&[
            "<TS>",
            " <tv2 x=\"1\" y=\"2\" />",
            " <tv3 x=\"1\" y=\"2\" z=\"3\" />",
            " <tv3p x=\"1\" y=\"2\" z=\"3\" />",
            " <tv4 x=\"1\" y=\"2\" z=\"3\" w=\"4\" />",
            "</TS>",
        ])
    );
}

#[test]
fn enums_and_flags_are_written_as_names() {
    let mut d = Vec::new();
    put(&mut d, 0, &2i32.to_le_bytes());
    put(&mut d, 4, &99i32.to_le_bytes());
    put(&mut d, 8, &5i32.to_le_bytes());
    put(&mut d, 12, &5i32.to_le_bytes());
    put(&mut d, 16, &2u16.to_le_bytes());
    put(&mut d, 20, &0i32.to_le_bytes());
    // 0x64 (short flags) is not in the flags branch of `enum_string`: the value 2 is looked up as a plain enum value.
    let mut m = single(
        vec![
            entry("te1", 0, ty::INT_ENUM, nm("EnumA")),
            entry("te2", 4, ty::INT_ENUM, nm("EnumA")),
            entry("tfl", 8, ty::INT_FLAGS2, nm("EnumF")),
            entry("tnokey", 12, ty::INT_FLAGS2, nm("EnumNone")),
            entry("tsfl", 16, ty::SHORT_FLAGS, nm("EnumF")),
            entry("tzero", 20, ty::INT_FLAGS2, nm("EnumF")),
        ],
        d,
    );
    m.enums.insert(nm("EnumA"), vec![(nm("ValA"), 1), (nm("ValB"), 2)]);
    m.enums.insert(nm("EnumF"), vec![(nm("FlA"), 0), (nm("FlB"), 1), (nm("FlC"), 2), (nm("FlHigh"), 40)]);
    assert_eq!(
        m.to_xml().0,
        lines(&[
            "<TS>",
            " <te1>ValB</te1>",
            " <te2>99</te2>",
            " <tfl>FlA, FlC</tfl>",
            " <tnokey>5</tnokey>",
            " <tsfl>FlC</tsfl>",
            " <tzero />",
            "</TS>",
        ])
    );
}

#[test]
fn rdr2_int_flags_are_one_byte() {
    let mut m = single(vec![entry("tfl", 0, ty::INT_FLAGS1, nm("EnumF"))], vec![0b11, 0xFF, 0xFF, 0xFF]);
    m.offset_shift = 20;
    m.enums.insert(nm("EnumF"), vec![(nm("FlA"), 0), (nm("FlB"), 1), (nm("FlC"), 2)]);
    assert_eq!(m.to_xml().0, lines(&["<TS>", " <tfl>FlA, FlB</tfl>", "</TS>"]));
}

#[test]
fn hashes_are_written_as_names_or_hex() {
    let mut d = Vec::new();
    put(&mut d, 0, &joaat("someasset").to_le_bytes());
    put(&mut d, 8, &0xABu64.to_le_bytes());
    put(&mut d, 16, &[0; 8]);
    let m = single(
        vec![
            entry("th", 0, ty::HASH, 0),
            entry("th0", 4, ty::HASH2, 0),
            entry("th64", 8, ty::HASH64, 0),
            entry("th640", 16, ty::HASH64, 0),
        ],
        d,
    );
    nm("someasset");
    assert_eq!(
        m.to_xml().0,
        lines(&[
            "<TS>",
            " <th>someasset</th>",
            " <th0 />",
            " <th64>hash_00000000000000AB</th64>",
            " <th640></th640>",
            "</TS>",
        ])
    );
}

#[test]
fn matrices_and_opaque_values_are_written_as_rows_and_bytes() {
    let mut d = Vec::new();
    for i in 0..16 {
        put(&mut d, i * 4, &(i as f32).to_le_bytes());
    }
    put(&mut d, 64, &[0xAB, 0x01]);
    put(&mut d, 80, &[0]);
    let m = single(
        vec![entry("tm4", 0, ty::MATRIX, 0), entry("tm34", 0, ty::MATRIX34, 0), entry("topq", 64, ty::OPAQUE16, 0)],
        d,
    );
    assert_eq!(
        m.to_xml().0,
        lines(&[
            "<TS>",
            " <tm4>",
            "  0 1 2 3",
            "  4 5 6 7",
            "  8 9 10 11",
            "  12 13 14 15",
            " </tm4>",
            " <tm34>",
            "  0 1 2 3",
            "  4 5 6 7",
            "  8 9 10 11",
            " </tm34>",
            " <topq>AB 01 00 00 00 00 00 00 00 00 00 00 00 00 00 00</topq>",
            "</TS>",
        ])
    );
}

#[test]
fn a_value_of_an_unknown_type_is_an_error_element() {
    let m = single(vec![entry("tx", 0, 0x7E, 0), entry("tb", 0, ty::BOOLEAN, 0)], vec![1]);
    let (xml, errors) = m.to_xml();
    assert_eq!(
        xml,
        lines(&["<TS>", " <error>tx: unexpected data type 0x7e</error>", " <tb value=\"true\" />", "</TS>"])
    );
    assert_eq!(errors, ["tx: unexpected data type 0x7e"]);
}

#[test]
fn reading_beyond_the_block_is_an_error_element() {
    let m = single(vec![entry("tu", 0, ty::UINT, 0), entry("tb", 8, ty::BOOLEAN, 0)], vec![1, 2]);
    let (_, errors) = m.to_xml();
    assert_eq!(errors, ["tu: read of 4 bytes at 0x0 beyond block of 0x2", "tb: offset 0x8 beyond block"]);
}

#[test]
fn strings_are_read_through_pointers() {
    let mut d = Vec::new();
    put(&mut d, 0, &ptr(2, 0).to_le_bytes());
    put(&mut d, 8, &5u16.to_le_bytes());
    put(&mut d, 16, &ptr(2, 6).to_le_bytes());
    put(&mut d, 24, b"abc\0xxxx");
    put(&mut d, 32, &0u64.to_le_bytes());
    put(&mut d, 40, &ptr(9, 0).to_le_bytes());
    put(&mut d, 48, &3u16.to_le_bytes());
    let m = meta(
        vec![(
            "TS",
            info(vec![
                entry("tcp", 0, ty::CHAR_POINTER, 0),
                entry("tsp", 16, ty::STRING_POINTER, 0),
                entry("tac", 24, ty::ARRAY_OF_CHARS, 8),
                entry("tnull", 32, ty::CHAR_POINTER, 0),
                entry("tnullsp", 32, ty::STRING_POINTER, 0),
                entry("tbad", 40, ty::CHAR_POINTER, 0),
            ]),
        )],
        vec![("TS", d), ("Str", b"hello <w>\0\0".to_vec())],
    );
    let (xml, errors) = m.to_xml();
    assert_eq!(
        xml,
        lines(&[
            "<TS>",
            " <tcp>hello</tcp>",
            " <tsp>&lt;w&gt;</tsp>",
            " <tac>abc</tac>",
            " <tnull></tnull>",
            " <tnullsp></tnullsp>",
            " <error>tbad: string block 9 missing</error>",
            "</TS>",
        ])
    );
    assert_eq!(errors, ["tbad: string block 9 missing"]);
}

#[test]
fn byte_arrays_follow_the_item_type_of_the_array_info() {
    let d = vec![1, 0xFF, 3, 4];
    let m = meta(
        vec![(
            "TS",
            info(vec![
                entry("", 0, ty::SBYTE, 0),
                entry("tsb", 0, ty::ARRAY_OF_BYTES, 4),
                entry("", 0, ty::USHORT, 0),
                entry("tus", 0, ty::ARRAY_OF_BYTES, 2),
                entry("", 0, 0x77, 0),
                entry("traw", 0, ty::ARRAY_OF_BYTES, 3),
            ]),
        )],
        vec![("TS", d)],
    );
    assert_eq!(
        m.to_xml().0,
        lines(&["<TS>", " <tsb>1 -1 3 4</tsb>", " <tus>65281 1027</tus>", " <traw>01FF03</traw>", "</TS>"])
    );
}

#[test]
fn a_data_block_pointer_shows_the_whole_block() {
    let mut d = Vec::new();
    put(&mut d, 0, &ptr(2, 0).to_le_bytes());
    put(&mut d, 8, &ptr(7, 0).to_le_bytes());
    let m = meta(
        vec![(
            "TS",
            info(vec![entry("tblob", 0, ty::DATA_BLOCK_POINTER, 0), entry("tnone", 8, ty::DATA_BLOCK_POINTER, 0)]),
        )],
        vec![("TS", d), ("Blob", vec![1, 0xAB])],
    );
    assert_eq!(m.to_xml().0, lines(&["<TS>", " <tblob>01 AB</tblob>", " <tnone />", "</TS>"]));
}

fn item_struct(entries: Vec<Entry>) -> Vec<(&'static str, StructInfo)> {
    vec![("TI", StructInfo { size: 4, entries: vec![entry("tv", 0, ty::UINT, 0)] }), ("TS", info(entries))]
}

#[test]
fn nested_structures_are_written_inline_or_through_pointers() {
    let mut d = Vec::new();
    put(&mut d, 0, &5u32.to_le_bytes());
    put(&mut d, 8, &ptr(2, 0).to_le_bytes());
    put(&mut d, 16, &[0; 8]);
    let m = meta(
        item_struct(vec![
            entry("tin", 0, ty::STRUCTURE, nm("TI")),
            entry("tptr", 8, ty::STRUCTURE_POINTER, 0),
            entry("tnull", 16, ty::STRUCTURE_POINTER, 0),
        ]),
        vec![("TS", d), ("TI", 9u32.to_le_bytes().to_vec())],
    );
    assert_eq!(
        m.to_xml().0,
        lines(&[
            "<TS>",
            " <tin>",
            "  <tv value=\"5\" />",
            " </tin>",
            " <tptr type=\"TI\">",
            "  <tv value=\"9\" />",
            " </tptr>",
            " <tnull />",
            "</TS>",
        ])
    );
}

#[test]
fn a_structure_that_contains_itself_stops_at_the_depth_limit() {
    let m = meta(vec![("TS", info(vec![entry("tself", 0, ty::STRUCTURE, nm("TS"))]))], vec![("TS", vec![0; 8])]);
    let (xml, errors) = m.to_xml();
    assert_eq!(errors, ["structures nested too deeply"]);
    assert_eq!(xml.matches("<tself>").count(), 65);
}

#[test]
fn a_missing_block_or_structure_info_is_an_error() {
    let mut m = single(vec![], vec![]);
    m.root_block = 3;
    assert_eq!(m.to_xml().1, ["block 3 not found"]);
    assert_eq!(m.root_type(), None);
    let m = meta(vec![], vec![("Unknown", vec![0])]);
    assert_eq!(m.to_xml().1, ["structure info Unknown not found"]);
    assert_eq!(m.root_type().as_deref(), Some("Unknown"));
}

#[test]
fn number_arrays_wrap_after_ten_items() {
    let mut d = Vec::new();
    put(&mut d, 0, &ptr(2, 0).to_le_bytes());
    put(&mut d, 8, &12u16.to_le_bytes());
    let items: Vec<u8> = (0..12u32).flat_map(|i| i.to_le_bytes()).collect();
    let m = meta(
        vec![("TS", info(vec![entry("", 0, ty::UINT, 0), entry("tarr", 0, ty::ARRAY, 0)]))],
        vec![("TS", d), ("Arr", items)],
    );
    assert_eq!(m.to_xml().0, lines(&["<TS>", " <tarr>", "  0 1 2 3 4 5 6 7 8 9", "  10 11", " </tarr>", "</TS>"]));
}

#[test]
fn every_integer_array_type_reads_its_items() {
    let cases: [(u8, Vec<u8>, &str); 7] = [
        (ty::UBYTE, vec![1, 255], "1 255"),
        (ty::USHORT, vec![1, 0, 255, 255], "1 65535"),
        (ty::SSHORT, vec![255, 255, 2, 0], "-1 2"),
        (ty::SINT, (-7i32).to_le_bytes().into_iter().chain(8i32.to_le_bytes()).collect(), "-7 8"),
        (ty::UINT, 4_000_000_000u32.to_le_bytes().into_iter().chain(1u32.to_le_bytes()).collect(), "4000000000 1"),
        (ty::ULONG, (1u64 << 33).to_le_bytes().into_iter().chain(2u64.to_le_bytes()).collect(), "8589934592 2"),
        (ty::FLOAT, 0.5f32.to_le_bytes().into_iter().chain(2.0f32.to_le_bytes()).collect(), "0.5 2"),
    ];
    for (kind, items, text) in cases {
        let mut d = Vec::new();
        put(&mut d, 0, &ptr(2, 0).to_le_bytes());
        put(&mut d, 8, &2u16.to_le_bytes());
        let m = meta(
            vec![("TS", info(vec![entry("", 0, kind, 0), entry("tarr", 0, ty::ARRAY, 0)]))],
            vec![("TS", d), ("Arr", items)],
        );
        assert_eq!(m.to_xml().0, lines(&["<TS>", &format!(" <tarr>{text}</tarr>"), "</TS>"]), "type {kind:#x}");
    }
}

#[test]
fn vector_and_hash_arrays_have_one_item_element_per_entry() {
    let mut d = Vec::new();
    put(&mut d, 0, &ptr(2, 0).to_le_bytes());
    put(&mut d, 8, &1u16.to_le_bytes());
    put(&mut d, 16, &ptr(3, 0).to_le_bytes());
    put(&mut d, 24, &2u16.to_le_bytes());
    put(&mut d, 32, &ptr(4, 0).to_le_bytes());
    put(&mut d, 40, &1u16.to_le_bytes());
    let floats: Vec<u8> = [1.0f32, 2.0, 3.0, 0.0].iter().flat_map(|f| f.to_le_bytes()).collect();
    let hashes: Vec<u8> = joaat("arrname").to_le_bytes().into_iter().chain(0u32.to_le_bytes()).collect();
    let m = meta(
        vec![(
            "TS",
            info(vec![
                entry("", 0, ty::FLOAT_XYZ, 0),
                entry("tvec", 0, ty::ARRAY, 0),
                entry("", 0, ty::HASH, 0),
                entry("thash", 16, ty::ARRAY, 0),
                entry("", 0, ty::HASH64, 0),
                entry("th64", 32, ty::ARRAY, 0),
            ]),
        )],
        vec![("TS", d), ("Vec", floats), ("Hash", hashes), ("H64", 0xFFu64.to_le_bytes().to_vec())],
    );
    nm("arrname");
    assert_eq!(
        m.to_xml().0,
        lines(&[
            "<TS>",
            " <tvec>",
            "  <Item>1, 2, 3, 0</Item>",
            " </tvec>",
            " <thash>",
            "  <Item>arrname</Item>",
            "  <Item></Item>",
            " </thash>",
            " <th64>",
            "  <Item>hash_00000000000000FF</Item>",
            " </th64>",
            "</TS>",
        ])
    );
}

#[test]
fn a_number_array_continues_in_the_next_block() {
    let mut d = Vec::new();
    put(&mut d, 0, &ptr(2, 0).to_le_bytes());
    put(&mut d, 8, &3u16.to_le_bytes());
    let first: Vec<u8> = [1u32, 2].iter().flat_map(|v| v.to_le_bytes()).collect();
    let m = meta(
        vec![("TS", info(vec![entry("", 0, ty::UINT, 0), entry("tarr", 0, ty::ARRAY, 0)]))],
        vec![("TS", d), ("Arr", first), ("Arr", 3u32.to_le_bytes().to_vec())],
    );
    assert_eq!(m.to_xml().0, lines(&["<TS>", " <tarr>1 2 3</tarr>", "</TS>"]));
    let mut broken = m.clone();
    broken.blocks.pop();
    assert_eq!(broken.to_xml().1, ["tarr: array block 3 missing"]);
}

#[test]
fn an_array_of_an_unsupported_item_type_is_an_error() {
    let m = single(vec![entry("", 0, ty::BOOLEAN, 0), entry("tarr", 0, ty::ARRAY, 0)], vec![0; 16]);
    assert_eq!(m.to_xml().1, ["tarr: unexpected array item type 0x01"]);
}

#[test]
fn structure_arrays_list_their_items() {
    let mut d = Vec::new();
    put(&mut d, 0, &ptr(2, 0).to_le_bytes());
    put(&mut d, 8, &2u16.to_le_bytes());
    put(&mut d, 16, &ptr(2, 0).to_le_bytes());
    put(&mut d, 24, &[0; 8]);
    let items: Vec<u8> = [1u32, 2].iter().flat_map(|v| v.to_le_bytes()).collect();
    let m = meta(
        item_struct(vec![
            entry("", 0, ty::STRUCTURE, nm("TI")),
            entry("tarr", 0, ty::ARRAY, 0),
            entry("", 0, ty::STRUCTURE, nm("TI")),
            entry("tempty", 16, ty::ARRAY, 0),
        ]),
        vec![("TS", d), ("TI", items)],
    );
    assert_eq!(
        m.to_xml().0,
        lines(&[
            "<TS>",
            " <tarr itemType=\"TI\">",
            "  <Item>",
            "   <tv value=\"1\" />",
            "  </Item>",
            "  <Item>",
            "   <tv value=\"2\" />",
            "  </Item>",
            " </tarr>",
            " <tempty itemType=\"TI\" />",
            "</TS>",
        ])
    );
}

#[test]
fn a_structure_array_of_an_unknown_structure_is_an_error() {
    let mut d = Vec::new();
    put(&mut d, 8, &1u16.to_le_bytes());
    let m = single(vec![entry("", 0, ty::STRUCTURE, 0x1234), entry("tarr", 0, ty::ARRAY, 0)], d);
    assert_eq!(m.to_xml().1, ["tarr: array item structure not found"]);
}

#[test]
fn structure_pointer_arrays_list_typed_items() {
    let mut d = Vec::new();
    put(&mut d, 0, &ptr(2, 0).to_le_bytes());
    put(&mut d, 8, &2u16.to_le_bytes());
    put(&mut d, 16, &0u64.to_le_bytes());
    put(&mut d, 24, &0u16.to_le_bytes());
    let mut pointers = ptr(3, 0).to_le_bytes().to_vec();
    pointers.extend_from_slice(&0u64.to_le_bytes());
    let m = meta(
        item_struct(vec![
            entry("", 0, ty::STRUCTURE_POINTER, 0),
            entry("tarr", 0, ty::ARRAY, 0),
            entry("", 0, ty::STRUCTURE_POINTER, 0),
            entry("tempty", 16, ty::ARRAY, 0),
        ]),
        vec![("TS", d), ("Ptrs", pointers), ("TI", 9u32.to_le_bytes().to_vec())],
    );
    assert_eq!(
        m.to_xml().0,
        lines(&[
            "<TS>",
            " <tarr>",
            "  <Item type=\"TI\">",
            "   <tv value=\"9\" />",
            "  </Item>",
            "  <Item />",
            " </tarr>",
            " <tempty />",
            "</TS>",
        ])
    );
}

fn map_meta(table: bool, pairs: Vec<u8>, header: &[u8]) -> Meta {
    let mut d = vec![0; 0x20];
    put(&mut d, 0, header);
    let (kind, key) = if table { (ty::HASH_TABLE, 1 << 16) } else { (ty::MAP, 1) };
    let entries =
        vec![entry("tkey", 0x18, ty::USHORT, 0), entry("tvalue", 0x1A, ty::UINT, 0), entry("tmap", 0, kind, key)];
    meta(vec![("TS", info(entries))], vec![("TS", d), ("Map", pairs)])
}

#[test]
fn a_map_lists_its_pairs() {
    let mut header = Vec::new();
    header.extend_from_slice(&1u64.to_le_bytes());
    header.extend_from_slice(&ptr(2, 0).to_le_bytes());
    header.extend_from_slice(&2u16.to_le_bytes());
    let mut pairs = vec![0u8; 32];
    put(&mut pairs, 0, &7u32.to_le_bytes());
    put(&mut pairs, 8, &70u32.to_le_bytes());
    put(&mut pairs, 16, &8u32.to_le_bytes());
    put(&mut pairs, 24, &80u32.to_le_bytes());
    let m = map_meta(false, pairs, &header);
    assert_eq!(
        m.to_xml().0,
        lines(&[
            "<TS>",
            " <tkey value=\"0\" />",
            " <tvalue value=\"0\" />",
            " <tmap>",
            "  <Item key=\"7\">",
            "   <Value value=\"70\" />",
            "  </Item>",
            "  <Item key=\"8\">",
            "   <Value value=\"80\" />",
            "  </Item>",
            " </tmap>",
            "</TS>",
        ])
    );
}

#[test]
fn an_empty_map_is_an_empty_element() {
    let m = map_meta(false, vec![], &[0; 24]);
    assert_eq!(
        m.to_xml().0,
        lines(&["<TS>", " <tkey value=\"0\" />", " <tvalue value=\"0\" />", " <tmap />", "</TS>"])
    );
}

#[test]
fn a_map_pair_beyond_its_block_ends_the_map() {
    let mut header = Vec::new();
    header.extend_from_slice(&1u64.to_le_bytes());
    header.extend_from_slice(&ptr(2, 0).to_le_bytes());
    header.extend_from_slice(&2u16.to_le_bytes());
    let m = map_meta(false, vec![0; 16], &header);
    let (xml, errors) = m.to_xml();
    assert_eq!(errors, ["map pair 1 beyond its block"]);
    assert!(xml.contains("<Item key=\"0\">"));
}

#[test]
fn a_map_with_bad_type_indices_or_block_is_an_error() {
    let mut header = vec![0u8; 24];
    put(&mut header, 8, &ptr(2, 0).to_le_bytes());
    put(&mut header, 16, &1u16.to_le_bytes());
    let mut m = map_meta(false, vec![0; 16], &header);
    m.structs.values_mut().next().unwrap().entries[2].key = 9 << 16 | 1;
    assert_eq!(m.to_xml().1, ["tmap: map key type not found"]);
    m.structs.values_mut().next().unwrap().entries[2].key = 1 << 16 | 9;
    assert_eq!(m.to_xml().1, ["tmap: map value type not found"]);
    let mut m = map_meta(false, vec![0; 16], &header);
    m.blocks.pop();
    assert_eq!(m.to_xml().1, ["tmap: map block 2 not found"]);
}

#[test]
fn a_hash_table_lists_the_used_places_of_its_groups() {
    let mut header = Vec::new();
    header.extend_from_slice(&1u32.to_le_bytes());
    header.extend_from_slice(&2u32.to_le_bytes());
    header.extend_from_slice(&ptr(2, 0).to_le_bytes());
    let mut pairs = vec![0x80u8; 16];
    pairs[0] = 0;
    pairs[3] = 1;
    pairs.resize(16 + 16 * 16, 0);
    put(&mut pairs, 16, &10u32.to_le_bytes());
    put(&mut pairs, 24, &100u32.to_le_bytes());
    put(&mut pairs, 16 + 3 * 16, &30u32.to_le_bytes());
    put(&mut pairs, 16 + 3 * 16 + 8, &300u32.to_le_bytes());
    let m = map_meta(true, pairs, &header);
    assert_eq!(
        m.to_xml().0,
        lines(&[
            "<TS>",
            " <tkey value=\"0\" />",
            " <tvalue value=\"0\" />",
            " <tmap>",
            "  <Item key=\"10\">",
            "   <Value value=\"100\" />",
            "  </Item>",
            "  <Item key=\"30\">",
            "   <Value value=\"300\" />",
            "  </Item>",
            " </tmap>",
            "</TS>",
        ])
    );
    let mut wrong = header.clone();
    put(&mut wrong, 4, &3u32.to_le_bytes());
    let m = map_meta(true, m.blocks[1].data.clone(), &wrong);
    assert_eq!(m.to_xml().1, ["tmap: 2 places used, the table names 3"]);
}

#[test]
fn hash_table_pairs_checks_its_bounds() {
    let block = Block { name: 0, data: vec![0; 16 + 16 * 8] };
    assert_eq!(hash_table_pairs(&block, 0, 1, 8).unwrap().len(), 16);
    assert_eq!(hash_table_pairs(&block, 0, 2, 8), Err("hash table beyond its block".to_owned()));
    assert_eq!(hash_table_pairs(&block, usize::MAX, 1, 8), Err("hash table beyond its block".to_owned()));
    assert_eq!(hash_table_pairs(&block, 0, usize::MAX, 8), Err("hash table beyond its block".to_owned()));
    assert_eq!(hash_table_pairs(&block, 0, 0, 8).unwrap(), Vec::<usize>::new());
}

#[test]
fn number_text_covers_every_plain_kind() {
    let d = [0xFE, 0xFF, 0x00, 0x3C, 0, 0, 0, 0, 0, 0, 0, 0];
    let text = |kind, name| number_text(kind, name, &d, 0);
    assert_eq!(text(ty::BOOLEAN, "x"), Ok(Some("true".to_owned())));
    assert_eq!(text(ty::SBYTE, "x"), Ok(Some("-2".to_owned())));
    assert_eq!(text(ty::UBYTE, "x"), Ok(Some("254".to_owned())));
    assert_eq!(text(ty::BYTE_ENUM, "x"), Ok(Some("254".to_owned())));
    assert_eq!(text(ty::SSHORT, "x"), Ok(Some("-2".to_owned())));
    assert_eq!(text(ty::USHORT, "x"), Ok(Some("65534".to_owned())));
    assert_eq!(text(ty::SHORT_ENUM, "x"), Ok(Some("65534".to_owned())));
    assert_eq!(text(ty::SINT, "x"), Ok(Some("1006698494".to_owned())));
    assert_eq!(text(ty::UINT, "color"), Ok(Some("0x3C00FFFE".to_owned())));
    assert_eq!(text(ty::ULONG2, "x"), Ok(Some("1006698494".to_owned())));
    assert_eq!(text(ty::STRUCTURE, "x"), Ok(None));
    assert_eq!(number_text(ty::UINT, "x", &d, 10), Err("read of 4 bytes at 0xa beyond block of 0xc".to_owned()));
    assert_eq!(number_text(ty::ULONG, "x", &d, 8), Err("read of 8 bytes at 0x8 beyond block of 0xc".to_owned()));
}

#[test]
fn vector_len_names_the_float_count() {
    assert_eq!(vector_len(ty::VEC2), Some(2));
    assert_eq!(vector_len(ty::VEC3), Some(3));
    assert_eq!(vector_len(ty::FLOAT_XYZ), Some(3));
    assert_eq!(vector_len(ty::VEC3_PADDED), Some(3));
    assert_eq!(vector_len(ty::FLOAT_XYZW), Some(4));
    assert_eq!(vector_len(ty::FLOAT), None);
}

#[test]
fn byte_array_text_reads_each_item_type() {
    let d = [1u8, 0xFF, 2, 0, 0xFE, 0xFF, 0, 0];
    assert_eq!(byte_array_text(&d, 0, 2, ty::SBYTE), Ok(vec!["1".to_owned(), "-1".to_owned()]));
    assert_eq!(byte_array_text(&d, 0, 2, ty::UBYTE), Ok(vec!["1".to_owned(), "255".to_owned()]));
    assert_eq!(byte_array_text(&d, 2, 2, ty::SSHORT), Ok(vec!["2".to_owned(), "-2".to_owned()]));
    assert_eq!(byte_array_text(&d, 2, 2, ty::USHORT), Ok(vec!["2".to_owned(), "65534".to_owned()]));
    assert_eq!(byte_array_text(&d, 4, 1, ty::SINT), Ok(vec!["65534".to_owned()]));
    assert_eq!(byte_array_text(&d, 4, 1, ty::UINT), Ok(vec!["65534".to_owned()]));
    assert_eq!(byte_array_text(&1.5f32.to_le_bytes(), 0, 1, ty::FLOAT), Ok(vec!["1.5".to_owned()]));
    assert_eq!(byte_array_text(&d, 0, 3, 0x77), Ok(vec!["01FF02".to_owned()]));
    assert_eq!(byte_array_text(&d, 6, 3, ty::UBYTE), Err("offset 0x8 beyond block".to_owned()));
    assert_eq!(byte_array_text(&d, 6, 2, ty::UINT), Err("read of 4 bytes at 0x6 beyond block of 0x8".to_owned()));
    assert_eq!(byte_array_text(&d, 7, 2, 0x77), Err("offset 0x8 beyond block".to_owned()));
}

#[test]
fn map_header_reads_maps_and_tables_differently() {
    let mut d = vec![0u8; 24];
    put(&mut d, 0, &5u32.to_le_bytes());
    put(&mut d, 4, &9u32.to_le_bytes());
    put(&mut d, 8, &0x1234u64.to_le_bytes());
    put(&mut d, 16, &3u16.to_le_bytes());
    let map = map_header(&d, 0, false).unwrap();
    assert_eq!((map.ptr, map.groups, map.count), (0x1234, 0, 3));
    let table = map_header(&d, 0, true).unwrap();
    assert_eq!((table.ptr, table.groups, table.count), (0x1234, 5, 9));
    assert_eq!(
        map_header(&d[..17], 0, false).map(|_| ()),
        Err("read of 2 bytes at 0x10 beyond block of 0x11".to_owned())
    );
    assert!(map_header(&d, 8, false).is_err());
}

#[test]
fn map_entry_indices_swap_for_tables() {
    assert_eq!(map_entry_indices(3 << 16 | 4, false), (3, 4));
    assert_eq!(map_entry_indices(3 << 16 | 4, true), (4, 3));
}

#[test]
fn array_items_know_their_sizes_and_texts() {
    let sizes = [
        (ty::UINT, 4, Some(10)),
        (ty::USHORT, 2, Some(10)),
        (ty::UBYTE, 1, Some(10)),
        (ty::ULONG, 8, Some(10)),
        (ty::ULONG2, 8, Some(10)),
        (ty::SINT, 4, Some(10)),
        (ty::SSHORT, 2, Some(10)),
        (ty::FLOAT, 4, Some(10)),
        (ty::FLOAT_XYZ, 16, None),
        (ty::FLOAT_XYZW, 16, None),
        (ty::HASH64, 8, None),
        (ty::HASH, 4, None),
        (ty::HASH2, 4, None),
    ];
    for (kind, size, columns) in sizes {
        let items = array_items(kind).unwrap_or_else(|| panic!("no items for {kind:#x}"));
        assert_eq!((items.size, items.columns), (size, columns), "type {kind:#x}");
    }
    assert!(array_items(ty::BOOLEAN).is_none());
    assert!(array_items(ty::STRUCTURE).is_none());
    assert_eq!((array_items(ty::SINT).unwrap().text)(&(-3i32).to_le_bytes()), "-3");
    assert_eq!((array_items(ty::UBYTE).unwrap().text)(&[200]), "200");
    assert_eq!((array_items(ty::HASH64).unwrap().text)(&[0; 8]), "");
    assert_eq!((array_items(ty::HASH).unwrap().text)(&[0; 4]), "");
}

#[test]
fn small_text_helpers_format_hashes_vectors_and_pointers() {
    assert_eq!(hash64_text(0), "");
    assert_eq!(hash64_text(0xAB), "hash_00000000000000AB");
    assert_eq!(vector_attrs("v", &[1.0, 2.5]), "v x=\"1\" y=\"2.5\"");
    assert_eq!(vector_attrs("v", &[1.0, 2.0, 3.0, 4.0, 5.0]), "v x=\"1\" y=\"2\" z=\"3\" w=\"4\"");
    assert_eq!(
        float4_text(&[1.0f32, 2.0, 3.0, 0.5].iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<_>>()),
        "1, 2, 3, 0.5"
    );
    assert_eq!(unpack_with(0x0030_5002, 12), (2, 0x305));
    assert_eq!(unpack_with(3 | 0x40 << 20, 20), (3, 0x40));
    assert_eq!(unpack_with(3 | 0x40 << 20, 12), (3, 0x40 << 8));
    assert!(is_meta(VERSION) && is_meta(VERSION_RDR2) && !is_meta(5));
    assert!(is_gta6(b"0123456789abcdef0DRP\x7a\0\0\0"));
    assert!(!is_gta6(b"short"));
}

#[test]
fn chars_are_cut_at_the_zero_or_the_data_end() {
    let site = |data| Site { level: 0, block_id: 1, data, at: 1, depth: 0 };
    let mut w = Writer::default();
    chars_entry(&mut w, &site(b"xab\0cd"), 5, "n");
    chars_entry(&mut w, &site(b"xabc"), 9, "n");
    chars_entry(&mut w, &site(b"x"), 4, "n");
    assert_eq!(w.out, "<n>ab</n>\n<n>abc</n>\n<n></n>\n");
}

#[test]
fn the_array_helper_rejects_missing_blocks_and_short_data() {
    let m = meta(vec![], vec![("Arr", vec![1, 2, 3, 4, 5, 6])]);
    assert_eq!(m.array(ptr(1, 0), 1, 4), Ok(vec![1, 2, 3, 4]));
    assert_eq!(m.array(ptr(2, 0), 1, 4), Err("array block 2 missing".to_owned()));
    assert_eq!(m.array(ptr(1, 4), 1, 4), Err("array block 2 missing".to_owned()));
    assert_eq!(m.array(ptr(0, 0), 1, 4), Err("array block 0 missing".to_owned()));
    assert_eq!(m.array(ptr(1, 0), 0, 4), Ok(vec![]));
}

#[test]
fn the_enum_string_of_a_flag_ignores_bits_outside_the_value() {
    let mut m = meta(vec![], vec![]);
    m.enums.insert(7, vec![(nm("FlA"), 0), (nm("FlB"), -1), (nm("FlC"), 31)]);
    let e = Entry { key: 7, kind: ty::INT_FLAGS1, ..Entry::default() };
    assert_eq!(m.enum_string(&e, -1), "FlA, FlC");
    let plain = Entry { key: 7, kind: ty::INT_ENUM, ..Entry::default() };
    assert_eq!(m.enum_string(&plain, 31), "FlC");
    assert_eq!(m.enum_string(&plain, 5), "5");
}

struct Image {
    system: Vec<u8>,
    version: u32,
    shift: usize,
}

impl Image {
    fn new(version: u32) -> Self {
        let shift = if version == VERSION_RDR2 { 8 } else { 0 };
        Self { system: vec![0; 0x800], version, shift }
    }

    fn at(offset: usize) -> u64 {
        Resource::ROOT + offset as u64
    }

    fn put(&mut self, at: usize, bytes: &[u8]) {
        put(&mut self.system, at, bytes);
    }

    fn resource(&self) -> Resource {
        Resource { version: self.version, system: self.system.clone(), graphics: vec![] }
    }

    /// One structure at 0x100 (entries at 0x200), one enum at 0x300 (values at 0x340), blocks at 0x400
    /// (data at 0x500 and 0x600) and the name at 0x700.
    fn filled(version: u32, name: bool) -> Self {
        let mut i = Self::new(version);
        let s = i.shift;
        i.put(0x1C + s - if s == 8 { 4 } else { 0 }, &1u32.to_le_bytes());
        i.put(0x20 + s, &Self::at(0x100).to_le_bytes());
        i.put(0x28 + s, &Self::at(0x300).to_le_bytes());
        i.put(0x30 + s, &Self::at(0x400).to_le_bytes());
        if name {
            i.put(0x38 + s, &Self::at(0x700).to_le_bytes());
            i.put(0x700, b"my.ymap\0");
        }
        i.put(0x48, &1i16.to_le_bytes());
        i.put(0x4A, &1i16.to_le_bytes());
        i.put(0x4C, &2i16.to_le_bytes());
        i.put(0x100, &nm("TS").to_le_bytes());
        i.put(0x110, &Self::at(0x200).to_le_bytes());
        i.put(0x118, &0x20u32.to_le_bytes());
        i.put(0x11E, &2i16.to_le_bytes());
        i.put(0x200, &nm("tb").to_le_bytes());
        i.put(0x204, &0u32.to_le_bytes());
        i.put(0x208, &[ty::BOOLEAN]);
        i.put(0x210, &nm("te").to_le_bytes());
        i.put(0x214, &4u32.to_le_bytes());
        i.put(0x218, &[ty::INT_ENUM]);
        i.put(0x21C, &nm("TE").to_le_bytes());
        i.put(0x300, &nm("TE").to_le_bytes());
        i.put(0x308, &Self::at(0x340).to_le_bytes());
        i.put(0x310, &2u32.to_le_bytes());
        if version == VERSION_RDR2 {
            i.put(0x340, &1i32.to_le_bytes());
            i.put(0x348, &nm("One").to_le_bytes());
            i.put(0x350, &2i32.to_le_bytes());
            i.put(0x358, &nm("Two").to_le_bytes());
        } else {
            i.put(0x340, &nm("One").to_le_bytes());
            i.put(0x344, &1i32.to_le_bytes());
            i.put(0x348, &nm("Two").to_le_bytes());
            i.put(0x34C, &2i32.to_le_bytes());
        }
        i.put(0x400, &nm("TS").to_le_bytes());
        i.put(0x404, &8u32.to_le_bytes());
        i.put(0x408, &Self::at(0x500).to_le_bytes());
        i.put(0x410, &nm("Other").to_le_bytes());
        i.put(0x414, &0u32.to_le_bytes());
        i.put(0x500, &[1, 0, 0, 0, 2, 0, 0, 0]);
        i
    }
}

const EXPECTED_FILLED: [&str; 4] = [" <tb value=\"true\" />", " <te>Two</te>", "</TS>", ""];

#[test]
fn a_gta5_meta_resource_is_parsed_and_written() {
    let image = Image::filled(VERSION, true);
    let meta = parse(&image.resource()).unwrap();
    assert_eq!(meta.name, "my.ymap");
    assert_eq!(meta.root_block, 1);
    assert_eq!(meta.offset_shift, 12);
    assert_eq!(meta.blocks.len(), 2);
    assert!(meta.blocks[1].data.is_empty());
    assert_eq!(meta.enums[&nm("TE")], [(nm("One"), 1), (nm("Two"), 2)]);
    assert_eq!(meta.structs[&nm("TS")].size, 0x20);
    let (xml, errors) = to_xml(&image.resource()).unwrap();
    assert!(errors.is_empty(), "{errors:?}");
    let expected = format!("{}\n<TS name=\"my.ymap\">\n{}", xml::HEADER, EXPECTED_FILLED.join("\n"));
    assert_eq!(xml, expected);
}

#[test]
fn an_rdr2_meta_resource_has_its_fields_moved_and_enum_values_swapped() {
    let image = Image::filled(VERSION_RDR2, false);
    let meta = parse(&image.resource()).unwrap();
    assert_eq!(meta.name, "");
    assert_eq!(meta.offset_shift, 20);
    assert_eq!(meta.root_block, 1);
    assert_eq!(meta.enums[&nm("TE")], [(nm("One"), 1), (nm("Two"), 2)]);
    let expected = format!("{}\n<TS>\n{}", xml::HEADER, EXPECTED_FILLED.join("\n"));
    assert_eq!(meta.to_xml().0, expected);
}

#[test]
fn a_gta6_meta_has_no_name() {
    let mut image = Image::filled(VERSION_RDR2, true);
    image.put(0x10, b"0DRP\x7a\0\0\0");
    assert_eq!(parse(&image.resource()).unwrap().name, "");
}

#[test]
fn an_unreadable_name_pointer_gives_an_empty_name() {
    let mut image = Image::filled(VERSION, true);
    image.put(0x38, &0x10u64.to_le_bytes());
    assert_eq!(parse(&image.resource()).unwrap().name, "");
}

#[test]
fn damaged_meta_resources_are_errors() {
    let image = Image::filled(VERSION, true);
    let message = |image: &Image| match parse(&image.resource()) {
        Err(e) => e.to_string(),
        Ok(_) => "parsed".to_owned(),
    };

    let mut no_blocks = Image::filled(VERSION, true);
    no_blocks.put(0x4C, &0i16.to_le_bytes());
    assert_eq!(message(&no_blocks), "invalid meta file: 0 blocks, 1 structures");

    let mut negative = Image::filled(VERSION, true);
    negative.put(0x4C, &(-1i16).to_le_bytes());
    assert_eq!(message(&negative), "invalid meta file: 0 blocks, 1 structures");

    let mut bad_struct_list = Image::filled(VERSION, true);
    bad_struct_list.put(0x20, &0x10u64.to_le_bytes());
    assert!(matches!(parse(&bad_struct_list.resource()), Err(MetaError::Resource(ResourceError::BadPointer(0x20)))));

    let mut bad_entries = Image::filled(VERSION, true);
    bad_entries.put(0x110, &Image::at(0x7F8).to_le_bytes());
    assert!(matches!(parse(&bad_entries.resource()), Err(MetaError::Resource(ResourceError::OutOfBounds { .. }))));

    let mut bad_block = Image::filled(VERSION, true);
    bad_block.put(0x404, &0x10000u32.to_le_bytes());
    assert!(matches!(parse(&bad_block.resource()), Err(MetaError::Resource(ResourceError::OutOfBounds { .. }))));

    let mut bad_enum = Image::filled(VERSION, true);
    bad_enum.put(0x308, &Image::at(0x7FC).to_le_bytes());
    assert!(matches!(parse(&bad_enum.resource()), Err(MetaError::Resource(_))));

    let short = Resource { version: VERSION, system: vec![0; 0x20], graphics: vec![] };
    assert!(matches!(parse(&short), Err(MetaError::Resource(ResourceError::OutOfBounds { .. }))));
    assert_eq!(message(&image), "parsed");
}
