use super::test_support::*;
use super::ty::*;
use super::*;

fn xml_header() -> String {
    format!("{}\n", xml::HEADER)
}

fn lines(body: &[&str]) -> String {
    let mut s = xml_header();
    for l in body {
        s.push_str(l);
        s.push('\n');
    }
    s
}

fn parse_spec(spec: &Spec) -> Pso {
    let file = spec.build();
    if spec.rdr2 { parse_rdr2(&file) } else { parse(&file) }.unwrap()
}

fn read(spec: &Spec) -> (String, Vec<String>) {
    parse_spec(spec).to_xml()
}

/// One structure `tsone` that is the root block; `fill` adds its fields and writes their data.
fn one(rdr2: bool, fill: impl FnOnce(&mut Def, &mut Vec<u8>)) -> Spec {
    let mut def = Def::new("tsone");
    let mut data = Vec::new();
    fill(&mut def, &mut data);
    data.resize(data.len().max(def.length()), 0);
    let mut spec = Spec::new(rdr2);
    spec.blocks.push((def.name, data));
    spec.structs.push(def);
    spec
}

fn be(v: u32) -> [u8; 4] {
    v.to_be_bytes()
}

/// The XML and the errors of a file with one field of this kind whose data is `data`.
fn field_xml(kind: u8, sub: u8, key: u32, data: &[u8]) -> (String, Vec<String>) {
    read(&one(false, |d, buf| {
        let at = d.add("tf", kind, sub, data.len().max(4), key);
        put(buf, at, data);
    }))
}

#[test]
fn scalar_fields_are_read_and_written_back_byte_for_byte() {
    let mut def = Def::new("tsscalars");
    let mut d = Vec::new();
    let mut at = |def: &mut Def, name, kind, sub, size, bytes: &[u8]| {
        let o = def.add(name, kind, sub, size, 0);
        put(&mut d, o, bytes);
    };
    at(&mut def, "fbool", BOOL, 0, 1, &[1]);
    at(&mut def, "fsbyte", SBYTE, 0, 1, &[0xFD]);
    at(&mut def, "fubyte", UBYTE, 0, 1, &[200]);
    at(&mut def, "fsshort", SSHORT, 0, 2, &(-300i16).to_be_bytes());
    at(&mut def, "fushort", USHORT, 0, 2, &60000u16.to_be_bytes());
    at(&mut def, "fsint", SINT, 0, 4, &(-70000i32).to_be_bytes());
    // An unsigned integer of subtype 0 is shown as a signed number.
    at(&mut def, "fuint", UINT, 0, 4, &be(4_000_000_000));
    at(&mut def, "fcolor", UINT, 1, 4, &be(0xFF80_00FF));
    at(&mut def, "ffloat", FLOAT, 0, 4, &2.5f32.to_be_bytes());
    at(&mut def, "fhalf", HFLOAT, 0, 2, &(-2i16).to_be_bytes());
    at(&mut def, "fulong", ULONG, 0, 8, &12_345_678_901_234u64.to_be_bytes());
    at(&mut def, "flong", LONG, 0, 8, &5u64.to_be_bytes());
    let floats = |v: &[f32]| v.iter().flat_map(|f| f.to_be_bytes()).collect::<Vec<u8>>();
    at(&mut def, "fv2", FLOAT2, 0, 8, &floats(&[1.0, 2.0]));
    at(&mut def, "fv3", FLOAT3, 0, 16, &floats(&[1.0, 2.0, 3.0]));
    at(&mut def, "fv4", FLOAT4, 0, 16, &floats(&[1.0, 2.0, 3.0, 4.0]));
    at(&mut def, "fv4v", VEC4V, 0, 16, &floats(&[0.5, 0.25, 0.125, 8.0]));
    let identity: Vec<f32> = (0..16).map(|i| if i % 5 == 0 { 1.0 } else { 0.0 }).collect();
    at(&mut def, "fmat", MATRIX44, 0, 64, &floats(&identity));
    let opaque: Vec<u8> = (0..16).map(|i| i * 17).collect();
    at(&mut def, "fopq", OPAQUE16, 0, 16, &opaque);
    d.resize(def.length(), 0);
    let mut spec = Spec::new(false);
    spec.blocks.push((def.name, d));
    spec.structs.push(def);
    let file = spec.build();
    let pso = parse(&file).unwrap();
    let (xml, errors) = pso.to_xml();
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(
        xml,
        lines(&[
            "<tsscalars>",
            " <fbool value=\"true\" />",
            " <fsbyte value=\"-3\" />",
            " <fubyte value=\"200\" />",
            " <fsshort value=\"-300\" />",
            " <fushort value=\"60000\" />",
            " <fsint value=\"-70000\" />",
            " <fuint value=\"-294967296\" />",
            " <fcolor value=\"0xFF8000FF\" />",
            " <ffloat value=\"2.5\" />",
            " <fhalf value=\"-2\" />",
            " <fulong value=\"12345678901234\" />",
            " <flong value=\"5\" />",
            " <fv2 x=\"1\" y=\"2\" />",
            " <fv3 x=\"1\" y=\"2\" z=\"3\" />",
            " <fv4 x=\"1\" y=\"2\" z=\"3\" w=\"4\" />",
            " <fv4v x=\"0.5\" y=\"0.25\" z=\"0.125\" w=\"8\" />",
            " <fmat>",
            "  1 0 0 0",
            "  0 1 0 0",
            "  0 0 1 0",
            "  0 0 0 1",
            " </fmat>",
            " <fopq value=\"00112233445566778899AABBCCDDEEFF\" />",
            "</tsscalars>",
        ])
    );
    assert_eq!(pso.from_xml(&xml).unwrap(), file);
    assert_eq!(pso.root_type().as_deref(), Some("tsscalars"));
}

