//! Vehicle definitions (`*.TRK`) from TRUCK.POD.
//!
//! The files are ordered key/value text. Keys may repeat (`sc[].pt`,
//! `stockPartList[]`, `colorList[]`), so values are stored as lists. Units are
//! the original engine's: feet for geometry, pounds for mass, ft-lb for torque.
//! `si()` converts once, at load, so simulation code is metric.

use std::collections::HashMap;

use macroquad::prelude::Vec3;

pub const FT: f32 = 0.3048;
/// Weight of each axle added to `dry_weight` by the retail set-up (lb).
pub const AXLE_WEIGHT_LB: f32 = 500.0;
pub const INCH_M: f32 = 0.0254;
pub const LB: f32 = 0.453_592_37;
pub const FT_LB: f32 = 1.355_817_95;

#[derive(Clone, Debug, Default)]
pub struct Axle {
    pub front: bool,
    pub tires: [Tire; 2],
    pub spring_rate: f32,
    pub max_compression: f32,
    pub max_angle: f32,
    pub torque_pct: f32,
    pub bias: f32,
    pub slip_diff: u8,
}

#[derive(Clone, Debug, Default)]
pub struct Tire {
    pub static_bpos: Vec3,
    pub spring_arm: f32,
    pub cf: f32,
}

#[derive(Clone, Debug)]
pub struct Engine {
    pub max_hp: f32,
    pub max_hp_rpm: f32,
    pub max_torque: f32,
    pub max_torque_rpm: f32,
    pub redline: f32,
    pub redline_timer: f32,
    pub torque_table: Vec<f32>,
    pub upshift_rpm: f32,
    pub downshift_rpm: f32,
    pub friction_cf: f32,
    pub displacement_l: f32,
    pub aspiration: u8,
}

impl Default for Engine {
    fn default() -> Self {
        Self {
            max_hp: 200.0,
            max_hp_rpm: 5000.0,
            max_torque: 235.0,
            max_torque_rpm: 3000.0,
            redline: 5500.0,
            redline_timer: 30.0,
            torque_table: vec![200.0; 13],
            upshift_rpm: 4500.0,
            downshift_rpm: 2000.0,
            friction_cf: 0.0,
            displacement_l: 3.5,
            aspiration: 0,
        }
    }
}

impl Engine {
    /// Torque in Nm at `rpm`, sampled from the original curve and scaled to
    /// the stated peak. The curve is normalized, so peak placement is kept
    /// while the absolute magnitude comes from `max_torque`.
    pub fn torque_nm(&self, rpm: f32) -> f32 {
        if self.torque_table.len() < 2 {
            return self.max_torque * FT_LB;
        }
        let peak = self
            .torque_table
            .iter()
            .cloned()
            .fold(0.0_f32, f32::max)
            .max(1.0);
        let n = self.torque_table.len() - 1;
        let t = (rpm / self.redline.max(1.0)).clamp(0.0, 1.0) * n as f32;
        let i = t.floor() as usize;
        let frac = t - i as f32;
        let a = self.torque_table[i.min(n)] / peak;
        let b = self.torque_table[(i + 1).min(n)] / peak;
        let curve = a + (b - a) * frac;
        self.max_torque * FT_LB * curve.max(0.0)
    }
}

