use super::*;

struct B {
    d: Vec<u8>,
    names: Vec<String>,
}

impl B {
    fn new() -> Self {
        Self { d: b"RBF0".to_vec(), names: Vec::new() }
    }

    fn wide() -> Self {
        Self { d: b"RBF0\xFB\xFF\x01\x00".to_vec(), names: Vec::new() }
    }

    fn head(&mut self, name: &str, kind: u8) {
        let known = self.names.iter().position(|n| n == name);
        let index = known.unwrap_or(self.names.len());
        self.d.extend([index as u8, kind]);
        if known.is_none() {
            self.names.push(name.to_owned());
            self.text(name);
        }
    }

    fn text(&mut self, s: &str) {
        self.d.extend((s.len() as u16).to_le_bytes());
        self.d.extend(s.as_bytes());
    }

    fn element(&mut self, name: &str, attributes: u16) -> &mut Self {
        self.head(name, TYPE_ELEMENT);
        self.d.extend([0, 0, 0, 0]);
        self.d.extend(attributes.to_le_bytes());
        self
    }

    fn string(&mut self, name: &str, value: &str) -> &mut Self {
        self.head(name, TYPE_STRING);
        self.text(value);
        self
    }

    fn uint(&mut self, name: &str, value: u32) -> &mut Self {
        self.head(name, TYPE_UINT);
        self.d.extend(value.to_le_bytes());
        self
    }

    fn uint64(&mut self, name: &str, value: u64) -> &mut Self {
        self.head(name, TYPE_UINT);
        self.d.extend(value.to_le_bytes());
        self
    }

    fn float(&mut self, name: &str, value: f32) -> &mut Self {
        self.head(name, TYPE_FLOAT);
        self.d.extend(value.to_le_bytes());
        self
    }

    fn float64(&mut self, name: &str, value: f64) -> &mut Self {
        self.head(name, TYPE_FLOAT);
        self.d.extend(value.to_le_bytes());
        self
    }

    fn float3(&mut self, name: &str, v: [f32; 3]) -> &mut Self {
        self.head(name, TYPE_FLOAT3);
        v.iter().for_each(|f| self.d.extend(f.to_le_bytes()));
        self
    }

    fn float3_wide(&mut self, name: &str, v: [f64; 3]) -> &mut Self {
        self.head(name, TYPE_FLOAT3);
        v.iter().for_each(|f| self.d.extend(f.to_le_bytes()));
        self
    }

    fn boolean(&mut self, name: &str, value: bool) -> &mut Self {
        self.head(name, if value { TYPE_TRUE } else { TYPE_FALSE });
        self
    }

    fn content(&mut self, bytes: &[u8]) -> &mut Self {
        self.d.extend([INDEX_CONTENT, MARKER_TAIL]);
        self.d.extend((bytes.len() as u32).to_le_bytes());
        self.d.extend(bytes);
        self
    }

    fn close(&mut self) -> &mut Self {
        self.d.extend([INDEX_CLOSE, MARKER_TAIL]);
        self
    }
}

fn xml_of(b: &B) -> String {
    let (text, errors, _) = to_xml(&b.d).unwrap();
    assert!(errors.is_empty(), "{errors:?}");
    text.strip_prefix(&format!("{}\n", xml::HEADER)).unwrap().to_owned()
}

fn message(data: &[u8]) -> String {
    parse(data).unwrap_err().to_string()
}

#[test]
fn attributes_and_children_of_every_value_type() {
    let mut b = B::new();
    b.element("r", 6)
        .string("s", "a<b")
        .float("f", 1.5)
        .uint("u", 7)
        .boolean("t", true)
        .boolean("b", false)
        .float3("skipped", [1.0, 2.0, 3.0])
        .float3("v", [1.0, 2.5, -3.0])
        .uint("n", 255)
        .boolean("flag", true)
        .float("x", 0.25)
        .string("name", "text")
        .string("empty", "")
        .close();
    assert_eq!(
        xml_of(&b),
        concat!(
            "<r s=\"a&lt;b\" f=\"1.5\" u=\"7\" t=\"True\" b=\"False\">\n",
            " <v x=\"1\" y=\"2.5\" z=\"-3\" />\n",
            " <n value=\"0x000000FF\" />\n",
            " <flag value=\"True\" />\n",
            " <x value=\"0.25\" />\n",
            " <name>text</name>\n",
            " <empty />\n",
            "</r>\n"
        )
    );
    let root = parse(&b.d).unwrap();
    assert_eq!((root.attributes.len(), root.children.len()), (6, 6));
}

