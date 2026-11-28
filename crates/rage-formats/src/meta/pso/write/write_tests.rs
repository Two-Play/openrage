use roxmltree::Document;

use crate::xml;

use super::super::test_support::*;
use super::super::ty::*;
use super::super::{parse, parse_rdr2};
use super::*;

fn element(xml: &str) -> Document<'_> {
    Document::parse(xml).unwrap()
}

fn gta_names(names: &[&str]) -> Vec<(u32, i32)> {
    names.iter().enumerate().map(|(i, n)| (nm(n), i as i32)).collect()
}

fn kitchen(rdr2: bool) -> Pso {
    let mut inner = Def::new("tsinner");
    inner.add("iv", UINT, 0, 4, 0);
    let mut d = Def::new("tsone");
    d.add("fu", UINT, 0, 4, 0);
    d.add("fb", BOOL, 0, 1, 0);
    d.add("ff", FLOAT, 0, 4, 0);
    d.add("fv2", FLOAT2, 0, 8, 0);
    d.add("fmat", MATRIX44, 0, 64, 0);
    d.add("fopq", OPAQUE16, 0, 16, 0);
    d.add("fenum", ENUM, 0, 4, nm("tenum"));
    d.add("fenum3", ENUM, 3, 4, nm("tenum"));
    let flags = d.info(0, 0, nm("tflag"));
    d.add("ffl", FLAGS, 0, 4, flags);
    d.add("fflplain", FLAGS, 0, 4, 0xFFF);
    d.add("ffl3", FLAGS, 3, 4, 0xFFF);
    d.add("fstr4", STRING, 0, 4, 4 << 16);
    d.add("fstr5", STRING, 5, 4, 0);
    d.add("fstr3", STRING, 3, 16, 0);
    d.add("fst", STRUCTURE, 0, 4, nm("tsinner"));
    d.add("fst1", STRUCTURE, 1, 4, 0);
    d.add("fptr", STRUCTURE, 3, 8, nm("tsinner"));
    d.add("fmap0", MAP, 0, 24, 0);
    d.add("fbad", 0x3F, 0, 4, 0);
    d.add("tself", STRUCTURE, 0, 4, nm("tsone"));
    let uint = d.info(UINT, 0, 0);
    d.add("arrinl", ARRAY, 1, 12, 3 << 16 | uint);
    d.add("arrbad", ARRAY, 3, 16, uint);
    d.add("arrnoinfo", ARRAY, 0, 16, 0x7F);
    let unsupported = d.info(0x3F, 0, 0);
    d.add("arrunsup", ARRAY, 0, 16, unsupported);
    let strings = d.info(STRING, 4, 0);
    d.add("arrstr4", ARRAY, 0, 16, strings);
    let inner_array = d.info(ARRAY, 0, uint);
    d.add("arrnested", ARRAY, 1, 32, 2 << 16 | inner_array);
    let unknown = d.info(STRUCTURE, 0, 0x1234);
    d.add("arrunknown", ARRAY, 0, 16, unknown);
    d.add("arrbig", ARRAY, 0, 16, uint);
    let (key, item) = (d.info(UINT, 0, 0), d.info(SINT, 0, 0));
    d.add("mnopair", MAP, 1, 24, key << 16 | item);
    d.add("mkeybad", MAP, 1, 24, 0xFFF << 16 | item);
    d.add("mitembad", MAP, 1, 24, key << 16 | 0xFFF);
    let (ukey, uitem) = (d.info(USHORT, 0, 0), d.info(SINT, 0, 0));
    d.add("mshort", MAP, 1, 24, ukey << 16 | uitem);
    let (skey, sitem) = (d.info(UINT, 0, 0), d.info(STRUCTURE, 3, nm("tsinner")));
    d.add("mptr", MAP, 1, 24, skey << 16 | sitem);
    d.add("mval", MAP, 1, 24, key << 16 | item);

    let mut pair_a = Def::new("tspair");
    pair_a.add_named(exact("Key"), USHORT, 0, 4, 0);
    pair_a.add_named(exact("Item"), SINT, 0, 4, 0);
    let mut pair_b = Def::new("tspairptr");
    pair_b.add_named(exact("Key"), UINT, 0, 4, 0);
    pair_b.add_named(exact("Item"), STRUCTURE, 3, 8, nm("tsinner"));

    let mut spec = Spec::new(rdr2);
    spec.enums.push((nm("tenum"), gta_names(&["ta", "tb"])));
    spec.enums.push((nm("tflag"), gta_names(&["fa", "fb"])));
    spec.blocks.push((d.name, vec![0; d.length()]));
    spec.structs.extend([inner, d, pair_a, pair_b]);
    parse_spec(&spec)
}