#[derive(Clone, Debug)]
pub struct Truck {
    pub file: String,
    pub name: String,
    pub make: String,
    pub model: String,
    pub class_name: String,
    pub class_id: i64,
    pub quick_class: i64,
    pub cost: i64,
    pub model_year: i32,
    pub length: f32,
    pub width: f32,
    pub height: f32,
    pub wheelbase: f32,
    pub front_track: f32,
    pub rear_track: f32,
    pub acceleration: f32,
    pub top_speed: f32,
    pub handling: f32,
    pub model_base: String,
    pub tire_model_base: String,
    pub dry_weight_lb: f32,
    pub inertia: Vec3,
    pub ref_area: Vec3,
    pub cl: f32,
    pub cd: f32,
    pub cy: f32,
    pub drive_type: u8,
    pub gear_ratios: Vec<f32>,
    pub gear_inertia: Vec<f32>,
    pub final_drive: f32,
    pub xfr_low_ratio: f32,
    pub engine: Engine,
    pub axles: [Axle; 2],
    pub stock_parts: Vec<String>,
    /// Factory paints (`colorList[]`): HSV adjustments of the body texture,
    /// see `paint.rs`. Empty for fixed liveries (e.g. the ARB Wrangler).
    pub paints: Vec<crate::paint::Paint>,
    /// Selected entry of `paints` (the retail `colorIndex`, `+0x2074`).
    pub paint_index: usize,
    /// Which game's data this truck came from (1 = 4x4 Evo, 2 = Evo 2).
    pub game: u8,
    /// Tyre radius measured from the truck's own tyre model
    /// (`<tireModelBaseName>16L.SMF`), metres. `None` until `load_roster`.
    pub tire_radius_measured_m: Option<f32>,
    pub chassis_points: Vec<Vec3>,
    pub driver_head: Vec3,
    pub shock_texture: String,
    pub raw: HashMap<String, Vec<String>>,
}

impl Truck {
    /// Simulated mass: `dry_weight` plus both axle weights. The retail set-up
    /// writes 500 lb into each axle (`decomp_4x42_full.c` 198 990-198 991,
    /// struct `+0x2f0` / `+0x5a8` = `0x43fa0000`) and sums them with
    /// `dry_weight` (`+0x1204`) at 154 597; the spring rates are sized for
    /// that total (≈70 % static sag).
    pub fn mass_kg(&self) -> f32 {
        ((self.dry_weight_lb + 2.0 * AXLE_WEIGHT_LB) * LB).max(200.0)
    }

    /// World-space wheel mount, relative to the chassis origin (feet).
    pub fn wheel_mount(&self, axle: usize, side: usize) -> Vec3 {
        self.axles[axle].tires[side].static_bpos
    }

    pub fn wheelbase_m(&self) -> f32 {
        self.wheelbase * FT
    }

    pub fn track_width_m(&self) -> f32 {
        ((self.front_track + self.rear_track) * 0.5) * FT
    }

    /// Tyre radius: measured from the truck's own tyre model (offroad4x4
    /// `trucks.rs` measures `<tire>08L.SMF` the same way). Trucks parsed
    /// without `load_roster` (tests, mods without a tyre mesh) fall back to
    /// half the track width; `load_roster` reports those as errors.
    pub fn tyre_radius_m(&self) -> f32 {
        self.tire_radius_measured_m
            .unwrap_or_else(|| (self.track_width_m() * 0.5).clamp(0.30, 0.55))
    }

    /// The selected factory paint, if the truck has any.
    pub fn paint(&self) -> Option<&crate::paint::Paint> {
        self.paints.get(self.paint_index)
    }

    /// Tyre mesh file for one side at full detail (`08`/`12`/`16` are LODs
    /// of the same tyre: 38/170/211 vertices, identical radius).
    pub fn tire_model_file(&self, left: bool) -> String {
        format!(
            "{}16{}.SMF",
            self.tire_model_base.trim().to_ascii_uppercase(),
            if left { "L" } else { "R" }
        )
    }

    /// Torque split front/rear for the transfer case.
    pub fn drive_split(&self) -> (f32, f32) {
        (self.axles[0].torque_pct, self.axles[1].torque_pct)
    }
}

fn parse_vec3(s: &str) -> Option<Vec3> {
    let mut it = s
        .split(',')
        .map(|v| v.trim().trim_start_matches('+').parse::<f32>());
    let x = it.next()?.ok()?;
    let y = it.next()?.ok()?;
    let z = it.next()?.ok()?;
    Some(Vec3::new(x, y, z))
}

fn class_id(name: &str) -> i64 {
    match name.trim().to_ascii_lowercase().as_str() {
        "compact truck" | "compact suv" => 1,
        "standard truck" | "standard suv" => 2,
        "full size truck" | "full size suv" => 3,
        "sport truck" | "sport suv" => 4,
        "concept" | "special" => 5,
        _ => 2,
    }
}

