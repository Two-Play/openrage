+++
title = "Command line"
description = "The openrage command-line tool: browse, search and extract archives, convert formats, edit archives, install .oiv packages, run plugins and verify the whole game."

[extra]
docs = true
lead = "`openrage` comes in the same download as the GUI: for scripts, automation and everyone who prefers a terminal."
toc = true
+++

Paths are virtual: archives — including archives inside archives — behave like directories.

## Browsing, search and extraction

```
openrage ls                                    # game directory
openrage ls x64a.rpf/data
openrage ls -r update/update.rpf               # recursive, descends into nested archives
openrage info x64e.rpf/levels/gta5/vehicles.rpf
openrage find adder.yft                        # search all archives of the game
openrage extract common.rpf/data/cargens.meta -o out/
openrage extract x64a.rpf -o out/              # whole archive
openrage extract some.rpf -o out/ --expand-archives
openrage index                                 # build / update the search index
openrage search weapon_pistol                  # names + full text, shows matching lines
openrage search 0xB779A091                     # hash lookup
openrage search Pegassi --lang american        # game text in another language (or `all`)
```

GTA IV works with the same commands: point `--game` at its folder (the Steam folder `Grand Theft Auto IV` or the `GTAIV`
folder inside it) and run `keys` once, which takes the archive key from your `GTAIV.exe`.

```
openrage --game "/path/to/Grand Theft Auto IV" keys
openrage ls pc/models/cdimages/vehicles.img     # .img archives open like folders
openrage info pc/models/cdimages/vehicles.img/admiral.wft
openrage extract pc/data/maps/manhat/manhat01.img -o out/
openrage index && openrage search admiral       # search index: names, hashes, .ide/.dat/.gxt text
openrage verify                                # read every entry of every .img and .rpf
openrage textures pc/models/cdimages/vehicles.img/admiral.wtd --export out/ --png
openrage verify --textures                     # parse every .wtd and decode every texture
openrage verify --rebuild                      # rebuild every .rpf table in memory, compare with the stored one
openrage ls pc/audio/sfx/general.rpf/general    # audio entries by name (from the game's audio configuration)
openrage textures pc/models/cdimages/vehicles.img/admiral.wtd --set name=new.dds [--remove name]   # change textures
openrage textures pc/anim/cutsprops.img/csak47.wdr [--set name=new.dds]   # textures inside a model (.wdr)
openrage model pc/models/cdimages/vehicles.img/admiral.wft --export admiral.glb --render admiral.png
openrage verify --models                       # parse every .wdr, .wdd and .wft
openrage text common/text/german.gxt -o german.txt   # text table as [TABLE] sections of 0xHASH = text
openrage text pc/data/maps/manhat/manhat01.wpl # placement file as IPL-style text
openrage text pc/data/maps/manhat/manhat01.wpl --import edited.txt   # write the edited text back (update folder)
openrage script common/data/cdimages/script.img/main.sco   # disassembly (--strings: only the strings)
openrage verify --scripts                      # disassemble every .sco, check every jump
openrage verify --text                         # every .gxt and .wpl (to text and back), and .sco
openrage map pc/data/maps/manhat/manhat03.img --render manhat03.png --export manhat03.glb   # a map archive in 3D
openrage map pc/data/maps/manhat/manhat03.img/manhat03_stream0.wpl                      # one placement file
openrage map --check                           # every placement file: how many objects have a model
openrage model pc/data/maps/manhat/manhat03.img/manhat03_1.wbn --render col.png   # collision as a mesh
openrage verify --collisions                   # every .wbn and .wbd, and every fragment's collision
openrage clips pc/anim/anim.img/move_m@generic.wad                    # the animations of a .wad
openrage model pc/models/cdimages/componentpeds.img/m_y_gmaf_hi_01.wdd --anim "pc/anim/anim.img/move_m@generic.wad:sprint@0.2" --render sprint.png
openrage verify --animations                   # every .wad, every frame of every bone track
openrage cutscene pc/anim/cuts.img/b_1.cut --models --map 40   # a cutscene: cast, outfits, the map around it
openrage verify --cutscenes                    # every .cut: sections, clips, camera and models
openrage put admiral.wtd pc/models/cdimages/vehicles.img/   # edit: GTA IV uses the update folder (needs FusionFix)
openrage put handling.dat common/data/          # loose files go to update/<same path>
openrage write-target game                     # GTA IV: write into the game files instead (originals kept)
openrage originals [--restore PATH | --restore-all] [-y]   # GTA IV: kept originals, put them back
```