fn parse_spec(spec: &Spec) -> Pso {
    let file = spec.build();
    if spec.rdr2 { parse_rdr2(&file) } else { parse(&file) }.unwrap()
}

fn failure(pso: &Pso, body: &str) -> String {
    match pso.from_xml(&format!("<tsone>{body}</tsone>")) {
        Err(e) => e.to_string(),
        Ok(_) => "written".to_owned(),
    }
}

#[test]
fn numbers_are_parsed_in_decimal_and_hex() {
    assert_eq!(int("12"), Ok(12));
    assert_eq!(int(" -7 "), Ok(-7));
    assert_eq!(int("0x1F"), Ok(31));
    assert_eq!(int("0XfF"), Ok(255));
    assert_eq!(int("zz"), Err("\"zz\" is not a number".to_owned()));
    assert_eq!(int("0xzz"), Err("\"0xzz\" is not a number".to_owned()));
    assert_eq!(float(" 1.5 "), Ok(1.5));
    assert_eq!(float("x"), Err("\"x\" is not a number".to_owned()));
    assert_eq!(parse_bool("true"), Ok(1));
    assert_eq!(parse_bool("True"), Ok(1));
    assert_eq!(parse_bool("1"), Ok(1));
    assert_eq!(parse_bool("false"), Ok(0));
    assert_eq!(parse_bool("0"), Ok(0));
    assert_eq!(parse_bool("yes"), Err("\"yes\" is not true or false".to_owned()));
}

#[test]
fn flag_bits_must_fit_the_words() {
    assert_eq!(flag_bit(3, 1), Some(3));
    assert_eq!(flag_bit(32, 1), None);
    assert_eq!(flag_bit(32, 2), Some(32));
    assert_eq!(flag_bit(-1, 2), None);
}

#[test]
fn named_flags_are_matched_in_order_or_by_name() {
    let values = [(nm("fa"), 0), (nm("fb"), 1), (nm("fc"), 33)];
    let words = |content: &str, n: usize| {
        let mut w = vec![9u32; n];
        flag_words_named(content, &values, &mut w).map(|_| w)
    };
    assert_eq!(words("fa fc", 2), Ok(vec![1 | 9, 2 | 9]));
    assert_eq!(words("", 2), Ok(vec![0, 0]));
    assert_eq!(words("fc fa", 2), Ok(vec![1, 2]));
    assert_eq!(words("fb  fa", 2), Ok(vec![3, 0]));
    assert_eq!(words("fa zz", 2), Err("\"zz\" is not a flag of this field".to_owned()));
    assert_eq!(words("fc", 1), Err("\"fc\" is not a flag of this field".to_owned()));
}

#[test]
fn plain_flags_are_a_number_or_hex_words() {
    let words = |content: &str, n: usize| {
        let mut w = vec![7u32; n];
        flag_words_plain(content, &mut w).map(|_| w)
    };
    assert_eq!(words("5", 1), Ok(vec![5]));
    assert_eq!(words("0x10", 1), Ok(vec![16]));
    assert_eq!(words("  ", 1), Ok(vec![7]));
    assert_eq!(words("00000001 0000000F", 2), Ok(vec![1, 15]));
    assert_eq!(words("1", 2), Ok(vec![1, 7]));
    assert_eq!(words("zz", 1), Err("\"zz\" is not a number".to_owned()));
    assert_eq!(words("1 zz", 2), Err("\"zz\" is not hex".to_owned()));
}