#[test]
fn strings_of_every_subtype_are_read_and_written_back_byte_for_byte() {
    for rdr2 in [false, true] {
        let mut def = Def::new("tsstr");
        let mut d = Vec::new();
        let mut field = |def: &mut Def, name, sub, size, key: u32, bytes: &[u8]| {
            let o = def.add(name, STRING, sub, size, key);
            put(&mut d, o, bytes);
        };
        field(&mut def, "s0", 0, 8, 8 << 16, b"abc");
        field(&mut def, "s1", 1, 8, 0, &pointer(1, 0, rdr2));
        field(&mut def, "s1empty", 1, 8, 0, &[]);
        field(&mut def, "s2", 2, 8, 0, &pointer(1, 6, rdr2));
        field(&mut def, "s2empty", 2, 8, 0, &pointer(1, 12, rdr2));
        let mut counted = pointer(1, 13, rdr2).to_vec();
        // GTA V writes the size with the terminator first, RDR2 the length.
        counted.extend(if rdr2 { [0, 7, 0, 8] } else { [0, 8, 0, 7] });
        field(&mut def, "s3", 3, 16, 0, &counted);
        field(&mut def, "s3empty", 3, 16, 0, &[]);
        field(&mut def, "s7", 7, 4, 0, &be(nm("somemodel")));
        d.resize(def.length(), 0);
        nm("somemodel");
        let mut spec = Spec::new(rdr2);
        spec.blocks.push((1, b"hello\0world\0\0counted\0".to_vec()));
        spec.blocks.push((def.name, d));
        spec.root = 2;
        spec.structs.push(def);
        let file = spec.build();
        let pso = if rdr2 { parse_rdr2(&file) } else { parse(&file) }.unwrap();
        let (xml, errors) = pso.to_xml();
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            xml,
            lines(&[
                "<tsstr>",
                " <s0>abc</s0>",
                " <s1>hello</s1>",
                " <s1empty />",
                " <s2>world</s2>",
                " <s2empty />",
                " <s3>counted</s3>",
                " <s3empty />",
                " <s7>somemodel</s7>",
                "</tsstr>",
            ]),
            "rdr2: {rdr2}"
        );
        assert_eq!(pso.from_xml(&xml).unwrap(), file, "rdr2: {rdr2}");
    }
}

fn enum_spec(rdr2: bool) -> Spec {
    // RDR2 counts the enum subtypes one higher.
    let sub = |s: u8| if rdr2 { s + 1 } else { s };
    let mut def = Def::new("tsenum");
    let mut d = Vec::new();
    let mut put_field = |def: &mut Def, name, kind, sub, size, key, bytes: &[u8]| {
        let o = def.add(name, kind, sub, size, key);
        put(&mut d, o, bytes);
    };
    let (e, f) = (nm("tenum"), nm("tflag"));
    put_field(&mut def, "e32", ENUM, sub(0), 4, e, &be(2));
    put_field(&mut def, "e16", ENUM, sub(1), 2, e, &1u16.to_be_bytes());
    put_field(&mut def, "e8", ENUM, sub(2), 1, e, &[0]);
    put_field(&mut def, "eunk", ENUM, sub(0), 4, e, &be(9));
    let flags = def.info(0, 0, f);
    put_field(&mut def, "fl32", FLAGS, 0, 4, flags, &be(0x21));
    put_field(&mut def, "fl16", FLAGS, 1, 2, flags, &2u16.to_be_bytes());
    put_field(&mut def, "fl8", FLAGS, 2, 1, flags, &[0]);
    let mut wide = be(2).to_vec();
    wide.extend(be(0x20));
    put_field(&mut def, "flwide", FLAGS, 4, 8, 40 << 16 | flags, &wide);
    put_field(&mut def, "flplain", FLAGS, 0, 4, 0xFFF, &be(5));
    put_field(&mut def, "flplain0", FLAGS, 0, 4, 0xFFF, &be(0));
    let mut hex = be(1).to_vec();
    hex.extend(be(0));
    put_field(&mut def, "flhex", FLAGS, 4, 8, 64 << 16 | 0xFFF, &hex);
    d.resize(def.length(), 0);
    let mut spec = Spec::new(rdr2);
    spec.enums.push((e, vec![(nm("ea"), 0), (nm("eb"), 1), (nm("ec"), 2)]));
    spec.enums.push((f, vec![(nm("fa"), 0), (nm("fb"), 1), (nm("fc"), 5), (nm("fhigh"), 37)]));
    spec.blocks.push((def.name, d));
    spec.structs.push(def);
    spec
}

