//! `--self-test` diagnostics. Level 0 is the fast gate used by CI; higher
//! levels add asset-dependent checks. Exit code is the number of failures.

use std::path::Path;

fn check(name: &str, ok: bool, detail: &str) -> bool {
    println!(
        "  [{}] {name}{}",
        if ok { "ok" } else { "FAIL" },
        if detail.is_empty() {
            String::new()
        } else {
            format!(" - {detail}")
        }
    );
    ok
}

fn asset_root() -> &'static str {
    "assets/extracted"
}

pub fn run(level: u32) -> i32 {
    println!("EvoX self-test level {level}");
    let mut fails = 0;

    let phys = crate::physics::self_test();
    fails += !check(
        "physics settles and brakes",
        phys.is_ok(),
        &phys.err().unwrap_or_default(),
    ) as i32;

    // Catalog parses with no errors.
    let parts = Path::new(asset_root()).join("data/ui/PARTS/PARTS.TXT");
    if parts.exists() {
        let text = std::fs::read(&parts)
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .unwrap_or_default();
        let (cat, errors) = crate::parts::parse_catalog(&text);
        fails += !check(
            "parts catalog parses",
            errors.is_empty() && cat.parts.len() > 80,
            &format!("{} parts, {} errors", cat.parts.len(), errors.len()),
        ) as i32;
        let types = cat
            .parts
            .iter()
            .map(|p| p.kind)
            .collect::<std::collections::HashSet<_>>();
        fails += !check(
            "all nine part types present",
            types.len() == 9,
            &format!("{} types", types.len()),
        ) as i32;
        let with_prereq = cat.parts.iter().filter(|p| p.prereq.is_some()).count();
        fails += !check(
            "prerequisite expressions load",
            with_prereq >= 20,
            &format!("{with_prereq} gated"),
        ) as i32;
    } else {
        println!("  [--] parts catalog missing (run tools/pod.py)");
    }

    // Boot cards, gauge needle, LCD digits and the interface font.
    let ui_art = Path::new(asset_root()).join("texture/ui/ART");
    if ui_art.exists() {
        fails += !check(
            "boot, gauge and font art present",
            ["TRILOGO", "LOGO256", "LOGO480", "NEEDLE", "ODOM", "4X4FONT"]
                .iter()
                .all(|n| ui_art.join(format!("{n}.RAW")).exists()),
            "splash cards, needle, odometer, font sheet",
        ) as i32;
    } else {
        // Source-only checkout (CI): reported like the other absent trees,
        // not counted as a failure. A partial extraction still fails above.
        println!("  [--] interface art missing (run tools/pod.py)");
    }

    // Vehicle definitions.
    let trucks = Path::new(asset_root()).join("model/vehicle/TRUCK");
    if trucks.exists() {
        let (parsed, errors) = crate::vehicle::load_dir(&trucks);
        fails += !check(
            "vehicle definitions parse",
            parsed.len() > 50 && errors.len() < parsed.len() / 20,
            &format!("{} ok, {} errors", parsed.len(), errors.len()),
        ) as i32;
        let missing_mesh: Vec<&str> = parsed
            .iter()
            .filter(|t| {
                !Path::new(asset_root())
                    .join("model/vehicle/MODELS")
                    .join(format!("{}.SMF", t.model_base.to_ascii_uppercase()))
                    .exists()
            })
            .map(|t| t.model_base.as_str())
            .take(3)
            .collect();
        fails += !check(
            "every truck body has a retail mesh",
            missing_mesh.is_empty(),
            &format!("{missing_mesh:?}"),
        ) as i32;

        if let Some(t) = parsed.first() {
            fails += !check(
                "truck parameters are sane",
                t.mass_kg() > 500.0 && t.gear_ratios.len() > 4,
                &format!("{} {} kg", t.name, t.mass_kg() as i32),
            ) as i32;
        }
    } else {
        println!("  [--] vehicles missing");
    }

    // Renderable assets: text meshes and their palette textures.
    let models = Path::new(asset_root()).join("model/vehicle/MODELS");
    if models.exists() {
        let body = std::fs::read_to_string(models.join("BLAZERZR2.SMF"))
            .ok()
            .and_then(|t| crate::smf::Smf::parse(&t).ok());
        let (ok, detail) = match &body {
            Some(s) => {
                let (lo, hi) = s.bounds();
                let len = hi.z - lo.z;
                (
                    s.meshes.len() > 30 && (4.0..6.0).contains(&len),
                    format!("{} meshes, {len:.2} m", s.meshes.len()),
                )
            }
            None => (false, "BLAZZR2.SMF failed to parse".to_string()),
        };
        fails += !check("truck SMF mesh parses in metres", ok, &detail) as i32;

        let wheel = std::fs::read_to_string(models.join("33X12_12L.SMF"))
            .ok()
            .and_then(|t| crate::smf::Smf::parse(&t).ok());
        fails += !check(
            "wheel SMF mesh parses",
            wheel
                .as_ref()
                .map(|w| !w.meshes.is_empty())
                .unwrap_or(false),
            "",
        ) as i32;

        let tex = Path::new(asset_root()).join("texture/texture/ART/AA3CHECK.RAW");
        let act = Path::new(asset_root()).join("texture/palette/ART/AA3CHECK.ACT");
        let opa = Path::new(asset_root()).join("texture/opacity/ART/AA3CHECK.OPA");
        fails += !check(
            "vehicle albedo + palette + opacity bind",
            tex.exists() && act.exists() && opa.exists(),
            &format!(
                "{} bytes",
                std::fs::metadata(&tex).map(|m| m.len()).unwrap_or(0)
            ),
        ) as i32;
    } else {
        println!("  [--] vehicle models missing");
    }

    // Tracks load with valid sized heightmaps.
    let tracks_dir = Path::new(asset_root()).join("track/track");
    if tracks_dir.exists() {
        let mut good = 0;
        let mut bad = Vec::new();
        if let Ok(rd) = std::fs::read_dir(tracks_dir.join("DATA")) {
            for p in rd.filter_map(|e| e.ok()).map(|e| e.path()) {
                if p.extension().map(|x| x.eq_ignore_ascii_case("rtd")) != Some(true) {
                    continue;
                }
                let Some(stem) = p.file_stem() else { continue };
                let id = stem.to_string_lossy().to_string();
                match crate::terrain::Track::load(&id, &tracks_dir) {
                    Ok(t) if t.heights.len() == crate::terrain::GRID * crate::terrain::GRID => {
                        good += 1
                    }
                    Ok(_) => bad.push(id),
                    Err(e) => bad.push(format!("{id}: {e}")),
                }
            }
        }
        if tracks_dir.join("DATA/ASPEN.RTD").exists() {
            match crate::terrain::Track::load("ASPEN", &tracks_dir) {
                Ok(t) => {
                    fails += !check(
                        "ASPEN heightmap loads above water",
                        t.heights.iter().any(|h| *h > 0)
                            && t.height_at(100.0, 100.0) > t.water_height,
                        &format!(
                            "{:.1} m at origin, water {:.1} m",
                            t.height_at(100.0, 100.0),
                            t.water_height
                        ),
                    ) as i32
                }
                Err(e) => fails += !check("ASPEN loads", false, &e) as i32,
            }
        }
        fails += !check(
            "tracks parse",
            good >= 12 && bad.len() <= 2,
            &format!("{good} ok, {bad:?}"),
        ) as i32;

        if let Ok(t) = crate::terrain::Track::load("ZONA", &tracks_dir) {
            let art = Path::new(asset_root()).join("texture/texture/ART");
            let bound = t.textures.iter().find(|r| {
                art.join(format!(
                    "{}.RAW",
                    r.file.trim_end_matches(".RAW").to_ascii_uppercase()
                ))
                .exists()
            });
            fails += !check(
                "track texture list resolves to art",
                bound.is_some(),
                &format!("{} entries", t.textures.len()),
            ) as i32;

            let colliders = crate::collide::build(&t);
            let rmin = colliders
                .iter()
                .map(|c| c.radius)
                .fold(f32::INFINITY, f32::min);
            let rmax = colliders.iter().map(|c| c.radius).fold(0.0f32, f32::max);
            let hmin = colliders
                .iter()
                .map(|c| c.half_height)
                .fold(f32::INFINITY, f32::min);
            fails += !check(
                "prop colliders cover the scenery",
                colliders.len() > 30
                    && colliders
                        .iter()
                        .all(|c| c.radius >= crate::collide::MIN_POST_RADIUS
                            && c.radius <= crate::collide::MAX_RADIUS
                            && c.half_height > 0.0),
                &format!(
                    "{} colliders from {} props, radius {rmin:.2}..{rmax:.2}, half height min {hmin:.2}",
                    colliders.len(),
                    t.props.len()
                ),
            ) as i32;
        }
    } else {
        println!("  [--] tracks missing");
    }

    // Audio bank decodes.
    if level >= 1 {
        let wav = Path::new(asset_root()).join("audio/sfx/SOUND/ENG_STOCK.WAV");
        fails += !check("engine loop present", wav.exists(), "") as i32;
        let ai_dir = Path::new(asset_root()).join("data/data/AI");
        let mut lines = 0;
        if let Ok(rd) = std::fs::read_dir(&ai_dir) {
            for cls in rd
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_dir())
            {
                if let Ok(files) = std::fs::read_dir(&cls) {
                    for f in files.filter_map(|e| e.ok()).map(|e| e.path()) {
                        if let Ok(text) = std::fs::read_to_string(&f) {
                            if crate::ai::RacingLine::parse(&text).is_ok() {
                                lines += 1;
                            }
                        }
                    }
                }
            }
        }
        fails += !check(
            "AI racing lines parse",
            lines > 100,
            &format!("{lines} lines"),
        ) as i32;
    }

    // Packaging invariants.
    for required in [
        "Cargo.toml",
        "tools/pod.py",
        "docs/ARCHITECTURE.md",
        "docs/PORT_REFERENCE.md",
        "AGENTS.md",
        "CLAUDE.md",
        "game/src/selftest_truck.trk",
    ] {
        fails += !check(
            &format!("{required} exists"),
            Path::new(required).exists(),
            "",
        ) as i32;
    }

    println!("{}", if fails == 0 { "PASS" } else { "FAIL" });
    fails
}
