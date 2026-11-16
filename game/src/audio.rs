//! Dynamic audio: engine note by RPM, surface-dependent one-shots, music.
//!
//! Ground truth for the mixer path is `quad-snd` itself: macroquad hands every
//! buffer to `quad_snd::mixer::load_samples_from_file`, which *resamples to
//! 44100 stereo* with a documented "stupid nearest-neighbor resampler" and
//! then sums all active voices without any headroom. Both facts drive this
//! module:
//!
//! * **Every buffer is produced at 44100 Hz.** At that rate the library
//!   resampler is a no-op, so its aliasing never touches our samples. The
//!   conversion from the bank's native 22 050 Hz is done here with a
//!   windowed-sinc kernel, which is what the retail bank expects (the
//!   original mixer ran the 22 kHz bank at 22 kHz).
//! * **Pitch is applied with the same kernel**, scaled by the ratio, so a
//!   2.4× engine note is band-limited instead of shredded by linear
//!   interpolation.
//! * **Cross-faded engine voices must sum to unity.** Seven loops all playing
//!   at once used to reach 1.12 before the master fader; the mixer clips the
//!   sum in the integer conversion, which is the distorted, chopping engine.
//!
//! Engine construction is the offroad4x4 prototype's: the recording is cut
//! into click-free loops at the `ENGINE_RPMS` points, all loops play
//! continuously, and only their volumes move, so a gear change never starts
//! or stops a voice.
//!
//! Music is decoded lazily with an LRU of `MUSIC_CACHE` tracks. The bank is
//! 8-bit and ~2 MB per song; eagerly decoding eighteen of them at boot (into
//! 44.1 kHz stereo f32 through the library decoder) was both a multi-second
//! stall and hundreds of megabytes of resident memory.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use macroquad::audio::{
    load_sound_from_bytes, play_sound, set_sound_volume, stop_sound, PlaySoundParams, Sound,
};
use macroquad::prelude::*;

/// Rpm points of the pre-rendered engine loops: index 0 is the dedicated
/// `ENG_IDLE3.WAV` idle voice, and indices 1..=12 are fine-grained band-limited
/// pitch steps of `ENG_STOCK.WAV`. Only one running step is active at steady
/// RPM (selected with hysteresis) so two copies of the same waveform never
/// comb-filter against each other.
pub const ENGINE_RPMS: [f32; 13] = [
    850.0, 1250.0, 1550.0, 1880.0, 2240.0, 2630.0, 3050.0, 3500.0, 3980.0, 4500.0, 5080.0, 5720.0,
    6450.0,
];
/// Rpm the running recording (`ENG_STOCK.WAV`) was captured near.
const REF_RPM: f32 = 3100.0;
/// Hysteresis band (RPM) around step midpoints so steady driving never toggles
/// between adjacent pitch steps.
const STEP_HYSTERESIS_RPM: f32 = 75.0;
/// Rate every buffer is produced at; see the module docs.
pub const OUTPUT_RATE: u32 = 44_100;
/// How many music tracks stay resident (current + one back-reference).
const MUSIC_CACHE: usize = 2;
/// Root of the extracted sound bank.
pub const SOUND_DIR: &str = "assets/extracted/audio/sfx/SOUND";
/// UI sounds are attenuated to ~40 % of their former level: quiet, pleasant
/// clicks rather than loud beeps. `Audio::ui_*` applies this scale.
const UI_SOUND_SCALE: f32 = 0.4;
/// Running engine recording. `ENG_STOCK.WAV` is what offroad4x4 converts to
/// `engine_run.wav` (`tools/evo2_sfx.py`); `ENG_STOCK2.WAV` in the same bank
/// is a saturated buzz (crest factor 1.07, ~80 % of samples at full scale)
/// that RMS-levelling turns into a harsh, static-like tone.
pub const ENGINE_RUN_WAV: &str = "ENG_STOCK.WAV";
/// Idle recording (offroad4x4 `engine_idle.wav`).
pub const ENGINE_IDLE_WAV: &str = "ENG_IDLE3.WAV";

/// Bank file for a music name: `UI-01`, `EVO3` and `INTRO` all live in the
/// same extracted folder (the intro is the converted movie track).
pub fn music_path(name: &str) -> String {
    if name == "INTRO" {
        crate::classic::intro_audio_path()
    } else {
        format!("{SOUND_DIR}/{name}.WAV")
    }
}

pub struct EngineVoice {
    loops: Vec<Sound>,
    playing: bool,
    smoothed_rpm: f32,
    active_step: usize,
    gains: Vec<f32>,
}

// ---------------------------------------------------------------------------
// Sample-rate conversion
// ---------------------------------------------------------------------------

/// Taps of the windowed-sinc kernel (half on each side of the read point).
const SINC_HALF: i32 = 8;

fn sinc(x: f32) -> f32 {
    if x.abs() < 1e-6 {
        1.0
    } else {
        let p = std::f32::consts::PI * x;
        p.sin() / p
    }
}

/// Hann-windowed sinc, `cutoff` in cycles per input sample (≤ 1).
fn kernel(x: f32, cutoff: f32) -> f32 {
    let w = 0.5 + 0.5 * (std::f32::consts::PI * x / SINC_HALF as f32).cos();
    cutoff * sinc(cutoff * x) * w
}

/// Resample `src` by `step` input samples per output sample, band-limited to
/// `1/step` when decimating. `step` > 1 compresses (higher pitch), < 1
/// stretches. Weights are normalised so the level does not track the ratio.
fn resample_by(src: &[f32], step: f32) -> Vec<f32> {
    if src.is_empty() || step <= 0.0 || step.is_nan() {
        return Vec::new();
    }
    let cutoff = if step > 1.0 { 1.0 / step } else { 1.0 };
    let out_len = (src.len() as f32 / step).floor() as usize;
    let mut out = Vec::with_capacity(out_len);
    let last = src.len() as i64 - 1;
    for n in 0..out_len {
        let centre = n as f32 * step;
        let base = centre.floor();
        let mut acc = 0.0f32;
        let mut norm = 0.0f32;
        for k in -SINC_HALF + 1..=SINC_HALF {
            let i = base as i64 + k as i64;
            if i < 0 || i > last {
                continue;
            }
            let w = kernel(centre - i as f32, cutoff);
            acc += src[i as usize] * w;
            norm += w;
        }
        out.push(if norm > 1e-6 { acc / norm } else { 0.0 });
    }
    out
}