#[test]
fn enums_and_flags_are_read_and_written_back_byte_for_byte() {
    for rdr2 in [false, true] {
        let spec = enum_spec(rdr2);
        let file = spec.build();
        let pso = parse_spec(&spec);
        let (xml, errors) = pso.to_xml();
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            xml,
            lines(&[
                "<tsenum>",
                " <e32>ec</e32>",
                " <e16>eb</e16>",
                " <e8>ea</e8>",
                " <eunk>9</eunk>",
                " <fl32>fa fc</fl32>",
                " <fl16>fb</fl16>",
                " <fl8 />",
                " <flwide>fb fhigh</flwide>",
                " <flplain value=\"5\" />",
                " <flplain0 />",
                " <flhex value=\"00000001 00000000\" />",
                "</tsenum>",
            ]),
            "rdr2: {rdr2}"
        );
        assert_eq!(pso.from_xml(&xml).unwrap(), file, "rdr2: {rdr2}");
    }
}

#[test]
fn nested_structures_are_read_inline_and_through_pointers() {
    let mut inner = Def::new("tsinner");
    inner.add("iname", STRING, 7, 4, 0);
    inner.add("ival", SINT, 0, 4, 0);
    let mut derived = Def::new("tsderived");
    derived.add("dval", UINT, 0, 4, 0);
    let mut outer = Def::new("tsouter");
    let (o_inl, o_ptr, o_ptr2, o_null) = (
        outer.add("inl", STRUCTURE, 0, 8, nm("tsinner")),
        outer.add("ptr", STRUCTURE, 3, 8, nm("tsinner")),
        outer.add("ptr2", STRUCTURE, 4, 8, nm("tsinner")),
        outer.add("nullptr", STRUCTURE, 3, 8, nm("tsinner")),
    );
    let mut root = vec![0u8; outer.length()];
    nm("somemodel");
    put(&mut root, o_inl, &be(nm("somemodel")));
    put(&mut root, o_inl + 4, &be(7));
    put(&mut root, o_ptr, &pointer(1, 0, false));
    put(&mut root, o_ptr2, &pointer(2, 0, false));
    let _ = o_null;
    let mut inner_data = be(0).to_vec();
    inner_data.extend((-1i32).to_be_bytes());
    let mut spec = Spec::new(false);
    spec.blocks = vec![(inner.name, inner_data), (derived.name, be(33).to_vec()), (outer.name, root)];
    spec.root = 3;
    spec.structs = vec![inner, derived, outer];
    let file = spec.build();
    let pso = parse(&file).unwrap();
    let (xml, errors) = pso.to_xml();
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(
        xml,
        lines(&[
            "<tsouter>",
            " <inl>",
            "  <iname>somemodel</iname>",
            "  <ival value=\"7\" />",
            " </inl>",
            " <ptr type=\"tsinner\">",
            "  <iname />",
            "  <ival value=\"-1\" />",
            " </ptr>",
            " <ptr2 type=\"tsderived\">",
            "  <dval value=\"33\" />",
            " </ptr2>",
            " <nullptr />",
            "</tsouter>",
        ])
    );
    assert_eq!(pso.from_xml(&xml).unwrap(), file);
}

fn array_spec(rdr2: bool) -> Spec {
    let mut item = Def::new("tsitem");
    item.add("ival", UINT, 0, 4, 0);
    let mut d = Def::new("tsarrays");
    let info = |d: &mut Def, kind, sub, key| d.info(kind, sub, key);
    let i = info(&mut d, UINT, 0, 0);
    d.add("nums", ARRAY, 0, 16, i);
    let i = info(&mut d, USHORT, 0, 0);
    d.add("inl", ARRAY, 1, 6, 3 << 16 | i);
    let i = info(&mut d, FLOAT, 0, 0);
    d.add("floats", ARRAY, 0, 16, i);
    let i = info(&mut d, FLOAT3, 0, 0);
    d.add("vecs", ARRAY, 0, 16, i);
    let i = info(&mut d, FLOAT2, 0, 0);
    d.add("uvs", ARRAY, 0, 16, i);
    let i = info(&mut d, FLOAT4, 0, 0);
    d.add("quads", ARRAY, 0, 16, i);
    let i = info(&mut d, UBYTE, 0, 0);
    d.add("bytes", ARRAY, 0, 16, i);
    let i = info(&mut d, ULONG, 0, 0);
    d.add("longs", ARRAY, 0, 16, i);
    let i = info(&mut d, STRING, 2, 0);
    d.add("strs", ARRAY, 0, 16, i);
    let i = info(&mut d, STRING, 7, 0);
    d.add("hashes", ARRAY, 0, 16, i);
    let i = info(&mut d, STRING, 3, 0);
    d.add("cstrs", ARRAY, 0, 16, i);
    let i = info(&mut d, ENUM, 0, nm("tsenum"));
    d.add("enums", ARRAY, 0, 16, i);
    let i = info(&mut d, STRUCTURE, 0, nm("tsitem"));
    d.add("items", ARRAY, 0, 16, i);
    let i = info(&mut d, STRUCTURE, 3, nm("tsitem"));
    d.add("ptrs", ARRAY, 0, 16, i);
    let unum = info(&mut d, UINT, 0, 0);
    let inner = info(&mut d, ARRAY, 0, unum);
    d.add("nested", ARRAY, 1, 32, 2 << 16 | inner);
    d.add("nothing", ARRAY, 0, 16, unum);
    let mut spec = Spec::new(rdr2);
    spec.enums.push((nm("tsenum"), vec![(nm("ea"), 0), (nm("eb"), 1), (nm("ec"), 2)]));
    spec.blocks = vec![(nm("tsitem"), vec![]), (d.name, vec![0; d.length()])];
    spec.root = 2;
    spec.structs = vec![item, d];
    spec
}