#[test]
fn tokens_and_vectors_are_read_from_elements() {
    let doc = element("<a x=\"1\" y=\"2.5\">1, 2\n3,4 </a>");
    let a = doc.root_element();
    assert_eq!(tokens(a), ["1", "2", "3", "4"]);
    assert_eq!(vector(a, 2), Ok(vec![1.0, 2.5]));
    assert_eq!(vector(a, 3), Err("<a> has no z".to_owned()));
    assert_eq!(text(a), "1, 2\n3,4 ");
    assert_eq!(value(a), Err("<a> has no value".to_owned()));
}

#[test]
fn array_counts_follow_the_item_type() {
    let doc = element("<a><Item>1</Item><Item>2</Item> 1 2 3 4 5</a>");
    let a = doc.root_element();
    assert_eq!(array_count(STRUCTURE, a), 2);
    assert_eq!(array_count(STRING, a), 2);
    assert_eq!(array_count(ENUM, a), 2);
    assert_eq!(array_count(FLOAT2, a), 3);
    assert_eq!(array_count(FLOAT3, a), 2);
    assert_eq!(array_count(FLOAT3A, a), 2);
    assert_eq!(array_count(FLOAT4, a), 2);
    assert_eq!(array_count(VEC4V, a), 2);
    assert_eq!(array_count(UINT, a), 5);
}

#[test]
fn alignment_is_the_largest_power_of_two_dividing_the_size() {
    for (size, align) in [(0, 1), (1, 1), (2, 2), (4, 4), (8, 8), (12, 4), (16, 16), (24, 8), (32, 16), (48, 16)] {
        assert_eq!(align_of(size), align, "size {size}");
    }
}

#[test]
fn array_layouts_by_item_type() {
    assert_eq!(string_array_layout(7), Ok((4, UINTS)));
    assert_eq!(string_array_layout(15), Ok((4, UINTS)));
    assert_eq!(string_array_layout(2), Ok((8, POINTERS)));
    assert_eq!(string_array_layout(3), Ok((16, COUNTED_STRINGS)));
    assert_eq!(string_array_layout(4), Err("unexpected string array subtype 4".to_owned()));
    let sizes = [(FLOAT2, 8), (FLOAT3, 16), (FLOAT4A, 16), (VEC4V, 16), (LONG, 8), (ULONG, 8), (UBYTE, 1), (BOOL, 1)]
        .into_iter()
        .chain([(SBYTE, 1), (USHORT, 2), (FLOAT, 4), (UINT, 4), (SINT, 4)]);
    for (kind, size) in sizes {
        assert_eq!(plain_array_layout(kind), Ok((size, kind as u32)), "kind {kind:#x}");
    }
    assert_eq!(plain_array_layout(0x3F), Err("unsupported array item type 0x3f".to_owned()));
}

#[test]
fn pointers_are_encoded_for_both_layouts() {
    let gta = kitchen(false);
    let b = Builder::new(&gta);
    assert_eq!(b.encode_pointer(2, 5), Ok([0, 0, 0x50, 0x02, 0, 0, 0, 0]));
    assert_eq!(b.encode_pointer(0x1000, 0), Err("4096 blocks are more than pointers can address".to_owned()));
    assert_eq!(b.encode_pointer(1, MAX_OFFSET), Err("a block of over 1 MB cannot be addressed".to_owned()));
    assert!(b.encode_pointer(1, MAX_OFFSET - 1).is_ok());
    let rdr2 = kitchen(true);
    let b = Builder::new(&rdr2);
    assert_eq!(b.encode_pointer(3, 0x12345), Ok([0x34, 0x50, 0x00, 0x03, 0x12, 0x00, 0, 0]));
    assert_eq!(b.encode_pointer(1, 1 << 28), Err("offset 0x10000000 is beyond what pointers can address".to_owned()));
    assert_eq!(unpack_both(&b, 3, 0x12345), (3, 0x12345));
}

