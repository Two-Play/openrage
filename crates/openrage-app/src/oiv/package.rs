//! Reading OpenIV packages (`.oiv`, format 2.x; see the OpenIV-PackageFormat
//! specification): a ZIP with `assembly.xml`, `content/` and an optional `icon.png`.

use std::collections::HashMap;
use std::io::{Read, Seek};
use std::path::Path;

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone, Default)]
pub struct Metadata {
    pub name: String,
    /// `major.minor` plus optional tag, e.g. `1.2 Beta`.
    pub version: String,
    pub author: String,
    pub web: String,
    pub description: String,
    pub large_description: String,
    pub licence: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Condition {
    Equal,
    StartWith,
    Mask,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextCmd {
    /// Append a line.
    Add(String),
    Insert {
        before: bool,
        line: String,
        condition: Condition,
        text: String,
    },
    Replace {
        line: String,
        condition: Condition,
        text: String,
    },
    Delete {
        line: String,
        condition: Condition,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Append {
    First,
    Last,
    Before,
    After,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XmlCmd {
    /// Insert `content` (raw XML) relative to the selected node.
    Add {
        xpath: String,
        append: Append,
        content: String,
    },
    Replace {
        xpath: String,
        content: String,
    },
    Remove {
        xpath: String,
    },
}

/// One command of the content script. Paths are as written in the package
/// (backslashes allowed), relative to the game folder or the enclosing archive.
#[derive(Debug, Clone)]
pub enum Command {
    Add {
        source: String,
        target: String,
    },
    Delete {
        target: String,
    },
    Archive {
        path: String,
        create: bool,
        kind: String,
        commands: Vec<Command>,
    },
    Text {
        path: String,
        create: bool,
        commands: Vec<TextCmd>,
    },
    Xml {
        path: String,
        commands: Vec<XmlCmd>,
    },
    /// PSO/META edits (`<pso>`); need a PSO converter, not supported yet.
    Pso {
        path: String,
    },
    /// Archive rebuild to reclaim space; optional, skipped.
    Defragment {
        archive: String,
    },
}

pub struct Package {
    pub format_version: String,
    pub id: String,
    pub target: String,
    pub metadata: Metadata,
    pub icon: Option<Vec<u8>>,
    pub commands: Vec<Command>,
    /// Content files, keyed by lowercase path relative to `content/` (with `/`).
    files: HashMap<String, Vec<u8>>,
}

/// Most files in a package.
const MAX_FILES: usize = 200_000;
/// Most bytes all files of a package may take once unpacked (the package is
/// held in memory while it is checked and installed).
const MAX_UNPACKED: u64 = 8 << 30;
/// Most bytes reserved for one file before reading it.
const PREALLOCATE: u64 = 64 << 20;

impl Package {
    pub fn open(path: &Path) -> Result<Self> {
        let f = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
        Self::read(f)
    }

    pub fn read<R: Read + Seek>(reader: R) -> Result<Self> {
        let mut zip = zip::ZipArchive::new(reader).context("not a valid .oiv (ZIP) package")?;
        let mut files = HashMap::new();
        let mut assembly = None;
        let mut icon = None;
        if zip.len() > MAX_FILES {
            bail!("the package has more than {MAX_FILES} files");
        }
        let mut total = 0u64;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i)?;
            if entry.is_dir() {
                continue;
            }
            let name = entry.name()?.replace('\\', "/");
            // The declared size cannot be trusted (ZIP bombs): never reserve
            // more than a little up front, and read at most what is left.
            let left = MAX_UNPACKED - total;
            let mut data = Vec::with_capacity(entry.size().min(PREALLOCATE) as usize);
            (&mut entry)
                .take(left + 1)
                .read_to_end(&mut data)
                .with_context(|| format!("reading {name} from the package"))?;
            total += data.len() as u64;
            if total > MAX_UNPACKED {
                bail!("the package unpacks to more than {} GB", MAX_UNPACKED >> 30);
            }
            let lower = name.to_ascii_lowercase();
            if lower == "assembly.xml" {
                assembly = Some(data);
            } else if lower == "icon.png" {
                icon = Some(data);
            } else if let Some(rest) = lower.strip_prefix("content/") {
                files.insert(rest.to_owned(), data);
            }
        }
        let assembly = assembly.context("package has no assembly.xml")?;
        let text =
            String::from_utf8_lossy(assembly.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&assembly)).into_owned();
        let mut pkg = parse_assembly(&text)?;
        pkg.icon = icon;
        pkg.files = files;
        Ok(pkg)
    }

    /// A content file by its `source` attribute.
    pub fn file(&self, source: &str) -> Option<&[u8]> {
        self.files.get(&source.replace('\\', "/").trim_start_matches('/').to_ascii_lowercase()).map(Vec::as_slice)
    }
}

fn child<'a>(node: roxmltree::Node<'a, 'a>, name: &str) -> Option<roxmltree::Node<'a, 'a>> {
    node.children().find(|c| c.is_element() && c.tag_name().name().eq_ignore_ascii_case(name))
}

fn text_of(node: Option<roxmltree::Node<'_, '_>>) -> String {
    node.map(|n| n.descendants().filter(|d| d.is_text()).filter_map(|d| d.text()).collect::<String>())
        .unwrap_or_default()
        .trim()
        .to_owned()
}

fn attr(node: roxmltree::Node<'_, '_>, name: &str) -> Option<String> {
    node.attributes().find(|a| a.name().eq_ignore_ascii_case(name)).map(|a| a.value().to_owned())
}

fn flag(node: roxmltree::Node<'_, '_>, name: &str) -> bool {
    attr(node, name).is_some_and(|v| v.eq_ignore_ascii_case("true"))
}

fn condition(node: roxmltree::Node<'_, '_>) -> Result<Condition> {
    Ok(match attr(node, "condition").unwrap_or_else(|| "Equal".into()).to_ascii_lowercase().as_str() {
        "equal" => Condition::Equal,
        "startwith" | "startswith" => Condition::StartWith,
        "mask" => Condition::Mask,
        other => bail!("unknown text condition \"{other}\""),
    })
}

/// The raw XML inside a command node (e.g. `<Item>…</Item>` of an `<add>`), dedented.
fn inner_xml(source: &str, node: roxmltree::Node<'_, '_>) -> String {
    let children: Vec<_> = node.children().collect();
    let (Some(first), Some(last)) = (children.first(), children.last()) else { return String::new() };
    let raw = &source[first.range().start..last.range().end];
    let lines: Vec<&str> = raw.trim().lines().collect();
    let indent = lines
        .iter()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .enumerate()
        .map(|(i, l)| if i == 0 { l.trim_start() } else { l.get(indent..).unwrap_or_else(|| l.trim_start()) })
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_commands(source: &str, node: roxmltree::Node<'_, '_>) -> Result<Vec<Command>> {
    let mut out = Vec::new();
    for c in node.children().filter(|c| c.is_element()) {
        let tag = c.tag_name().name().to_ascii_lowercase();
        let cmd = match tag.as_str() {
            "add" => {
                Command::Add { source: attr(c, "source").context("<add> without source")?, target: text_of(Some(c)) }
            }
            "delete" => Command::Delete { target: text_of(Some(c)) },
            "archive" => Command::Archive {
                path: attr(c, "path").context("<archive> without path")?,
                create: flag(c, "createIfNotExist"),
                kind: attr(c, "type").unwrap_or_else(|| "RPF7".into()),
                commands: parse_commands(source, c)?,
            },
            "text" => {
                let mut commands = Vec::new();
                for t in c.children().filter(|t| t.is_element()) {
                    let value = text_of(Some(t));
                    commands.push(match t.tag_name().name().to_ascii_lowercase().as_str() {
                        "add" => TextCmd::Add(value),
                        "insert" => TextCmd::Insert {
                            before: attr(t, "where").is_some_and(|w| w.eq_ignore_ascii_case("before")),
                            line: attr(t, "line").unwrap_or_default(),
                            condition: condition(t)?,
                            text: value,
                        },
                        "replace" => TextCmd::Replace {
                            line: attr(t, "line").unwrap_or_default(),
                            condition: condition(t)?,
                            text: value,
                        },
                        "delete" => TextCmd::Delete { line: value, condition: condition(t)? },
                        other => bail!("unknown text command <{other}>"),
                    });
                }
                Command::Text {
                    path: attr(c, "path").context("<text> without path")?,
                    create: flag(c, "createIfNotExist"),
                    commands,
                }
            }
            "xml" => {
                let mut commands = Vec::new();
                for x in c.children().filter(|x| x.is_element()) {
                    let xpath = attr(x, "xpath").context("XML command without xpath")?;
                    commands.push(match x.tag_name().name().to_ascii_lowercase().as_str() {
                        "add" => XmlCmd::Add {
                            append: match attr(x, "append")
                                .unwrap_or_else(|| "Last".into())
                                .to_ascii_lowercase()
                                .as_str()
                            {
                                "first" => Append::First,
                                "before" => Append::Before,
                                "after" => Append::After,
                                _ => Append::Last,
                            },
                            content: inner_xml(source, x),
                            xpath,
                        },
                        "replace" => XmlCmd::Replace { content: inner_xml(source, x), xpath },
                        "remove" => XmlCmd::Remove { xpath },
                        other => bail!("unknown XML command <{other}>"),
                    });
                }
                Command::Xml { path: attr(c, "path").context("<xml> without path")?, commands }
            }
            "pso" => Command::Pso { path: attr(c, "path").unwrap_or_default() },
            "defragmentation" => Command::Defragment { archive: attr(c, "archive").unwrap_or_default() },
            other => bail!("unknown package command <{other}>"),
        };
        out.push(cmd);
    }
    Ok(out)
}

fn parse_assembly(text: &str) -> Result<Package> {
    let doc = roxmltree::Document::parse(text).context("assembly.xml is not valid XML")?;
    let root = doc.root_element();
    if !root.tag_name().name().eq_ignore_ascii_case("package") {
        bail!("assembly.xml has no <package> root");
    }
    let format_version = attr(root, "version").unwrap_or_default();
    if !format_version.starts_with('2') {
        bail!("package format {format_version} is not supported (only 2.x)");
    }
    let target = attr(root, "target").unwrap_or_default();
    if !target.is_empty() && !target.eq_ignore_ascii_case("Five") {
        bail!("this package is for game \"{target}\", not GTA V");
    }
    let meta = child(root, "metadata").context("assembly.xml has no <metadata>")?;
    let version = child(meta, "version").map(|v| {
        let mut s = format!("{}.{}", text_of(child(v, "major")), text_of(child(v, "minor")));
        let tag = text_of(child(v, "tag"));
        if !tag.is_empty() {
            s.push(' ');
            s.push_str(&tag);
        }
        s
    });
    let author = child(meta, "author");
    let metadata = Metadata {
        name: text_of(child(meta, "name")),
        version: version.unwrap_or_default(),
        author: text_of(author.and_then(|a| child(a, "displayName"))),
        web: text_of(author.and_then(|a| child(a, "web"))),
        description: text_of(child(meta, "description")),
        large_description: text_of(child(meta, "largeDescription")),
        licence: text_of(child(meta, "licence")),
    };
    let content = child(root, "content").context("assembly.xml has no <content>")?;
    Ok(Package {
        format_version,
        id: attr(root, "id").unwrap_or_default(),
        target,
        metadata,
        icon: None,
        commands: parse_commands(text, content)?,
        files: HashMap::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ASSEMBLY: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="2.2" id="{X}" target="Five">
  <metadata>
    <name>Test</name>
    <version><major>1</major><minor>2</minor><tag>Beta</tag></version>
    <author><displayName>Me</displayName></author>
    <description><![CDATA[Hello.]]></description>
  </metadata>
  <colors/>
  <content>
    <add source="a.asi">a.asi</add>
    <archive path="update\update.rpf" createIfNotExist="False" type="RPF7">
      <add source="water.xml">data\water.xml</add>
      <archive path="x64\inner.rpf" createIfNotExist="True" type="RPF7">
        <delete>old.txt</delete>
      </archive>
      <text path="t.txt" createIfNotExist="True">
        <insert where="Before" line="Line 1" condition="Equal">first</insert>
        <delete condition="Mask">#*</delete>
      </text>
      <xml path="common\data\dlclist.xml">
        <add xpath="/SMandatoryPacksData/Paths" append="First">
          <Item>dlcpacks:\x\</Item>
        </add>
        <remove xpath="/A/B[2]"/>
      </xml>
    </archive>
    <defragmentation archive="update\update.rpf"/>
  </content>
</package>"#;

    #[test]
    fn parses_assembly() {
        let p = parse_assembly(ASSEMBLY).unwrap();
        assert_eq!(p.metadata.name, "Test");
        assert_eq!(p.metadata.version, "1.2 Beta");
        assert_eq!(p.metadata.description, "Hello.");
        assert_eq!(p.commands.len(), 3);
        let Command::Archive { path, commands, .. } = &p.commands[1] else { panic!() };
        assert_eq!(path, "update\\update.rpf");
        assert!(matches!(&commands[1], Command::Archive { create: true, .. }));
        let Command::Xml { commands: xml, .. } = &commands[3] else { panic!() };
        assert_eq!(
            xml[0],
            XmlCmd::Add {
                xpath: "/SMandatoryPacksData/Paths".into(),
                append: Append::First,
                content: "<Item>dlcpacks:\\x\\</Item>".into()
            }
        );
        let Command::Text { commands: text, create: true, .. } = &commands[2] else { panic!() };
        assert_eq!(text[1], TextCmd::Delete { line: "#*".into(), condition: Condition::Mask });
    }

    #[test]
    fn rejects_other_games() {
        let other = ASSEMBLY.replace("target=\"Five\"", "target=\"IV\"");
        assert!(parse_assembly(&other).is_err());
    }
}
