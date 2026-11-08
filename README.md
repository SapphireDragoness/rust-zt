# rust-zt

A clean-room engine, written in Rust, that runs the 2001 zoo tycoon game
(Complete Collection: base game, Dinosaur Digs, Marine Mania) natively on
modern Windows and Linux. It is data-driven: it reads the game files from your
own copy. No game data is included in this repository.

Behaviour is rebuilt from the data files (`.ai`, `.cfg`, `.scn`, `.lyt`, …), the
manuals, and comparison with the original game. The original executable is
not used.

Made with the help of Claude Opus 5.5.

## Layout

| Crate | Purpose |
|---|---|
| `crates/zt-vfs` | Layered, case-insensitive resource filesystem over `.ztd` zips; game-folder detection |
| `crates/zt-formats` | Parsers: INI dialect, palettes, sprites, animations, PE string tables |
| `crates/zt-render` | wgpu renderer: isometric camera, terrain mesh + blending shader, headless capture |
| `apps/zt-tool` | CLI for inspecting and verifying data |
| `apps/rust-zt` | The viewer (milestone 1: terrain, scenery, fences, paths, buildings, animals and guests of any map or save, all four rotations) |

## Usage

```sh
export RUST_ZT_DATA="/path/to/Zoo Tycoon Complete Collection"   # disc copy or install folder
cargo run --release -p zt-tool -- layers     # detected layers
cargo run --release -p zt-tool -- check      # parse + round-trip every file
cargo run --release -p zt-tool -- png animals/zebra/m/walk/walk.ani /tmp/zebra
cargo run --release -p zt-tool -- zoo        # parse every map
cargo run --release -p rust-zt -- nile       # open the viewer
cargo run --release -p rust-zt -- path/to/save.zoo   # saved games work too
cargo run --release -p rust-zt -- nile --screenshot nile.png --zoom 0.25 --rotation 1 --center 60,60
cargo run --release -p zt-tool -- zoo -v maps/scn01.zoo  # list every object record
cargo test                                   # data-backed tests run when RUST_ZT_DATA is set
```

Viewer controls: arrows/WASD scroll, right/middle-drag pan, wheel zoom, Q/E
rotate, PageUp/PageDown cycle maps, F11 fullscreen, Esc quit.

```
```

## Data layering

Layers are mounted lowest to highest priority; later layers override earlier
ones. Installed copies follow `zoo.ini`'s `[resource] path=` list. For the disc
copy the same order is used: base (+loc) → ZUPDATE → XPACK1 (Dinosaur Digs,
+loc) → ZUPDATE1 → DUPDATE → XPACK2 (Marine Mania, +loc) → CONTENT/UPDATES.
This matches the Complete Collection's `zoo.ini` (`./dlupdate;./updates;
./xpack2/loc;./xpack2;./dupdate;./zupdate1;./xpack1/loc;./xpack1;./zupdate;./loc;.`).
Inside a folder, `.ztd` files are mounted alphabetically (unverified).

## Format notes

* **Sprites**: see `crates/zt-formats/src/sprite.rs`. Marine Mania adds *mask*
  frames (bit 15 of the height set): runs carry no pixel bytes.
* **.lle** (`ztatb/<type>/<8 hex>/<8 hex>/<shape><variant>.lle`): a single
  sprite frame without size prefix, palette in the sibling `.pal`. These are
  pre-rendered terrain blends (a partial cache: ~30% of mixed tiles in a
  shipped map have an entry). The first 8-hex directory holds the orthogonal
  neighbours' types, the second the diagonal ones.
* **Terrain rendering** is calibrated against those tiles: per-vertex blending
  (each tile corner mixes the four tiles meeting there; weight exponent 0.4),
  textures spanning 3 tiles, and no slope shading (cached slopes are as bright
  as flat ground, ≈0.97× the texture). Height discontinuities (water banks,
  tank pits) are vertical walls in a dark earth texture that
  matches `terrain/aphalt.tga`: ≈0.72× on screen-left-facing walls, ≈1.2× on
  screen-right-facing ones (measured on a river drop and through tank water).
  The map's outer sides get no walls: the original shows a black void.
* **.zoo**: see `crates/zt-formats/src/zoo.rs`. Header, exhibits, the
  terrain grid and object positions are decoded (x/y in 1/64 tile, z in 1/16
  elevation step, facing 0–7, id, name). The class-specific data after each
  record's name (fence health, animal needs, behaviour state …) is not yet.
  Marine Mania exhibits (versions h–j) carry the gate and, for tanks, wall
  height, water level, salinity and purity; show tanks add a variable-length
  show block. Tank walls are `tankwall` records placed at the pit floor.
* **Tanks** (`apps/rust-zt/src/water.rs`): a tank is a sunk pit (`tanks.cfg`)
  enclosed by tank walls; the interior is a flood fill from the exhibit tile.
  Walls stack `[f/Animations]` pieces (bottom, mid…, top; `l`/`r`/`lr` post
  variants at line ends; `tb*` for one-step walls). Glass walls have empty mid
  pieces. Water is drawn 50/50 translucent at ≈93% brightness (fitted to
  screenshots): a `top` sprite over each floor tile and a `front`/`right` face
  per wall per step up to the water level, from `water/<salt|fresh><scum>`
  (scum `ls`/`hs`/`es` at purity 60/20/1). Above the water, glass panes from
  `water/glass*`. Walls, faces and floor tints take their depth from their
  plane, so terrain clips them exactly. Gates: the `platform` (with the
  handrails) lies on the rim; `ladrin` hangs down the inside and, fitted to
  the original in rotation 0, also down the outside (0.3 below the rim, 0.1
  tile along the wall) when the rim is above the ground. Swimmers at the surface show
  `surfswim` over a tinted `undersurfswim`; below it, their tinted animation.
* **Water effects**: `waterwv1` waves pop up now and then on terrain water;
  tank surfaces get `twaterwv`/`frshwav` waves and `edgrip` ripples along the
  walls. Their frequency matches the original by eye (user-checked).
* **Object graphics**: scenery `objects/<type>/idle/idle.ani` (NE, SE, SW, NW),
  fences `fences/<type>/<f|g>/idle/idle.ani`, paths `paths/<type>/idle` (20
  variants: 1–4 slopes, 5–20 = 5 + 8·UL + 4·UR + 2·LR + LL connections),
  animals `animals/<type>/<m|y>/<anim>/` with N…S views (west mirrored;
  females use the `m` graphics).