fn unpack_both(b: &Builder, id: usize, offset: usize) -> (usize, usize) {
    let bytes = b.encode_pointer(id, offset).unwrap();
    super::super::unpack_with(super::super::ptr_at(&bytes, 0).unwrap(), 20)
}

#[test]
fn a_field_that_cannot_be_written_names_the_field_and_the_reason() {
    let pso = kitchen(false);
    let cases: [(&str, &str); 38] = [
        ("<fu value=\"abc\" />", "fu: \"abc\" is not a number"),
        ("<fu />", "fu: <fu> has no value"),
        ("<fb value=\"maybe\" />", "fb: \"maybe\" is not true or false"),
        ("<ff value=\"x\" />", "ff: \"x\" is not a number"),
        ("<fv2 x=\"1\" />", "fv2: <fv2> has no y"),
        ("<fmat>1 2</fmat>", "fmat: a matrix needs 16 numbers, not 2"),
        ("<fmat>1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 x</fmat>", "fmat: \"x\" is not a number"),
        ("<fopq value=\"00\" />", "fopq: \"00\" is not 16 bytes of hex"),
        ("<fenum>zz</fenum>", "fenum: \"zz\" is not a value of the enum"),
        ("<fenum3>1</fenum3>", "fenum3: unexpected enum subtype 3"),
        ("<ffl>zz</ffl>", "ffl: \"zz\" is not a flag of this field"),
        ("<fflplain value=\"zz\" />", "fflplain: \"zz\" is not a number"),
        ("<ffl3 value=\"1\" />", "ffl3: unexpected flags subtype 3"),
        ("<fstr4>toolong</fstr4>", "fstr4: \"toolong\" is longer than the 4 bytes the field holds"),
        ("<fstr5>x</fstr5>", "fstr5: unexpected string subtype 5"),
        ("<fst1 />", "fst1: unexpected structure subtype 1"),
        ("<fptr type=\"nope\" />", "fptr: unknown structure type \"nope\""),
        ("<fmap0 />", "fmap0: unexpected map subtype 0"),
        ("<fbad value=\"1\" />", "fbad: unexpected data type 0x3f"),
        ("<zz />", "tsone has no field zz"),
        ("<fu value=\"1\" /><fu value=\"2\" />", "tsone has no field fu"),
        ("<error>boom</error>", "the XML holds an error from reading the original: boom"),
        ("<fst><iv value=\"x\" /></fst>", "fst: iv: \"x\" is not a number"),
        ("<fst><zz /></fst>", "fst: tsinner has no field zz"),
        ("<arrinl>1 2 3 4</arrinl>", "arrinl: 4 items where the structure holds 3"),
        ("<arrbad>1</arrbad>", "arrbad: unsupported array subtype 3"),
        ("<arrnoinfo>1</arrnoinfo>", "arrnoinfo: ARRAYINFO not found"),
        ("<arrunsup>1</arrunsup>", "arrunsup: unsupported array item type 0x3f"),
        ("<arrstr4><Item>a</Item></arrstr4>", "arrstr4: unexpected string array subtype 4"),
        (
            "<arrnested><Item>1</Item><Item>2</Item><Item>3</Item></arrnested>",
            "arrnested: 3 arrays where the structure holds 2",
        ),
        (
            "<arrunknown><Item><iv value=\"1\" /></Item></arrunknown>",
            "arrunknown: structure info hash_00001234 not found",
        ),
        ("<arrbig>1 x</arrbig>", "arrbig: \"x\" is not a number"),
        ("<mkeybad />", "mkeybad: map Key entry missing"),
        ("<mitembad />", "mitembad: map Item entry missing"),
        (
            "<mnopair><Item key=\"a\"><Value value=\"1\" /></Item></mnopair>",
            "mnopair: no structure describes the pairs of this map",
        ),
        ("<mshort><Item key=\"zz\"><Value value=\"1\" /></Item></mshort>", "mshort: \"zz\" is not a number"),
        ("<mshort><Item key=\"1\" /></mshort>", "mshort: map item without Value"),
        ("<mshort><Item><Value value=\"1\" /></Item></mshort>", "mshort: <Item> has no key"),
    ];
    for (body, message) in cases {
        assert_eq!(failure(&pso, body), format!("invalid PSO file: {message}"), "{body}");
    }
    assert_eq!(failure(&pso, "<mptr><Item key=\"1\" /></mptr>"), "invalid PSO file: mptr: <Item> has no type");
}

