//! Installing and uninstalling an OpenIV package on a tiny fake game.

use std::io::Write;
use std::path::{Path, PathBuf};

use openrage_app::oiv::Package;
use openrage_app::oiv::install::{install, preview, uninstall};
use rage_rpf::ArchiveWriter;
use rage_rpf::vfs::Node;

const DLCLIST: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<SMandatoryPacksData>\n\t<Paths>\n\t\t<Item>dlcpacks:/mpbeach/</Item>\n\t</Paths>\n</SMandatoryPacksData>\n";

fn fake_game(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("openrage-oiv-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("update")).unwrap();
    let mut w = ArchiveWriter::new();
    w.add_file("common/data/dlclist.xml", DLCLIST.as_bytes().to_vec()).unwrap();
    w.add_file("common/data/settings.txt", b"a=1\nb=2\n".to_vec()).unwrap();
    w.add_file("common/data/old.dat", b"old".to_vec()).unwrap();
    w.add_file("common/data/peds.ymt", tiny_pso()).unwrap();
    std::fs::write(root.join("update/update.rpf"), w.build().unwrap()).unwrap();
    std::fs::write(root.join("GTA5.exe"), b"").unwrap();
    root
}

/// A PSO file like the game's: a list of two items (a hash and a float each),
/// the items in block 1 and the root structure in block 2.
fn tiny_pso() -> Vec<u8> {
    use rage_core::hash::{jenkins_bytes, joaat};
    let h = |name: &str| jenkins_bytes(name.as_bytes());
    let (root, item) = (h("CPedModelInfo__InitDataList"), h("CPedModelInfo__InitData"));
    let be = |values: &[u32]| values.iter().flat_map(|v| v.to_be_bytes()).collect::<Vec<u8>>();
    let mut psin = b"PSIN\0\0\0\0pppppppp".to_vec();
    psin.extend(be(&[joaat("adder"), 1.5f32.to_bits(), joaat("banshee"), 2f32.to_bits()]));
    psin.extend(be(&[1, 0, 0x0002_0002, 0])); // pointer to block 1, count and capacity 2
    psin[4..8].copy_from_slice(&48u32.to_be_bytes());
    let mut pmap = b"PMAP".to_vec();
    pmap.extend(be(&[48, 2, 0x0002_7070, item, 16, 0, 16, root, 32, 0, 16]));
    // Structure infos: entry count, length, 0, then (name, type << 24 | subtype << 16 | offset, key).
    let root_info = be(&[2, 16, 0, 0x100, 0x0c00_0000, item, h("InitDatas"), 0x0d00_0000, 0]);
    let item_info = be(&[2, 8, 0, h("Name"), 0x0b07_0000, 0, h("Radius"), 0x0700_0004, 0]);
    let mut psch = b"PSCH".to_vec();
    psch.extend(be(&[28 + 36 + 36, 2, root, 28, item, 28 + 36]));
    psch.extend(root_info);
    psch.extend(item_info);
    [psin, pmap, psch].concat()
}

fn package(assembly: &str, files: &[(&str, &[u8])]) -> Package {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file("assembly.xml", opts).unwrap();
        zip.write_all(assembly.as_bytes()).unwrap();
        for (name, data) in files {
            zip.start_file(format!("content/{name}"), opts).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    }
    buf.set_position(0);
    Package::read(buf).unwrap()
}

fn read(game: &Path, vpath: &str) -> Option<Vec<u8>> {
    match rage_mods::resolve(game, vpath, None).ok()?.0 {
        Node::ArchiveFile(rpf, id) => rpf.read(id).ok(),
        Node::PlainFile(p) => std::fs::read(p).ok(),
        _ => None,
    }
}

const ASSEMBLY: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="2.2" id="{1}" target="Five">
  <metadata><name>Test Mod</name><version><major>1</major><minor>0</minor></version>
    <author><displayName>Me</displayName></author><description>Test</description></metadata>
  <colors/>
  <content>
    <add source="plugin.asi">plugin.asi</add>
    <archive path="update\update.rpf" createIfNotExist="False" type="RPF7">
      <add source="water.xml">common\data\levels\gta5\water.xml</add>
      <delete>common\data\old.dat</delete>
      <archive path="x64\new_inner.rpf" createIfNotExist="True" type="RPF7">
        <add source="inside.txt">deep\inside.txt</add>
      </archive>
      <text path="common\data\settings.txt" createIfNotExist="False">
        <replace line="b=" condition="StartWith">b=42</replace>
        <add>c=3</add>
      </text>
      <pso path="common\data\peds.ymt">
        <replace xpath="/CPedModelInfo__InitDataList/InitDatas/Item[Name = 'banshee']/Radius">
          <Radius value="4.5" />
        </replace>
        <add xpath="/CPedModelInfo__InitDataList/InitDatas" append="Last">
          <Item>
            <Name>zentorno</Name>
            <Radius value="3" />
          </Item>
        </add>
      </pso>
      <xml path="common\data\dlclist.xml">
        <add xpath="/SMandatoryPacksData/Paths" append="Last">
          <Item>dlcpacks:/testmod/</Item>
        </add>
      </xml>
    </archive>
    <archive path="update\x64\dlcpacks\testmod\dlc.rpf" createIfNotExist="True" type="RPF7">
      <add source="setup2.xml">setup2.xml</add>
    </archive>
  </content>
</package>"#;

#[test]
fn installs_and_uninstalls() {
    let game = fake_game("full");
    let original = std::fs::read(game.join("update/update.rpf")).unwrap();
    let pkg = package(
        ASSEMBLY,
        &[("plugin.asi", b"ASI"), ("water.xml", b"<water/>"), ("inside.txt", b"inside"), ("setup2.xml", b"<setup/>")],
    );

    let plan = preview(&game, None, &pkg).unwrap();
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    let actions: Vec<String> = plan.steps.iter().map(|s| format!("{} {}", s.action, s.target)).collect();
    assert!(actions.contains(&"Add plugin.asi".to_owned()), "{actions:?}");
    assert!(actions.contains(&"Delete update/update.rpf/common/data/old.dat".to_owned()));
    assert!(actions.contains(&"Create update/x64/dlcpacks/testmod/dlc.rpf".to_owned()));
    let pso_step = plan.steps.iter().find(|s| s.target.ends_with("peds.ymt")).expect("PSO edit planned");
    assert_eq!(pso_step.detail, "2 XML command(s) on the PSO data: 5 line(s) added, 1 removed");
    // The dry run changed nothing.
    assert!(!game.join("mods").exists() && !game.join("plugin.asi").exists());

    install(&game, None, &pkg).unwrap();
    assert_eq!(std::fs::read(game.join("plugin.asi")).unwrap(), b"ASI", "new loose files go to the game folder");
    assert_eq!(read(&game, "update/update.rpf/common/data/levels/gta5/water.xml").unwrap(), b"<water/>");
    assert!(read(&game, "update/update.rpf/common/data/old.dat").is_none());
    assert_eq!(read(&game, "update/update.rpf/x64/new_inner.rpf/deep/inside.txt").unwrap(), b"inside");
    assert_eq!(read(&game, "update/update.rpf/common/data/settings.txt").unwrap(), b"a=1\nb=42\nc=3\n");
    let dlclist = String::from_utf8(read(&game, "update/update.rpf/common/data/dlclist.xml").unwrap()).unwrap();
    assert!(
        dlclist.contains("\t\t<Item>dlcpacks:/mpbeach/</Item>\n\t\t<Item>dlcpacks:/testmod/</Item>\n\t</Paths>"),
        "{dlclist}"
    );
    assert_eq!(read(&game, "update/x64/dlcpacks/testmod/dlc.rpf/setup2.xml").unwrap(), b"<setup/>");
    let peds = read(&game, "update/update.rpf/common/data/peds.ymt").unwrap();
    let (xml, errors) = rage_formats::pso::to_xml(&peds).unwrap();
    assert!(errors.is_empty(), "{errors:?}");
    assert!(xml.contains("<Radius value=\"4.5\" />") && xml.contains("<Radius value=\"1.5\" />"), "{xml}");
    assert_eq!(xml.matches("<Item>").count(), 3, "{xml}");
    assert_eq!(std::fs::read(game.join("update/update.rpf")).unwrap(), original, "game archive untouched");

    let undone = uninstall(&game, "Test Mod 1.0").unwrap();
    assert!(!undone.is_empty());
    assert!(!game.join("plugin.asi").exists());
    assert!(!game.join("mods/update/x64/dlcpacks/testmod/dlc.rpf").exists());
    assert!(!game.join("mods/update/update.rpf").exists());
    assert_eq!(read(&game, "update/update.rpf/common/data/old.dat").unwrap(), b"old");
    assert_eq!(read(&game, "update/update.rpf/common/data/peds.ymt").unwrap(), tiny_pso());
}

#[test]
fn reports_blockers() {
    let game = fake_game("blockers");
    let bad = ASSEMBLY.replace(
        r#"createIfNotExist="False" type="RPF7">
      <add source="water.xml">"#,
        r#"createIfNotExist="False" type="RPF7">
      <pso path="x.ymt"><remove xpath="/A"/></pso>
      <add source="water.xml">"#,
    );
    let pkg = package(&bad, &[("plugin.asi", b"ASI")]);
    let plan = preview(&game, None, &pkg).unwrap();
    assert!(plan.blockers.iter().any(|b| b.contains("PSO")), "{:?}", plan.blockers);
    assert!(plan.blockers.iter().any(|b| b.contains("water.xml")), "missing content file is reported");
    assert!(install(&game, None, &pkg).is_err());
    assert!(!game.join("mods").exists());
}

#[test]
fn refuses_paths_outside_the_game() {
    let game = fake_game("traversal");
    let escape = |content: &str| {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="2.2" id="{{1}}" target="Five">
  <metadata><name>Evil</name><version><major>1</major><minor>0</minor></version>
    <author><displayName>Me</displayName></author><description>x</description></metadata>
  <content>{content}</content>
</package>"#
        )
    };
    let cases = [
        r#"<add source="x.txt">..\..\outside.txt</add>"#,
        r#"<add source="x.txt">C:\Windows\x.txt</add>"#,
        r#"<delete>../../outside.txt</delete>"#,
        r#"<text path="../../../.bashrc" createIfNotExist="True"><add>evil</add></text>"#,
        r#"<archive path="..\..\x.rpf" createIfNotExist="True" type="RPF7"><add source="x.txt">a.txt</add></archive>"#,
        r#"<archive path="update\update.rpf" createIfNotExist="False" type="RPF7"><add source="x.txt">..\..\x.txt</add></archive>"#,
    ];
    for content in cases {
        let pkg = package(&escape(content), &[("x.txt", b"x")]);
        let plan = preview(&game, None, &pkg).unwrap();
        assert!(plan.blockers.iter().any(|b| b.contains("leaves the game folder")), "{content}: {:?}", plan.blockers);
        assert!(install(&game, None, &pkg).is_err(), "{content}");
    }
    assert!(!game.parent().unwrap().join("outside.txt").exists());
    assert!(!game.join("mods").exists());
}
