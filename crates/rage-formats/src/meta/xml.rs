//! A small XML writer that formats metadata the way CodeWalker does (one space
//! per level, `value="…"` attributes for numbers), so files exported by
//! OpenRAGE can be imported by CodeWalker and compared with its exports.

pub const HEADER: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>";

/// Escapes text and attribute values.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c if (c as u32) < 0x20 && c != '\t' && c != '\n' && c != '\r' => {}
            c => out.push(c),
        }
    }
    out
}

/// Formats a float the shortest way that reads back to the same value.
pub fn float(v: f32) -> String {
    format!("{v}")
}

/// The most text a writer produces. A damaged or misread file can nest
/// arrays inside arrays and grow without bound (one 475 KB RDR2 file asked
/// for over 5 GB); past this the writer stops and records why.
pub const MAX_OUTPUT: usize = 64 << 20;

#[derive(Default)]
pub struct Writer {
    pub out: String,
    /// Things that could not be read; also written as `<error>` elements.
    pub errors: Vec<String>,
    /// The output reached the limit; nothing more is written.
    truncated: bool,
    /// The most text to write (0: [`MAX_OUTPUT`]).
    limit: usize,
}

impl Writer {
    pub fn new() -> Self {
        let mut w = Self::default();
        w.out.push_str(HEADER);
        w.out.push('\n');
        w
    }

    /// A writer with another limit than [`MAX_OUTPUT`] (for big files read
    /// by the application rather than shown).
    pub fn with_limit(limit: usize) -> Self {
        Self { limit, ..Self::new() }
    }

    /// Whether the output is full; callers walking nested data stop then.
    pub fn full(&self) -> bool {
        self.truncated
    }

    /// Whether there is room for more output; at the limit, notes the truncation once.
    fn room(&mut self) -> bool {
        if self.truncated {
            return false;
        }
        let limit = if self.limit == 0 { MAX_OUTPUT } else { self.limit };
        if self.out.len() > limit {
            self.truncated = true;
            let msg = format!("output truncated at {} MB: the data nests too much to show in full", limit >> 20);
            self.out.push_str("<error>");
            self.out.push_str(&msg);
            self.out.push_str("</error>\n");
            self.errors.push(msg);
            return false;
        }
        true
    }

    fn indent(&mut self, level: usize) {
        for _ in 0..level {
            self.out.push(' ');
        }
    }

    /// `<tag>` (the tag may include attributes) followed by a newline.
    pub fn open(&mut self, level: usize, tag: &str) {
        if !self.room() {
            return;
        }
        self.indent(level);
        self.out.push('<');
        self.out.push_str(tag);
        self.out.push_str(">\n");
    }

    pub fn close(&mut self, level: usize, name: &str) {
        if !self.room() {
            return;
        }
        self.indent(level);
        self.out.push_str("</");
        self.out.push_str(name);
        self.out.push_str(">\n");
    }

    /// `<name value="v" />`
    pub fn value(&mut self, level: usize, name: &str, v: &str) {
        if !self.room() {
            return;
        }
        self.indent(level);
        self.out.push('<');
        self.out.push_str(name);
        self.out.push_str(" value=\"");
        self.out.push_str(v);
        self.out.push_str("\" />\n");
    }

    /// `<content />`
    pub fn empty(&mut self, level: usize, content: &str) {
        if !self.room() {
            return;
        }
        self.indent(level);
        self.out.push('<');
        self.out.push_str(content);
        self.out.push_str(" />\n");
    }

    /// `<name>text</name>`, text escaped; `name` may carry attributes.
    pub fn text(&mut self, level: usize, name: &str, text: &str) {
        if !self.room() {
            return;
        }
        self.indent(level);
        self.out.push('<');
        self.out.push_str(name);
        self.out.push('>');
        self.out.push_str(&escape(text));
        self.out.push_str("</");
        self.out.push_str(name.split(' ').next().unwrap_or(name));
        self.out.push_str(">\n");
    }

    /// Like [`text`](Self::text), but `<name />` when empty.
    pub fn string(&mut self, level: usize, name: &str, text: &str) {
        if text.is_empty() { self.empty(level, name) } else { self.text(level, name, text) }
    }

    pub fn error(&mut self, level: usize, msg: String) {
        if !self.room() {
            return;
        }
        self.indent(level);
        self.out.push_str("<error>");
        self.out.push_str(&escape(&msg));
        self.out.push_str("</error>\n");
        self.errors.push(msg);
    }

    /// Values separated by spaces, `per_row` to a line.
    pub fn raw_array(&mut self, level: usize, name: &str, items: &[String], per_row: usize) {
        if !self.room() {
            return;
        }
        if items.is_empty() {
            return self.empty(level, name);
        }
        let per_row = per_row.max(1);
        if items.len() <= per_row {
            self.indent(level);
            self.out.push('<');
            self.out.push_str(name);
            self.out.push('>');
            self.out.push_str(&items.join(" "));
            self.out.push_str("</");
            self.out.push_str(name);
            self.out.push_str(">\n");
            return;
        }
        self.open(level, name);
        for row in items.chunks(per_row) {
            self.indent(level + 1);
            self.out.push_str(&row.join(" "));
            self.out.push('\n');
        }
        self.close(level, name);
    }

    /// One value per line (element content).
    pub fn raw_array_lines(&mut self, level: usize, items: &[String]) {
        if !self.room() {
            return;
        }
        for item in items {
            self.indent(level);
            self.out.push_str(item);
            self.out.push('\n');
        }
    }

