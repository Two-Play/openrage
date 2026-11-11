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

#[derive(Default)]
pub struct Writer {
    pub out: String,
    /// Things that could not be read; also written as `<error>` elements.
    pub errors: Vec<String>,
}

impl Writer {
    pub fn new() -> Self {
        let mut w = Self::default();
        w.out.push_str(HEADER);
        w.out.push('\n');
        w
    }

    fn indent(&mut self, level: usize) {
        for _ in 0..level {
            self.out.push(' ');
        }
    }

    /// `<tag>` (the tag may include attributes) followed by a newline.
    pub fn open(&mut self, level: usize, tag: &str) {
        self.indent(level);
        self.out.push('<');
        self.out.push_str(tag);
        self.out.push_str(">\n");
    }

    pub fn close(&mut self, level: usize, name: &str) {
        self.indent(level);
        self.out.push_str("</");
        self.out.push_str(name);
        self.out.push_str(">\n");
    }

    /// `<name value="v" />`
    pub fn value(&mut self, level: usize, name: &str, v: &str) {
        self.indent(level);
        self.out.push('<');
        self.out.push_str(name);
        self.out.push_str(" value=\"");
        self.out.push_str(v);
        self.out.push_str("\" />\n");
    }

    /// `<content />`
    pub fn empty(&mut self, level: usize, content: &str) {
        self.indent(level);
        self.out.push('<');
        self.out.push_str(content);
        self.out.push_str(" />\n");
    }

    /// `<name>text</name>`, text escaped; `name` may carry attributes.
    pub fn text(&mut self, level: usize, name: &str, text: &str) {
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
        self.indent(level);
        self.out.push_str("<error>");
        self.out.push_str(&escape(&msg));
        self.out.push_str("</error>\n");
        self.errors.push(msg);
    }

    /// Values separated by spaces, `per_row` to a line.
    pub fn raw_array(&mut self, level: usize, name: &str, items: &[String], per_row: usize) {
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
        for item in items {
            self.indent(level);
            self.out.push_str(item);
            self.out.push('\n');
        }
    }

    /// `<Item>value</Item>` per item.
    pub fn item_array(&mut self, level: usize, name: &str, items: &[String]) {
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
}
