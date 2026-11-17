#!/usr/bin/env python3
"""Import 4x4 Evolution 2 exclusive tracks into evox's extracted tree.

Mirror of tools/evo2_trucks.py for tracks.

Scans the unpacked Evo 2 track pods in `4x4evo2-work/pods` and copies every
exclusive track's data:
  1. Level definitions: .LVL -> assets/extracted/track/track/LEVELS/
  2. Heightmaps & terrain: .RAW (131,072 bytes), .CLR, .TEX, .SDW, .WAT -> track/track/DATA/
  3. World scenery & placements: .SIT -> track/track/WORLD/
  4. Quick Race preview art: decoded from 256x256 palette TIFFs into:
       <STEM>.RAW (65,536 bytes), <STEM>.ACT (768 bytes), <STEM>.OPA
       -> texture/ui/ART/
  5. Scenery prop models: .SMF -> evo2/MODELS/
  6. Prop & terrain textures:
       - Palette TIFFs decoded into .RAW, .ACT, .OPA -> evo2/texture/ART, evo2/palette/ART, evo2/opacity/ART
       - Indexed terrain tiles (.RAW, .ACT) -> texture/texture/ART, texture/palette/ART
  7. AI lines: CLASS1, CLASS2, CLASS3 racing lines -> data/data/AI/
  8. Course: report the `.SIT` `*** Course ***` gate count and `*** Paths ***`
     splines, the real lap geometry the game parses (`terrain::parse_course`)

Existing files win on name collisions (never overwritten).
"""
import argparse
import json
import os
import re
import struct
import sys

DEFAULT_SRC = os.path.join('4x4evo2-work', 'pods')


def listing(d):
    """Upper-cased file name -> real name, for case-insensitive lookups."""
    try:
        return {f.upper(): f for f in os.listdir(d)}
    except FileNotFoundError:
        return {}


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


def sit_course_counts(path):
    """(gate count, path count, longest path points) in a `.SIT`.

    Evo 2 stores the race course in the level's own `.SIT`: race levels list
    `*** Course ***` crossing lines (`cstart`/`cend` per gate), while the
    mission-only level packs carry `*** Paths ***` splines instead. Both are
    parsed at runtime by `terrain::parse_course`.
    """
    text = open(path, 'rb').read().decode('latin1')
    lines = [l.rstrip('\r').strip() for l in text.splitlines()]
    gates = sum(1 for l in lines if l == 'cend')
    paths = 0
    longest = 0
    cur = 0
    for l in lines:
        if l.startswith('Path '):
            longest = max(longest, cur)
            cur = 0
            paths += 1
        elif l == 'wPos':
            cur += 1
    longest = max(longest, cur)
    return gates, paths, longest