#[test]
fn documents_of_the_wrong_kind_are_refused() {
    let pso = kitchen(false);
    let message = |xml: &str| pso.from_xml(xml).unwrap_err().to_string();
    assert!(message("not xml").starts_with("invalid PSO file: the XML cannot be read: "));
    assert_eq!(message("<Other />"), "invalid PSO file: the root element is <Other>, the file needs <tsone>");
    let mut spec = Spec::new(false);
    spec.root = 4;
    spec.blocks.push((1, vec![]));
    let pso = parse_spec(&spec);
    assert_eq!(pso.from_xml("<a />").unwrap_err().to_string(), "invalid PSO file: the original has no root block");
}

#[test]
fn text_and_counts_beyond_what_fits_are_refused() {
    let pso = kitchen(false);
    let long = "x".repeat(70_000);
    assert_eq!(failure(&pso, &format!("<fstr3>{long}</fstr3>")), "invalid PSO file: fstr3: string too long");
    let many = "1 ".repeat(70_000);
    assert_eq!(
        failure(&pso, &format!("<arrbig>{many}</arrbig>")),
        "invalid PSO file: arrbig: 70000 items are more than an array holds"
    );
    let pairs = "<Item key=\"1\" />".repeat(70_000);
    assert_eq!(
        failure(&pso, &format!("<mshort>{pairs}</mshort>")),
        "invalid PSO file: mshort: 70000 pairs are more than a map holds"
    );
}

#[test]
fn nesting_beyond_the_depth_limit_is_refused() {
    let pso = kitchen(false);
    let body = format!("{}{}", "<tself>".repeat(70), "</tself>".repeat(70));
    let message = failure(&pso, &body);
    assert!(message.starts_with("invalid PSO file: tself: tself: "), "{message}");
    assert!(message.ends_with("tself: structures nested too deeply"), "{message}");
}

#[test]
fn rdr2_maps_without_a_pair_structure_use_the_owners_entries() {
    let pso = kitchen(true);
    assert!(pso.from_xml("<tsone><mnopair><Item key=\"a\"><Value value=\"1\" /></Item></mnopair></tsone>").is_ok());
}

#[test]
fn empty_values_write_zero_or_nothing() {
    let mut d = Def::new("tsone");
    d.add("fenum", ENUM, 0, 4, nm("tenum"));
    let flags = d.info(0, 0, nm("tflag"));
    d.add("ffl", FLAGS, 0, 4, flags);
    d.add("fstr", STRING, 1, 8, 0);
    d.add("fptr", STRUCTURE, 3, 8, nm("tsone"));
    let mut spec = Spec::new(false);
    spec.enums.push((nm("tenum"), gta_names(&["ta", "tb"])));
    spec.enums.push((nm("tflag"), gta_names(&["fa", "fb"])));
    spec.blocks.push((d.name, vec![0; d.length()]));
    spec.structs.push(d);
    let pso = parse_spec(&spec);
    let written = pso.from_xml("<tsone><fenum></fenum><ffl></ffl><fstr></fstr><fptr /></tsone>").unwrap();
    let (out, errors) = parse(&written).unwrap().to_xml();
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(
        out,
        format!("{}\n<tsone>\n <fenum>ta</fenum>\n <ffl />\n <fstr />\n <fptr />\n</tsone>\n", xml::HEADER)
    );
}

