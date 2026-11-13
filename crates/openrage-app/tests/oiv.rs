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
    std::fs::write(root.join("update/update.rpf"), w.build().unwrap()).unwrap();
    std::fs::write(root.join("GTA5.exe"), b"").unwrap();
    root
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
    assert_eq!(std::fs::read(game.join("update/update.rpf")).unwrap(), original, "game archive untouched");

    let undone = uninstall(&game, "Test Mod 1.0").unwrap();
    assert!(!undone.is_empty());
    assert!(!game.join("plugin.asi").exists());
    assert!(!game.join("mods/update/x64/dlcpacks/testmod/dlc.rpf").exists());
    assert!(!game.join("mods/update/update.rpf").exists());
    assert_eq!(read(&game, "update/update.rpf/common/data/old.dat").unwrap(), b"old");
}

#[test]
fn reports_blockers() {
    let game = fake_game("blockers");
    let bad = ASSEMBLY.replace(
        r#"createIfNotExist="False" type="RPF7">
      <add source="water.xml">"#,
        r#"createIfNotExist="False" type="RPF7">
      <pso path="x.ymt"/>
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
