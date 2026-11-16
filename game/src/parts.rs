//! Parts catalog: parsing, prerequisite algebra, and shop transactions.
//!
//! The catalog is the game's original `PARTS.TXT` (extracted from PARTS.POD).
//! Its grammar is line oriented, with multi-line values and a small boolean
//! language: `&` (and), `|` (or), parentheses. A line break inside a value is
//! an implicit OR. `Incompatible` is a plain id list.
//!
//! Everything here is pure logic so it can be unit tested without a window,
//! and so the UI is a thin projection of the rules rather than the rules.

use std::collections::{HashMap, HashSet};

pub const PART_TYPES: [PartType; 9] = [
    PartType::Engine,
    PartType::Drivetrain,
    PartType::Suspension,
    PartType::Tires,
    PartType::Brakes,
    PartType::Chassis,
    PartType::Electronics,
    PartType::Lights,
    PartType::Appearance,
];

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum PartType {
    Engine,
    Drivetrain,
    Suspension,
    Tires,
    Brakes,
    Chassis,
    Electronics,
    Lights,
    Appearance,
}

impl PartType {
    pub fn from_label(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "engine" => Self::Engine,
            "drivetrain" => Self::Drivetrain,
            "suspension" => Self::Suspension,
            "tires" | "tyres" => Self::Tires,
            "brakes" => Self::Brakes,
            "chassis" => Self::Chassis,
            "electronics" => Self::Electronics,
            "lights" => Self::Lights,
            "appearance" => Self::Appearance,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Engine => "Engine",
            Self::Drivetrain => "Drivetrain",
            Self::Suspension => "Suspension",
            Self::Tires => "Tires",
            Self::Brakes => "Brakes",
            Self::Chassis => "Chassis",
            Self::Electronics => "Electronics",
            Self::Lights => "Lights",
            Self::Appearance => "Appearance",
        }
    }

    /// Texture stem used by the original shop tabs (UI.POD `ART\*`).
    pub fn ui_icon(self) -> &'static str {
        match self {
            Self::Engine => "ICONA",
            Self::Drivetrain => "ICONB",
            Self::Suspension => "ICONC",
            Self::Tires => "ICOND",
            Self::Brakes => "ICONA",
            Self::Chassis => "ICONB",
            Self::Electronics => "ICONC",
            Self::Lights => "ICOND",
            Self::Appearance => "ICONA",
        }
    }

    pub fn index(self) -> usize {
        PART_TYPES.iter().position(|t| *t == self).unwrap_or(0)
    }

    /// Display order in the shop is engine-first: performance before looks.
    pub fn sort_key(self) -> u8 {
        self.index() as u8
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StatMods {
    pub aero: f32,
    pub brake: f32,
    pub cg: f32,
    pub tire_cf: f32,
    pub torque: f32,
    pub weight: f32,
}

impl StatMods {
    pub fn add(&mut self, other: &StatMods) {
        self.aero += other.aero;
        self.brake += other.brake;
        self.cg += other.cg;
        self.tire_cf += other.tire_cf;
        self.torque += other.torque;
        self.weight += other.weight;
    }

    /// Signed, formatted rows for the stat panel. Empty values are skipped.
    pub fn rows(&self) -> Vec<(&'static str, f32, bool)> {
        let mut out = Vec::new();
        for (label, v, higher_better) in [
            ("Aero", self.aero, false),
            ("Brakes", self.brake, true),
            ("CG height", self.cg, false),
            ("Tire grip", self.tire_cf, true),
            ("Torque", self.torque, true),
            ("Weight", self.weight, false),
        ] {
            if v.abs() > f32::EPSILON {
                out.push((label, v, higher_better));
            }
        }
        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compare {
    Equal,
    AtLeast,
    AtMost,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Id(String),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Compare(Compare, String, i64),
}

impl Expr {
    pub fn eval(&self, equipped: &HashSet<String>, values: &HashMap<String, i64>) -> bool {
        match self {
            Expr::Id(id) => equipped.contains(id),
            Expr::And(a, b) => a.eval(equipped, values) && b.eval(equipped, values),
            Expr::Or(a, b) => a.eval(equipped, values) || b.eval(equipped, values),
            Expr::Compare(cmp, key, n) => {
                let v = values.get(key).copied().unwrap_or(0);
                match cmp {
                    Compare::Equal => v == *n,
                    Compare::AtLeast => v >= *n,
                    Compare::AtMost => v <= *n,
                }
            }
        }
    }

    /// Human readable form, used in lock tooltips.
    pub fn render(&self) -> String {
        match self {
            Expr::Id(id) => id.clone(),
            Expr::And(a, b) => format!("({} & {})", a.render(), b.render()),
            Expr::Or(a, b) => format!("({} | {})", a.render(), b.render()),
            Expr::Compare(Compare::Equal, k, n) => format!("{k}=={n}"),
            Expr::Compare(Compare::AtLeast, k, n) => format!("{k}>={n}"),
            Expr::Compare(Compare::AtMost, k, n) => format!("{k}<={n}"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Part {
    pub index: usize,
    pub name: String,
    pub code: String,
    pub kind: PartType,
    pub cost: i64,
    pub mods: StatMods,
    pub models: Vec<String>,
    pub prereq: Option<Expr>,
    pub incompatible: Vec<String>,
    pub incompatible_trucks: Vec<String>,
    pub notes: String,
    pub description: String,
    /// Slot group: exactly one part per `PartType` can be equipped.
    pub slot: PartType,
}

impl Part {
    pub fn is_locked_for_truck(&self, truck: &str) -> bool {
        self.incompatible_trucks
            .iter()
            .any(|t| t.eq_ignore_ascii_case(truck))
    }
}

#[derive(Clone, Debug, Default)]
pub struct Catalog {
    pub parts: Vec<Part>,
    by_code: HashMap<String, usize>,
}

const KEYS: [&str; 14] = [
    "Name",
    "Code",
    "Type",
    "Cost",
    "Aero Mod",
    "Brake Mod",
    "CG Mod",
    "Tire CF Mod",
    "Torque Mod",
    "Weight Mod",
    "Models",
    "Prerequisite",
    "Incompatible",
    "Incompatible Trucks",
];

fn is_key(line: &str) -> bool {
    KEYS.iter().any(|k| k.eq_ignore_ascii_case(line))
}

fn ends_open(s: &str) -> bool {
    let t = s.trim_end();
    t.ends_with('&') || t.ends_with('|') || t.ends_with('(')
}

fn starts_operator(s: &str) -> bool {
    let t = s.trim_start();
    t.starts_with('&') || t.starts_with('|') || t.starts_with(')')
}

/// Parse `PARTS.TXT`. Malformed parts are reported with their line numbers;
/// the rest of the catalog still loads (the original tolerates this too).
pub fn parse_catalog(text: &str) -> (Catalog, Vec<String>) {
    let mut catalog = Catalog::default();
    let mut errors = Vec::new();
    let mut fields: HashMap<&'static str, Vec<String>> = HashMap::new();
    let mut current: Option<&'static str> = None;
    let mut block_start = 0usize;

    let flush = |fields: &mut HashMap<&'static str, Vec<String>>,
                 catalog: &mut Catalog,
                 errors: &mut Vec<String>,
                 block_start: usize| {
        fn take(fields: &mut HashMap<&'static str, Vec<String>>, k: &str) -> Vec<String> {
            fields.remove(k).unwrap_or_default()
        }
        fn num(fields: &mut HashMap<&'static str, Vec<String>>, k: &str) -> f32 {
            fields
                .remove(k)
                .and_then(|v| v.join(" ").trim().parse::<f32>().ok())
                .unwrap_or(0.0)
        }
        let name = take(fields, "Name").join(" ").trim().to_string();
        let code = take(fields, "Code").join(" ").trim().to_string();
        let kind_raw = take(fields, "Type").join(" ");
        if name.is_empty() && code.is_empty() {
            fields.clear();
            return;
        }
        let Some(kind) = PartType::from_label(&kind_raw) else {
            errors.push(format!(
                "part near line {block_start} ({code}): unknown type {kind_raw:?}"
            ));
            fields.clear();
            return;
        };
        let cost = take(fields, "Cost")
            .join(" ")
            .trim()
            .parse::<i64>()
            .unwrap_or(0);
        let mods = StatMods {
            aero: num(fields, "Aero Mod"),
            brake: num(fields, "Brake Mod"),
            cg: num(fields, "CG Mod"),
            tire_cf: num(fields, "Tire CF Mod"),
            torque: num(fields, "Torque Mod"),
            weight: num(fields, "Weight Mod"),
        };
        let split_ids = |v: Vec<String>| -> Vec<String> {
            v.join(" ")
                .split([' ', ',', '|', '\t'])
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        };
        let models = split_ids(take(fields, "Models"));
        let incompatible_trucks = split_ids(take(fields, "Incompatible Trucks"));
        let incompatible = split_ids(take(fields, "Incompatible"));

        // Keep a flat, normalized expression with line breaks as OR.
        let prereq_src = take(fields, "Prerequisite");
        let mut joined = String::new();
        for (i, line) in prereq_src.iter().enumerate() {
            if i > 0 {
                let prev = &prereq_src[i - 1];
                if ends_open(prev) || starts_operator(line) || line.trim_start().starts_with(')') {
                    joined.push(' ');
                } else {
                    joined.push_str(" | ");
                }
            }
            joined.push_str(line.trim());
        }
        let prereq = if joined.trim().is_empty() {
            None
        } else {
            match parse_expr(&joined) {
                Ok(e) => Some(e),
                Err(e) => {
                    errors.push(format!("part near line {block_start} ({code}): {e}"));
                    None
                }
            }
        };
        let notes = take(fields, "Notes").join(" ");

        let index = catalog.parts.len();
        catalog.by_code.insert(code.to_ascii_lowercase(), index);
        catalog.parts.push(Part {
            index,
            name,
            code,
            kind,
            cost,
            mods,
            models,
            prereq,
            incompatible,
            incompatible_trucks,
            notes,
            description: String::new(),
            slot: kind,
        });
        fields.clear();
    };

    for (lineno, raw) in text.lines().enumerate() {
        let line = raw.trim_end_matches('\r');
        let trimmed = line.trim();
        if trimmed.starts_with("--- Part") {
            flush(&mut fields, &mut catalog, &mut errors, block_start);
            block_start = lineno + 1;
            current = None;
            continue;
        }
        if is_key(trimmed) {
            let key = KEYS
                .iter()
                .find(|k| k.eq_ignore_ascii_case(trimmed))
                .copied()
                .unwrap_or("");
            current = Some(key);
            fields.entry(key).or_default();
            continue;
        }
        if let Some(key) = current {
            if key == "Prerequisite" && trimmed.is_empty() {
                continue;
            }
            if trimmed.is_empty() && key != "Prerequisite" {
                continue;
            }
            fields.entry(key).or_default().push(trimmed.to_string());
        }
    }
    flush(&mut fields, &mut catalog, &mut errors, block_start);
    (catalog, errors)
}

/// `.LOC` files carry the long descriptions, keyed by `Name`.
pub fn attach_descriptions(catalog: &mut Catalog, loc: &str) {
    let mut name: Option<String> = None;
    let mut lines: Vec<String> = Vec::new();
    let finish = |name: &mut Option<String>, lines: &mut Vec<String>, catalog: &mut Catalog| {
        if let Some(n) = name.take() {
            if let Some(p) = catalog
                .parts
                .iter_mut()
                .find(|p| p.name.eq_ignore_ascii_case(&n))
            {
                p.description = lines.join("\n");
            }
        }
        lines.clear();
    };
    for raw in loc.lines() {
        let line = raw.trim_end_matches('\r');
        if line.trim().starts_with("--- Part") {
            finish(&mut name, &mut lines, catalog);
            continue;
        }
        if line.trim().eq_ignore_ascii_case("Name") {
            finish(&mut name, &mut lines, catalog);
            name = Some(String::new());
            continue;
        }
        if line.trim().eq_ignore_ascii_case("Description") {
            continue;
        }
        match &mut name {
            Some(n) if n.is_empty() => *n = line.trim().to_string(),
            Some(_) => lines.push(line.trim().to_string()),
            None => {}
        }
    }
    finish(&mut name, &mut lines, catalog);
}

// --- expression parser -----------------------------------------------------

struct Parser<'a> {
    tokens: Vec<String>,
    pos: usize,
    src: &'a str,
}

fn tokenize(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in src.chars() {
        match ch {
            '&' | '|' | '(' | ')' => {
                if !cur.trim().is_empty() {
                    out.push(cur.trim().to_string());
                }
                cur.clear();
                out.push(ch.to_string());
            }
            c => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

fn parse_expr(src: &str) -> Result<Expr, String> {
    let mut p = Parser {
        tokens: tokenize(src),
        pos: 0,
        src,
    };
    let e = p.parse_or()?;
    if p.pos != p.tokens.len() {
        return Err(format!("trailing tokens in {:?}", p.src));
    }
    Ok(e)
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&str> {
        self.tokens.get(self.pos).map(|s| s.as_str())
    }
    fn parse_or(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_and()?;
        while self.peek() == Some("|") {
            self.pos += 1;
            let right = self.parse_and()?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }
    fn parse_and(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_atom()?;
        while self.peek() == Some("&") {
            self.pos += 1;
            let right = self.parse_atom()?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }
    fn parse_atom(&mut self) -> Result<Expr, String> {
        match self.peek() {
            Some("(") => {
                self.pos += 1;
                let e = self.parse_or()?;
                if self.peek() != Some(")") {
                    return Err(format!("unclosed group in {:?}", self.src));
                }
                self.pos += 1;
                Ok(e)
            }
            Some("&") | Some("|") | Some(")") | None => {
                Err(format!("expected term, found {:?}", self.peek()))
            }
            Some(tok) => {
                let tok = tok.to_string();
                self.pos += 1;
                // Optional comparison stub: `TruckClass >= 2`.
                if let Some(op) = self.peek().map(|o| o.to_string()) {
                    let cmp = match op.as_str() {
                        ">=" => Some(Compare::AtLeast),
                        "<=" => Some(Compare::AtMost),
                        "==" => Some(Compare::Equal),
                        _ => None,
                    };
                    if let Some(cmp) = cmp {
                        self.pos += 1;
                        let n = self
                            .peek()
                            .and_then(|s| s.parse::<i64>().ok())
                            .ok_or_else(|| format!("expected number after {op}"))?;
                        self.pos += 1;
                        return Ok(Expr::Compare(cmp, tok, n));
                    }
                }
                Ok(Expr::Id(tok))
            }
        }
    }
}

// --- shop ------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum Lock {
    NotEnoughMoney(i64),
    MissingPrereq(String),
    Conflicts(Vec<String>),
    WrongTruck,
    NotOwned,
    AlreadyEquipped,
}

impl Lock {
    pub fn text(&self) -> String {
        match self {
            Lock::NotEnoughMoney(n) => format!("Need ${n} more"),
            Lock::MissingPrereq(e) => format!("Requires {e}"),
            Lock::Conflicts(v) => format!("Conflicts with {}", v.join(", ")),
            Lock::WrongTruck => "Not available for this truck".into(),
            Lock::NotOwned => "Buy before equipping".into(),
            Lock::AlreadyEquipped => "Installed".into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Shop {
    pub catalog: Catalog,
    pub budget: i64,
    pub owned: HashSet<String>,
    pub equipped: HashMap<PartType, String>,
    pub truck: String,
    pub truck_class: i64,
    pub selected_type: PartType,
    pub cursor: usize,
    pub message: Option<(String, f32)>,
    pub mods: StatMods,
}

impl Shop {
    pub fn new(catalog: Catalog, budget: i64, truck: &str, truck_class: i64) -> Self {
        Self {
            catalog,
            budget,
            owned: HashSet::new(),
            equipped: HashMap::new(),
            truck: truck.to_string(),
            truck_class,
            selected_type: PartType::Engine,
            cursor: 0,
            message: None,
            mods: StatMods::default(),
        }
    }

    pub fn parts_of_type(&self, ty: PartType) -> Vec<&Part> {
        self.catalog.parts.iter().filter(|p| p.kind == ty).collect()
    }

    pub fn filtered(&self) -> Vec<&Part> {
        let mut v = self.parts_of_type(self.selected_type);
        v.sort_by_key(|p| (p.cost, p.name.clone()));
        v
    }

    pub fn selected(&self) -> Option<&Part> {
        self.filtered().get(self.cursor).copied()
    }

    fn equipped_ids(&self) -> HashSet<String> {
        self.equipped.values().cloned().collect()
    }

    fn values(&self) -> HashMap<String, i64> {
        let mut m = HashMap::new();
        m.insert("TruckClass".to_string(), self.truck_class);
        m
    }

    pub fn prereq_ok(&self, part: &Part) -> bool {
        match &part.prereq {
            None => true,
            Some(e) => e.eval(&self.equipped_ids(), &self.values()),
        }
    }

    pub fn conflicts(&self, part: &Part) -> Vec<String> {
        part.incompatible
            .iter()
            .filter(|c| {
                self.equipped
                    .values()
                    .any(|equipped| equipped.eq_ignore_ascii_case(c))
            })
            .map(|c| {
                self.catalog
                    .parts
                    .iter()
                    .find(|p| p.code.eq_ignore_ascii_case(c))
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| c.clone())
            })
            .collect()
    }

    /// Why `part` cannot be bought right now, if it cannot.
    pub fn buy_lock(&self, part: &Part) -> Option<Lock> {
        if self.owned.contains(&part.code) {
            return Some(Lock::AlreadyEquipped);
        }
        if !self.prereq_ok(part) {
            return Some(Lock::MissingPrereq(
                part.prereq.as_ref().map(|e| e.render()).unwrap_or_default(),
            ));
        }
        let conflicts = self.conflicts(part);
        if !conflicts.is_empty() {
            return Some(Lock::Conflicts(conflicts));
        }
        if part.is_locked_for_truck(&self.truck) {
            return Some(Lock::WrongTruck);
        }
        if self.budget < part.cost {
            return Some(Lock::NotEnoughMoney(part.cost - self.budget));
        }
        None
    }

    pub fn buy(&mut self, code: &str) -> Result<(), Lock> {
        let part = self
            .catalog
            .parts
            .iter()
            .find(|p| p.code == code)
            .cloned()
            .ok_or(Lock::NotOwned)?;
        if self.owned.contains(&part.code) {
            return self.equip(code);
        }
        if let Some(lock) = self.buy_lock(&part) {
            self.message = Some((lock.text(), 2.5));
            return Err(lock);
        }
        self.budget -= part.cost;
        self.owned.insert(part.code.clone());
        self.message = Some((format!("Bought {}", part.name), 2.0));
        self.equip(code).map_err(|l| l)?;
        Ok(())
    }

    /// Install an owned part, removing whatever occupies its slot and any
    /// mutually exclusive item in another slot.
    pub fn equip(&mut self, code: &str) -> Result<(), Lock> {
        let part = self
            .catalog
            .parts
            .iter()
            .find(|p| p.code == code)
            .cloned()
            .ok_or(Lock::NotOwned)?;
        if !self.owned.contains(&part.code) {
            return Err(Lock::NotOwned);
        }
        if !self.prereq_ok(&part) {
            return Err(Lock::MissingPrereq(
                part.prereq.as_ref().map(|e| e.render()).unwrap_or_default(),
            ));
        }
        if part.is_locked_for_truck(&self.truck) {
            return Err(Lock::WrongTruck);
        }
        self.equipped.insert(part.slot, part.code.clone());
        // Drop conflicting items from their own slots.
        let conflicts = part.incompatible.clone();
        for c in conflicts {
            let slot = self
                .catalog
                .parts
                .iter()
                .find(|p| p.code.eq_ignore_ascii_case(&c))
                .map(|p| p.slot);
            if let Some(slot) = slot {
                if self.equipped.get(&slot).map(|v| v.eq_ignore_ascii_case(&c)) == Some(true) {
                    self.equipped.remove(&slot);
                }
            }
        }
        self.recompute();
        Ok(())
    }

    pub fn unequip(&mut self, slot: PartType) {
        self.equipped.remove(&slot);
        self.recompute();
    }

    pub fn sell(&mut self, code: &str) {
        if let Some(pos) = self.catalog.parts.iter().position(|p| p.code == code) {
            let part = self.catalog.parts[pos].clone();
            if self.owned.remove(&part.code) {
                if self.equipped.get(&part.slot).map(|c| c == &part.code) == Some(true) {
                    self.equipped.remove(&part.slot);
                }
                self.budget += (part.cost as f32 * 0.5) as i64;
                self.recompute();
                self.message = Some((format!("Sold {} at 50%", part.name), 2.0));
            }
        }
    }

    pub fn recompute(&mut self) {
        let mut mods = StatMods::default();
        let codes: Vec<String> = self.equipped.values().cloned().collect();
        for code in codes {
            if let Some(p) = self.catalog.parts.iter().find(|p| p.code == code) {
                mods.add(&p.mods);
            }
        }
        self.mods = mods;
    }

    pub fn tick(&mut self, dt: f32) {
        if let Some((_, t)) = &mut self.message {
            *t -= dt;
            if *t <= 0.0 {
                self.message = None;
            }
        }
    }

    pub fn move_cursor(&mut self, delta: i32) {
        let n = self.filtered().len();
        if n == 0 {
            self.cursor = 0;
            return;
        }
        let next = self.cursor as i32 + delta;
        self.cursor = next.clamp(0, n as i32 - 1) as usize;
    }

    pub fn cycle_type(&mut self, delta: i32) {
        let i = (self.selected_type.index() as i32 + delta).rem_euclid(PART_TYPES.len() as i32);
        self.selected_type = PART_TYPES[i as usize];
        self.cursor = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TXT: &str = "\
partCount
4
--- Part 1 ---
Name
Sport Exhaust
Code
SportExhaust
Type
Engine
Cost
1000
Aero Mod
0.0
Brake Mod
0.0
CG Mod
0.0
Tire CF Mod
0.0
Torque Mod
0.02
Weight Mod
-10
Models

Prerequisite

Incompatible
RaceExhaust FIExhaust
Incompatible Trucks

Notes

--- Part 2 ---
Name
Race Exhaust
Code
RaceExhaust
Type
Engine
Cost
4500
Aero Mod
0.0
Brake Mod
0.0
CG Mod
0.0
Tire CF Mod
0.0
Torque Mod
0.05
Weight Mod
-20
Models

Prerequisite

Incompatible
SportExhaust FIExhaust
Incompatible Trucks
RustBucket
Notes

--- Part 3 ---
Name
Dyno 2
Code
DynoTune2
Type
Engine
Cost
2500
Aero Mod
0.0
Brake Mod
0.0
CG Mod
0.0
Tire CF Mod
0.0
Torque Mod
0.05
Weight Mod
0
Models

Prerequisite
DynoTune1 & FuelSystem & ( SportExhaust | RaceExhaust )
Incompatible

Incompatible Trucks

Notes

--- Part 4 ---
Name
Lift 4
Code
4LiftKit
Type
Suspension
Cost
2000
Aero Mod
0.0
Brake Mod
0.0
CG Mod
0.1
Tire CF Mod
0.0
Torque Mod
0.0
Weight Mod
40
Models

Prerequisite

Incompatible
2LiftKit 6LiftKit
Incompatible Trucks

Notes

";

    fn shop() -> Shop {
        let (mut c, errs) = parse_catalog(TXT);
        assert!(errs.is_empty(), "{errs:?}");
        attach_descriptions(&mut c, "");
        Shop::new(c, 10_000, "OtherTruck", 1)
    }

    #[test]
    fn parses_catalog() {
        let (c, errs) = parse_catalog(TXT);
        assert!(errs.is_empty());
        assert_eq!(c.parts.len(), 4);
        assert_eq!(c.parts[0].kind, PartType::Engine);
        assert_eq!(c.parts[0].mods.torque, 0.02);
        assert_eq!(
            c.parts[2].prereq.as_ref().unwrap().render(),
            "((DynoTune1 & FuelSystem) & (SportExhaust | RaceExhaust))"
        );
    }

    #[test]
    fn prereq_gate() {
        let mut s = shop();
        let race = s
            .catalog
            .parts
            .iter()
            .find(|p| p.code == "RaceExhaust")
            .unwrap()
            .clone();
        assert!(s.buy_lock(&race).is_none());
        s.buy("SportExhaust").unwrap();
        assert!(matches!(s.buy_lock(&race), Some(Lock::Conflicts(_))));
        assert!(s.buy("RaceExhaust").is_err(), "conflicts with SportExhaust");
        s.unequip(PartType::Engine);
        s.buy("RaceExhaust").unwrap();
        assert_eq!(s.budget, 10_000 - 1000 - 4500);
    }

    #[test]
    fn budget_and_slot() {
        let mut s = shop();
        s.buy("SportExhaust").unwrap();
        s.buy("4LiftKit").unwrap();
        s.buy("4LiftKit").unwrap();
        assert_eq!(s.budget, 10_000 - 1000 - 2000);
        assert_eq!(s.mods.weight, 30.0);
        s.unequip(PartType::Suspension);
        assert_eq!(s.mods.weight, -10.0);
    }

    #[test]
    fn truck_blacklist() {
        let mut s = shop();
        s.truck = "RustBucket".into();
        let race = s
            .catalog
            .parts
            .iter()
            .find(|p| p.code == "RaceExhaust")
            .unwrap()
            .clone();
        assert!(matches!(s.buy_lock(&race), Some(Lock::WrongTruck)));
        s.owned.insert(race.code.clone());
        assert!(matches!(s.equip("RaceExhaust"), Err(Lock::WrongTruck)));
        assert_eq!(s.budget, 10_000);
    }

    #[test]
    fn or_expression() {
        let e = parse_expr("( A | B ) & C").unwrap();
        let mut set = HashSet::new();
        let empty = HashMap::new();
        assert!(!e.eval(&set, &empty));
        set.insert("A".into());
        set.insert("C".into());
        assert!(e.eval(&set, &empty));
        set.remove("A");
        set.insert("B".into());
        assert!(e.eval(&set, &empty));
    }

    #[test]
    fn rejects_broken_expression() {
        assert!(parse_expr("( A & B").is_err());
        assert!(parse_expr("& A").is_err());
    }

    #[test]
    fn sell_returns_half() {
        let mut s = shop();
        s.buy("SportExhaust").unwrap();
        s.sell("SportExhaust");
        assert_eq!(s.budget, 10_000 - 500);
        assert!(s.equipped.is_empty());
    }
}
