//! The text layer of openFormats: indented `Key values` lines and blocks in
//! braces.

use rage_resource::ResourceError;

use crate::drawable::bad;

/// Writes lines indented with tabs, as OpenIV does.
#[derive(Default)]
pub(crate) struct Writer {
    pub(crate) out: String,
    depth: usize,
}

impl Writer {
    pub(crate) fn line(&mut self, text: impl AsRef<str>) {
        for _ in 0..self.depth {
            self.out.push('\t');
        }
        self.out.push_str(text.as_ref());
        self.out.push_str("\r\n");
    }

    pub(crate) fn open(&mut self) {
        self.line("{");
        self.depth += 1;
    }

    pub(crate) fn close(&mut self) {
        self.depth = self.depth.saturating_sub(1);
        self.line("}");
    }

    /// `name` followed by a block that `body` fills.
    pub(crate) fn block(&mut self, name: impl AsRef<str>, body: impl FnOnce(&mut Self)) {
        self.line(name);
        self.open();
        body(self);
        self.close();
    }
}

/// A number as OpenIV writes it: rounded to seven significant digits (what
/// a float holds), with eight decimals.
pub(crate) fn num(v: f32) -> String {
    if v == 0.0 || !v.is_finite() {
        return if v.is_finite() { "0.00000000".to_owned() } else { format!("{v}") };
    }
    let rounded: f64 = format!("{v:.6e}").parse().unwrap_or(f64::from(v));
    format!("{rounded:.8}")
}

pub(crate) fn nums(v: &[f32]) -> String {
    v.iter().map(|&x| num(x)).collect::<Vec<_>>().join(" ")
}

/// A line and, if a block follows it, the block's lines.
#[derive(Debug, Clone)]
pub(crate) struct Node {
    /// The line's words.
    pub(crate) words: Vec<String>,
    pub(crate) children: Option<Vec<Node>>,
    /// Line number (1-based) for messages.
    pub(crate) line: usize,
}

impl Node {
    pub(crate) fn key(&self) -> &str {
        self.words.first().map_or("", String::as_str)
    }

    /// The words after the key.
    pub(crate) fn args(&self) -> &[String] {
        self.words.get(1..).unwrap_or(&[])
    }

    pub(crate) fn arg(&self, i: usize) -> Result<&str, ResourceError> {
        self.args()
            .get(i)
            .map(String::as_str)
            .ok_or_else(|| bad(format!("line {}: {} needs a value", self.line, self.key())))
    }

    pub(crate) fn children(&self) -> &[Node] {
        self.children.as_deref().unwrap_or(&[])
    }

    /// The first child with this key (case-insensitive).
    pub(crate) fn get(&self, key: &str) -> Option<&Node> {
        self.children().iter().find(|n| n.key().eq_ignore_ascii_case(key))
    }

    pub(crate) fn all<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.children().iter().filter(move |n| n.key().eq_ignore_ascii_case(key))
    }

    pub(crate) fn floats(&self) -> Result<Vec<f32>, ResourceError> {
        self.args().iter().map(|w| parse_f32(w, self.line)).collect()
    }

    pub(crate) fn float(&self) -> Result<f32, ResourceError> {
        parse_f32(self.arg(0)?, self.line)
    }

    pub(crate) fn vec3(&self) -> Result<[f32; 3], ResourceError> {
        let v = self.floats()?;
        v.get(..3)
            .map(|s| [s[0], s[1], s[2]])
            .ok_or_else(|| bad(format!("line {}: {} needs three numbers", self.line, self.key())))
    }

    pub(crate) fn int<T: std::str::FromStr>(&self) -> Result<T, ResourceError> {
        let w = self.arg(0)?;
        w.parse().map_err(|_| bad(format!("line {}: {w} is not a whole number", self.line)))
    }

    pub(crate) fn bool(&self) -> Result<bool, ResourceError> {
        Ok(self.arg(0)?.eq_ignore_ascii_case("true"))
    }
}

pub(crate) fn parse_f32(w: &str, line: usize) -> Result<f32, ResourceError> {
    w.parse().map_err(|_| bad(format!("line {line}: {w} is not a number")))
}

/// The block of the file's `Version` line.
pub(crate) struct Document {
    pub(crate) root: Node,
}

/// Splits a file into lines and blocks.
pub(crate) fn parse(text: &str) -> Result<Document, ResourceError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    // Stack of open blocks; the bottom one collects the top level.
    let mut stack: Vec<Vec<Node>> = vec![Vec::new()];
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        match line {
            "{" => {
                if stack.last().is_none_or(|b| b.is_empty()) {
                    return Err(bad(format!("line {}: a block without a name", i + 1)));
                }
                stack.push(Vec::new());
            }
            "}" => {
                let children = stack.pop().filter(|_| !stack.is_empty());
                let (Some(children), Some(parent)) = (children, stack.last_mut()) else {
                    return Err(bad(format!("line {}: a closing brace too many", i + 1)));
                };
                parent.last_mut().expect("checked when opening").children = Some(children);
            }
            _ => {
                let words = line.split_whitespace().map(str::to_owned).collect();
                stack.last_mut().expect("bottom").push(Node { words, children: None, line: i + 1 });
            }
        }
    }
    if stack.len() != 1 {
        return Err(bad("the file ends inside a block"));
    }
    let top = stack.pop().expect("bottom");
    let version = top.first().filter(|n| n.key().eq_ignore_ascii_case("Version"));
    let Some(node) = version else { return Err(bad("the file does not start with a Version line")) };
    node.arg(0)?.parse::<u32>().map_err(|_| bad(format!("line {}: bad Version line", node.line)))?;
    let root = Node { words: Vec::new(), children: node.children.clone(), line: node.line };
    if root.children.is_none() {
        return Err(bad("the Version line has no block"));
    }
    Ok(Document { root })
}