/// Keys that carry free-text values and therefore cannot be found by looking
/// at the next line alone.
fn is_structural_key(t: &str) -> bool {
    if t.contains('.') || t.contains('_') || t.contains('[') {
        return true;
    }
    matches!(
        t.to_ascii_lowercase().as_str(),
        "version"
            | "truckname"
            | "truckmake"
            | "truckmodel"
            | "truckclass"
            | "truckcost"
            | "truckmodelyear"
            | "trucklength"
            | "truckwidth"
            | "truckheight"
            | "truckwheelbase"
            | "truckfronttrack"
            | "truckreartrack"
            | "truckacceleration"
            | "trucktopspeed"
            | "truckhandling"
            | "truckquickclass"
            | "truckmodelbasename"
            | "tiremodelbasename"
            | "axlemodelname"
            | "shocktexturename"
            | "bartexturename"
            | "dry_weight"
            | "refarea"
            | "cl"
            | "cd"
            | "cy"
            | "drive_type"
            | "ixx,iyy,izz"
            | "stockpartlist[]"
            | "numcolors"
            | "colorlist[]"
            | "wave file"
            | "instrument cluster"
    )
}

fn looks_like_value(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return false;
    }
    if t == "NULL" || t.eq_ignore_ascii_case("null.raw") {
        return true;
    }
    let first = t.chars().next().unwrap_or('x');
    if first == '+' || first == '-' || first == '.' || first.is_ascii_digit() {
        return true;
    }
    // Bare boolean-ish tokens are values.
    matches!(t.to_ascii_lowercase().as_str(), "true" | "false")
}

