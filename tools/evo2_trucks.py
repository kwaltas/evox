#!/usr/bin/env python3
"""Import the 4x4 Evolution 2 truck roster into evox's extracted tree.

The Evo 1 data evox ships with (`assets/extracted`) has no Jeep: `4x4.exe` only
lists "Jeep" among manufacturer names. The Jeeps (Wrangler Sport, the yellow
ARB Wrangler, Grand Cherokee, Liberty) and 22 other vehicles exist only in the
Evo 2 `TRUCK.POD`. This tool copies the Evo 2-only `.TRK` files, every model
they reference, and converts the referenced textures into the triplet evox
already reads:

    <STEM>.RAW  palette indices, w*h bytes          -> evo2/texture/ART
    <STEM>.ACT  256 x RGB palette, 768 bytes        -> evo2/palette/ART
    <STEM>.OPA  paint mask / alpha, w*h bytes       -> evo2/opacity/ART

Evo 2 body textures are uncompressed palette TIFFs with two samples per pixel
(index + mask) that Pillow cannot open, so they are parsed directly. The mask
is the same channel Evo 1 ships as `.OPA`: 255 = body paint recoloured by
`colorList[]`, <=128 = kept (and drawn with that alpha).

Evo 1 files win on name collisions (same-named Evo 2 `.TRK` files point at
remodelled bodies and would duplicate roster entries); collisions are listed.

    python3 tools/evo2_trucks.py [--src EVO2_TRUCK_POD_DIR] [--evo1 assets/extracted]
"""
import argparse
import json
import os
import re
import struct
import sys

DEFAULT_SRC = os.path.join('4x4evo2-work', 'pods', 'TRUCK')
TIRE_LODS = ('08', '12', '16')


def listing(d):
    """Upper-cased file name -> real name, for case-insensitive lookups."""
    try:
        return {f.upper(): f for f in os.listdir(d)}
    except FileNotFoundError:
        return {}


def read_trk(path):
    lines = open(path, 'rb').read().decode('latin1').replace('\r', '').split('\n')
    get = lambda k: lines[lines.index(k) + 1].strip() if k in lines else ''
    return {
        'name': get('truckName'),
        'body': get('truckModelBaseName').upper(),
        'tire': get('tireModelBaseName').upper(),
    }


def read_palette_tif(path):
    """(width, height, indices, palette768, mask) of an uncompressed palette
    TIFF with one or two samples per pixel."""
    d = open(path, 'rb').read()
    if d[:4] != b'II*\x00':
        raise ValueError(f'{path}: not a little-endian TIFF')
    off = struct.unpack('<I', d[4:8])[0]
    n = struct.unpack('<H', d[off:off + 2])[0]
    tags = {}
    for k in range(n):
        tag, typ, cnt, val = struct.unpack('<HHII', d[off + 2 + 12 * k: off + 14 + 12 * k])
        size = {1: 1, 2: 1, 3: 2, 4: 4}.get(typ, 4)
        raw = d[off + 10 + 12 * k: off + 14 + 12 * k] if size * cnt <= 4 else d[val: val + size * cnt]
        fmt = {1: 'B', 2: 'B', 3: 'H', 4: 'I'}.get(typ, 'I')
        tags[tag] = list(struct.unpack('<%d%s' % (cnt, fmt), raw[:size * cnt]))
    w, h = tags[256][0], tags[257][0]
    spp = tags.get(277, [1])[0]
    if tags.get(259, [1])[0] != 1 or tags.get(262, [0])[0] != 3 or 320 not in tags:
        raise ValueError(f'{path}: not an uncompressed palette TIFF')
    data = b''.join(d[o:o + c] for o, c in zip(tags[273], tags[279]))
    if len(data) < w * h * spp:
        raise ValueError(f'{path}: truncated pixel data')
    cm = tags[320]
    pal = bytearray()
    for i in range(256):
        pal += bytes((cm[i] >> 8, cm[256 + i] >> 8, cm[512 + i] >> 8))
    idx = data[0:w * h * spp:spp]
    mask = data[1:w * h * spp:spp] if spp >= 2 else b'\xff' * (w * h)
    return w, h, bytes(idx), bytes(pal), bytes(mask)