const ARRAYS_XML: &[&str] = &[
    "<tsarrays>",
    " <nums>",
    "  1 2 3 4 5 6 7 8 9 10",
    "  11 12",
    " </nums>",
    " <inl>1 2 3</inl>",
    " <floats>0.5 -1.25</floats>",
    " <vecs>",
    "  1, 2, 3",
    "  4, 5, 6",
    " </vecs>",
    " <uvs>1, 2</uvs>",
    " <quads>1, 2, 3, 4</quads>",
    " <bytes>1 255</bytes>",
    " <longs>8589934592</longs>",
    " <strs>",
    "  <Item>alpha</Item>",
    "  <Item></Item>",
    " </strs>",
    " <hashes>",
    "  <Item>tsname1</Item>",
    "  <Item>tsname2</Item>",
    " </hashes>",
    " <cstrs>",
    "  <Item>counted</Item>",
    " </cstrs>",
    " <enums>",
    "  <Item>eb</Item>",
    "  <Item>ec</Item>",
    " </enums>",
    " <items itemType=\"tsitem\">",
    "  <Item>",
    "   <ival value=\"1\" />",
    "  </Item>",
    "  <Item>",
    "   <ival value=\"2\" />",
    "  </Item>",
    " </items>",
    " <ptrs>",
    "  <Item type=\"tsitem\">",
    "   <ival value=\"3\" />",
    "  </Item>",
    "  <Item />",
    " </ptrs>",
    " <nested>",
    "  <Item>1 2</Item>",
    "  <Item>3</Item>",
    " </nested>",
    " <nothing />",
    "</tsarrays>",
];

fn arrays_xml() -> String {
    lines(ARRAYS_XML)
}

#[test]
fn arrays_of_every_kind_survive_a_round_trip() {
    nm("tsname1");
    nm("tsname2");
    for rdr2 in [false, true] {
        let spec = array_spec(rdr2);
        let pso = parse_spec(&spec);
        let written = pso.from_xml(&arrays_xml()).unwrap_or_else(|e| panic!("rdr2 {rdr2}: {e}"));
        let again = if rdr2 { parse_rdr2(&written) } else { parse(&written) }.unwrap();
        let (xml, errors) = again.to_xml();
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(xml, arrays_xml(), "rdr2: {rdr2}");
        assert_eq!(again.from_xml(&xml).unwrap(), written, "rdr2: {rdr2}");
    }
}

fn map_spec(rdr2: bool) -> Spec {
    let mut item = Def::new("tsitem");
    item.add("ival", UINT, 0, 4, 0);
    let mut spec = Spec::new(rdr2);
    let mut owner = Def::new("tsmaps");
    let (uint, sint) = (owner.info(UINT, 0, 0), owner.info(SINT, 0, 0));
    owner.add("themap", MAP, 1, 24, uint << 16 | sint);
    owner.add("emptymap", MAP, 1, 24, uint << 16 | sint);
    if !rdr2 {
        let mut pair = Def::new("tspair");
        pair.add_named(exact("Key"), UINT, 0, 4, 0);
        pair.add_named(exact("Item"), SINT, 0, 4, 0);
        let mut pair2 = Def::new("tspair2");
        pair2.add_named(exact("Key"), STRING, 2, 8, 0);
        pair2.add_named(exact("Item"), STRING, 2, 8, 0);
        let mut pair3 = Def::new("tspair3");
        pair3.add_named(exact("Key"), UINT, 0, 4, 0);
        pair3.add_named(exact("Item"), STRUCTURE, 0, 4, nm("tsitem"));
        let mut pair4 = Def::new("tspair4");
        pair4.add_named(exact("Key"), USHORT, 0, 4, 0);
        pair4.add_named(exact("Item"), STRUCTURE, 3, 8, nm("tsitem"));
        let s = owner.info(STRING, 2, 0);
        owner.add("strmap", MAP, 1, 24, s << 16 | s);
        let st = owner.info(STRUCTURE, 0, nm("tsitem"));
        owner.add("structmap", MAP, 1, 24, uint << 16 | st);
        let us = owner.info(USHORT, 0, 0);
        let sp = owner.info(STRUCTURE, 3, nm("tsitem"));
        owner.add("ptrmap", MAP, 1, 24, us << 16 | sp);
        spec.structs.extend([pair, pair2, pair3, pair4]);
    }
    spec.blocks = vec![(owner.name, vec![0; owner.length()])];
    spec.structs.extend([item, owner]);
    spec
}

