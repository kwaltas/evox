//! Career, series, and tournament progression parsed from SERIES.POD.
//!
//! A series file lists its tracks, entry fee, purse, class cap and required
//! finishing position. Standings accumulate across the events; the player must
//! place inside the cutoff to stay in the series. Difficulty scales with the
//! series tier and a monotonic "skill" value, never with the player's money,
//! so a rich player cannot buy an easier championship.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub track: String,
    pub laps: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Series {
    pub id: String,
    pub name: String,
    pub tier: u8,
    pub entry_fee: i64,
    pub purse: i64,
    pub max_class: u8,
    pub required_place: u32,
    pub events: Vec<Event>,
    pub ai_skill: f32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Standings {
    pub points: HashMap<String, i64>,
    pub events_done: usize,
    pub finished: bool,
    pub failed: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Career {
    pub series: Vec<Series>,
    pub active: Option<usize>,
    pub standings: Option<Standings>,
    pub career_earnings: i64,
    pub completed_series: Vec<String>,
    pub unlocked_tiers: u8,
    /// Index into the active series' event queue.
    #[serde(default)]
    pub current_event: usize,
    #[serde(default)]
    pub last_award: Option<Award>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Award {
    pub final_place: u32,
    pub purse: i64,
    pub passed: bool,
    pub unlocked_tier: Option<u8>,
    pub series_name: String,
}

#[derive(Clone, Debug)]
pub enum Progress {
    /// The series has more tracks; this is the next queue index.
    Advanced {
        event: usize,
        track: String,
    },
    Complete(Award),
}

fn num(map: &HashMap<String, String>, key: &str, default: i64) -> i64 {
    map.get(key)
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

pub fn parse_series(id: &str, text: &str) -> Result<Series, String> {
    // The retail files are INI: `[Series]` for metadata and `[EventNN]` per
    // queue entry. Track ids carry a `.sit` suffix and are case-insensitive.
    let mut sections: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut current = String::new();
    for raw in text.lines() {
        let line = raw.trim_end_matches('\r').trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with("signature:") {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            current = line[1..line.len() - 1].to_ascii_lowercase();
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            sections
                .entry(current.clone())
                .or_default()
                .insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let series = sections.get("series").ok_or("missing [Series]")?;
    let mut events = Vec::new();
    let declared = num(series, "eventcount", 0).max(0) as usize;
    for i in 1..=declared.max(1) {
        let key = format!("event{i:02}");
        let Some(e) = sections.get(&key) else {
            continue;
        };
        let track = e
            .get("track")
            .map(|t| {
                let stem = t.rsplit('/').next().unwrap_or(t);
                stem.rsplit('.')
                    .next_back()
                    .unwrap_or(stem)
                    .to_ascii_uppercase()
            })
            .unwrap_or_default();
        if track.is_empty() {
            continue;
        }
        let laps = num(e, "laps", 2).clamp(1, 20) as u32;
        events.push(Event { track, laps });
    }
    if events.is_empty() {
        return Err(format!("{id}: no events"));
    }
    let classes = series
        .get("allowedclasses")
        .map(|v| v.split_whitespace().count())
        .unwrap_or(3)
        .clamp(1, 9) as u8;
    let map = &series;
    let tier = num(map, "tier", 1).clamp(1, 6) as u8;
    let purse = num(map, "purse", 5_000 * tier as i64);
    let entry_fee = num(map, "entryfee", (purse / 5).max(0));
    let max_class = num(map, "maxclass", classes as i64)
        .clamp(1, 9)
        .min(classes as i64) as u8;
    Ok(Series {
        id: id.to_string(),
        name: map.get("name").cloned().unwrap_or_else(|| id.to_string()),
        tier,
        entry_fee,
        purse,
        max_class,
        required_place: num(map, "requiredplace", 3).clamp(1, 12) as u32,
        events,
        ai_skill: 0.72 + tier as f32 * 0.045,
    })
}

pub fn points_for(place: u32) -> i64 {
    match place {
        1 => 20,
        2 => 16,
        3 => 13,
        4 => 10,
        5 => 8,
        6 => 6,
        7 => 4,
        8 => 3,
        9 => 2,
        10 => 1,
        _ => 0,
    }
}

#[derive(Clone, Debug)]
pub enum CareerError {
    Busy,
    LockedTier,
    ClassTooHigh,
    EntryFee,
    NoEvents,
}

impl Career {
    pub fn start(&mut self, index: usize, budget: &mut i64, class: u8) -> Result<(), CareerError> {
        if self.active.is_some() {
            return Err(CareerError::Busy);
        }
        let s = self.series.get(index).ok_or(CareerError::NoEvents)?.clone();
        if s.tier > self.unlocked_tiers + 1 {
            return Err(CareerError::LockedTier);
        }
        if class > s.max_class {
            return Err(CareerError::ClassTooHigh);
        }
        if *budget < s.entry_fee {
            return Err(CareerError::EntryFee);
        }
        if s.events.is_empty() {
            return Err(CareerError::NoEvents);
        }
        *budget -= s.entry_fee;
        self.standings = Some(Standings::default());
        self.active = Some(index);
        self.current_event = 0;
        self.last_award = None;
        Ok(())
    }

    /// Track id of the event the player is on right now.
    pub fn current_track(&self) -> Option<&str> {
        let active = self.active?;
        let series = self.series.get(active)?;
        series
            .events
            .get(self.current_event)
            .map(|e| e.track.as_str())
    }

    pub fn current_laps(&self) -> u32 {
        self.active
            .and_then(|a| self.series.get(a))
            .and_then(|s| s.events.get(self.current_event))
            .map(|e| e.laps)
            .unwrap_or(2)
    }

    /// Award points for one event. The purse, final standings, tier unlock and
    /// `active` release happen only after the last event in the queue.
    pub fn record_place(
        &mut self,
        place: u32,
        player: &str,
        ai: &[String],
        budget: &mut i64,
    ) -> Progress {
        let Some(active) = self.active else {
            return Progress::Complete(Award {
                final_place: 1,
                purse: 0,
                passed: false,
                unlocked_tier: None,
                series_name: String::new(),
            });
        };
        let series = self.series[active].clone();
        if let Some(st) = &mut self.standings {
            *st.points.entry(player.to_string()).or_default() += points_for(place);
            for (i, name) in ai.iter().enumerate() {
                *st.points.entry(name.clone()).or_default() += points_for(i as u32 + 2);
            }
            st.events_done += 1;
        }

        let last = self.current_event + 1 >= series.events.len();
        if !last {
            self.current_event += 1;
            let track = series.events[self.current_event].track.clone();
            return Progress::Advanced {
                event: self.current_event,
                track,
            };
        }

        // Final event done: standings are frozen now.
        let mine = self
            .standings
            .as_ref()
            .and_then(|st| st.points.get(player).copied())
            .unwrap_or(0);
        let final_place = 1 + self
            .standings
            .as_ref()
            .map(|st| st.points.values().filter(|v| **v > mine).count())
            .unwrap_or(0) as u32;
        let passed = final_place <= series.required_place;
        let purse = if passed {
            (series.purse as f32 * (1.0 - (final_place as f32 - 1.0) * 0.11)).max(0.0) as i64
        } else {
            0
        };
        *budget += purse;
        self.career_earnings += purse;

        let mut unlocked_tier = None;
        if let Some(st) = &mut self.standings {
            st.finished = true;
            st.failed = !passed;
        }
        if passed {
            self.completed_series.push(series.id.clone());
            if series.tier > self.unlocked_tiers {
                self.unlocked_tiers = series.tier;
                unlocked_tier = Some(series.tier + 1);
            }
        }
        self.active = None;
        self.current_event = 0;
        let award = Award {
            final_place,
            purse,
            passed,
            unlocked_tier,
            series_name: series.name,
        };
        self.last_award = Some(award.clone());
        Progress::Complete(award)
    }

    pub fn is_unlocked(&self, tier: u8) -> bool {
        tier <= self.unlocked_tiers + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "; Series .INI\n\n[Series]\nname=Desert Dash\nallowedClasses=CompactTruck StandardTruck Full-SizeTruck\npurse=3000\nentryfee=500\nrequiredPlace=2\neventCount=2\n\n[Event01]\ntrack=Zona.sit\nlaps=2\n\n[Event02]\ntrack=Aspen.sit\nlaps=3\n";

    #[test]
    fn parses_series() {
        let s = parse_series("desert", FILE).unwrap();
        assert_eq!(s.name, "Desert Dash");
        assert_eq!(s.events.len(), 2);
        assert_eq!(s.events[1].laps, 3);
        assert!((s.ai_skill - 0.765).abs() < 1e-4);
    }

    #[test]
    fn entry_fee_and_class_guards() {
        let s = parse_series("d", FILE).unwrap();
        let mut c = Career {
            series: vec![s],
            ..Default::default()
        };
        let mut budget = 100;
        assert!(matches!(
            c.start(0, &mut budget, 1),
            Err(CareerError::EntryFee)
        ));
        budget = 10_000;
        assert!(matches!(
            c.start(0, &mut budget, 9),
            Err(CareerError::ClassTooHigh)
        ));
        c.start(0, &mut budget, 2).unwrap();
        assert_eq!(budget, 9_500);
        assert!(matches!(c.start(0, &mut budget, 2), Err(CareerError::Busy)));
    }

    fn started() -> (Career, i64) {
        let s = parse_series("d", FILE).unwrap();
        let mut c = Career {
            series: vec![s],
            ..Default::default()
        };
        let mut budget = 10_000;
        c.start(0, &mut budget, 2).unwrap();
        (c, budget)
    }

    #[test]
    fn series_advances_through_every_event() {
        let (mut c, mut budget) = started();
        assert_eq!(c.current_track(), Some("ZONA"));
        let p = c.record_place(1, "You", &["A".into(), "B".into()], &mut budget);
        let Progress::Advanced { event, track } = p else {
            panic!("should continue: {p:?}")
        };
        assert_eq!(event, 1);
        assert_eq!(track, "ASPEN");
        assert_eq!(c.current_track(), Some("ASPEN"));
        assert!(c.active.is_some(), "series must stay open");
        // No purse yet, no unlock, not completed.
        assert_eq!(budget, 9_500);
        assert!(c.completed_series.is_empty());
        assert_eq!(c.unlocked_tiers, 0);
        assert!(c.last_award.is_none());
    }

    #[test]
    fn only_final_event_pays_and_unlocks() {
        let (mut c, mut budget) = started();
        c.record_place(1, "You", &["A".into(), "B".into()], &mut budget);
        let p = c.record_place(1, "You", &["A".into(), "B".into()], &mut budget);
        let Progress::Complete(award) = p else {
            panic!("should finalize: {p:?}")
        };
        assert!(award.passed);
        assert_eq!(award.final_place, 1);
        assert!(award.purse > 0);
        assert_eq!(award.unlocked_tier, Some(2));
        assert_eq!(budget, 9_500 + award.purse);
        assert_eq!(c.active, None);
        assert_eq!(c.completed_series, vec!["d".to_string()]);
        assert_eq!(c.unlocked_tiers, 1);
        assert!(c.standings.as_ref().unwrap().finished);
    }

    #[test]
    fn missing_the_cut_keeps_money_and_tier() {
        let (mut c, mut budget) = started();
        c.record_place(9, "You", &["A".into(), "B".into()], &mut budget);
        let p = c.record_place(9, "You", &["A".into(), "B".into()], &mut budget);
        let Progress::Complete(award) = p else {
            panic!()
        };
        assert!(!award.passed);
        assert_eq!(award.purse, 0);
        assert_eq!(budget, 9_500);
        assert_eq!(award.unlocked_tier, None);
        assert!(c.completed_series.is_empty());
        assert!(c.standings.as_ref().unwrap().failed);
    }

    #[test]
    fn events_are_queued_not_restarted() {
        let (mut c, mut budget) = started();
        c.record_place(1, "You", &["A".into()], &mut budget);
        assert_eq!(c.standings.as_ref().unwrap().events_done, 1);
        c.record_place(2, "You", &["A".into()], &mut budget);
        assert_eq!(c.standings.as_ref().unwrap().events_done, 2);
        assert_eq!(c.current_event, 0);
    }

    #[test]
    fn points_table() {
        assert_eq!(points_for(1), 20);
        assert_eq!(points_for(11), 0);
    }
}