/// Convert to `OUTPUT_RATE` from `from`.
fn to_output_rate(src: &[f32], from: u32) -> Vec<f32> {
    if from == 0 || from == OUTPUT_RATE {
        src.to_vec()
    } else {
        resample_by(src, from as f32 / OUTPUT_RATE as f32)
    }
}

/// One-pole low-pass to take the fizz off a pitched note.
fn low_pass(x: &[f32], alpha: f32) -> Vec<f32> {
    let mut out = Vec::with_capacity(x.len());
    let mut y = x.first().copied().unwrap_or(0.0);
    for &s in x {
        y += alpha * (s - y);
        out.push(y);
    }
    out
}

/// Select the active running pitch step (`1..ENGINE_RPMS.len()`) with
/// hysteresis around step midpoints so steady driving never toggles between
/// two `ENG_STOCK` buffers or causes dual-voice comb filtering.
fn select_run_step(current: usize, rpm: f32) -> usize {
    let last = ENGINE_RPMS.len() - 1;
    let mut step = current.clamp(1, last);
    while step < last {
        let mid = 0.5 * (ENGINE_RPMS[step] + ENGINE_RPMS[step + 1]);
        if rpm > mid + STEP_HYSTERESIS_RPM {
            step += 1;
        } else {
            break;
        }
    }
    while step > 1 {
        let mid = 0.5 * (ENGINE_RPMS[step - 1] + ENGINE_RPMS[step]);
        if rpm < mid - STEP_HYSTERESIS_RPM {
            step -= 1;
        } else {
            break;
        }
    }
    step
}

/// Target voice weight for loop `i` at `rpm` using the TRX two-source
/// (`ENG_IDLE3.WAV` + single active `ENG_STOCK.WAV` step) state machine.
/// Idle (index 0) and Run (index `active_step`) are distinct uncorrelated
/// recordings, so blending idle-to-load never comb-filters; within the run
/// range only `active_step` receives weight `1.0 - idle_w`, so all weights
/// sum to `1.0` and two pitched copies of `ENG_STOCK` never play together.
fn engine_weight_state(i: usize, rpm: f32, throttle: f32, active_step: usize) -> f32 {
    let rpm_clamped = rpm.clamp(ENGINE_RPMS[0], ENGINE_RPMS[ENGINE_RPMS.len() - 1]);
    let idle_span = (ENGINE_RPMS[2] - ENGINE_RPMS[0]).max(1.0);
    let rpm_idle = (1.0 - (rpm_clamped - ENGINE_RPMS[0]) / idle_span).clamp(0.0, 1.0);
    let load_trim = (1.0 - throttle.clamp(0.0, 1.0) * 0.65).clamp(0.0, 1.0);
    let idle_w = (rpm_idle * load_trim).clamp(0.0, 1.0);
    let step = active_step.clamp(1, ENGINE_RPMS.len() - 1);
    if i == 0 {
        idle_w
    } else if i == step {
        1.0 - idle_w
    } else {
        0.0
    }
}

/// Stateless helper used by unit tests to query steady-state voice weights.
fn engine_weight(i: usize, rpm: f32) -> f32 {
    let step = select_run_step(1, rpm);
    engine_weight_state(i, rpm, 0.0, step)
}

/// Make a recording loop without a click (raised-cosine cross-fade of the tail
/// into the head) and normalise its RMS level.
fn make_loop(x: &[f32], rms_target: f32) -> Vec<f32> {
    let n = x.len();
    if n < 16 {
        return x.to_vec();
    }
    let m = (n / 6).clamp(1, n / 2);
    let l = n - m;
    let mut out: Vec<f32> = x[..l].to_vec();
    for i in 0..m {
        let t = i as f32 / m as f32;
        let w = 0.5 - 0.5 * (std::f32::consts::PI * t).cos();
        out[i] = x[i] * w + x[l + i] * (1.0 - w);
    }
    let mean = out.iter().sum::<f32>() / l as f32;
    out.iter_mut().for_each(|v| *v -= mean);
    let rms = (out.iter().map(|v| v * v).sum::<f32>() / l as f32)
        .sqrt()
        .max(1e-6);
    out.iter_mut()
        .for_each(|v| *v = (*v * rms_target / rms).clamp(-0.92, 0.92));
    out
}

/// Engine note for an rpm point, in `OUTPUT_RATE`: the idle recording at the
/// bottom, the running recording pitched from `REF_RPM` for the rest. The
/// pitch shift is band-limited and gently low-passed so the top of the range
/// is warm and free of aliasing.
fn engine_note(rpm: f32, idle: &[f32], run: &[f32]) -> Vec<f32> {
    let filtered = if rpm <= ENGINE_RPMS[0] {
        low_pass(&low_pass(idle, 0.30), 0.40)
    } else {
        let ratio = (rpm / REF_RPM).clamp(0.40, 2.25);
        let lp_alpha = (0.32 / ratio.sqrt()).clamp(0.16, 0.36);
        let first = low_pass(&resample_by(run, ratio), lp_alpha);
        if rpm > 3500.0 {
            low_pass(&low_pass(&first, 0.32), 0.36)
        } else {
            low_pass(&first, 0.45)
        }
    };
    make_loop(&filtered, ENGINE_RMS)
}

/// Level of one engine loop. Balanced below background music so the mixer
/// never clips even when skid/splash one-shots fire.
const ENGINE_RMS: f32 = 0.22;

// ---------------------------------------------------------------------------
// WAV
// ---------------------------------------------------------------------------

/// Decoded WAV: rate, channels and interleaved mono samples.
struct Pcm {
    rate: u32,
    samples: Vec<f32>,
}

/// Microsoft ADPCM (format tag 0x0002) default coefficient pairs.
const ADPCM_COEFS: [(i32, i32); 7] = [
    (256, 0),
    (512, -256),
    (0, 0),
    (192, 64),
    (240, 0),
    (460, -208),
    (392, -232),
];
const ADPCM_ADAPT: [i32; 16] = [
    230, 230, 230, 230, 307, 409, 512, 614, 768, 614, 512, 409, 307, 230, 230, 230,
];

