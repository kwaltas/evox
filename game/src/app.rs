//! Application state machine and per-frame dispatch.
//!
//! Screens are plain functions over `&mut App`; transitions go through
//! `App::go`, which enforces stack rules (Back always has a valid parent).

use std::collections::HashMap;
use std::path::PathBuf;

use macroquad::prelude::*;

use crate::assets::Assets;
use crate::audio::Audio;
use crate::career::Career;
use crate::master::Browser;
use crate::parts::{self, Shop};
use crate::physics::Input;
use crate::race::{Race, RaceConfig};
use crate::render::Renderer;
use crate::save::{Profile, SaveError, Store};
use crate::ui::screens;
use crate::ui::{Focus, Ui};
use crate::vehicle::Truck;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    BootSequence,
    Boot,
    Title,
    MainMenu,
    QuickRace,
    Career,
    Garage,
    Shop,
    Settings,
    Multiplayer,
    Browser,
    Loading,
    Race,
    Editor,
    Results,
    Credits,
    Mods,
    Quit,
}

#[derive(Clone, Debug, Default)]
pub struct RaceSetup {
    pub track: String,
    pub class: u8,
    pub laps: u32,
    pub opponents: usize,
    pub career: Option<usize>,
    /// Quick Race navigation: 0 = track list, 1 = session options.
    pub focus: u8,
    /// Selected session option while `focus` is 1.
    pub opt: u8,
    /// Factory paint for the player's truck (`colorList[]` index).
    pub paint: usize,
    /// Quick Race manufacturer filter: 0 = All, then index into the makes.
    pub brand: usize,
}

pub struct App {
    pub assets: Assets,
    pub audio: Audio,
    pub ui: Ui,
    pub focus: Focus,
    pub state: State,
    pub boot_t: f32,
    pub stack: Vec<State>,
    pub profile: Profile,
    pub store: Store,
    pub shop: Option<Shop>,
    pub trucks: Vec<Truck>,
    pub truck_errors: Vec<String>,
    pub tracks: Vec<(String, PathBuf)>,
    /// Retail display name from each level's `.SIT` (`!Race Track Name`).
    pub track_names: HashMap<String, String>,
    pub race: Option<Race>,
    pub editor: Option<crate::editor::EditorState>,
    pub setup: RaceSetup,
    pub pending: Option<RaceSetup>,
    pub browser: Browser,
    pub mods: Option<std::sync::mpsc::Receiver<crate::save::ModFile>>,
    pub mod_rx_seen: usize,
    pub toast: Option<(String, f32)>,
    pub install_id: String,
    pub career: Career,
    pub selected_truck: usize,
    pub garage_tab: usize,
    pub results: Vec<(String, f32)>,
    pub loading_step: f32,
    pub online: Option<crate::net::Client>,
    pub frame_ms: f32,
    pub show_debug: bool,
    pub quit: bool,
    pub load_error: Option<String>,
    pub race_career: Option<usize>,
    pub telemetry: Option<std::fs::File>,
    pub lb_tx: std::sync::mpsc::Sender<String>,
    pub lb_rx: std::sync::mpsc::Receiver<String>,
    pub career_progress: Option<crate::career::Progress>,
    pub render: Renderer,
    pub chunks: crate::terrain::ChunkCache,
    /// Off-screen bay for the garage turntable.
    pub garage_rt: Option<RenderTarget>,
}

pub const START_BUDGET: i64 = 25_000;
pub const MASTER_URL: &str = "http://127.0.0.1:8790";

