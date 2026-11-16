# Modding

Two supported routes, both landing in `assets/mods/`:

| Content | Folder | Format |
|---|---|---|
| Vehicles | `assets/mods/vehicles/` | `.TRK` (or a `.POD` with `TRUCK\*.TRK`) |
| Tracks | `assets/mods/tracks/` | `.LTE`/`.POD` track set |

The watcher polls every two seconds, validates, and adds vehicles to the garage
without a restart. Tracks appear in Quick Race on the next list rebuild. Bad
files are reported in the Mods screen instead of crashing.

## Vehicle definition (TRK)

A vehicle is a flat key/value document. See `docs/PHYSICS.md` for every field.
Minimum viable model:

```
version
6
truckName
My Truck
truckClass
Standard Truck
truckCost
18000
truckLength
15.0
truckWidth
6.0
truckHeight
6.0
truckWheelbase
9.0
dry_weight
3400.0
faxle.rtire.static_bpos
+2.5,-0.5,+5.4
...
```

Coordinates are feet, mass pounds, torque ft-lb, angles radians. Keys ending
in `[]` are repeatable lists. `stockPartList[]` names entries from
`PARTS.TXT` by `Code`.

## Meshes (SMF)

Collision cylinders are generated from the model bounds at load time, so keep
trunks, poles and walls as upright geometry; wide flat boards become solid
discs up to 2.2 m across.

`tools/blender/io_pod.py` installs an exporter (`File > Export > EvoX Model`).
It writes `C3DModel` text and an optional TRK stub. Keep meshes under 60k
triangles per object; indices are `u16`. Texture names are bare `.RAW` stems
and must exist in the vehicle's ART folder.

## Packing

```bash
python3 tools/podpack.py my_vehicle/ MYVEHICLE.POD --compress
```

`podpack` sorts entries and round-trips through `tools/pod.py`; CI includes a
round-trip test.

## Track set

A track needs, under one POD:

```
texture\texture\DATA\<ID>.RAW   256×256 u16 heightmap (1/32 ft units, 32 ft cells)
DATA\<ID>.RTD   route/course grid (not height)
DATA\<ID>.CLR   256×256 u16 texture index
DATA\<ID>.SDW   256×256 u16 shadow
DATA\<ID>.TEX   material → texture map
LEVELS\<ID>.LVL header (spawn, water, detail texture, light)
WORLD\<ID>.SIT  prop placement
ART\*.RAW+ACT   textures
data\AI\*.TXT   racing lines (optional but required by the current AI)
```

The `.LVL` parser reads `!waterHeight`, `#detailTexture`, `$lightSourceVector`
and `%vegTexture`. DTX/`TEX` lines are `index,blend,file`.

## Racing lines

Interface text uses `4X4FONT.RAW` with its `.OPA` mask: a packed strip of
glyphs in code order from `!` (33), 20 px rows. `ODOM.RAW` holds LCD digits
(one row per digit); `NEEDLE.RAW` is a colour-keyed gauge pointer. Keep new
interface art in `texture/ui/ART` and, if the font is replaced, keep the
code-order packing.

`AI_<class><track>.TXT` starts with an eight-line header (`Driver Name`,
`Class`, `Checkpoint Count`, `Point Count`) followed by `x,y,z` / speed / flag
triples in feet and km/h, sharing world space with the heightmap. Node headings
are derived from the recorded path, so keep the points dense (2–15 ft apart).
Use a clean lap; the loader rejects lines shorter than eight points.

## Compatibility promise

The POD reader accepts compressed and uncompressed entries, both `POD\0` and
`POD2` revisions, and refuses traversal names. If a tool from the 2000s writes
a variant that fails, send the header bytes — the parser reports precise
offsets rather than guessing.