## Formats: textures, models, audio, animations, metadata, text

```
openrage textures x64e.rpf/levels/gta5/vehicles.rpf/adder.ytd                 # list textures
openrage textures …/adder.ytd --export out/ [--png]                            # DDS (default) or PNG
openrage textures …/blips.ytd --set radar_darts=new.dds --remove radar_taco_van   # written into mods/
openrage textures …/prop_x.ydr [--set name=new.dds]          # a model's own textures (.ydr, .yft, .ydd)
openrage model …/vehicles.rpf/adder.yft                       # meshes, shaders, textures
openrage model …/adder.yft --export adder.glb                  # glTF with embedded textures
openrage model …/adder.yft --render adder.png [--yaw 1.2]      # software-rendered image
openrage audio x64/audio/sfx/RADIO_02_POP.rpf/alright.awc --export out/   # sounds → .wav
openrage clips x64c.rpf/anim/ingame/clip_move_.rpf/move_m@generic.ycd     # clips of a .ycd
openrage model …/a_m_y_hipster_01.ydd --render pose.png --anim "…/move_m@generic.ycd:walk@0.5"
openrage model …/csb_bogdan.ydd --render pose.png --anim "…/cuts_sil.rpf/sil_int.cut:csb_bogdan_dual@75"   # cutscene pose
openrage xml …/ufo.ymap [-o ufo.ymap.xml]      # metadata (.ymap/.ytyp/.ymt/.ymf/.cut/.pso) → XML
openrage xml …/ufo.ymap --tree                 # the readable structure, as in the GUI
openrage text …/american_rel.rpf/iap.gxt2 [-o iap.oxt]   # text table → OpenIV .oxt text
openrage particles …/ptfx.rpf/core.ypt         # effects, emitters, textures and models of a .ypt
openrage script …/script_rel.rpf/omega2.ysc [-o omega2.txt] [--strings]   # disassemble a script
openrage media …/scaleform_generic.rpf/hud.gfx [--swf hud.swf]   # Scaleform movie or Bink video facts
openrage natives natives.json                  # native names for script disassembly
openrage rdr2-keys --dump RDR2.dmp <RDR2>      # RDR2 archive keys from a memory dump of the running game
openrage rdr2-keys --file secrets.bin <RDR2>   # … or from a key file
openrage rdr2-names <RDR2>                     # find RDR2 file names in the game's files (cached)
openrage rpf8 --list <RDR2>/common_0.rpf       # RDR2 archive contents
openrage rpf8 --text <RDR2>                    # read every RDR2 text table (.yldb)
openrage rpf8 --meta <RDR2>                    # convert all RDR2 metadata (.ymap, .ytyp, .ymt, .ymf) to XML
openrage rpf8 --audio <RDR2>/x64/audio        # decode every RDR2 sound
openrage rpf8 --anims <RDR2>/anim_0.rpf       # read every RDR2 clip dictionary
openrage --game <RDR2> rdr2-ped                # list the RDR2 peds (people and animals) with their base skeletons
openrage --game <RDR2> rdr2-ped cs_johnmarston # a ped's outfits: model, textures, palette of every part (--outfit N)
openrage --game <RDR2> rdr2-ped --check        # resolve every part of every ped
openrage --game <RDR2> rdr2-ped --check --load # … and put every ped together as the viewer does
openrage --game <RDR2> model …/metapeds.rpf/cs_johnmarston.ymt --outfit 5 --render john.png   # a ped from its parts
openrage world --radius 40 -- 350 4870 -60     # map objects near a point (builds the world cache)
openrage map …/hei_dt1_03_strm_0.ymap --render map.png --export map.glb   # a map file's objects in 3D
openrage map --check                           # every map file: objects and missing models
```

## Editing, packages and the mods folder