#[test]
fn nested_and_empty_elements() {
    let mut b = B::new();
    b.element("root", 0).element("empty", 0).close().element("outer", 1).string("k", "v").element("inner", 0).close();
    b.close().close();
    assert_eq!(xml_of(&b), "<root>\n <empty />\n <outer k=\"v\">\n  <inner />\n </outer>\n</root>\n");
    let (_, _, name) = to_xml(&b.d).unwrap();
    assert_eq!(name, "root");
}

#[test]
fn a_name_used_again_is_not_spelled_out_again() {
    let mut b = B::new();
    b.element("root", 0).element("item", 1).uint("id", 1).close().element("item", 1).uint("id", 2).close().close();
    assert_eq!(b.names, ["root", "item", "id"]);
    assert_eq!(xml_of(&b), "<root>\n <item id=\"1\" />\n <item id=\"2\" />\n</root>\n");
}

#[test]
fn a_single_text_child_stays_on_one_line() {
    let mut b = B::new();
    b.element("t", 0).content(b"hi <you>\0").close();
    assert_eq!(xml_of(&b), "<t>hi &lt;you&gt;</t>\n");
    let mut b = B::new();
    b.element("t", 1).string("a", "1").content(b"hi\0").close();
    assert_eq!(xml_of(&b), "<t a=\"1\">\n hi\n</t>\n");
}

#[test]
fn content_is_shown_by_its_declared_type() {
    let floats = |v: &[f32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
    let cases: [(&str, Vec<u8>, &str); 9] = [
        ("char_array", vec![1, 2, 255], " 1\n 2\n 255\n"),
        ("short_array", vec![1, 0, 0, 1, 9], " 1\n 256\n"),
        ("matrix34", floats(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]), " 1, 2, 3\n 4, 5, 6\n"),
        ("matrix44", floats(&[1.0, 2.0, 3.0, 4.0, 5.0]), " 1, 2, 3, 4\n 5\n"),
        ("float_array", floats(&[0.5, 1.5]), " 0.5\n 1.5\n"),
        ("vector2_array", floats(&[1.0, 2.0, 3.0, 4.0]), " 1, 2\n 3, 4\n"),
        ("vec2v_array", floats(&[1.0, 2.0, 3.0]), " 1, 2\n 3\n"),
        ("vector3_array", floats(&[1.0, 2.0, 3.0, 4.0]), " 1, 2, 3\n 4\n"),
        ("vector4_array", floats(&[1.0, 2.0, 3.0, 4.0, 5.0]), " 1, 2, 3, 4\n 5\n"),
    ];
    for (kind, bytes, lines) in cases {
        let mut b = B::new();
        b.element("c", 1).string("content", kind).content(&bytes).close();
        assert_eq!(xml_of(&b), format!("<c content=\"{kind}\">\n{lines}</c>\n"), "{kind}");
    }
}

#[test]
fn content_of_an_unknown_type_is_an_error_element() {
    let mut b = B::new();
    b.element("c", 1).string("content", "mystery").content(&[1]).close();
    let (text, errors, _) = to_xml(&b.d).unwrap();
    assert_eq!(errors, ["unexpected content type mystery"]);
    assert!(
        text.ends_with("<c content=\"mystery\">\n <error>unexpected content type mystery</error>\n</c>\n"),
        "{text}"
    );
}

