+++
title = "Command line"
description = "The openrage command-line tool: browse, search and extract archives, convert formats, edit archives, install .oiv packages and verify the whole game."

[extra]
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
openrage verify                                # read every entry of every .img and .rpf
```

## Formats: textures, models, audio, animations, metadata, text

```
openrage textures x64e.rpf/levels/gta5/vehicles.rpf/adder.ytd                 # list textures
openrage textures …/adder.ytd --export out/ [--png]                            # DDS (default) or PNG
openrage textures …/blips.ytd --set radar_darts=new.dds --remove radar_taco_van   # written into mods/
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
openrage oiv info mod.oiv                      # what a package would change
openrage oiv install mod.oiv                   # install into the mods folder
openrage oiv list / oiv uninstall "Name 1.0"   # installed packages, uninstall
openrage history                               # changes made to the mods folder
openrage undo                                  # revert the newest change
openrage mods                                  # files in the OpenIV mods folder
openrage mods --changes [--deep]               # every added/removed/modified entry
```

## Verifying against the whole game

```
openrage verify                                # decode every entry of every archive
openrage verify --textures [--rebuild]         # decode (and rebuild) every texture dictionary
openrage verify --models                       # parse every model of the game
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