#[test]
fn maps_survive_a_round_trip() {
    nm("mapkeya");
    nm("mapkeyb");
    nm("mapkeyc");
    let themap = [
        " <themap>",
        "  <Item key=\"mapkeya\">",
        "   <Value value=\"5\" />",
        "  </Item>",
        "  <Item key=\"mapkeyb\">",
        "   <Value value=\"-6\" />",
        "  </Item>",
        " </themap>",
        " <emptymap />",
    ];
    let gta_only = [
        " <strmap>",
        "  <Item type=\"String\" key=\"k1\">v1</Item>",
        "  <Item type=\"String\" key=\"k2\"></Item>",
        " </strmap>",
        " <structmap>",
        "  <Item type=\"tsitem\" key=\"mapkeyc\">",
        "   <ival value=\"9\" />",
        "  </Item>",
        " </structmap>",
        " <ptrmap>",
        "  <Item type=\"tsitem\" key=\"0x10\">",
        "   <ival value=\"4\" />",
        "  </Item>",
        " </ptrmap>",
    ];
    for rdr2 in [false, true] {
        let mut body = vec!["<tsmaps>"];
        body.extend(themap);
        if !rdr2 {
            body.extend(gta_only);
        }
        body.push("</tsmaps>");
        let expected = lines(&body);
        let pso = parse_spec(&map_spec(rdr2));
        let written = pso.from_xml(&expected).unwrap_or_else(|e| panic!("rdr2 {rdr2}: {e}"));
        let again = if rdr2 { parse_rdr2(&written) } else { parse(&written) }.unwrap();
        let (xml, errors) = again.to_xml();
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(xml, expected, "rdr2: {rdr2}");
        assert_eq!(again.from_xml(&xml).unwrap(), written, "rdr2: {rdr2}");
    }
}

fn header(block: u32, off: u32, count: u16) -> Vec<u8> {
    let mut h = pointer(block, off, false).to_vec();
    h.extend(count.to_be_bytes());
    h.extend(count.to_be_bytes());
    h
}

fn array_field(item: (u8, u8, u32), sub: u8, header: &[u8]) -> Spec {
    one(false, |d, buf| {
        let i = d.info(item.0, item.1, item.2);
        let at = d.add("tf", ARRAY, sub, 16, i);
        put(buf, at, header);
    })
}

fn root_only(spec: &Spec) -> Vec<String> {
    read(spec).1
}

#[test]
fn unexpected_types_and_subtypes_are_errors_in_the_field() {
    let zero = [0u8; 16];
    let cases: [(u8, u8, &str); 8] = [
        (0x3F, 0, "unexpected data type 0x3f"),
        (UINT, 2, "unexpected integer subtype 2"),
        (FLAGS, 3, "unexpected flags subtype 3"),
        (ENUM, 5, "unexpected enum subtype 5"),
        (STRING, 5, "unexpected string subtype 5"),
        (STRUCTURE, 1, "unexpected structure subtype 1"),
        (MAP, 0, "unexpected map subtype 0"),
        (ARRAY, 1, "ARRAYINFO not found for tf"),
    ];
    for (kind, sub, message) in cases {
        let key = if kind == ARRAY { 0x7F } else { 0 };
        let (xml, errors) = field_xml(kind, sub, key, &zero);
        assert_eq!(errors, [format!("tf: {message}")], "kind {kind:#x}");
        assert_eq!(xml, lines(&["<tsone>", &format!(" <error>tf: {message}</error>"), "</tsone>"]), "kind {kind:#x}");
    }
}

#[test]
fn a_field_beyond_the_data_is_an_error() {
    let spec = one(false, |d, _| {
        d.entries.push(Field { name: nm("tf"), kind: UINT, sub: 0, offset: 60000, key: 0 });
    });
    let errors = root_only(&spec);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].starts_with("tf: read of 4 bytes at 0xea70 beyond data of 0x"), "{errors:?}");
    let spec = one(false, |d, _| {
        d.entries.push(Field { name: nm("tb"), kind: BOOL, sub: 0, offset: 60000, key: 0 });
        d.entries.push(Field { name: nm("to"), kind: OPAQUE16, sub: 0, offset: 60000, key: 0 });
    });
    let errors = root_only(&spec);
    assert!(errors[0].starts_with("tb: offset 0xea70 beyond data"), "{errors:?}");
    assert_eq!(errors[1], "to: 16 bytes beyond the data");
}

#[test]
fn a_missing_root_block_or_structure_is_an_error() {
    let mut spec = one(false, |d, _| {
        d.add("tf", UINT, 0, 4, 0);
    });
    spec.root = 3;
    let pso = parse_spec(&spec);
    assert_eq!(pso.to_xml().1, ["block 3 not found"]);
    assert_eq!(pso.root_type(), None);
    let mut spec = one(false, |_, _| {});
    spec.blocks[0].0 = nm("tsnone");
    let pso = parse_spec(&spec);
    assert_eq!(pso.to_xml().1, ["structure info tsnone not found"]);
    assert_eq!(pso.root_type().as_deref(), Some("tsnone"));
}