impl App {
    pub async fn new() -> Self {
        let mut assets = Assets::load().await;
        let mut audio = Audio::new();
        audio.enabled = true;
        // Engine voice from the original bank: the running and idle
        // recordings offroad4x4 ships (`tools/evo2_sfx.py`).
        audio
            .load_engine(
                "ENG_STOCK",
                &format!(
                    "{}/{}",
                    crate::audio::SOUND_DIR,
                    crate::audio::ENGINE_RUN_WAV
                ),
                &format!(
                    "{}/{}",
                    crate::audio::SOUND_DIR,
                    crate::audio::ENGINE_IDLE_WAV
                ),
            )
            .await;
        for (name, file) in [
            // UI-* are music tracks, not clicks: menu keys use the short click.
            ("CLICK", "CHKCLICK.WAV"),
            ("CHECKPOINT", "ABCHKPT.01.WAV"),
            ("SKID_01", "SKID_01.WAV"),
            ("SKID_C1", "SKID-C1.WAV"),
            ("SPLASHDC", "WATER_SPLASH_01.WAV"),
            ("WATER_SPLASH_01", "WATER_SPLASH_01.WAV"),
            ("ICECRACK", "ICECRACK.02.WAV"),
            ("LANDING", "SUSPENSION_03.WAV"),
        ] {
            audio
                .load_sfx(name, &format!("assets/extracted/audio/sfx/SOUND/{file}"))
                .await;
        }

        // The music bank is decoded on demand (see `Audio::request_music`);
        // eighteen two-megabyte tracks at boot was a stall and a lot of RAM.

        let store = Store::new(dirs::profile().join("profile.json"));
        let mut load_error = None;
        let profile = match store.load() {
            Ok(p) => p,
            Err(SaveError::Io(_)) => Profile::default(),
            Err(e) => {
                load_error = Some(e.to_string());
                store.load_backup().unwrap_or_default()
            }
        };
        audio.master = profile.settings.master_volume.clamp(0.0, 1.0);
        audio.music_volume = profile.settings.music_volume.clamp(0.0, 1.0);
        // Opening-movie soundtrack, when the movie has been converted. The
        // request is queued after the profile so `intro` starts at the saved
        // volume even though the decode finishes a few frames later.
        if crate::classic::intro_frames() > 0 {
            audio.request_music("INTRO", &crate::classic::intro_audio_path());
        }

        // Evo 1 roster, then the Evo 2-only trucks (`tools/evo2_trucks.py`).
        let (trucks, truck_errors) = crate::vehicle::load_roster(std::path::Path::new("."));
        let tracks = assets.list_tracks();
        let track_names: HashMap<String, String> = tracks
            .iter()
            .filter_map(|(id, _)| assets.track_display_name(id).map(|n| (id.clone(), n)))
            .collect();

        // Restore in-flight progress verbatim; only parse the catalog on a
        // fresh profile so a mid-series save keeps its queue position.
        let mut career = profile.career.clone();
        if career.series.is_empty() {
            if let Ok(rd) = std::fs::read_dir("assets/extracted/data/career/SERIES") {
                for p in rd.filter_map(|e| e.ok()).map(|e| e.path()) {
                    if p.extension().map(|e| e.eq_ignore_ascii_case("ini")) != Some(true) {
                        continue;
                    }
                    if let Ok(bytes) = std::fs::read(&p) {
                        let text = String::from_utf8_lossy(&bytes);
                        let id = p
                            .file_stem()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_default();
                        if let Ok(s) = crate::career::parse_series(&id, &text) {
                            career.series.push(s);
                        }
                    }
                }
            }
            // Retail files carry no explicit tier; order is the progression.
            career.series.sort_by(|a, b| a.id.cmp(&b.id));
            for (i, s) in career.series.iter_mut().enumerate() {
                s.tier = (1 + i / 3).min(6) as u8;
                s.ai_skill = 0.72 + s.tier as f32 * 0.045;
            }
        }
        career.unlocked_tiers = career.unlocked_tiers.max(profile.career.unlocked_tiers);

        let lb = std::sync::mpsc::channel::<String>();
        let install_id = format!(
            "{:016x}",
            fnv(&profile.name.as_bytes()) ^ fnv(env!("CARGO_PKG_VERSION").as_bytes())
        );

        let mut ui = Ui::default();
        if let Some(t) = assets.load_font() {
            ui.set_font(t);
        }
        let _ = assets.load_needle();

        Self {
            assets,
            audio,
            ui,
            focus: Focus::default(),
            state: State::BootSequence,
            boot_t: 0.0,
            stack: Vec::new(),
            profile,
            store,
            shop: None,
            trucks,
            truck_errors,
            tracks,
            track_names,
            race: None,
            editor: None,
            setup: RaceSetup {
                class: 1,
                laps: 2,
                opponents: 5,
                ..Default::default()
            },
            pending: None,
            browser: Browser::new(MASTER_URL),
            mods: Some(crate::save::watch_mods("assets/mods")),
            mod_rx_seen: 0,
            toast: None,
            install_id,
            career,
            selected_truck: 0,
            garage_tab: 0,
            results: Vec::new(),
            loading_step: 0.0,
            online: None,
            frame_ms: 16.0,
            show_debug: false,
            quit: false,
            load_error,
            race_career: None,
            telemetry: std::env::var("EVOX_TELEMETRY")
                .ok()
                .and_then(|p| std::fs::File::create(p).ok()),
            lb_tx: lb.0,
            lb_rx: lb.1,
            career_progress: None,
            render: Renderer::new(),
            chunks: crate::terrain::ChunkCache::new(),
            garage_rt: Some(render_target(420, 340)),
        }
    }