#[test]
fn the_checksum_section_goes_when_the_data_changes_and_other_sections_stay() {
    let mut d = Def::new("tsone");
    d.add("fu", UINT, 0, 4, 0);
    let mut spec = Spec::new(false);
    spec.blocks.push((d.name, vec![0; 4]));
    spec.structs.push(d);
    spec.extra = vec![(*b"STRF", vec![1, 2, 3, 4]), (*b"CHKS", vec![9; 8])];
    let file = spec.build();
    let pso = parse(&file).unwrap();
    assert_eq!(pso.from_xml("<tsone><fu value=\"0\" /></tsone>").unwrap(), file);
    let changed = pso.from_xml("<tsone><fu value=\"5\" /></tsone>").unwrap();
    assert!(changed.windows(4).any(|w| w == b"STRF"));
    assert!(!changed.windows(4).any(|w| w == b"CHKS"));
    let again = parse(&changed).unwrap();
    assert_eq!(again.to_xml().0, format!("{}\n<tsone>\n <fu value=\"5\" />\n</tsone>\n", xml::HEADER));
    assert_eq!(again.sections.iter().map(|(i, _)| *i).collect::<Vec<_>>(), [*b"PSIN", *b"PMAP", *b"PSCH", *b"STRF"]);
}

#[test]
fn blocks_of_new_types_follow_the_originals_in_the_order_they_are_met() {
    let mut d = Def::new("tsone");
    d.add("fs", STRING, 1, 8, 0);
    let mut spec = Spec::new(false);
    spec.blocks.push((d.name, vec![0; d.length()]));
    spec.structs.push(d);
    let pso = parse_spec(&spec);
    let written = pso.from_xml("<tsone><fs>text</fs></tsone>").unwrap();
    let again = parse(&written).unwrap();
    let names: Vec<u32> = again.blocks.iter().map(|b| b.name).collect();
    assert_eq!(names, [nm("tsone"), CHARS]);
    assert_eq!(again.to_xml().0, format!("{}\n<tsone>\n <fs>text</fs>\n</tsone>\n", xml::HEADER));
}

#[test]
fn rdr2_pointers_past_one_block_page_survive_a_round_trip() {
    let mut d = Def::new("tsone");
    let uint = d.info(UINT, 0, 0);
    d.add("big", ARRAY, 0, 16, uint);
    d.add("late", ARRAY, 0, 16, uint);
    let mut spec = Spec::new(true);
    spec.blocks.push((d.name, vec![0; d.length()]));
    spec.structs.push(d);
    let pso = parse_spec(&spec);
    let ones = vec!["1"; 1500].join(" ");
    let xml = format!("<tsone><big>{ones}</big><late>7 8 9</late></tsone>");
    let written = pso.from_xml(&xml).unwrap();
    let (out, errors) = parse_rdr2(&written).unwrap().to_xml();
    assert!(errors.is_empty(), "{errors:?}");
    assert!(out.contains(" <late>7 8 9</late>\n"), "{out}");
    assert_eq!(out.matches("1 1 1 1 1 1 1 1 1 1").count(), 150);
}

#[test]
fn a_gta_block_of_over_a_megabyte_cannot_be_addressed() {
    let mut d = Def::new("tsone");
    let ulong = d.info(ULONG, 0, 0);
    for name in ["a1", "a2", "a3", "a4"] {
        d.add(name, ARRAY, 0, 16, ulong);
    }
    let mut spec = Spec::new(false);
    spec.blocks.push((d.name, vec![0; d.length()]));
    spec.structs.push(d);
    let pso = parse_spec(&spec);
    let zeros = "0 ".repeat(65_000);
    let xml = format!("<tsone><a1>{zeros}</a1><a2>{zeros}</a2><a3>{zeros}</a3><a4>{zeros}</a4></tsone>");
    assert_eq!(
        pso.from_xml(&xml).unwrap_err().to_string(),
        "invalid PSO file: a block of over 1 MB cannot be addressed"
    );
}