#[test]
fn a_structure_that_contains_itself_stops_at_the_depth_limit() {
    let spec = one(false, |d, _| {
        d.add("tself", STRUCTURE, 0, 4, nm("tsone"));
    });
    let (xml, errors) = read(&spec);
    assert_eq!(errors, ["structures nested too deeply"]);
    assert_eq!(xml.matches("<tself>").count(), 65);
}

#[test]
fn the_output_stops_at_the_limit() {
    let spec = array_spec(false);
    let (xml, errors) = parse_spec(&spec).to_xml_with_limit(40);
    assert_eq!(errors, ["output truncated at 0 MB: the data nests too much to show in full"]);
    assert!(xml.ends_with("</error>\n"));
}

#[test]
fn array_problems_are_errors_in_the_field() {
    let missing_block = array_field((STRUCTURE, 0, nm("tsone")), 0, &header(9, 0, 2));
    assert_eq!(root_only(&missing_block), ["tf: array block 9 not found"]);
    let unknown_item = array_field((STRUCTURE, 0, 0x1234), 0, &header(1, 0, 2));
    assert_eq!(root_only(&unknown_item), ["tf: array item structure not found"]);
    let beyond = array_field((UINT, 0, 0), 0, &header(1, 0, 1000));
    assert_eq!(root_only(&beyond), ["tf: array of 1000 at 0x10 beyond data"]);
    let unsupported = array_field((0x3F, 0, 0), 0, &header(1, 0, 1));
    assert_eq!(root_only(&unsupported), ["tf: unsupported array item type 0x3f"]);
    let strings = array_field((STRING, 4, 0), 0, &header(1, 0, 1));
    assert_eq!(root_only(&strings), ["tf: unexpected string array subtype 4"]);
    let subtype = array_field((UINT, 0, 0), 3, &header(1, 0, 1));
    assert_eq!(root_only(&subtype), ["tf: unsupported array subtype 3"]);
    let pointers = array_field((STRUCTURE, 3, nm("tsone")), 0, &header(1, 0, 1000));
    assert_eq!(root_only(&pointers), ["tf: array of 1000 at 0x10 beyond data"]);
    let enums = array_field((ENUM, 0, 0), 0, &header(1, 0, 1000));
    assert_eq!(root_only(&enums), ["tf: array of 1000 at 0x10 beyond data"]);
}

#[test]
fn empty_arrays_are_empty_elements() {
    let mut item = Def::new("tsitem");
    item.add("ival", UINT, 0, 4, 0);
    for kind in [(STRUCTURE, 0, nm("tsitem")), (STRUCTURE, 3, nm("tsitem")), (UINT, 0, 0)] {
        let mut spec = array_field(kind, 0, &header(1, 0, 0));
        spec.structs.push(item.clone());
        let (xml, errors) = read(&spec);
        assert!(errors.is_empty(), "{errors:?}");
        let tag = if kind == (STRUCTURE, 0, nm("tsitem")) { "tf itemType=\"tsitem\"" } else { "tf" };
        assert_eq!(xml, lines(&["<tsone>", &format!(" <{tag} />"), "</tsone>"]));
    }
}

#[test]
fn a_structure_array_longer_than_its_block_stops_at_the_block_end() {
    let mut item = Def::new("tsitem");
    item.add("ival", UINT, 0, 4, 0);
    let mut spec = array_field((STRUCTURE, 0, nm("tsitem")), 0, &header(1, 0, 3));
    spec.blocks.insert(0, (item.name, [be(1), be(2)].concat()));
    spec.root = 2;
    spec.structs.push(item);
    let (xml, errors) = read(&spec);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(xml.matches("<Item>").count(), 2);
}

#[test]
fn map_problems_are_reported() {
    let owner = |key: fn(u32, u32) -> u32, header: &[u8]| {
        one(false, |d, buf| {
            let (k, i) = (d.info(UINT, 0, 0), d.info(SINT, 0, 0));
            let at = d.add("tf", MAP, 1, 24, key(k, i));
            put(buf, at, header);
        })
    };
    let mut h = vec![1, 0, 0, 0, 0, 0, 0, 0];
    h.extend(pointer(9, 0, false));
    h.extend(1u16.to_be_bytes());
    assert_eq!(root_only(&owner(|k, i| k << 16 | i, &h)), ["tf: map block 9 not found"]);
    let mut h = vec![1, 0, 0, 0, 0, 0, 0, 0];
    h.extend(pointer(1, 0, false));
    h.extend(1u16.to_be_bytes());
    assert_eq!(root_only(&owner(|_, i| 0xFFF << 16 | i, &h)), ["tf: map Key entry missing"]);
    assert_eq!(root_only(&owner(|k, _| k << 16 | 0xFFF, &h)), ["tf: map Item entry missing"]);

    let mut far = vec![1, 0, 0, 0, 0, 0, 0, 0];
    far.extend(pointer(1, 0x10000, false));
    far.extend(2u16.to_be_bytes());
    let (xml, errors) = read(&owner(|k, i| k << 16 | i, &far));
    assert_eq!(errors, ["map offset out of range (count 2)"]);
    assert_eq!(
        xml,
        lines(&["<tsone>", " <tf>", "  <error>map offset out of range (count 2)</error>", " </tf>", "</tsone>"])
    );

    let mut to_nowhere = vec![1, 0, 0, 0, 0, 0, 0, 0];
    to_nowhere.extend(pointer(2, 0, false));
    to_nowhere.extend(1u16.to_be_bytes());
    let mut pair = vec![0u8; 16];
    put(&mut pair, 8, &pointer(9, 0, false));
    let mut spec = one(false, |d, buf| {
        let (k, i) = (d.info(UINT, 0, 0), d.info(STRUCTURE, 3, nm("tsone")));
        let at = d.add("tf", MAP, 1, 24, k << 16 | i);
        put(buf, at, &to_nowhere);
    });
    spec.blocks.push((ARRAYINFO, pair));
    assert_eq!(root_only(&spec), ["tf: map item block not found"]);
}