pub fn parse(file: &str, text: &str) -> Result<Truck, String> {
    let lines: Vec<&str> = text.lines().map(|l| l.trim_end_matches('\r')).collect();
    let mut kv: HashMap<String, Vec<String>> = HashMap::new();
    let mut key: Option<String> = None;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim();
        if line.is_empty() || line.starts_with("signature:") {
            i += 1;
            continue;
        }
        if looks_like_value(line) {
            if let Some(k) = &key {
                kv.entry(k.clone()).or_default().push(line.to_string());
            } else {
                i += 1;
                continue;
            }
        } else if is_structural_key(line) || (i + 1 < lines.len() && looks_like_value(lines[i + 1]))
        {
            key = Some(line.to_string());
            kv.entry(line.to_string()).or_default();
        } else if let Some(k) = &key {
            // Free-text value that happens to contain letters ("Test Truck"),
            // or a second line of a multi-token string.
            if let Some(v) = kv.get_mut(k) {
                if let Some(last) = v.last_mut() {
                    last.push(' ');
                    last.push_str(line);
                } else {
                    v.push(line.to_string());
                }
            }
        }
        i += 1;
    }
    // Strings and numeric scalars look alike; a key whose only value is
    // numeric is a scalar. Header text (truckName...) is arbitrary text.
    let get = |k: &str| kv.get(k).and_then(|v| v.first()).cloned();
    let getf = |k: &str, d: f32| -> f32 {
        kv.get(k)
            .and_then(|v| v.first())
            .and_then(|s| s.parse::<f32>().ok())
            .unwrap_or(d)
    };
    let geti = |k: &str, d: i64| -> i64 {
        kv.get(k)
            .and_then(|v| v.first())
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(d)
    };

    let name = get("truckName").ok_or("missing truckName")?;
    let class_name = get("truckClass").unwrap_or_else(|| "Standard SUV".into());

    let mut axles = [Axle::default(), Axle::default()];
    for (i, prefix) in ["faxle", "raxle"].iter().enumerate() {
        axles[i].front = i == 0;
        axles[i].spring_rate = getf(&format!("{prefix}.spring_rate"), 1200.0);
        axles[i].max_compression = getf(&format!("{prefix}.maxcompr"), 1.25).max(0.1);
        axles[i].max_angle = getf(&format!("{prefix}.maxangle"), 0.5);
        axles[i].torque_pct = getf(
            &format!("{prefix}.torque_pct"),
            if i == 0 { 0.4 } else { 0.6 },
        );
        axles[i].bias = getf(&format!("{prefix}.axleBias"), 1.0);
        axles[i].slip_diff = geti(&format!("{prefix}.slipDiffType"), 1) as u8;
        for (j, side) in ["ltire", "rtire"].iter().enumerate() {
            axles[i].tires[j].cf = getf(&format!("{prefix}.{side}.CF"), 1.75);
            axles[i].tires[j].spring_arm = getf(&format!("{prefix}.{side}.spring_arm"), 5.0);
            axles[i].tires[j].static_bpos = get(&format!("{prefix}.{side}.static_bpos"))
                .as_deref()
                .and_then(parse_vec3)
                .unwrap_or_else(|| {
                    Vec3::new(
                        if j == 0 { -2.5 } else { 2.5 },
                        -0.5,
                        if i == 0 { 5.0 } else { -4.5 },
                    )
                });
        }
    }

    let torque_table = kv
        .get("eng.torqueTable[]")
        .map(|v| v.iter().filter_map(|s| s.parse::<f32>().ok()).collect())
        .unwrap_or_else(|| vec![200.0; 13]);
    let engine = Engine {
        max_hp: getf("eng.maxHP", 200.0),
        max_hp_rpm: getf("eng.maxHPRPM", 5000.0),
        max_torque: getf("eng.maxTorque", 235.0),
        max_torque_rpm: getf("eng.maxTorqueRPM", 3000.0),
        redline: getf("eng.redline", 5500.0),
        redline_timer: getf("eng.redlineTimer", 30.0),
        torque_table,
        upshift_rpm: getf("eng.upshift_rpm", 4500.0),
        downshift_rpm: getf("eng.dnshift_rpm", 2000.0),
        friction_cf: getf("eng.friction_cf", 0.0),
        displacement_l: getf("eng.displacement", 3.5),
        aspiration: geti("eng.aspiration", 0) as u8,
    };

    let parse_vec = |k: &str| -> Vec3 {
        kv.get(k)
            .and_then(|v| v.first())
            .and_then(|s| parse_vec3(s))
            .unwrap_or(Vec3::ZERO)
    };

    let paints = kv
        .get("colorList[]")
        .map(|v| {
            v.iter()
                .filter_map(|l| crate::paint::Paint::parse(l))
                .collect()
        })
        .unwrap_or_default();

    let chassis_points = kv
        .get("sc[].pt")
        .map(|v| v.iter().filter_map(|s| parse_vec3(s)).collect())
        .unwrap_or_default();

    Ok(Truck {
        file: file.to_string(),
        name,
        make: get("truckMake").unwrap_or_default(),
        model: get("truckModel").unwrap_or_default(),
        class_id: class_id(&class_name).max(geti("truckQuickClass", 2)),
        class_name,
        quick_class: geti("truckQuickClass", 2),
        cost: geti("truckCost", 0),
        model_year: geti("truckModelYear", 2000) as i32,
        length: getf("truckLength", 15.0),
        width: getf("truckWidth", 6.0),
        height: getf("truckHeight", 6.0),
        wheelbase: getf("truckWheelbase", 9.0),
        front_track: getf("truckFrontTrack", 5.0),
        rear_track: getf("truckRearTrack", 5.0),
        acceleration: getf("truckAcceleration", 0.28),
        top_speed: getf("truckTopSpeed", 0.05),
        handling: getf("truckHandling", 0.26),
        model_base: get("truckModelBaseName").unwrap_or_default(),
        tire_model_base: get("tireModelBaseName").unwrap_or_default(),
        dry_weight_lb: getf("dry_weight", 3600.0),
        inertia: parse_vec("ixx,iyy,izz"),
        ref_area: parse_vec("refArea"),
        cl: getf("CL", 0.1),
        cd: getf("CD", 0.4),
        cy: getf("CY", 5.0),
        drive_type: geti("drive_type", 1) as u8,
        gear_ratios: kv
            .iter()
            .find_map(|(k, v)| k.starts_with("xm.gear_ratio[]").then_some(v))
            .map(|v| v.iter().filter_map(|s| s.parse().ok()).collect())
            .filter(|v: &Vec<f32>| v.len() >= 4)
            .unwrap_or_else(|| vec![0.0, -3.9, 0.0, 3.8, 2.1, 1.4, 1.0, 0.73]),
        gear_inertia: kv
            .get("xm.gearInertia[]")
            .map(|v| v.iter().filter_map(|s| s.parse().ok()).collect())
            .unwrap_or_default(),
        final_drive: getf("xm.final_drive", 4.3),
        xfr_low_ratio: getf("xm.xfrLowRatio", 1.9),
        engine,
        axles,
        stock_parts: kv.get("stockPartList[]").cloned().unwrap_or_default(),
        paints,
        paint_index: 0,
        game: 1,
        tire_radius_measured_m: None,
        chassis_points,
        driver_head: parse_vec("driverHead.initBPos"),
        shock_texture: get("shockTextureName").unwrap_or_default(),
        raw: kv,
    })
}