def main():
    ap = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    ap.add_argument('--src', default=DEFAULT_SRC, help='unpacked Evo 2 pods root directory')
    ap.add_argument('--evo1', default='assets/extracted', help='evox extracted Evo 1 tree')
    ap.add_argument('--out', default=None, help='output root (default: <evo1>)')
    a = ap.parse_args()

    out = a.out or a.evo1
    if not os.path.isdir(a.src):
        sys.exit(f'{a.src} not found: unpack the Evo 2 pods first')

    # Target directory structure
    dst_lvl = os.path.join(out, 'track/track/LEVELS')
    dst_data = os.path.join(out, 'track/track/DATA')
    dst_world = os.path.join(out, 'track/track/WORLD')
    dst_ui_art = os.path.join(out, 'texture/ui/ART')
    dst_tex_raw = os.path.join(out, 'texture/texture/ART')
    dst_tex_pal = os.path.join(out, 'texture/palette/ART')

    dst_evo2_mod = os.path.join(out, 'evo2/MODELS')
    dst_evo2_raw = os.path.join(out, 'evo2/texture/ART')
    dst_evo2_pal = os.path.join(out, 'evo2/palette/ART')
    dst_evo2_opa = os.path.join(out, 'evo2/opacity/ART')

    dst_ai_base = os.path.join(out, 'data/data/AI')

    for d in [
        dst_lvl, dst_data, dst_world, dst_ui_art, dst_tex_raw, dst_tex_pal,
        dst_evo2_mod, dst_evo2_raw, dst_evo2_pal, dst_evo2_opa,
        os.path.join(dst_ai_base, 'CLASS1'),
        os.path.join(dst_ai_base, 'CLASS2'),
        os.path.join(dst_ai_base, 'CLASS3'),
    ]:
        os.makedirs(d, exist_ok=True)

    # Existing Evo 1 files to preserve
    e1_lvl = listing(dst_lvl)
    e1_data = listing(dst_data)
    e1_world = listing(dst_world)
    e1_ui_art = listing(dst_ui_art)
    e1_tex_raw = listing(dst_tex_raw)
    e1_models = listing(os.path.join(out, 'model/vehicle/MODELS'))

    ui_pod_art = listing(os.path.join(a.src, 'UI/ART'))

    report = {
        'tracks': [],
        'skipped_same_file': [],
        'previews': [],
        'models': [],
        'textures': [],
        'ai_lines': [],
        'course': [],
        'missing': [],
    }

    # Discover all track pods
    pod_names = sorted(os.listdir(a.src))
    for pod in pod_names:
        pod_dir = os.path.join(a.src, pod)
        if not os.path.isdir(pod_dir):
            continue
        lvl_dir = os.path.join(pod_dir, 'LEVELS')
        if not os.path.isdir(lvl_dir):
            continue

        pod_data_dir = os.path.join(pod_dir, 'DATA')
        pod_world_dir = os.path.join(pod_dir, 'WORLD')
        pod_models_dir = os.path.join(pod_dir, 'MODELS')
        pod_art_dir = os.path.join(pod_dir, 'ART')

        pod_data = listing(pod_data_dir)
        pod_world = listing(pod_world_dir)
        pod_models = listing(pod_models_dir)
        pod_art = listing(pod_art_dir)

        # Process each .LVL in this pod
        pod_imported = False
        for lvl_name in sorted(os.listdir(lvl_dir)):
            if not lvl_name.upper().endswith('.LVL'):
                continue
            stem = lvl_name[:-4].upper()

            # Collision policy: an existing level always wins. Evo 2 ships
            # Evo 1-named tracks in its own pods, and re-importing them would
            # replace the Evo 1 extraction with the Evo 2 variant.
            if os.path.exists(os.path.join(dst_lvl, f'{stem}.LVL')):
                report['skipped_same_file'].append(f'{stem}.LVL')
                continue

            lvl_path = os.path.join(lvl_dir, lvl_name)
            lvl_bytes = open(lvl_path, 'rb').read()

            # Parse declared files from first 3 lines of .LVL
            lines = lvl_bytes.decode('latin1', errors='ignore').splitlines()
            raw_name = f'{stem}.RAW'
            clr_name = f'{stem}.CLR'
            tex_name = f'{stem}.TEX'
            for line in lines[:3]:
                line = line.strip()
                if line.upper().endswith('.RAW'):
                    raw_name = line.upper()
                elif line.upper().endswith('.CLR'):
                    clr_name = line.upper()
                elif line.upper().endswith('.TEX'):
                    tex_name = line.upper()

            # 1. Copy .LVL
            open(os.path.join(dst_lvl, f'{stem}.LVL'), 'wb').write(lvl_bytes)
            pod_imported = True

            # 2. Copy DATA files (heightmap .RAW, .CLR, .TEX, .SDW, .WAT, .RTD)
            for declared in [raw_name, clr_name, tex_name, f'{stem}.SDW', f'{stem}.WAT', f'{stem}.RTD']:
                real = pod_data.get(declared)
                if real:
                    src_f = os.path.join(pod_data_dir, real)
                    dst_f = os.path.join(dst_data, declared)
                    if declared not in e1_data:
                        open(dst_f, 'wb').write(open(src_f, 'rb').read())

            # 3. Copy .SIT world placements
            sit_real = pod_world.get(f'{stem}.SIT') or pod_world.get(f'{pod}.SIT')
            if sit_real:
                src_sit = os.path.join(pod_world_dir, sit_real)
                dst_sit = os.path.join(dst_world, f'{stem}.SIT')
                if f'{stem}.SIT' not in e1_world:
                    open(dst_sit, 'wb').write(open(src_sit, 'rb').read())
                # 3b. Course gates / AI splines: the `.SIT` is the only place
                # Evo 2 stores a course. Without either the game would fall
                # back to the pentagon placeholder circuit.
                dst_sit = os.path.join(dst_world, f'{stem}.SIT')
                gates, paths, longest = sit_course_counts(src_sit)
                course = {'gates': gates, 'paths': paths, 'longest_path': longest}
                report['course'].append({'track': stem, **course})
                if gates == 0 and longest < 8:
                    report['missing'].append(
                        f'{stem}: no course gates and no usable path spline')
            else:
                report['missing'].append(f'{stem}: missing .SIT in {pod}')

            # 4. Quick Race Preview Art (256x256 raw + act)
            # Check candidate names in pod/ART or UI/ART (prefer full color .TIF over dark overlay -DP.TIF)
            preview_written = False
            candidate_paths = []
            for cand in [f'{stem}.TIF', f'{pod}.TIF', f'{stem}-DP.TIF', f'{pod}-DP.TIF']:
                if cand in pod_art:
                    candidate_paths.append(os.path.join(pod_art_dir, pod_art[cand]))
                if cand in ui_pod_art:
                    candidate_paths.append(os.path.join(a.src, 'UI/ART', ui_pod_art[cand]))

            for cand_tif in candidate_paths:
                try:
                    w, h, idx, pal, mask = read_palette_tif(cand_tif)
                    if w == 256 and h == 256:
                        open(os.path.join(dst_ui_art, f'{stem}.RAW'), 'wb').write(idx)
                        open(os.path.join(dst_ui_art, f'{stem}.ACT'), 'wb').write(pal)
                        open(os.path.join(dst_ui_art, f'{stem}.OPA'), 'wb').write(mask)
                        report['previews'].append(stem)
                        preview_written = True
                        break
                except Exception:
                    continue

            if not preview_written:
                report['missing'].append(f'{stem}: no valid 256x256 palette preview TIFF found')

            report['tracks'].append(stem)

        # 5. Copy and convert prop models and textures from this pod. A pod
        # whose levels were all already present belongs to an existing intake
        # (Evo 1 names); its assets must not be mixed in.
        if pod_imported:
            # Models
            for smf_name, real_smf in pod_models.items():
                if not smf_name.endswith('.SMF'):
                    continue
                if smf_name in e1_models:
                    continue
                src_smf = os.path.join(pod_models_dir, real_smf)
                dst_smf = os.path.join(dst_evo2_mod, smf_name)
                open(dst_smf, 'wb').write(open(src_smf, 'rb').read())
                report['models'].append(smf_name)

                # Convert referenced textures
                for tex_stem in smf_textures(src_smf):
                    tif_name = pod_art.get(f'{tex_stem}.TIF')
                    if tif_name:
                        src_tif = os.path.join(pod_art_dir, tif_name)
                        try:
                            w, h, idx, pal, mask = read_palette_tif(src_tif)
                            open(os.path.join(dst_evo2_raw, f'{tex_stem}.RAW'), 'wb').write(idx)
                            open(os.path.join(dst_evo2_pal, f'{tex_stem}.ACT'), 'wb').write(pal)
                            open(os.path.join(dst_evo2_opa, f'{tex_stem}.OPA'), 'wb').write(mask)
                            # Also copy RAW and ACT to texture/texture/ART if not present
                            if f'{tex_stem}.RAW' not in e1_tex_raw:
                                open(os.path.join(dst_tex_raw, f'{tex_stem}.RAW'), 'wb').write(idx)
                                open(os.path.join(dst_tex_pal, f'{tex_stem}.ACT'), 'wb').write(pal)
                            report['textures'].append(tex_stem)
                        except Exception as e:
                            pass
                    # If texture is already .RAW + .ACT in pod ART
                    raw_name = pod_art.get(f'{tex_stem}.RAW')
                    act_name = pod_art.get(f'{tex_stem}.ACT')
                    if raw_name and act_name:
                        if f'{tex_stem}.RAW' not in e1_tex_raw:
                            open(os.path.join(dst_tex_raw, f'{tex_stem}.RAW'), 'wb').write(
                                open(os.path.join(pod_art_dir, raw_name), 'rb').read())
                            open(os.path.join(dst_tex_pal, f'{tex_stem}.ACT'), 'wb').write(
                                open(os.path.join(pod_art_dir, act_name), 'rb').read())
                            report['textures'].append(tex_stem)

            # Ground tile textures (.RAW + .ACT)
            for art_name, real_art in pod_art.items():
                if art_name.endswith('.RAW'):
                    base_stem = art_name[:-4]
                    act_real = pod_art.get(f'{base_stem}.ACT')
                    if act_real and art_name not in e1_tex_raw:
                        src_raw = os.path.join(pod_art_dir, real_art)
                        src_act = os.path.join(pod_art_dir, act_real)
                        open(os.path.join(dst_tex_raw, art_name), 'wb').write(open(src_raw, 'rb').read())
                        open(os.path.join(dst_tex_pal, f'{base_stem}.ACT'), 'wb').write(open(src_act, 'rb').read())
                        open(os.path.join(dst_evo2_raw, art_name), 'wb').write(open(src_raw, 'rb').read())
                        open(os.path.join(dst_evo2_pal, f'{base_stem}.ACT'), 'wb').write(open(src_act, 'rb').read())

    # 6. Copy AI racing lines from AI/AI/
    ai_src = os.path.join(a.src, 'AI/AI')
    if os.path.isdir(ai_src):
        for cls in ['CLASS1', 'CLASS2', 'CLASS3']:
            cls_src = os.path.join(ai_src, cls)
            cls_dst = os.path.join(dst_ai_base, cls)
            if not os.path.isdir(cls_src):
                continue
            for ai_f in os.listdir(cls_src):
                if not ai_f.upper().endswith('.TXT'):
                    continue
                # Skip if already exists in Evo 1
                dst_path = os.path.join(cls_dst, ai_f.upper())
                if not os.path.exists(dst_path):
                    open(dst_path, 'wb').write(open(os.path.join(cls_src, ai_f), 'rb').read())
                    report['ai_lines'].append(f'{cls}/{ai_f.upper()}')

    # Deduplicate report lists
    report['models'] = sorted(set(report['models']))
    report['textures'] = sorted(set(report['textures']))
    report['ai_lines'] = sorted(set(report['ai_lines']))
    report['course'].sort(key=lambda c: c['track'])

    json_path = os.path.join(out, 'evo2', 'evo2_tracks.json')
    os.makedirs(os.path.dirname(json_path), exist_ok=True)
    json.dump(report, open(json_path, 'w'), indent=1)

    print(
        f"tracks {len(report['tracks'])}, "
        f"previews {len(report['previews'])}, "
        f"models {len(report['models'])}, "
        f"textures {len(report['textures'])}, "
        f"ai_lines {len(report['ai_lines'])}, "
        f"course {sum(1 for c in report['course'] if c['gates'] or c['longest_path'] >= 8)}/{len(report['course'])}, "
        f"skipped (existing level wins) {len(report['skipped_same_file'])}, "
        f"missing {len(report['missing'])}"
    )
    for m in report['missing']:
        print('  missing:', m)

    return 1 if report['missing'] else 0


if __name__ == '__main__':
    sys.exit(main())