#[test]
fn a_map_header_without_the_leading_one_has_its_count_and_pointer_elsewhere() {
    let mut header = vec![0u8; 24];
    header[2..4].copy_from_slice(&1u16.to_be_bytes());
    header[16..24].copy_from_slice(&pointer(2, 0, false));
    let mut pair = vec![0u8; 16];
    put(&mut pair, 0, &be(0x1234));
    put(&mut pair, 8, &(77i32).to_be_bytes());
    let mut spec = one(false, |d, buf| {
        let (k, i) = (d.info(UINT, 0, 0), d.info(SINT, 0, 0));
        let at = d.add("tf", MAP, 1, 24, k << 16 | i);
        put(buf, at, &header);
    });
    spec.blocks.push((ARRAYINFO, pair));
    let (xml, errors) = read(&spec);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(
        xml,
        lines(&[
            "<tsone>",
            " <tf>",
            "  <Item key=\"hash_00001234\">",
            "   <Value value=\"77\" />",
            "  </Item>",
            " </tf>",
            "</tsone>",
        ])
    );
}

#[test]
fn damaged_files_are_errors() {
    let message = |file: &[u8]| parse(file).unwrap_err().to_string();
    assert_eq!(message(b""), "invalid PSO file: no PSIN section");
    assert!(!is_pso(b"") && is_pso(b"PSIN") && !is_pso(b"PMAP"));
    assert_eq!(message(&[b"PSIN".as_slice(), &be(4)].concat()), "invalid PSO file: section PSIN has bad length 4");
    assert_eq!(
        message(&[b"PSIN".as_slice(), &be(0x100)].concat()),
        "invalid PSO file: section PSIN has bad length 256"
    );
    let section = |ident: &[u8; 4]| [ident.as_slice(), &be(8)].concat();
    assert_eq!(message(&section(b"PSIN")), "invalid PSO file: no PMAP section");
    assert_eq!(message(&[section(b"PSIN"), section(b"PMAP")].concat()), "invalid PSO file: no PSCH section");

    let spec = one(false, |d, _| {
        d.add("tf", UINT, 0, 4, 0);
    });
    let file = spec.build();
    let mut with_tail = file.clone();
    with_tail.extend([0; 4]);
    assert!(parse(&with_tail).is_ok());

    let at = |ident: &[u8; 4]| file.windows(4).position(|w| w == ident).unwrap();
    let psch = at(b"PSCH");
    let element = psch + u32::from_be_bytes(file[psch + 16..psch + 20].try_into().unwrap()) as usize;
    let mut unknown = file.clone();
    unknown[element] = 2;
    assert_eq!(message(&unknown), "invalid PSO file: unknown schema element type 2");
    let mut many_entries = file.clone();
    many_entries[element + 2..element + 4].copy_from_slice(&500u16.to_be_bytes());
    assert!(message(&many_entries).starts_with("invalid PSO file: read of 4 bytes at 0x"));
    let mut many_blocks = file.clone();
    let pmap = at(b"PMAP");
    many_blocks[pmap + 12..pmap + 14].copy_from_slice(&500u16.to_be_bytes());
    assert!(message(&many_blocks).starts_with("invalid PSO file: read of 4 bytes at 0x"));
}

#[test]
fn an_rdr2_pointer_that_misses_its_block_falls_back_to_the_older_layout() {
    // The offset 5 is stored the GTA V way; the bytes after the word, which RDR2 would read as
    // the high bits of the offset, point far beyond the block.
    let mut raw = pointer(1, 5, false);
    raw[4] = 1;
    let mut spec = one(true, |d, buf| {
        let at = d.add("tf", STRING, 1, 8, 0);
        put(buf, at, &raw);
    });
    spec.blocks.insert(0, (1, b"xxxxxhello\0".to_vec()));
    spec.root = 2;
    let (xml, errors) = read(&spec);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(xml, lines(&["<tsone>", " <tf>hello</tf>", "</tsone>"]));
}