/// Load every `.TRK` in a directory. Bad files are returned as errors, not
/// silently dropped, so the garage can report them.
pub fn load_dir(dir: &std::path::Path) -> (Vec<Truck>, Vec<String>) {
    let mut out = Vec::new();
    let mut errors = Vec::new();
    let mut entries: Vec<_> = match std::fs::read_dir(dir) {
        Ok(rd) => rd.filter_map(|e| e.ok()).map(|e| e.path()).collect(),
        Err(e) => return (out, vec![format!("{}: {e}", dir.display())]),
    };
    entries.sort();
    for path in entries {
        if path.extension().map(|e| e.eq_ignore_ascii_case("trk")) != Some(true) {
            continue;
        }
        match std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|b| parse(&path.to_string_lossy(), &String::from_utf8_lossy(&b)))
        {
            Ok(t) => out.push(t),
            Err(e) => errors.push(format!("{}: {e}", path.display())),
        }
    }
    (out, errors)
}

/// Every roster directory in load order, with the game it comes from: the
/// Evo 1 extraction, then the Evo 2-only trucks `tools/evo2_trucks.py`
/// imports (the Jeeps and 22 other vehicles absent from Evo 1).
pub const ROSTER: [(&str, u8); 2] = [
    ("assets/extracted/model/vehicle/TRUCK", 1),
    ("assets/extracted/evo2/TRUCK", 2),
];

/// Model directories searched for body and tyre meshes, Evo 1 first.
pub const MODEL_DIRS: [&str; 2] = [
    "assets/extracted/model/vehicle/MODELS",
    "assets/extracted/evo2/MODELS",
];

/// Find `file` in `MODEL_DIRS` under `root` (case-insensitive file name).
pub fn find_model(root: &std::path::Path, file: &str) -> Option<std::path::PathBuf> {
    MODEL_DIRS.iter().find_map(|d| {
        let dir = root.join(d);
        let direct = dir.join(file);
        if direct.exists() {
            return Some(direct);
        }
        std::fs::read_dir(&dir)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.eq_ignore_ascii_case(file))
            })
    })
}

/// Load the whole roster: Evo 1 trucks first (so saved garage indices stay
/// valid), then the Evo 2-only ones, each tagged with its game and with its
/// tyre radius measured from its own tyre mesh. A missing Evo 2 directory is
/// not an error (the import is optional); a truck without a tyre mesh is.
pub fn load_roster(root: &std::path::Path) -> (Vec<Truck>, Vec<String>) {
    let mut all = Vec::new();
    let mut errors = Vec::new();
    for (dir, game) in ROSTER {
        let path = root.join(dir);
        if game > 1 && !path.exists() {
            continue;
        }
        let (mut trucks, errs) = load_dir(&path);
        errors.extend(errs);
        for t in &mut trucks {
            t.game = game;
            match measure_tyre(root, &t.tire_model_file(true)) {
                Some(r) => t.tire_radius_measured_m = Some(r),
                None => errors.push(format!(
                    "{}: tyre model {} not found",
                    t.file,
                    t.tire_model_file(true)
                )),
            }
        }
        all.extend(trucks);
    }
    (all, errors)
}