/// Yamaha ADPCM (format tag 0x0020 / 32) tables.
///
/// The nibble is a *step index*, not a multiplier: `diff` picks the signed
/// delta and `index_shift` moves a cursor 0..=88 through the 89-entry
/// `YAMAHA_STEP` table (YMZ/Yamaha CE codec; `Sounds.Samples.cxx` refuses these
/// WAVs off WinCE because the platform decodes them in hardware). Used by
/// `SKID-D1N.WAV` in the retail sound bank.
const YAMAHA_DIFFLOOKUP: [i32; 16] = [1, 3, 5, 7, 9, 11, 13, 15, -1, -3, -5, -7, -9, -11, -13, -15];
const YAMAHA_INDEX_SHIFT: [i32; 16] = [-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8];
/// Step sizes in 1/8 sample units; index 0 is the quietest, 88 the loudest.
/// MAME's `ymz280b` table stops at 32767 on index 80 and declares 89 slots;
/// the last eight repeat the maximum so the documented 0..=88 clamp is safe.
const YAMAHA_STEP: [i32; 89] = [
    16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66, 73, 80, 88, 97, 107, 118, 130,
    143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658, 724, 796,
    876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272, 2499, 2749, 3024, 3327, 3660,
    4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493, 10442, 11487, 12635, 13899, 15289,
    16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767, 32767, 32767, 32767, 32767, 32767,
    32767, 32767, 32767,
];

/// Decode an 8/16/24/32-bit integer, 32-bit float or MS-ADPCM WAV to mono
/// floats. Anything else is refused rather than guessed at.
fn decode_wav(bytes: &[u8]) -> Option<Pcm> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut pos = 12;
    let mut tag = 0u16;
    let mut channels = 0usize;
    let mut rate = 0u32;
    let mut bits = 0u16;
    let mut block_align = 0usize;
    let mut data: Option<&[u8]> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().ok()?) as usize;
        let body = bytes.get(pos + 8..pos + 8 + len)?;
        if id == b"fmt " && len >= 16 {
            tag = u16::from_le_bytes([body[0], body[1]]);
            channels = u16::from_le_bytes([body[2], body[3]]) as usize;
            rate = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
            block_align = u16::from_le_bytes([body[12], body[13]]) as usize;
            bits = u16::from_le_bytes([body[14], body[15]]);
        } else if id == b"data" {
            data = Some(body);
        }
        pos += 8 + len + (len & 1);
    }
    let data = data?;
    if channels == 0 || channels > 2 || rate == 0 {
        return None;
    }
    // MS ADPCM blocks are 256 bytes per channel.
    let block_align = if tag == 0x0002 && block_align < 7 * channels {
        256 * channels
    } else {
        block_align
    };
    let mut samples = Vec::new();
    match tag {
        // WAVE_FORMAT_PCM
        1 => {
            let width = bits as usize / 8;
            if width == 0 || !matches!(bits, 8 | 16 | 24 | 32) {
                return None;
            }
            for frame in data.chunks_exact(width * channels) {
                let mut acc = 0.0f32;
                for ch in frame.chunks_exact(width) {
                    acc += match width {
                        1 => (ch[0] as f32 - 128.0) / 128.0,
                        2 => i16::from_le_bytes([ch[0], ch[1]]) as f32 / 32768.0,
                        3 => {
                            let hi = if ch[2] & 0x80 != 0 { 0xff } else { 0 };
                            let v = i32::from_le_bytes([ch[0], ch[1], ch[2], hi]);
                            v as f32 / 8_388_608.0
                        }
                        _ => {
                            i32::from_le_bytes([ch[0], ch[1], ch[2], ch[3]]) as f32
                                / 2_147_483_648.0
                        }
                    };
                }
                samples.push(acc / channels as f32);
            }
        }
        // WAVE_FORMAT_IEEE_FLOAT
        3 => {
            if bits != 32 {
                return None;
            }
            for frame in data.chunks_exact(4 * channels) {
                let mut acc = 0.0f32;
                for ch in frame.chunks_exact(4) {
                    acc += f32::from_le_bytes([ch[0], ch[1], ch[2], ch[3]]);
                }
                samples.push(acc / channels as f32);
            }
        }
        // WAVE_FORMAT_ADPCM (Microsoft ADPCM)
        0x0002 => {
            if bits != 4 || block_align < 7 * channels {
                return None;
            }
            let per_block = 2 + (block_align - 7 * channels) * 2 / channels;
            for block in data.chunks(block_align) {
                if block.len() < 7 {
                    break;
                }
                let mut it = block.iter().copied();
                let predictor = (it.next()? as usize).min(6);
                let mut delta = {
                    let lo = it.next()?;
                    let hi = it.next()?;
                    i16::from_le_bytes([lo, hi]) as i32
                };
                let mut s1 = i16::from_le_bytes([it.next()?, it.next()?]) as i32;
                let mut s2 = i16::from_le_bytes([it.next()?, it.next()?]) as i32;
                let (c1, c2) = ADPCM_COEFS[predictor];
                let emit = |v: i32, samples: &mut Vec<f32>| {
                    samples.push((v as i16) as f32 / 32768.0);
                };
                emit(s2, &mut samples);
                emit(s1, &mut samples);
                let mut written = 2;
                while written < per_block {
                    let Some(byte) = it.next() else { break };
                    for nib in [(byte >> 4) & 0x0f, byte & 0x0f] {
                        if written >= per_block {
                            break;
                        }
                        let nib = nib as i32;
                        let signed = if nib >= 8 { nib - 16 } else { nib };
                        let mut v = ((s1 as i64 * c1 as i64 + s2 as i64 * c2 as i64) / 256) as i32
                            + signed * delta;
                        v = v.clamp(-32768, 32767);
                        emit(v, &mut samples);
                        written += 1;
                        s2 = s1;
                        s1 = v;
                        delta = ((delta as i64 * ADPCM_ADAPT[nib as usize] as i64 / 256) as i32)
                            .clamp(16, 1 << 22);
                    }
                }
            }
        }
        // WAVE_FORMAT_YAMAHA_ADPCM (Tag 0x0020 / 32)
        0x0020 => {
            if bits != 4 {
                return None;
            }
            let mut predictor = 0i32;
            let mut index = 0usize;
            for &byte in data {
                for nibble in [(byte & 0x0f) as usize, ((byte >> 4) & 0x0f) as usize] {
                    let step = YAMAHA_STEP[index];
                    predictor += (step * YAMAHA_DIFFLOOKUP[nibble]) >> 3;
                    predictor = predictor.clamp(-32768, 32767);
                    index = (index as i32 + YAMAHA_INDEX_SHIFT[nibble]).clamp(0, 88) as usize;
                    samples.push((predictor as f32) / 32768.0);
                }
            }
        }
        _ => return None,
    }
    if samples.is_empty() {
        return None;
    }
    Some(Pcm { rate, samples })
}