#[test]
fn pointers_and_numbers_are_unpacked() {
    assert_eq!(ptr_at(&[0, 0, 0, 1], 0), Ok(1));
    assert_eq!(ptr_at(&[0, 0, 0, 1, 0x02], 0), Ok(1));
    assert_eq!(ptr_at(&[0, 0, 0, 1, 0x02, 0x01], 0), Ok(1 | 0x0102 << 32));
    assert_eq!(ptr_at(&[0, 0], 0), Err("read of 4 bytes at 0x0 beyond data of 0x2".to_owned()));
    assert_eq!(unpack_with(0x0012_3002, 12), (2, 0x123));
    assert_eq!(unpack_with(0x0030_5002, 20), (2, 3));
    assert_eq!(u64_at(&[0, 0, 0, 0, 0, 0, 1, 0], 0), Ok(256));
    assert_eq!(f32_at(&1.5f32.to_be_bytes(), 0), Ok(1.5));
    assert_eq!(u16_at(&[1], 0), Err("read of 2 bytes at 0x0 beyond data of 0x1".to_owned()));
    assert_eq!(byte_at(&[1], 3), Err("offset 0x3 beyond data".to_owned()));
}

#[test]
fn flag_words_follow_the_subtype() {
    let d = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
    let entry = |sub, key| Entry { sub, key, ..Entry::default() };
    assert_eq!(flag_words(&d, 0, &entry(0, 0)), Ok(vec![0x0102_0304]));
    assert_eq!(flag_words(&d, 0, &entry(1, 0)), Ok(vec![0x0102]));
    assert_eq!(flag_words(&d, 0, &entry(2, 0)), Ok(vec![1]));
    assert_eq!(flag_words(&d, 0, &entry(4, 64 << 16)), Ok(vec![0x0102_0304, 0x0506_0708]));
    assert_eq!(flag_words(&d, 0, &entry(4, 0)), Ok(vec![0x0102_0304]));
    assert_eq!(flag_words(&d, 0, &entry(3, 0)), Err("unexpected flags subtype 3".to_owned()));
    assert_eq!(flag_words(&d, 6, &entry(0, 0)), Err("read of 4 bytes at 0x6 beyond data of 0x8".to_owned()));
    assert!(bit_set(&[0b100, 1], 2) && bit_set(&[0, 1], 32) && !bit_set(&[0b100], 1));
    assert!(!bit_set(&[1], -1) && !bit_set(&[1], 32));
}

#[test]
fn small_helpers_describe_vectors_and_arrays() {
    assert_eq!(vector_len(FLOAT2), Some(2));
    assert_eq!(vector_len(FLOAT3A), Some(3));
    assert_eq!(vector_len(FLOAT4), Some(4));
    assert_eq!(vector_len(FLOAT), None);
    assert_eq!(vector_array_dims(FLOAT2), Some((8, 2)));
    assert_eq!(vector_array_dims(FLOAT3), Some((16, 3)));
    assert_eq!(vector_array_dims(VEC4V), Some((16, 4)));
    assert_eq!(vector_array_dims(UINT), None);
    assert_eq!(vector_attrs("v", &[1.0, 2.0]), "v x=\"1\" y=\"2\"");
    assert_eq!(be_floats(&[0x3F, 0x80, 0, 0, 0x40, 0, 0, 0], 2), "1, 2");
    assert_eq!(be_u32s(vec![&[0, 0, 1, 0], &[0, 0, 0, 2]]), [256, 2]);
    let texts = [
        // A long field is shown unsigned, a long array signed.
        (LONG, vec![0xFF; 8], "-1"),
        (SBYTE, vec![0x80], "-128"),
        (BOOL, vec![1], "1"),
        (USHORT, vec![0xFF, 0xFE], "65534"),
        (SINT, vec![0xFF, 0xFF, 0xFF, 0xFF], "4294967295"),
        (FLOAT, 0.25f32.to_be_bytes().to_vec(), "0.25"),
    ];
    for (kind, bytes, text) in texts {
        let (size, format) = scalar_format(kind).unwrap();
        assert_eq!((size, format(&bytes)), (bytes.len(), text.to_owned()), "kind {kind:#x}");
    }
    assert!(scalar_format(FLOAT3).is_none());
}

#[test]
fn array_item_entries_are_found_by_the_low_bits_of_the_key() {
    let info = StructInfo {
        length: 0,
        entries: vec![Entry { kind: UINT, ..Entry::default() }, Entry { kind: USHORT, ..Entry::default() }],
    };
    let entry = |key| Entry { key, ..Entry::default() };
    assert_eq!(array_item_entry(&entry(1), &info).map(|e| e.kind), Some(USHORT));
    assert_eq!(array_item_entry(&entry(5 << 16 | 1), &info).map(|e| e.kind), Some(USHORT));
    assert_eq!(array_item_entry(&entry(0x1_0000), &info).map(|e| e.kind), Some(UINT));
    assert!(array_item_entry(&entry(7), &info).is_none());
    let (plain, string) = (Entry { sub: 1, ..Entry::default() }, Entry { kind: STRING, ..Entry::default() });
    assert!(!is_external_array(&plain, &Entry::default()));
    assert!(is_external_array(&Entry { sub: 5, ..plain }, &Entry::default()));
    assert!(is_external_array(&plain, &string));
    assert!(is_external_array(&Entry { sub: 4, ..plain }, &Entry { sub: 3, ..Entry::default() }));
}