#[test]
fn wide_numbers_are_shown_as_numbers_or_hashes() {
    let mut b = B::wide();
    b.element("r", 4).uint64("small", 7).uint64("big", 0x8000_0000).float64("exact", 0.25).float64("long", 0.1);
    b.uint64("hash", 0xDEAD_BEEF_0000_0001).float64("third", 1.0 / 3.0).float3_wide("pos", [1.0, 2.0, 3.0]).close();
    assert_eq!(
        xml_of(&b),
        concat!(
            "<r small=\"7\" big=\"0x0000000080000000\" exact=\"0.25\" long=\"0.1\">\n",
            " <hash value=\"0xDEADBEEF00000001\" />\n",
            " <third value=\"0.3333333333333333\" />\n",
            " <pos x=\"1\" y=\"2\" z=\"3\" />\n",
            "</r>\n"
        )
    );
}

#[test]
fn small_helpers_format_numbers_and_content() {
    assert_eq!(float64(0.5), "0.5");
    assert_eq!(float64(0.1), "0.1");
    assert_eq!(float64(1.0 / 3.0), "0.3333333333333333");
    assert_eq!(uint64(5), "5");
    assert_eq!(uint64((1 << 31) - 1), "2147483647");
    assert_eq!(uint64(1 << 31), "0x0000000080000000");
    assert_eq!((bool_text(true), bool_text(false)), ("True", "False"));
    assert_eq!(floats_per_row("matrix34"), 3);
    assert_eq!(floats_per_row("vector3_array"), 3);
    assert_eq!(floats_per_row("float_array"), 1);
    assert_eq!(floats_per_row("vector2_array"), 2);
    assert_eq!(floats_per_row("vec2v_array"), 2);
    assert_eq!(floats_per_row("matrix44"), 4);
    assert_eq!(content_text(b"abc\0"), "abc");
    assert_eq!(content_text(b"abc"), "abc");
    assert_eq!(content_text(b"a\0b\0"), "a\0b");
    assert_eq!(content_text(&[0xFF, b'a']), "\u{FFFD}a");
    assert!(attribute_text(&Value::Bytes(vec![])).is_none());
    assert!(attribute_text(&Value::Float3("n".into(), [0.0; 3])).is_none());
    assert_eq!(attribute_text(&Value::Bool("n".into(), true)).map(|(_, t)| t), Some("True".to_owned()));
}

#[test]
fn damaged_files_are_errors() {
    assert!(!is_rbf(b"RBF") && is_rbf(b"RBF0"));
    assert_eq!(message(b""), "invalid RBF file: missing RBF0 header");
    assert_eq!(message(b"XBF0\0\0"), "invalid RBF file: missing RBF0 header");
    assert_eq!(message(b"RBF0"), "invalid RBF file: unexpected end of file");

    let mut unfinished = B::new();
    unfinished.element("r", 0);
    assert_eq!(message(&unfinished.d), "invalid RBF file: unexpected end of file");
    let cut = &unfinished.d[..unfinished.d.len() - 1];
    assert_eq!(message(cut), "invalid RBF file: unexpected end of file");

    assert_eq!(message(b"RBF0\xFF\x00"), "invalid RBF file: bad close tag");
    assert_eq!(message(b"RBF0\xFF\xFF"), "invalid RBF file: close tag without element");
    assert_eq!(message(b"RBF0\xFD\x00"), "invalid RBF file: bad content marker");
    assert_eq!(message(b"RBF0\xFD\xFF\0\0\0\0"), "invalid RBF file: content outside element");

    let mut bad_content = B::new();
    bad_content.element("r", 0);
    bad_content.d.extend([0xFD, 0xFF, 99, 0, 0, 0, 1]);
    assert_eq!(message(&bad_content.d), "invalid RBF file: unexpected end of file");

    assert_eq!(message(b"RBF0\x05\x00"), "invalid RBF file: unknown name 5");
    let mut value_first = B::new();
    value_first.uint("n", 1);
    assert_eq!(message(&value_first.d), "invalid RBF file: value outside element");
    let mut bad_type = B::new();
    bad_type.element("r", 0);
    bad_type.d.extend([0, 0x70]);
    assert_eq!(message(&bad_type.d), "invalid RBF file: unsupported data type 0x70");
}