def smf_textures(path):
    """Diffuse texture stems referenced by an SMF (bump maps excluded)."""
    text = open(path, 'rb').read().decode('latin1')
    names = set()
    for line in text.replace('\r', '').split('\n'):
        line = line.strip()
        if line.startswith('"'):
            continue  # Evo 2 bump-map record
        m = re.search(r',([A-Za-z0-9_\-]+)\.(TIF|RAW)$', line, re.I)
        if m:
            names.add(m.group(1).upper())
    return sorted(names)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--src', default=DEFAULT_SRC, help='unpacked Evo 2 TRUCK.POD (ART/, MODELS/, TRUCK/)')
    ap.add_argument('--evo1', default='assets/extracted', help='evox extracted Evo 1 tree')
    ap.add_argument('--out', default=None, help='output root (default: <evo1>/evo2)')
    a = ap.parse_args()
    out = a.out or os.path.join(a.evo1, 'evo2')
    src_trk, src_mod, src_art = (os.path.join(a.src, d) for d in ('TRUCK', 'MODELS', 'ART'))
    if not os.path.isdir(src_trk):
        sys.exit(f'{src_trk} not found: unpack the Evo 2 TRUCK.POD first')

    e1_trk = listing(os.path.join(a.evo1, 'model/vehicle/TRUCK'))
    e1_mod = listing(os.path.join(a.evo1, 'model/vehicle/MODELS'))
    e1_art = listing(os.path.join(a.evo1, 'texture/texture/ART'))
    s_trk, s_mod, s_art = listing(src_trk), listing(src_mod), listing(src_art)
    dirs = {k: os.path.join(out, v) for k, v in {
        'trk': 'TRUCK', 'mod': 'MODELS', 'raw': 'texture/ART', 'act': 'palette/ART', 'opa': 'opacity/ART'}.items()}
    for d in dirs.values():
        os.makedirs(d, exist_ok=True)

    report = {'trucks': [], 'skipped_same_file': [], 'models': [], 'textures': [],
              'texture_collisions': [], 'missing': []}
    models = set()
    for up, real in sorted(s_trk.items()):
        if not up.endswith('.TRK'):
            continue
        if up in e1_trk:
            report['skipped_same_file'].append(up)
            continue
        info = read_trk(os.path.join(src_trk, real))
        data = open(os.path.join(src_trk, real), 'rb').read()
        open(os.path.join(dirs['trk'], up), 'wb').write(data)
        report['trucks'].append({'file': up, **info})
        models.update({info['body'] + '.SMF', info['body'] + '0.SMF'})
        models.update(f"{info['tire']}{lod}{side}.SMF" for lod in TIRE_LODS for side in 'LR')

    stems = set()
    for m in sorted(models):
        if m in e1_mod:
            continue  # Evo 1 already ships this mesh
        if m not in s_mod:
            report['missing'].append(m)
            continue
        src = os.path.join(src_mod, s_mod[m])
        open(os.path.join(dirs['mod'], m), 'wb').write(open(src, 'rb').read())
        report['models'].append(m)
        stems.update(smf_textures(src))

    for st in sorted(stems):
        tif = s_art.get(st + '.TIF')
        if tif is None:
            report['missing'].append(st + '.TIF')
            continue
        if st + '.RAW' in e1_art:
            report['texture_collisions'].append(st)
        w, h, idx, pal, mask = read_palette_tif(os.path.join(src_art, tif))
        if w != h:
            report['missing'].append(f'{st}: non-square {w}x{h}')
            continue
        open(os.path.join(dirs['raw'], st + '.RAW'), 'wb').write(idx)
        open(os.path.join(dirs['act'], st + '.ACT'), 'wb').write(pal)
        open(os.path.join(dirs['opa'], st + '.OPA'), 'wb').write(mask)
        report['textures'].append(st)

    json.dump(report, open(os.path.join(out, 'evo2_trucks.json'), 'w'), indent=1)
    print(f"trucks {len(report['trucks'])}, models {len(report['models'])}, "
          f"textures {len(report['textures'])}, skipped (Evo 1 has the file) {len(report['skipped_same_file'])}, "
          f"texture name collisions {len(report['texture_collisions'])}, missing {len(report['missing'])}")
    for m in report['missing']:
        print('  missing:', m)
    for c in report['texture_collisions']:
        print('  collision:', c)
    return 1 if report['missing'] else 0


if __name__ == '__main__':
    sys.exit(main())