    /// Track for the current screen; `EVO*` for races keyed by track.
    fn music_track(&self) -> Option<String> {
        let race_evo = || {
            let track = self
                .race
                .as_ref()
                .map(|r| r.track_id.clone())
                .or_else(|| self.pending.as_ref().map(|p| p.track.clone()))?;
            let h: usize = track.bytes().map(|b| b as usize).sum();
            let bank = [1usize, 2, 3, 4, 6, 7, 8, 9, 10];
            Some(format!("EVO{}", bank[h % bank.len()]))
        };
        let menu = |i: usize| Some(format!("UI-{:02}", 1 + i % 9));
        match self.state {
            State::Boot | State::BootSequence | State::Title => None,
            State::Loading => race_evo(),
            State::Race => race_evo(),
            State::MainMenu => menu(0),
            State::QuickRace => menu(1),
            State::Career => menu(2),
            State::Garage => menu(3),
            State::Shop => menu(4),
            State::Settings => menu(5),
            State::Multiplayer | State::Browser => menu(6),
            State::Results => menu(7),
            State::Editor => race_evo(),
            State::Credits => Some("UI-10".into()),
            State::Mods => menu(8),
            State::Quit => None,
        }
    }

    fn sync_music(&mut self) {
        self.audio.pump_music();
        match self.music_track() {
            Some(name) => {
                self.audio
                    .request_music(&name, &crate::audio::music_path(&name));
                self.audio.play_music(&name);
            }
            None => self.audio.stop_music(),
        }
        self.audio.duck_music(self.state == State::Race);
    }

    pub fn go(&mut self, next: State) {
        if next == self.state {
            return;
        }
        if next == State::Quit {
            self.quit = true;
            return;
        }
        if next == State::Race || next == State::Loading {
            // Race owns its own back target.
            self.stack.clear();
        } else if next != State::Boot {
            self.stack.push(self.state);
        }
        self.focus.reset();
        self.state = next;
        self.toast = None;
    }

    pub fn back(&mut self) {
        if is_key_pressed(KeyCode::Escape) {
            self.audio.ui_back(get_time());
            match self.state {
                State::Race => {
                    self.save_profile();
                    self.state = State::MainMenu;
                }
                State::Shop => self.state = State::Garage,
                State::Results => self.state = State::MainMenu,
                _ => {
                    self.state = self.stack.pop().unwrap_or(State::MainMenu);
                }
            }
            self.focus.reset();
        }
    }