fn encode_wav(rate: u32, samples: &[f32]) -> Vec<u8> {
    let data_len = samples.len() * 2;
    let mut out = Vec::with_capacity(44 + data_len);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    for s in samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    out
}

/// Any supported WAV as 16-bit mono at `OUTPUT_RATE`, ready for the mixer.
pub fn clean_wav(bytes: &[u8]) -> Option<Vec<u8>> {
    let Pcm { rate, samples } = decode_wav(bytes)?;
    let out = to_output_rate(&samples, rate);
    if out.is_empty() {
        return None;
    }
    Some(encode_wav(OUTPUT_RATE, &out))
}

/// Minimal one-poll executor. macroquad's loaders are ready on the first poll
/// on native targets (the yield loop there is `cfg(wasm32)`), and the extra
/// arms keep wasm honest.
fn block_on<F: Future>(fut: F) -> F::Output {
    fn noop_waker() -> Waker {
        const VTABLE: RawWakerVTable = RawWakerVTable::new(
            |_| RawWaker::new(std::ptr::null(), &VTABLE),
            |_| {},
            |_| {},
            |_| {},
        );
        unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) }
    }
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    let mut fut = Box::pin(fut);
    loop {
        match Pin::as_mut(&mut fut).poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

// ---------------------------------------------------------------------------
// Player
// ---------------------------------------------------------------------------

pub struct Audio {
    pub enabled: bool,
    pub master: f32,
    engine: HashMap<String, Option<EngineVoice>>,
    one_shots: HashMap<String, Sound>,
    last_play: HashMap<String, f64>,
    listener: Vec3,
    pub engine_gain: f32,
    pub skid_gain: f32,
    pub wind_gain: f32,
    /// Resident music, by bank name. `None` marks a failed decode.
    music: HashMap<String, Option<Sound>>,
    music_used: HashMap<String, u64>,
    music_clock: u64,
    music_tx: Sender<(String, Option<Vec<u8>>)>,
    music_rx: Receiver<(String, Option<Vec<u8>>)>,
    pending_music: Vec<String>,
    current_music: Option<String>,
    /// A voice is actually playing `current_music`.
    music_live: bool,
    pub music_volume: f32,
    intro_playing: bool,
}

impl Audio {
    pub fn new() -> Self {
        let (music_tx, music_rx) = channel();
        Self {
            enabled: true,
            master: 0.8,
            engine: HashMap::new(),
            one_shots: HashMap::new(),
            last_play: HashMap::new(),
            listener: Vec3::ZERO,
            engine_gain: 0.0,
            skid_gain: 0.0,
            wind_gain: 0.0,
            music: HashMap::new(),
            music_used: HashMap::new(),
            music_clock: 0,
            music_tx,
            music_rx,
            pending_music: Vec::new(),
            current_music: None,
            music_live: false,
            music_volume: 0.45,
            intro_playing: false,
        }
    }

    /// Build the engine loops from a running recording and, when present, an
    /// idle one (both from SOUND.POD).
    pub async fn load_engine(&mut self, name: &str, run_path: &str, idle_path: &str) {
        let read = |p: &str| -> Option<Pcm> {
            let bytes = std::fs::read(p).ok()?;
            decode_wav(&bytes)
        };
        let Some(run) = read(run_path) else {
            self.engine.insert(name.to_string(), None);
            return;
        };
        // Both recordings are brought to the output rate before any pitch
        // work, so the kernel never has to fight the source rate as well.
        let run = to_output_rate(&run.samples, run.rate);
        if run.len() < 1024 {
            self.engine.insert(name.to_string(), None);
            return;
        }
        let idle = match read(idle_path) {
            Some(p) if p.rate == run.len() as u32 || p.rate > 0 => {
                let v = to_output_rate(&p.samples, p.rate);
                if v.len() >= 1024 {
                    v
                } else {
                    resample_by(&run, ENGINE_RPMS[0] / REF_RPM)
                }
            }
            _ => resample_by(&run, ENGINE_RPMS[0] / REF_RPM),
        };
        let mut loops = Vec::with_capacity(ENGINE_RPMS.len());
        for rpm in ENGINE_RPMS {
            let note = engine_note(rpm, &idle, &run);
            match load_sound_from_bytes(&encode_wav(OUTPUT_RATE, &note)).await {
                Ok(s) => loops.push(s),
                Err(_) => {
                    self.engine.insert(name.to_string(), None);
                    return;
                }
            }
        }
        let gains = vec![0.0; loops.len()];
        self.engine.insert(
            name.to_string(),
            Some(EngineVoice {
                loops,
                playing: false,
                smoothed_rpm: ENGINE_RPMS[0],
                active_step: 1,
                gains,
            }),
        );
    }

    pub async fn load_sfx(&mut self, name: &str, path: &str) {
        if let Ok(bytes) = load_file(path).await {
            if let Some(cleaned) = clean_wav(&bytes) {
                if let Ok(s) = load_sound_from_bytes(&cleaned).await {
                    self.one_shots.insert(name.to_string(), s);
                }
            }
        }
    }

    pub fn set_listener(&mut self, p: Vec3) {
        self.listener = p;
    }

    /// Start decoding `name` on a worker thread unless it is resident or
    /// already queued. The decode is pure CPU (no macroquad context), so it
    /// leaves the frame alone.
    pub fn request_music(&mut self, name: &str, path: &str) {
        if self.music.contains_key(name) || self.pending_music.iter().any(|p| p == name) {
            return;
        }
        // A bank missing a music file (e.g. a level with no redbook track) must
        // not crash the game: log it once and continue silent.
        if !std::path::Path::new(path).exists() {
            eprintln!("audio: music file {path} not found — continuing without background music");
            return;
        }
        self.pending_music.push(name.to_string());
        let tx = self.music_tx.clone();
        let name = name.to_string();
        let path = path.to_string();
        std::thread::spawn(move || {
            let cleaned = std::fs::read(&path).ok().and_then(|b| clean_wav(&b));
            let _ = tx.send((name, cleaned));
        });
    }

    /// Install finished decodes and drop tracks beyond the cache budget.
    /// Called once per frame; a started track is picked up here as soon as
    /// its bytes arrive.
    pub fn pump_music(&mut self) {
        while let Ok((name, bytes)) = self.music_rx.try_recv() {
            self.pending_music.retain(|p| p != &name);
            let loaded = bytes.and_then(|b| block_on(load_sound_from_bytes(&b)).ok());
            self.music.insert(name.clone(), loaded);
            self.music_used.insert(name.clone(), self.music_clock);
        }
        self.music_clock += 1;
        if let Some(name) = self.current_music.clone() {
            if !self.music_live && self.music.get(&name).is_some_and(|m| m.is_some()) {
                self.start_music(&name);
            }
        }
        if self.music.len() > MUSIC_CACHE {
            let current = self.current_music.clone();
            let mut order: Vec<(u64, String)> = self
                .music_used
                .iter()
                .filter(|(n, _)| !(self.music_live && Some(*n) == current.as_ref()))
                .map(|(n, t)| (*t, n.clone()))
                .collect();
            order.sort();
            let drop_n = self.music.len() - MUSIC_CACHE;
            for (_, name) in order.into_iter().take(drop_n) {
                self.music.remove(&name);
                self.music_used.remove(&name);
            }
        }
    }

    fn playing(&self, name: &str) -> bool {
        self.music_live && self.current_music.as_deref() == Some(name)
    }

    /// Number of queued (not yet decoded) music requests, for tests.
    fn pending_len(&self) -> usize {
        self.pending_music.len()
    }

    fn start_music(&mut self, name: &str) {
        if let Some(Some(s)) = self.music.get(name) {
            play_sound(
                s,
                PlaySoundParams {
                    looped: true,
                    volume: self.music_volume * self.master,
                },
            );
            if std::env::var("EVOX_MUSIC").is_ok() {
                eprintln!("music {name}");
            }
            self.current_music = Some(name.to_string());
            self.music_live = true;
        }
    }

    /// Loop the requested track, ignoring repeats of the current one. Starts
    /// immediately when the track is resident; otherwise it starts from
    /// `pump_music` once the worker hands it over.
    pub fn play_music(&mut self, name: &str) {
        if !self.enabled || self.playing(name) {
            return;
        }
        self.stop_music();
        self.current_music = Some(name.to_string());
        self.music_used.insert(name.to_string(), self.music_clock);
        self.start_music(name);
    }

    /// Opening-movie soundtrack: started once with the first frame, stopped
    /// on skip (offroad4x4 `Audio::intro`).
    pub fn intro(&mut self, on: bool) {
        if on == self.intro_playing {
            return;
        }
        if let Some(Some(s)) = self.music.get("INTRO") {
            if on {
                if self.enabled {
                    play_sound(
                        s,
                        PlaySoundParams {
                            looped: false,
                            volume: self.master,
                        },
                    );
                }
            } else {
                stop_sound(s);
            }
        }
        self.intro_playing = on;
    }

    pub fn stop_music(&mut self) {
        self.music_live = false;
        if let Some(name) = self.current_music.take() {
            if let Some(Some(s)) = self.music.get(&name) {
                stop_sound(s);
            }
        }
    }

    /// Fade the music down slightly while a race is running so both music and
    /// engine/surface SFX sit cleanly in the mix without clipping.
    pub fn duck_music(&mut self, ducked: bool) {
        let v = if ducked {
            self.music_volume * 0.95
        } else {
            self.music_volume
        };
        if let Some(Some(s)) = self.current_music.as_ref().and_then(|n| self.music.get(n)) {
            set_sound_volume(s, v * self.master);
        }
    }

    /// Called every frame with the local vehicle's engine state. Uses the TRX
    /// idle-to-load state machine (`ENG_IDLE3.WAV` + single active
    /// `ENG_STOCK.WAV` pitch step with hysteresis) and fast gain slewing so
    /// buffers never restart and two pitched copies of `ENG_STOCK` never
    /// comb-filter.
    pub fn update_engine(
        &mut self,
        voice: &str,
        rpm: f32,
        _redline: f32,
        throttle: f32,
        speed: f32,
        _gear: usize,
        _shift_timer: f32,
        dt: f32,
    ) {
        if !self.enabled {
            return;
        }
        // Keep engine gain below background music so the combined mix has
        // generous headroom for surface/collision one-shots.
        let load = 0.45 + 0.55 * throttle.clamp(0.0, 1.0);
        self.engine_gain = load * 0.22 * self.master;
        let gain = self.engine_gain;
        let Some(Some(v)) = self.engine.get_mut(voice) else {
            return;
        };
        if !v.playing {
            v.smoothed_rpm = rpm;
            v.active_step = select_run_step(1, rpm);
            for s in &v.loops {
                play_sound(
                    s,
                    PlaySoundParams {
                        looped: true,
                        volume: 0.0,
                    },
                );
            }
            v.playing = true;
        }
        let dt_clamped = dt.clamp(1.0 / 240.0, 0.1);
        let rpm_alpha = (dt_clamped * 10.0).clamp(0.0, 1.0);
        v.smoothed_rpm += (rpm - v.smoothed_rpm) * rpm_alpha;
        v.active_step = select_run_step(v.active_step, v.smoothed_rpm);

        // Smooth gain slewing (~35 ms time constant) when transitioning steps.
        let gain_slew = (dt_clamped * 28.0).clamp(0.0, 1.0);
        for (i, s) in v.loops.iter().enumerate() {
            let target = engine_weight_state(i, v.smoothed_rpm, throttle, v.active_step) * gain;
            let next = v.gains[i] + (target - v.gains[i]) * gain_slew;
            if (next - v.gains[i]).abs() > 1e-4 || (target == 0.0 && v.gains[i] > 0.0) {
                v.gains[i] = if next < 1e-4 && target == 0.0 {
                    0.0
                } else {
                    next
                };
                set_sound_volume(s, v.gains[i]);
            }
        }
        self.wind_gain = (speed / 220.0).clamp(0.0, 1.0) * 0.16 * self.master;
    }

    pub fn stop_engine(&mut self, voice: &str) {
        if let Some(Some(v)) = self.engine.get_mut(voice) {
            for s in &v.loops {
                set_sound_volume(s, 0.0);
                stop_sound(s);
            }
            v.playing = false;
        }
    }

    pub fn play(&mut self, name: &str, volume: f32, now: f64, cooldown: f64) -> bool {
        if !self.enabled {
            return false;
        }
        if let Some(t) = self.last_play.get(name) {
            if now - *t < cooldown {
                return false;
            }
        }
        let Some(s) = self.one_shots.get(name) else {
            return false;
        };
        play_sound(
            s,
            PlaySoundParams {
                looped: false,
                volume: volume * self.master,
            },
        );
        self.last_play.insert(name.to_string(), now);
        true
    }

    /// Spatialised one-shot: attenuates by distance from the listener.
    pub fn play_at(&mut self, name: &str, pos: Vec3, volume: f32, now: f64, cooldown: f64) -> bool {
        let d = (pos - self.listener).length();
        let att = (1.0 - d / 90.0).clamp(0.0, 1.0);
        if att <= 0.02 {
            return false;
        }
        self.play(name, volume * att, now, cooldown)
    }

    pub fn surface_event(&mut self, surface: &str, slip: f32, speed: f32, pos: Vec3, now: f64) {
        if slip > 0.25 && speed > 4.0 {
            let name = match surface {
                "gravel" | "dirt" => "SKID_01",
                "mud" | "water" => "SPLASHDC",
                "snow" => "ICECRACK",
                _ => "SKID_D1N",
            };
            self.play_at(name, pos, (slip * 0.5).min(0.65), now, 0.45);
        }
        if surface == "water" && speed > 2.0 {
            self.play_at("WATER_SPLASH_01", pos, 0.52, now, 0.30);
        }
        self.skid_gain = slip;
    }

    pub fn ui_click(&mut self, now: f64) {
        self.play("CLICK", 0.55 * UI_SOUND_SCALE, now, 0.06);
    }

    pub fn ui_move(&mut self, now: f64) {
        self.play("CLICK", 0.30 * UI_SOUND_SCALE, now, 0.06);
    }

    pub fn ui_back(&mut self, now: f64) {
        self.play("CLICK", 0.45 * UI_SOUND_SCALE, now, 0.08);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(n: usize, hz: f32, rate: u32) -> Vec<f32> {
        (0..n)
            .map(|i| (i as f32 / rate as f32 * hz * std::f32::consts::TAU).sin() * 0.5)
            .collect()
    }

    /// Zero-crossing rate is a cheap pitch/alias probe.
    fn zcr(x: &[f32]) -> f32 {
        x.windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count() as f32
            / x.len() as f32
    }

    fn wav(tag: u16, bits: u16, rate: u32, channels: u16, block: u16, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&((36 + data.len()) as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * channels as u32 * bits as u32 / 8).to_le_bytes());
        out.extend_from_slice(&block.to_le_bytes());
        out.extend_from_slice(&bits.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        out
    }

    #[test]
    fn wav_roundtrip() {
        let pcm: Vec<f32> = (0..1000).map(|i| (i as f32 / 30.0).sin() * 0.5).collect();
        let bytes = encode_wav(OUTPUT_RATE, &pcm);
        let p = decode_wav(&bytes).unwrap();
        assert_eq!(p.rate, OUTPUT_RATE);
        assert_eq!(p.samples.len(), 1000);
        assert!((p.samples[0] - pcm[0]).abs() < 0.001);
    }

    #[test]
    fn decodes_24_and_32_bit_and_float() {
        let s24 = [-8_388_608i32, 0, 8_388_607];
        let mut d24 = Vec::new();
        for v in s24 {
            let b = v.to_le_bytes();
            d24.extend_from_slice(&b[0..3]);
        }
        let p = decode_wav(&wav(1, 24, 44_100, 1, 3, &d24)).unwrap();
        assert!((p.samples[0] + 1.0).abs() < 1e-4);
        assert!((p.samples[2] - 1.0).abs() < 1e-4);

        let d32: Vec<u8> = [i32::MIN, 0, i32::MAX]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let p = decode_wav(&wav(1, 32, 44_100, 1, 4, &d32)).unwrap();
        assert!((p.samples[0] + 1.0).abs() < 1e-4);

        let dfloat: Vec<u8> = [0.5f32, -0.5f32]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let p = decode_wav(&wav(3, 32, 44_100, 1, 4, &dfloat)).unwrap();
        assert!((p.samples[0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn decodes_ms_adpcm() {
        // One block: predictor 0 (c1=256, c2=0), delta 16, s1=0, s2=0.
        let mut block = vec![0u8, 16, 0, 0, 0, 0, 0];
        block.extend(std::iter::repeat_n(0b0001_0001u8, 249));
        let p = decode_wav(&wav(0x02, 4, 22_050, 1, 256, &block)).unwrap();
        assert_eq!(p.samples.len(), 2 + 249 * 2);
        // Each nibble adds delta and then grows it, so the ramp must rise.
        assert!(p.samples[10] > p.samples[4]);
        assert!(p.samples.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn decodes_yamaha_adpcm() {
        // Tag 0x0020: Yamaha ADPCM (used in SKID-D1N.WAV). A run of `0x11`
        // nibbles is +1 with index shift −1, so it is the *quiet* direction and
        // must never clip; a run of `0x55` (+3, +4 steps) must grow louder.
        let p = decode_wav(&wav(0x20, 4, 22_050, 1, 1, &[0x11u8; 100])).unwrap();
        assert_eq!(p.samples.len(), 200);
        assert!(p.samples.iter().all(|s| s.is_finite()));
        assert!(p.samples.iter().all(|s| s.abs() <= 1.0));
        let peak = p.samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.001 && peak < 0.5, "quiet ramp peak {peak}");

        let loud = decode_wav(&wav(0x20, 4, 22_050, 1, 1, &[0x55u8; 100])).unwrap();
        let loud_peak = loud.samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(loud_peak > peak * 4.0, "step index must scale up");
        assert!(loud.samples.iter().all(|s| s.abs() <= 1.0));
    }

    /// `SKID-D1N.WAV` is the bank's only Yamaha ADPCM file. It used to be
    /// decoded with a fixed `step` multiply/`>> 8` approximation that walked
    /// the step size out of range and clipped; the 89-entry step table keeps
    /// it inside int16 and at full sample count (2 per byte).
    #[test]
    fn skid_wav_decodes_without_clipping() {
        let Some(dir) = [
            "assets/extracted/audio/sfx/SOUND",
            "../assets/extracted/audio/sfx/SOUND",
        ]
        .into_iter()
        .find(|d| std::path::Path::new(d).is_dir())
        .or_else(|| {
            std::path::Path::new("assets/extracted/audio/sfx/SOUND")
                .is_dir()
                .then_some("assets/extracted/audio/sfx/SOUND")
        }) else {
            return;
        };
        let path = format!("{dir}/SKID-D1N.WAV");
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        let pcm = decode_wav(&bytes).expect("yamaha adpcm decodes");
        assert_eq!(pcm.rate, 22_050);
        let clipped = pcm.samples.iter().filter(|s| s.abs() >= 0.999).count();
        assert!(
            clipped * 100 < pcm.samples.len(),
            "{clipped} of {} samples clipped",
            pcm.samples.len()
        );
        let peak = pcm.samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.5, "skid is too quiet: peak {peak}");
    }

    #[test]
    fn engine_and_skid_waveforms_are_smooth_not_white_noise() {
        require_assets!();
        let dir = [
            "assets/extracted/audio/sfx/SOUND",
            "../assets/extracted/audio/sfx/SOUND",
        ]
        .into_iter()
        .find(|d| std::path::Path::new(d).exists())
        .expect("sound bank directory");

        for name in [
            ENGINE_RUN_WAV,
            ENGINE_IDLE_WAV,
            "SKID-D1N.WAV",
            "SKID_01.WAV",
        ] {
            let path = format!("{dir}/{name}");
            let bytes =
                std::fs::read(&path).unwrap_or_else(|e| panic!("failed to read {path}: {e}"));
            let pcm = decode_wav(&bytes).unwrap_or_else(|| panic!("failed to decode {name}"));
            let samples = &pcm.samples;
            assert!(
                samples.len() > 1000,
                "{name}: expected >1000 samples, got {}",
                samples.len()
            );
            assert!(
                samples.iter().all(|s| s.is_finite()),
                "{name}: non-finite samples found"
            );

            // Compute sample-to-sample difference (derivative)
            let mut diff_sum = 0.0f32;
            let mut max_diff = 0.0f32;
            let mut zero_crossings = 0usize;
            for i in 1..samples.len() {
                let d = (samples[i] - samples[i - 1]).abs();
                diff_sum += d;
                if d > max_diff {
                    max_diff = d;
                }
                if (samples[i] >= 0.0) != (samples[i - 1] >= 0.0) {
                    zero_crossings += 1;
                }
            }
            let avg_diff = diff_sum / (samples.len() - 1) as f32;
            let zcr = zero_crossings as f32 / samples.len() as f32;

            // White noise static produces avg_diff > 0.40, max_diff = 2.0 (rail-to-rail jump),
            // and high zero-crossing rates. Authentic tonal skid and engine sounds have low
            // sample deltas (smooth derivative).
            assert!(
                avg_diff < 0.20,
                "{name}: avg sample delta {avg_diff:.4} is too high (white noise static detected)"
            );
            assert!(
                max_diff < 1.6,
                "{name}: max sample delta {max_diff:.4} indicates rail-to-rail clipping noise"
            );
            assert!(
                zcr < 0.35,
                "{name}: zero crossing rate {zcr:.4} is too high for tonal audio"
            );
        }
    }

    #[test]
    fn rejects_unknown_codec() {
        assert!(decode_wav(&wav(0x11, 4, 8000, 1, 256, &[0; 64])).is_none());
    }

    #[test]
    fn clean_wav_resamples_to_output_rate() {
        let src = tone(22_050, 440.0, 22_050);
        let cleaned = clean_wav(&encode_wav(22_050, &src)).unwrap();
        let p = decode_wav(&cleaned).unwrap();
        assert_eq!(p.rate, OUTPUT_RATE);
        assert!((p.samples.len() as f32 - 44_100.0).abs() < 4.0);
        // Pitch is preserved by a cutoff-class resample (≈ 440 Hz).
        assert!((zcr(&p.samples) - 2.0 * 440.0 / OUTPUT_RATE as f32).abs() < 0.004);
    }

    #[test]
    fn pitch_shift_keeps_the_note_and_the_length() {
        let src = tone(44_100, 500.0, 44_100);
        let up = resample_by(&src, 2.0);
        assert!((up.len() as f32 - 22_050.0).abs() < 2.0);
        assert!((zcr(&up) - 2.0 * 1000.0 / OUTPUT_RATE as f32).abs() < 0.01);
        // A 2× decimation of a 3 kHz tone would alias without a cutoff.
        let high = tone(88_200, 3000.0, 44_100);
        let dec = resample_by(&high, 4.0);
        // 4× speed on a 3 kHz tone is 12 kHz. Unfiltered it folds back to
        // 12 kHz − 11.025 kHz ≈ 975 Hz, a tenfold difference in crossings.
        assert!((zcr(&dec) - 2.0 * 12_000.0 / OUTPUT_RATE as f32).abs() < 0.03);
        assert!(zcr(&dec) > 0.4);
    }

    /// A missing music file (no redbook track for a level) must be skipped, not
    /// crash the game on `std::fs::read`.
    #[test]
    fn missing_music_is_skipped_gracefully() {
        let mut a = Audio::new();
        let before = a.pending_len();
        a.request_music("ALASKA1", "assets/extracted/audio/sfx/SOUND/ALASKA1.WAV");
        assert_eq!(a.pending_len(), before, "no decode should be queued");
        assert!(!a.playing("ALASKA1"));
        a.pump_music();
    }

    /// Every file in the retail bank must decode; a rejection here is a
    /// silently missing sound.
    #[test]
    fn whole_bank_decodes() {
        require_assets!();
        // Tests run with the crate directory as cwd.
        let dir = [
            "assets/extracted/audio/sfx/SOUND",
            "../assets/extracted/audio/sfx/SOUND",
        ]
        .into_iter()
        .find(|d| std::path::Path::new(d).is_dir());
        let Some(dir) = dir else {
            panic!("sound bank not found");
        };
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let mut bad = Vec::new();
        let mut n = 0;
        for path in rd.filter_map(|e| e.ok()).map(|e| e.path()) {
            if path.extension().map(|e| !e.eq_ignore_ascii_case("wav")) != Some(false) {
                continue;
            }
            n += 1;
            let bytes = std::fs::read(&path).unwrap();
            match clean_wav(&bytes) {
                Some(w) => {
                    let p = decode_wav(&w).unwrap();
                    assert_eq!(p.rate, OUTPUT_RATE, "{path:?}");
                    assert!(p.samples.iter().all(|s| s.is_finite()), "{path:?}");
                }
                None => bad.push(path.file_name().unwrap().to_string_lossy().into_owned()),
            }
        }
        assert!(n > 300, "bank not found ({n} files)");
        assert!(bad.is_empty(), "undecodable: {bad:?}");
    }

    #[test]
    fn engine_weights_crossfade_and_sum_to_one() {
        for rpm in [900.0, 1500.0, 2800.0, 4000.0, 7600.0, 9000.0, 300.0] {
            let sum: f32 = (0..ENGINE_RPMS.len()).map(|i| engine_weight(i, rpm)).sum();
            assert!((sum - 1.0).abs() < 1e-3, "rpm {rpm} sum {sum}");
            // Above the idle transition, only ONE running step may be non-zero
            // so two copies of ENG_STOCK never comb-filter.
            let active_run_voices = (1..ENGINE_RPMS.len())
                .filter(|&i| engine_weight(i, rpm) > 1e-4)
                .count();
            assert!(
                active_run_voices <= 1,
                "rpm {rpm} had {active_run_voices} simultaneous run voices"
            );
        }
        assert!(engine_weight(3, 1880.0) > 0.99);
    }

    #[test]
    fn run_step_hysteresis_prevents_chatter() {
        let mid = 0.5 * (ENGINE_RPMS[4] + ENGINE_RPMS[5]);
        let s0 = select_run_step(4, mid + 20.0);
        assert_eq!(s0, 4, "must hold step 4 inside hysteresis band");
        let s1 = select_run_step(4, mid + STEP_HYSTERESIS_RPM + 10.0);
        assert_eq!(s1, 5, "must advance to step 5 past upper hysteresis");
        let s2 = select_run_step(s1, mid - 20.0);
        assert_eq!(s2, 5, "must hold step 5 inside hysteresis band on drop");
    }

    #[test]
    fn loops_are_dc_free_and_seam_free() {
        let x: Vec<f32> = (0..4000)
            .map(|i| (i as f32 / 17.0).sin() * 0.3 + 0.2)
            .collect();
        let l = make_loop(&x, 0.2);
        let mean = l.iter().sum::<f32>() / l.len() as f32;
        assert!(mean.abs() < 1e-3);
        assert!((l[0] - l[l.len() - 1]).abs() < 0.2);
    }

    #[test]
    fn engine_notes_stay_level_and_finite() {
        let run = tone(44_100, 90.0, 44_100);
        let idle = tone(44_100, 40.0, 44_100);
        for rpm in ENGINE_RPMS {
            let n = engine_note(rpm, &idle, &run);
            assert!(n.iter().all(|s| s.is_finite()));
            let rms = (n.iter().map(|s| s * s).sum::<f32>() / n.len() as f32).sqrt();
            assert!((rms - ENGINE_RMS).abs() < 0.05, "rpm {rpm} rms {rms}");
        }
    }

    /// Mix headroom report on the real bank: peak / RMS of each decoded
    /// buffer and the worst case of music + engine + the loudest one-shot at
    /// the gains `Audio` uses. `cargo test --release -p evox mix_report -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn mix_report() {
        let dir = [
            "../assets/extracted/audio/sfx/SOUND",
            "assets/extracted/audio/sfx/SOUND",
        ]
        .into_iter()
        .find(|d| std::path::Path::new(d).exists())
        .expect("extracted SOUND bank");
        let stats = |name: &str| -> Option<(f32, f32)> {
            let b = std::fs::read(format!("{dir}/{name}")).ok()?;
            let pcm = decode_wav(&b)?;
            let x = &pcm.samples;
            let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            let rms = (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt();
            Some((peak, rms))
        };
        let a = Audio::new();
        let mut music_peak = 0.0f32;
        for n in (1..=10)
            .map(|i| format!("EVO{i}.WAV"))
            .chain((1..=10).map(|i| format!("UI-{i:02}.WAV")))
        {
            if let Some((p, r)) = stats(&n) {
                println!("music {n:12} peak {p:.3} rms {r:.3}");
                music_peak = music_peak.max(p);
            }
        }
        let mut sfx_peak = 0.0f32;
        for n in [
            "CHKCLICK.WAV",
            "SKID_01.WAV",
            "WATER_SPLASH_01.WAV",
            "ROCKHIT.WAV",
            "ABCHKPT.01.WAV",
        ] {
            if let Some((p, r)) = stats(n) {
                println!("sfx   {n:20} peak {p:.3} rms {r:.3}");
                sfx_peak = sfx_peak.max(p);
            }
        }
        let run = stats("ENG_STOCK.WAV").unwrap();
        let idle = stats("ENG_IDLE3.WAV").unwrap();
        println!(
            "engine run peak {:.3} rms {:.3}, idle peak {:.3} rms {:.3}",
            run.0, run.1, idle.0, idle.1
        );
        // Engine loops are RMS-levelled to ENGINE_RMS; crest factor from the source.
        let engine_peak = ENGINE_RMS * (run.0 / run.1.max(1e-4)).max(idle.0 / idle.1.max(1e-4));
        let engine_gain = 1.0 * 0.22 * a.master;
        let music_gain = a.music_volume * a.master;
        let sfx_gain = 0.65 * a.master;
        let worst = music_peak * music_gain + engine_peak * engine_gain + sfx_peak * sfx_gain;
        println!(
            "gains: music {music_gain:.2} engine {engine_gain:.2} sfx<= {sfx_gain:.2}; worst-case summed peak {worst:.3} (1.0 = clip)"
        );
    }

    #[test]
    fn engine_run_recording_is_not_saturated() {
        require_assets!();
        let Some(dir) = [
            "../assets/extracted/audio/sfx/SOUND",
            "assets/extracted/audio/sfx/SOUND",
        ]
        .into_iter()
        .find(|d| std::path::Path::new(d).exists()) else {
            panic!("extracted SOUND bank not found");
        };
        for name in [ENGINE_RUN_WAV, ENGINE_IDLE_WAV] {
            let pcm = decode_wav(&std::fs::read(format!("{dir}/{name}")).unwrap()).unwrap();
            let x = &pcm.samples;
            let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            let rms = (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
            let full = x.iter().filter(|v| v.abs() > 0.97).count() as f32 / x.len() as f32;
            assert!(
                peak / rms > 1.4,
                "{name}: crest {:.2} (saturated)",
                peak / rms
            );
            assert!(
                full < 0.05,
                "{name}: {:.0}% of samples at full scale",
                full * 100.0
            );
        }
    }
}