```
openrage put new.meta x64e.rpf/data/            # import into an archive (mods folder)
openrage mv x64e.rpf/a.xml x64e.rpf/b.xml       # rename / move inside an archive
openrage rm x64e.rpf/levels/gta5/paths.rpf       # remove a file or folder
openrage mkdir x64e.rpf/my_folder
openrage replace-model x64i.rpf/…/prop_x.ydr model.obj --mods   # new geometry for an Enhanced .ydr (preview)
openrage replace-model x64i.rpf/…/prop_x.ydr model.obj --out new.ydr --shader 0
openrage replace-model x64i.rpf/…/prop_x.ydr hi.obj --lod med.obj --lod low.obj --mods   # with detail levels
openrage oiv info mod.oiv                      # what a package would change
openrage oiv install mod.oiv                   # install into the mods folder
openrage oiv list / oiv uninstall "Name 1.0" [-y]   # installed packages, uninstall
openrage history                               # changes made to the mods folder
openrage undo                                  # revert the newest change
openrage mods                                  # files in the OpenIV mods folder
openrage mods --changes [--deep]               # every added/removed/modified entry
```

## Plugins

```
openrage plugin list                           # installed plugins, their state, the plugin folders
openrage plugin info strings                   # what a plugin adds and the permissions it asks for
openrage plugin enable strings                 # enable it, granting its permissions (asks first; -y to skip)
openrage plugin disable strings
openrage plugin install obj-tools.orp          # install a plugin package (shows what it is and its SHA-256 first)
openrage plugin uninstall obj-tools [-y]       # remove it from your plugin folder
openrage plugin pack my-plugin/ --wasm target/wasm32-wasip2/release/my_plugin.wasm   # make a package
openrage plugin preview x64a.rpf/data/x.dat    # show a file the way a plugin previews it
openrage plugin preview --file x.bin --plugin strings
openrage plugin preview --file x.xyz --png x.png   # save an image preview
openrage plugin preview --file car.obj --glb car.glb   # save a 3D model preview as glTF
openrage plugin run obj-tools export x64e.rpf/levels/gta5/vehicles.rpf/adder.yft --out adder/
openrage plugin run strings export x64a.rpf/data/x.dat --out exported/
openrage plugin run strings export x64a.rpf/data --out exported/   # batch actions also take folders and archives
openrage plugin run gxt2-patch apply update/update.rpf/x64/data/lang/american_rel.rpf/global.gxt2 --pick patch.oxt
```

Actions that change game files list the changes and ask before applying them to the `mods` folder (`-y` skips the
question); `openrage undo` reverts them.

Previews and actions also work without a game set up, on files from disk (`--file`). See
[Writing plugins](@/plugins.md) for the plugin folders and how to write your own.

## About and licenses

```
openrage licenses                              # OpenRAGE's license, credits and all third-party crates
openrage licenses --text slint                 # full license text of one crate (`openrage`: the GPL, `notice`: NOTICE.md)
openrage licenses --all                        # every license text
```

## Verifying against the whole game

```
openrage verify                                # decode every entry of every archive
openrage verify --textures [--rebuild]         # decode (and rebuild) every texture dictionary
openrage verify --models                       # parse every model of the game
openrage verify --models --rebuild             # write every Enhanced .ydr anew with its meshes, read back identical
openrage verify --audio                        # decode every sound of the game
openrage verify --animations                   # parse every clip dictionary, evaluate all frames
openrage verify --meta                         # convert every metadata file of the game
openrage verify --text                         # every .gxt2/.nametable → text → back, identical
openrage verify --particles                    # parse every particle dictionary
openrage verify --collisions                   # build the meshes of every collision file
openrage verify --paths                        # read every path node file, recording and nav mesh
openrage verify --scripts                      # disassemble every script and check every jump
openrage verify --media                        # read every Scaleform movie and Bink video
```

Set up the keys first: `openrage --game "/path/to/Grand Theft Auto V" keys`, or see [Getting started](@/getting-started.md).

## In scripts

`--json` prints machine-readable results for `ls`, `search` and `history`. `ls` writes one JSON array, row by row,
so `ls -r` over a whole archive starts right away:

```
openrage --json ls -r update/update.rpf | jq -r '.[] | select(.kind == "resource") | .path'
openrage --json search adder | jq '.hits[].path'
openrage --json history
```

Commands that write files take `-o`/`--out`; for `model`, `map`, `audio` and `textures` it is the same as `--export`.

Commands that install, uninstall or put originals back ask first; `-y`/`--yes` answers for you. Without a terminal to ask on (in a script or a pipe) they stop instead of guessing.

The exit code is `0` when everything worked, `2` for wrong arguments and `1` for everything else, also when only some files could not be extracted or a question was answered with no. Messages go to standard error, results to standard output.