    pub fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), 3.0));
    }

    pub fn save_profile(&mut self) {
        if let Err(e) = self.store.save(&self.profile) {
            self.toast(format!("Save failed: {e}"));
        }
    }

    pub fn current_truck(&self) -> Option<&Truck> {
        self.garage_truck()
    }

    /// Owned trucks first, then the whole retail catalog, so every body can be
    /// selected without buying it. `selected_truck` indexes this list.
    pub fn garage_files(&self) -> Vec<&Truck> {
        let mut out: Vec<&Truck> = Vec::new();
        for file in &self.profile.garage {
            if let Some(t) = self.trucks.iter().find(|t| t.file.ends_with(file.as_str())) {
                out.push(t);
            }
        }
        for t in &self.trucks {
            if !out.iter().any(|o| o.file == t.file) {
                out.push(t);
            }
        }
        out
    }

    pub fn is_owned(&self, truck: &Truck) -> bool {
        self.profile
            .garage
            .iter()
            .any(|f| truck.file.ends_with(f.as_str()))
    }

    fn garage_truck(&self) -> Option<&Truck> {
        self.garage_files().get(self.selected_truck).copied()
    }

    pub fn open_shop(&mut self) {
        let Some(t) = self.garage_truck().cloned() else {
            self.toast("No truck selected");
            return;
        };
        let text = std::fs::read(&self.assets.parts_txt)
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .unwrap_or_default();
        let loc = std::fs::read(&self.assets.parts_loc)
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .unwrap_or_default();
        let (mut catalog, errors) = parts::parse_catalog(&text);
        parts::attach_descriptions(&mut catalog, &loc);
        if !errors.is_empty() {
            self.toast(format!("{} catalog issues", errors.len()));
        }
        let mut shop = Shop::new(catalog, self.profile.budget, &t.file, t.class_id);
        shop.owned = self.profile.owned.clone();
        // Restore installed parts for this truck.
        for (key, code) in &self.profile.equipped {
            if key.starts_with(&t.file) {
                if let Some(slot) = key.rsplit(':').next().and_then(parts::PartType::from_label) {
                    shop.equipped.insert(slot, code.clone());
                }
            }
        }
        shop.recompute();
        self.shop = Some(shop);
        self.go(State::Shop);
    }

    pub fn start_loading(&mut self, career_series: Option<usize>) {
        if self.garage_truck().is_none() {
            self.toast("Pick a truck in the Garage first");
            return;
        }
        let track = if career_series.is_some() {
            self.career.current_track().map(|t| t.to_string())
        } else {
            Some(self.setup.track.clone())
        };
        let laps = if career_series.is_some() {
            self.career.current_laps()
        } else {
            self.setup.laps
        };
        let Some(track) = track.filter(|t| !t.is_empty()) else {
            self.toast("Pick a track");
            return;
        };
        self.pending = Some(RaceSetup {
            track,
            class: self.setup.class,
            laps,
            opponents: self.setup.opponents,
            career: career_series,
            paint: self.setup.paint,
            ..Default::default()
        });
        self.race_career = career_series;
        self.career_progress = None;
        self.loading_step = 0.0;
        self.go(State::Loading);
    }

    /// Advance to the queued event after a finished career race. The entry fee
    /// was charged once in `Career::start`, so this only re-arms the session.
    pub fn start_next_event(&mut self) {
        let Some(index) = self.career.active else {
            self.toast("No active series");
            return;
        };
        self.start_loading(Some(index));
    }

    async fn build_race(&mut self, setup: RaceSetup) -> Result<(), String> {
        let track = self.assets.load_track(&setup.track)?;
        let lines = self.assets.ai_lines(&setup.track);
        let mut player = self.garage_truck().cloned().ok_or("no truck")?;
        player.paint_index = setup.paint % player.paints.len().max(1);
        let mut ai_pool: Vec<Truck> = self
            .trucks
            .iter()
            .filter(|t| t.class_id <= setup.class as i64 + 1 && t.file != player.file)
            .cloned()
            .collect();
        if ai_pool.is_empty() {
            ai_pool.push(player.clone());
        }
        let skill = self
            .career
            .series
            .get(setup.career.unwrap_or(usize::MAX))
            .map(|s| s.ai_skill)
            .unwrap_or(0.8);
        let mut race = Race::build(
            RaceConfig {
                track_id: &setup.track,
                class: setup.class,
                laps: setup.laps,
                opponents: setup.opponents,
                player_truck: player,
                ai_vehicles: ai_pool,
                lines,
                skill,
            },
            track,
        )?;
        race.player.mods = self.shop.as_ref().map(|s| s.mods).unwrap_or_default();
        self.race = Some(race);
        Ok(())
    }

    pub fn update(&mut self, dt: f32) {
        self.frame_ms = self.frame_ms * 0.9 + dt * 1000.0 * 0.1;
        if let Some((_, t)) = &mut self.toast {
            *t -= dt;
            if *t <= 0.0 {
                self.toast = None;
            }
        }
        self.back();
        self.sync_music();
        match self.state {
            State::Loading => {
                self.loading_step += dt;
                if self.loading_step > 0.35 {
                    if let Some(setup) = self.pending.take() {
                        match futures_block(self.build_race_sync(setup.clone())) {
                            Ok(()) => {
                                self.audio.play("CLICK", 0.4, get_time(), 0.1);
                                self.state = State::Race;
                                self.focus.reset();
                            }
                            Err(e) => {
                                self.toast(e);
                                self.state = State::QuickRace;
                            }
                        }
                    }
                }
            }
            State::Race => {
                // F12 detaches into the track editor (physics/AI frozen).
                if is_key_pressed(KeyCode::F12) {
                    if let Some(r) = &self.race {
                        self.editor = Some(crate::editor::EditorState::from_chase(
                            r.cam_pos, r.cam_look,
                        ));
                    }
                    self.state = State::Editor;
                    return;
                }
                // H: the original's "Rescue", back on the course upright.
                if is_key_pressed(KeyCode::H) {
                    if let Some(r) = &mut self.race {
                        if r.phase == crate::race::Phase::Racing {
                            r.rescue_player();
                        }
                    }
                }
                // 4 cycles the transfer case: 2WD, 4WD-HI, 4WD-LO (offroad4x4).
                if is_key_pressed(KeyCode::Key4) {
                    if let Some(r) = &mut self.race {
                        r.drive_mode = (r.drive_mode + 1) % 3;
                    }
                }
                let input = self.read_driving_input();
                let now = get_time();
                if let Some(race) = &mut self.race {
                    race.tick(dt, input, &mut self.audio, now);
                    if let Some(f) = &mut self.telemetry {
                        use std::io::Write;
                        let _ = writeln!(
                            f,
                            "{{\"t\":{:.4},\"speed\":{:.3},\"rpm\":{:.1},\"slip\":{:.3},\"suspension\":{:.4},\"gear\":{},\"x\":{:.2},\"y\":{:.2},\"z\":{:.2},\"vy\":{:.3},\"roll\":{:.2},\"pitch\":{:.2},\"grounded\":{},\"air\":{:.3},\"ground_y\":{:.2},\"cam_y\":{:.2},\"cam_ground\":{:.2},\"phase\":\"{:?}\"}}",
                            race.time,
                            race.player.speed_kmh(),
                            race.player.rpm,
                            race.player.skid,
                            race.player.wheels[0].compression,
                            race.player.gear,
                            race.player.pos.x,
                            race.player.pos.y,
                            race.player.pos.z,
                            race.player.vel.y,
                            race.player.roll_deg(),
                            race.player.pitch_deg(),
                            race.player.wheels.iter().filter(|w| w.grounded).count(),
                            race.player.airtime,
                            race.track.height_at(race.player.pos.x, race.player.pos.z),
                            race.cam_pos.y,
                            race.track.height_at(race.cam_pos.x, race.cam_pos.z),
                            race.phase
                        );
                    }
                    if race.phase == crate::race::Phase::Finished && race.replay.timer > 1.0 {
                        let place = race.place();
                        self.finish_race(place);
                    }
                }
            }
            State::Editor => {
                // F12 returns to the frozen race.
                if is_key_pressed(KeyCode::F12) {
                    self.state = State::Race;
                    return;
                }
                let mut placed = false;
                let mut save = false;
                {
                    let ed = self.editor.as_mut();
                    let race = self.race.as_mut();
                    if let (Some(ed), Some(race)) = (ed, race) {
                        ed.update(dt);
                        ed.update_cursor(&race.track);
                        // Left click drops a copy of the track's first prop at
                        // the cursor (a dummy edit proving cursor -> world).
                        if is_mouse_button_pressed(MouseButton::Left) {
                            if let (Some(c), Some(model)) =
                                (ed.cursor, race.track.props.first().map(|p| p.model.clone()))
                            {
                                ed.placed.push(crate::terrain::Prop {
                                    pos: vec3(c.x, race.track.height_at_world(c.x, c.z), c.z),
                                    yaw: 0.0,
                                    model,
                                    ..Default::default()
                                });
                                placed = true;
                            }
                        }
                        if is_key_pressed(KeyCode::F5) {
                            save = true;
                        }
                    }
                }
                if placed {
                    self.toast("Prop placed");
                }
                if save {
                    let res = match (self.editor.as_ref(), self.race.as_ref()) {
                        (Some(ed), Some(race)) => {
                            Some(crate::editor::save_track_overlay(&race.track, &ed.placed))
                        }
                        _ => None,
                    };
                    match res {
                        Some(Ok(p)) => self.toast(format!("Saved {}", p.display())),
                        Some(Err(e)) => self.toast(format!("Save failed: {e}")),
                        None => {}
                    }
                }
            }
            _ => {}
        }
        self.poll_mods();
        self.poll_leaderboard();
    }

    fn build_race_sync(&mut self, setup: RaceSetup) -> Result<(), String> {
        // Track/AI/truck loading is synchronous I/O; the loading screen gives
        // the frame loop a chance to draw twice before this runs.
        let track = self.assets.load_track(&setup.track)?;
        let lines = self.assets.ai_lines(&setup.track);
        let mut player = self.garage_truck().cloned().ok_or("no truck")?;
        player.paint_index = setup.paint % player.paints.len().max(1);
        let mut ai_pool: Vec<Truck> = self
            .trucks
            .iter()
            .filter(|t| t.class_id <= setup.class as i64 + 1 && t.file != player.file)
            .cloned()
            .collect();
        if ai_pool.is_empty() {
            ai_pool.push(player.clone());
        }
        let skill = setup
            .career
            .and_then(|i| self.career.series.get(i))
            .map(|s| s.ai_skill)
            .unwrap_or(0.8);
        let mut race = Race::build(
            RaceConfig {
                track_id: &setup.track,
                class: setup.class,
                laps: setup.laps,
                opponents: setup.opponents,
                player_truck: player,
                ai_vehicles: ai_pool,
                lines,
                skill,
            },
            track,
        )?;
        race.player.mods = self.shop.as_ref().map(|s| s.mods).unwrap_or_default();
        self.assets.track_albedo(&race.track);
        self.assets.track_atlas(&mut race.track);
        self.assets.track_sky(&mut race.track);
        // Warm the field's meshes while the loading screen is still up.
        let bases: Vec<String> = std::iter::once(race.player.truck.model_base.clone())
            .chain(
                race.opponents
                    .iter()
                    .map(|o| o.vehicle.truck.model_base.clone()),
            )
            .collect();
        for base in bases {
            let _ = self.render.model_length(&mut self.assets, &base);
        }
        self.race = Some(race);
        Ok(())
    }

    fn read_driving_input(&self) -> Input {
        Input {
            throttle: if is_key_down(KeyCode::W) || is_key_down(KeyCode::Up) {
                1.0
            } else {
                0.0
            },
            brake: if is_key_down(KeyCode::S) || is_key_down(KeyCode::Down) {
                1.0
            } else {
                0.0
            },
            steer: (if is_key_down(KeyCode::D) || is_key_down(KeyCode::Right) {
                1.0
            } else {
                0.0
            }) - (if is_key_down(KeyCode::A) || is_key_down(KeyCode::Left) {
                1.0
            } else {
                0.0
            }),
            handbrake: is_key_down(KeyCode::Space),
            shift_up: is_key_pressed(KeyCode::E),
            shift_down: is_key_pressed(KeyCode::Q),
            drive_mode: self.race.as_ref().map_or(1, |r| r.drive_mode),
        }
    }

    fn finish_race(&mut self, place: u32) {
        let Some(race) = &self.race else { return };
        let time = race.finished_at.unwrap_or(race.time);
        let track = race.track_id.clone();
        let mut prize = 0;
        let mut career_row = None;
        if self.career.active.is_some() {
            let names: Vec<String> = self.trucks.iter().take(5).map(|t| t.name.clone()).collect();
            let progress = self
                .career
                .record_place(place, "You", &names, &mut self.profile.budget);
            self.profile.career = self.career.clone();
            match &progress {
                crate::career::Progress::Advanced { track, event } => {
                    career_row = Some(format!(
                        "Event {}/{} cleared — next: {}",
                        event,
                        self.career
                            .series
                            .get(self.career.active.unwrap_or(0))
                            .map(|s| s.events.len())
                            .unwrap_or(0),
                        track
                    ));
                }
                crate::career::Progress::Complete(award) => {
                    prize = award.purse;
                    career_row = Some(format!(
                        "{}: P{} — {}",
                        award.series_name,
                        award.final_place,
                        if award.passed {
                            "series won"
                        } else {
                            "cut missed"
                        }
                    ));
                }
            }
            self.career_progress = Some(progress);
        } else {
            prize = match place {
                1 => 1500,
                2 => 900,
                3 => 500,
                _ => 100,
            };
            self.profile.budget += prize;
        }
        let best = self
            .profile
            .best_times
            .entry(track.clone())
            .or_insert(f32::MAX);
        if time < *best {
            *best = time;
        }
        self.save_profile();
        self.submit_time(&track, self.setup.class, (time * 1000.0) as u32);
        self.results = self
            .race
            .as_ref()
            .map(|r| r.standings())
            .unwrap_or_default();
        if let Some(row) = career_row {
            self.results.insert(0, (row, 0.0));
        }
        if prize > 0 {
            self.results.insert(0, (format!("Prize ${prize}"), 0.0));
        }
        self.state = State::Results;
        self.audio.stop_engine("ENG_STOCK");
    }

    fn submit_time(&mut self, track: &str, class: u8, time_ms: u32) {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(format!("{track}|{class}|{time_ms}|{}", self.profile.name));
        let replay = h
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let client = crate::leaderboard::Client::new(
            crate::app::MASTER_URL.replace("8790", "8791"),
            &self.install_id,
            &self.profile.name,
        );
        let rec = client.make(track, class, time_ms, &replay);
        let tx = self.lb_tx.clone();
        std::thread::spawn(move || {
            let msg = match client.submit(&rec) {
                Ok(_) => "Time submitted to leaderboard".to_string(),
                Err(e) => format!("Leaderboard: {e}"),
            };
            let _ = tx.send(msg);
        });
    }

    fn poll_leaderboard(&mut self) {
        while let Ok(msg) = self.lb_rx.try_recv() {
            self.toast(msg);
        }
    }

    fn poll_mods(&mut self) {
        let Some(rx) = &self.mods else { return };
        let mut added = 0;
        while let Ok(m) = rx.try_recv() {
            added += 1;
            if m.kind == crate::save::ModKind::Vehicle {
                if let Ok(text) = std::fs::read_to_string(&m.path) {
                    if let Ok(t) = crate::vehicle::parse(&m.path.to_string_lossy(), &text) {
                        if !self.trucks.iter().any(|e| e.file == t.file) {
                            self.profile.garage.push(t.file.clone());
                            self.trucks.push(t);
                        }
                    }
                }
            }
        }
        if added > 0 {
            self.mod_rx_seen += added;
            self.toast(format!("{added} mod file(s) loaded"));
        }
    }

    pub fn draw(&mut self) {
        clear_background(Color::new(0.02, 0.025, 0.035, 1.0));
        match self.state {
            State::BootSequence => screens::bootsequence(self),
            State::Boot => screens::boot(self),
            State::Title => screens::title(self),
            State::MainMenu => screens::main_menu(self),
            State::QuickRace => screens::quick_race(self),
            State::Career => screens::career(self),
            State::Garage => screens::garage(self),
            State::Shop => screens::shop(self),
            State::Settings => screens::settings(self),
            State::Multiplayer => screens::multiplayer(self),
            State::Browser => screens::browser(self),
            State::Loading => screens::loading(self),
            State::Race => screens::race(self),
            State::Editor => screens::editor(self),
            State::Results => screens::results(self),
            State::Credits => screens::credits(self),
            State::Mods => screens::mods(self),
            State::Quit => {}
        }
        if let Some((msg, t)) = &self.toast {
            let a = (*t / 0.5).clamp(0.0, 1.0);
            draw_rectangle(
                0.0,
                64.0,
                screen_width(),
                38.0,
                Color::new(0.35, 0.12, 0.08, 0.85 * a),
            );
            self.ui.centered(
                screen_width() * 0.5,
                89.0,
                20.0,
                msg,
                Color::new(1.0, 0.95, 0.9, a),
            );
        }
        if self.show_debug {
            let s = format!(
                "{:.1} ms  state={:?} trucks={} tracks={} parts={}",
                self.frame_ms,
                self.state,
                self.trucks.len(),
                self.tracks.len(),
                self.shop
                    .as_ref()
                    .map(|s| s.catalog.parts.len())
                    .unwrap_or(0)
            );
            draw_rectangle(
                0.0,
                0.0,
                screen_width(),
                22.0,
                Color::new(0.0, 0.0, 0.0, 0.6),
            );
            self.ui
                .text(8.0, 16.0, 15.0, &s, Color::new(0.6, 1.0, 0.6, 1.0));
        }
    }
}

fn fnv(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Minimal executor: the loading screen only needs already-resolved futures.
fn futures_block<T>(v: T) -> T {
    v
}

/// Profile directory handling without an extra crate.
mod dirs {
    use std::path::PathBuf;

    pub fn profile() -> PathBuf {
        let base = std::env::var("EVOX_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME")
                    .or_else(|_| std::env::var("USERPROFILE"))
                    .unwrap_or_else(|_| ".".into());
                PathBuf::from(home).join(".evox")
            });
        base
    }
}

pub fn unused(_: &HashMap<String, String>) {}