    /// `<Item>value</Item>` per item.
    pub fn item_array(&mut self, level: usize, name: &str, items: &[String]) {
        if !self.room() {
            return;
        }
        if items.is_empty() {
            return self.empty(level, name);
        }
        self.open(level, name);
        for item in items {
            self.text(level + 1, "Item", item);
        }
        self.close(level, name);
    }

    pub fn finish(self) -> (String, Vec<String>) {
        (self.out, self.errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_stops_at_the_cap() {
        let mut w = Writer::default();
        let long = "x".repeat(1 << 20);
        for _ in 0..100 {
            w.value(0, "V", &long);
        }
        assert!(w.full());
        let (text, errors) = w.finish();
        assert!(text.len() < MAX_OUTPUT + (2 << 20));
        assert_eq!(errors.len(), 1, "{errors:?}");
    }

    #[test]
    fn formatting() {
        let mut w = Writer::default();
        w.open(0, "Root");
        w.value(1, "a", "1");
        w.string(1, "b", "x<y");
        w.string(1, "c", "");
        w.raw_array(1, "d", &["1".into(), "2".into(), "3".into()], 2);
        w.close(0, "Root");
        assert_eq!(w.out, "<Root>\n <a value=\"1\" />\n <b>x&lt;y</b>\n <c />\n <d>\n  1 2\n  3\n </d>\n</Root>\n");
        assert_eq!(float(1.0), "1");
        assert_eq!(float(0.25), "0.25");
    }

    #[test]
    fn escaping_covers_markup_and_drops_control_characters() {
        assert_eq!(escape("a&b<c>d\"e'f"), "a&amp;b&lt;c&gt;d&quot;e'f");
        assert_eq!(escape("tab\tnl\ncr\r\u{1}\u{1f}end"), "tab\tnl\ncr\rend");
        assert_eq!(escape("\u{e9}\u{1F600}"), "\u{e9}\u{1F600}");
        assert_eq!(escape(""), "");
    }

    #[test]
    fn new_writers_start_with_the_header() {
        let w = Writer::new();
        assert_eq!(w.out, format!("{HEADER}\n"));
        assert!(!w.full());
        assert_eq!(Writer::with_limit(10).out, w.out);
    }

    #[test]
    fn elements_are_indented_one_space_per_level() {
        let mut w = Writer::default();
        w.open(0, "a x=\"1\"");
        w.empty(1, "b y=\"2\"");
        w.text(1, "c z=\"3\"", "1 < 2");
        w.value(2, "d", "v");
        w.string(1, "e", "text");
        w.error(1, "bad <thing>".into());
        w.close(0, "a");
        assert_eq!(
            w.out,
            "<a x=\"1\">\n <b y=\"2\" />\n <c z=\"3\">1 &lt; 2</c>\n  <d value=\"v\" />\n <e>text</e>\n <error>bad &lt;thing&gt;</error>\n</a>\n"
        );
        assert_eq!(w.errors, ["bad <thing>"]);
    }

    #[test]
    fn raw_arrays_fit_one_line_or_break_into_rows() {
        let items: Vec<String> = (1..=5).map(|i| i.to_string()).collect();
        let mut w = Writer::default();
        w.raw_array(1, "a", &items[..3], 3);
        w.raw_array(1, "b", &items, 3);
        w.raw_array(0, "c", &[], 3);
        w.raw_array(0, "d", &items[..2], 0);
        assert_eq!(w.out, " <a>1 2 3</a>\n <b>\n  1 2 3\n  4 5\n </b>\n<c />\n<d>\n 1\n 2\n</d>\n");
    }

    #[test]
    fn raw_array_lines_and_item_arrays() {
        let items = vec!["x".to_owned(), "y&z".to_owned()];
        let mut w = Writer::default();
        w.raw_array_lines(2, &items);
        w.item_array(0, "list", &items);
        w.item_array(0, "none", &[]);
        assert_eq!(w.out, "  x\n  y&z\n<list>\n <Item>x</Item>\n <Item>y&amp;z</Item>\n</list>\n<none />\n");
    }

    #[test]
    fn a_writer_with_a_small_limit_stops_and_says_so_once() {
        let mut w = Writer::with_limit(60);
        for i in 0..10 {
            w.value(0, "value", &i.to_string());
        }
        assert!(w.full());
        w.open(0, "ignored");
        w.close(0, "ignored");
        w.empty(0, "ignored");
        w.text(0, "ignored", "x");
        w.error(0, "ignored".into());
        w.raw_array(0, "ignored", &["1".to_owned()], 1);
        w.raw_array_lines(0, &["ignored".to_owned()]);
        w.item_array(0, "ignored", &["1".to_owned()]);
        let (out, errors) = w.finish();
        assert!(!out.contains("ignored"));
        assert_eq!(errors, ["output truncated at 0 MB: the data nests too much to show in full"]);
        assert_eq!(out.matches("<error>").count(), 1);
        assert!(out.starts_with(HEADER) && out.ends_with("</error>\n"));
        assert_eq!(out.matches("<value value=").count(), 2);
    }

    #[test]
    fn floats_read_back_to_the_same_value() {
        for v in [0.1f32, -3.5, 1e-7, 123456.79, f32::MAX] {
            assert_eq!(float(v).parse::<f32>().unwrap(), v);
        }
    }
}