/// Half the vertical extent of a tyre mesh, metres.
fn measure_tyre(root: &std::path::Path, file: &str) -> Option<f32> {
    let path = find_model(root, file)?;
    let text = String::from_utf8_lossy(&std::fs::read(path).ok()?).to_string();
    let smf = crate::smf::Smf::parse(&text).ok()?;
    let (lo, hi) = smf.bounds();
    let r = (hi.y - lo.y) * 0.5;
    (r > 0.1 && r < 1.5).then_some(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRK: &str = "\
version
6
truckName
Test Truck
truckMake
Test
truckModel
T1
truckClass
Standard SUV
truckCost
1000
truckLength
15.0
truckWidth
6.0
truckHeight
6.0
truckWheelbase
9.0
faxle.rtire.static_bpos
+2.5,-0.5,+5.4
faxle.ltire.static_bpos
-2.5,-0.5,+5.4
raxle.rtire.static_bpos
+2.5,-0.5,-4.2
raxle.ltire.static_bpos
-2.5,-0.5,-4.2
faxle.spring_rate
1000.0
raxle.spring_rate
1200.0
faxle.torque_pct
0.4
raxle.torque_pct
0.6
dry_weight
3000.0
ixx,iyy,izz
1000,1000,1500
refArea
150,190,70
eng.maxTorque
200.0
eng.redline
5000.0
eng.torqueTable[]
100.0
200.0
100.0
xm.gear_ratio[] - Park/Reverse/Neutral/1st/2nd/3rd/4th/5th
0.0
-3.0
0.0
3.0
2.0
1.0
stockPartList[]
RacingClutch
colorList[]
124.5,75,-42.08,20,147,63,NULL
signature: 00 00
";

    #[test]
    fn parses_values_and_lists() {
        let t = parse("t.trk", TRK).unwrap();
        assert_eq!(t.name, "Test Truck");
        assert_eq!(t.stock_parts, vec!["RacingClutch"]);
        assert_eq!(t.paints.len(), 1);
        assert_eq!(t.gear_ratios.len(), 6);
        assert!((t.dry_weight_lb - 3000.0).abs() < 0.1);
        // dry_weight + two 500 lb axles (retail set-up) = 4000 lb.
        assert!((t.mass_kg() - 1814.4).abs() < 1.0);
        assert_eq!(t.wheel_mount(0, 1).z, 5.4);
        assert!((t.drive_split().1 - 0.6).abs() < 1e-6);
    }

    #[test]
    fn torque_curve_peaks_at_max() {
        let t = parse("t.trk", TRK).unwrap();
        assert!(t.engine.torque_nm(2500.0) > t.engine.torque_nm(100.0));
        assert!(t.engine.torque_nm(2500.0) > 0.0);
    }

    #[test]
    fn missing_name_is_error() {
        assert!(parse("x", "version\n6\n").is_err());
    }
}

/// Roster audit against the extracted retail data (Evo 1 plus the Evo 2
/// import from `tools/evo2_trucks.py`).
#[cfg(test)]
mod roster_tests {
    use super::*;
    use std::path::Path;

    fn root() -> &'static Path {
        let r = if Path::new("assets/extracted").exists() {
            Path::new(".")
        } else {
            Path::new("..")
        };
        assert!(
            r.join("assets/extracted/evo2/TRUCK").exists(),
            "Evo 2 trucks not imported: run `python3 tools/evo2_trucks.py`"
        );
        r
    }

    /// RAW + ACT + OPA for a texture stem, from the Evo 1 or Evo 2 tree.
    fn art(root: &Path, stem: &str) -> Option<(Vec<u8>, Vec<u8>, Vec<u8>)> {
        let stem = stem.to_ascii_uppercase();
        let read = |dirs: &[&str], ext: &str| {
            dirs.iter()
                .find_map(|d| std::fs::read(root.join(d).join(format!("{stem}.{ext}"))).ok())
        };
        let raw = read(
            &[
                "assets/extracted/texture/texture/ART",
                "assets/extracted/evo2/texture/ART",
            ],
            "RAW",
        )?;
        let act = read(
            &[
                "assets/extracted/texture/palette/ART",
                "assets/extracted/evo2/palette/ART",
            ],
            "ACT",
        )?;
        let opa = read(
            &[
                "assets/extracted/texture/opacity/ART",
                "assets/extracted/evo2/opacity/ART",
            ],
            "OPA",
        )?;
        Some((raw, act, opa))
    }

    fn body_texture(root: &Path, t: &Truck) -> Option<String> {
        let path = find_model(root, &format!("{}.SMF", t.model_base.to_ascii_uppercase()))?;
        let smf =
            crate::smf::Smf::parse(&String::from_utf8_lossy(&std::fs::read(path).ok()?)).ok()?;
        let tex = smf
            .meshes
            .iter()
            .find(|m| !m.texture.is_empty())?
            .texture
            .clone();
        Some(
            Path::new(&tex)
                .file_stem()?
                .to_string_lossy()
                .to_ascii_uppercase(),
        )
    }

    #[test]
    fn roster_includes_every_retail_truck_and_the_jeeps() {
        require_assets!();
        let (trucks, errors) = load_roster(root());
        assert!(errors.is_empty(), "roster errors: {errors:?}");
        assert_eq!(trucks.iter().filter(|t| t.game == 1).count(), 151);
        assert_eq!(trucks.iter().filter(|t| t.game == 2).count(), 41);
        let names: std::collections::BTreeSet<&str> =
            trucks.iter().map(|t| t.name.as_str()).collect();
        // 108 Evo 1 names + 25 Evo 2-only. Evo 2's `MPRT2D.TRK` renames the
        // Evo 1 "Mitsubishi Pajero Rally 2 Door" to "Montero" (new body
        // TROPHYMONT); the file name collides, so the Evo 1 truck is kept.
        assert_eq!(names.len(), 133, "unique vehicle names");
        for want in [
            "Jeep Wrangler Sport",
            "Jeep ARB  Wrangler",
            "Jeep Grand Cherokee Limited 4WD",
            "Jeep Liberty Sport 4WD",
        ] {
            assert!(names.contains(want), "{want} missing from the roster");
        }
        let wrangler = trucks
            .iter()
            .find(|t| t.file.ends_with("JWS4.TRK"))
            .unwrap();
        assert_eq!(wrangler.paints.len(), 9);
        let arb = trucks
            .iter()
            .find(|t| t.file.ends_with("AJWS.TRK"))
            .unwrap();
        assert!(arb.paints.is_empty(), "the ARB Wrangler is a fixed livery");
    }

    #[test]
    fn every_truck_resolves_body_tyres_and_textures() {
        require_assets!();
        let root = root();
        let (trucks, _) = load_roster(root);
        let mut bad = Vec::new();
        for t in &trucks {
            let body = format!("{}.SMF", t.model_base.to_ascii_uppercase());
            if find_model(root, &body).is_none() {
                bad.push(format!("{}: {body}", t.file));
            }
            for left in [true, false] {
                if find_model(root, &t.tire_model_file(left)).is_none() {
                    bad.push(format!("{}: {}", t.file, t.tire_model_file(left)));
                }
            }
            match body_texture(root, t) {
                Some(stem) if art(root, &stem).is_some() => {}
                other => bad.push(format!("{}: body texture {other:?}", t.file)),
            }
            let r = t.tyre_radius_m();
            if !(0.25..0.9).contains(&r) {
                bad.push(format!("{}: tyre radius {r:.2} m", t.file));
            }
        }
        assert!(bad.is_empty(), "{} unresolved: {bad:#?}", bad.len());
    }

    #[test]
    fn arb_wrangler_body_is_yellow() {
        require_assets!();
        let root = root();
        let (trucks, _) = load_roster(root);
        let arb = trucks
            .iter()
            .find(|t| t.file.ends_with("AJWS.TRK"))
            .unwrap();
        let (raw, act, opa) = art(root, &body_texture(root, arb).unwrap()).unwrap();
        let (mut r, mut g, mut b, mut n) = (0f32, 0f32, 0f32, 0f32);
        for (i, &p) in raw.iter().enumerate() {
            if opa[i] == 255 {
                r += act[p as usize * 3] as f32;
                g += act[p as usize * 3 + 1] as f32;
                b += act[p as usize * 3 + 2] as f32;
                n += 1.0;
            }
        }
        let (r, g, b) = (r / n, g / n, b / n);
        assert!(
            r > 150.0 && g > 120.0 && b < 90.0,
            "ARB body is not yellow: {r:.0},{g:.0},{b:.0}"
        );
    }

    /// The data carries its own answer key: each `colorList[]` entry's RGB
    /// swatch is the colour the menu shows. Recolouring the body with the
    /// entry's HSV shift (`paint.rs`, the retail `colorAdjustTexture`) must
    /// land on that hue for chromatic swatches.
    #[test]
    fn factory_paints_match_their_swatches() {
        require_assets!();
        use crate::paint::{adjusted_palette, rgb_to_hsv};
        let root = root();
        let (trucks, _) = load_roster(root);
        let (mut checked, mut matched) = (0, 0);
        let mut misses = Vec::new();
        for t in &trucks {
            let Some(stem) = body_texture(root, t) else {
                continue;
            };
            let Some((raw, act, opa)) = art(root, &stem) else {
                continue;
            };
            let original: Vec<[u8; 3]> = (0..256)
                .map(|i| [act[i * 3], act[i * 3 + 1], act[i * 3 + 2]])
                .collect();
            let mut counts = [0u32; 256];
            for (i, &p) in raw.iter().enumerate() {
                if opa[i] > 250 {
                    counts[p as usize] += 1;
                }
            }
            let dominant = (0..256).max_by_key(|&i| counts[i]).unwrap();
            for p in &t.paints {
                let sw = p.swatch.map(|c| c as f32 / 255.0);
                let (sh, ss, sv) = rgb_to_hsv(sw[0], sw[1], sw[2]);
                if ss < 0.45 || sv < 0.25 {
                    continue; // greys, blacks and whites carry no hue to compare
                }
                let pal = adjusted_palette(&raw, &original, &opa, p.shift());
                let c = pal[dominant].map(|c| c as f32 / 255.0);
                let (h, _, _) = rgb_to_hsv(c[0], c[1], c[2]);
                let d = (h - sh).abs();
                let d = d.min(1.0 - d) * 360.0;
                checked += 1;
                if d < 25.0 {
                    matched += 1;
                } else {
                    misses.push(format!("{} {:?}: {:.0}°", t.file, p.swatch, d));
                }
            }
        }
        assert!(checked > 300, "only {checked} chromatic paints checked");
        let rate = matched as f32 / checked as f32;
        println!(
            "paint swatch check: {matched}/{checked} within 25° ({:.1}%)",
            rate * 100.0
        );
        for m in misses.iter().take(10) {
            println!("  miss {m}");
        }
        assert!(
            rate > 0.9,
            "{matched}/{checked} paints land on their swatch hue; misses: {:?}",
            &misses[..misses.len().min(20)]
        );
    }

    #[test]
    fn every_evo2_model_parses() {
        require_assets!();
        let dir = root().join("assets/extracted/evo2/MODELS");
        let mut n = 0;
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let text = String::from_utf8_lossy(&std::fs::read(e.path()).unwrap()).to_string();
            let smf = crate::smf::Smf::parse(&text)
                .unwrap_or_else(|err| panic!("{}: {err}", e.path().display()));
            assert!(
                smf.meshes.iter().any(|m| !m.positions.is_empty()),
                "{}",
                e.path().display()
            );
            n += 1;
        }
        // 110 truck models + 619 track prop models from exclusive Evo 2 tracks
        assert_eq!(n, 729);
    }
}
