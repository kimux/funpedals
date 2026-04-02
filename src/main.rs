// FunPedals - Real-time guitar multi-effects processor
// https://github.com/kimux/funpedals
//
// A Rust-based guitar effects processor running on Raspberry Pi Zero 2W,
// featuring a touchscreen GUI, chainable effects, and TOML preset management.
// Powered by FunDSP (https://github.com/SamiPerttu/fundsp) for DSP processing.
//
// Developed with the assistance of Claude (https://claude.ai) by Anthropic.
//
// Copyright (c) 2026 kimux
// Licensed under the MIT License
// SPDX-License-Identifier: MIT

fn main() {
    #[cfg(target_os = "linux")]
    linux::run();

    #[cfg(target_os = "macos")]
    macos::run();
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod common {
    use fundsp::prelude::*;

    // ── Parameter definitions ────────────────────────────────────────────
    // Shared parameter definitions for GUI, TOML, and terminal
    #[derive(Clone)]
    pub struct ParamDef {
        pub name:  &'static str,
        pub min:   f32,
        pub max:   f32,
        pub value: f32,
        pub unit:  &'static str, // Display unit ("", "dB", "ms", "Hz", etc.)
    }

    impl ParamDef {
        pub fn new(name: &'static str, min: f32, max: f32, value: f32) -> Self {
            Self { name, min, max, value, unit: "" }
        }
        pub fn with_unit(name: &'static str, min: f32, max: f32, value: f32, unit: &'static str) -> Self {
            Self { name, min, max, value, unit }
        }
    }

    // ── Effect trait ──────────────────────────────────────────
    pub trait Effect: Send {
        fn process_sample(&mut self, l: f32, r: f32) -> (f32, f32);
        fn name(&self) -> &str;
        fn init(&mut self, sample_rate: f64);

        // Returns parameter list (default: empty)
        fn params(&self) -> Vec<ParamDef> { vec![] }
        // Sets parameter by index (default: no-op)
        fn set_param(&mut self, _index: usize, _value: f32) {}
        // Per-effect (name, params) list for PARAM tab
        fn params_by_unit(&self) -> Vec<(String, Vec<ParamDef>)> {
            let p = self.params();
            if p.is_empty() { vec![] }
            else { vec![(self.name().to_string(), p)] }
        }
        // Inject input RMS for NoiseGate etc. (default: no-op)
        fn inject_input_rms(&mut self, _rms: std::sync::Arc<std::sync::Mutex<f32>>) {}
    }

    // ── EffectChain ───────────────────────────────────────────────
    // Pipeline of Vec<Box<dyn Effect>>
    // Basic unit returned by make_effect()
    pub struct EffectChain {
        pub effects: Vec<Box<dyn Effect>>,
        pub name: String,
    }

    impl EffectChain {
        #[allow(dead_code)]
        pub fn new(name: &str) -> Self {
            Self { effects: Vec::new(), name: name.to_string() }
        }

        #[allow(dead_code)]
        pub fn push(mut self, e: Box<dyn Effect>) -> Self {
            self.effects.push(e);
            self
        }

        // Returns (effect name, param list) per effect
        // Used for PARAM tab generation
        pub fn params_by_unit(&self) -> Vec<(String, Vec<ParamDef>)> {
            self.effects.iter()
                .filter_map(|e| {
                    let p = e.params();
                    if p.is_empty() { None }
                    else { Some((e.name().to_string(), p)) }
                })
                .collect()
        }
    }

    impl Effect for EffectChain {
        fn process_sample(&mut self, l: f32, r: f32) -> (f32, f32) {
            let mut out = (l, r);
            for e in self.effects.iter_mut() {
                out = e.process_sample(out.0, out.1);
            }
            out
        }

        fn name(&self) -> &str { &self.name }

        fn init(&mut self, sample_rate: f64) {
            for e in self.effects.iter_mut() {
                e.init(sample_rate);
            }
        }

        // EffectChain params = all effect params flattened
        // index is sequential across all effects
        fn params(&self) -> Vec<ParamDef> {
            self.effects.iter().flat_map(|e| e.params()).collect()
        }

        fn set_param(&mut self, index: usize, value: f32) {
            let mut offset = 0usize;
            for e in self.effects.iter_mut() {
                let n = e.params().len();
                if index < offset + n {
                    e.set_param(index - offset, value);
                    return;
                }
                offset += n;
            }
        }

        fn params_by_unit(&self) -> Vec<(String, Vec<ParamDef>)> {
            self.params_by_unit()
        }

        fn inject_input_rms(&mut self, rms: std::sync::Arc<std::sync::Mutex<f32>>) {
            for e in self.effects.iter_mut() {
                e.inject_input_rms(std::sync::Arc::clone(&rms));
            }
        }
    }

    // ── FunDSP wrapper ──────────────────────────────────────────
    pub struct FxUnit {
        unit: Box<dyn AudioUnit>,
        dry: f32,
        wet: f32,
        name: String,
    }

    impl FxUnit {
        pub fn new(unit: Box<dyn AudioUnit>, dry: f32, wet: f32, name: &str) -> Self {
            Self { unit, dry, wet, name: name.to_string() }
        }
    }

    impl Effect for FxUnit {
        fn process_sample(&mut self, l: f32, r: f32) -> (f32, f32) {
            let mut out = [0.0f32; 2];
            self.unit.tick(&[l, r], &mut out);
            (l * self.dry + out[0] * self.wet,
             r * self.dry + out[1] * self.wet)
        }
        fn name(&self) -> &str { &self.name }
        fn init(&mut self, sample_rate: f64) {
            self.unit.set_sample_rate(sample_rate);
            self.unit.reset();
        }
    }

    // ── dB conversion ─────────────────────────────────────────────
    pub fn db_to_linear(db: f32) -> f32 {
        10.0f32.powf(db / 20.0)
    }

    // ── ms to sample coefficient conversion ──────────────────────────────────
    // Allows specifying attack/release etc. in ms units
    // e.g. ms_to_coef(5.0, 48000.0) → change per sample
    pub fn ms_to_coef(ms: f32, sample_rate: f32) -> f32 {
        if ms <= 0.0 { return 1.0; }
        1.0 / (ms * sample_rate / 1000.0)
    }

    // ── Signal analysis ─────────────────────────────────────────────
    // ── Noise Gate ───────────────────────────────────────────────
    pub struct NoiseGate {
        // User-facing parameters (GUI/TOML units)
        threshold_db: f32,  // in dB (-60 to 0dB)
        attack_ms:    f32,  // in ms
        decay_ms:     f32,  // in ms
        sample_rate:  f32,
        // Internally converted values
        threshold_lin: f32, // linear-converted value
        attack_coef:   f32, // sample coefficient
        decay_coef:    f32, // sample coefficient
        envelope:      f32,
        pub input_rms: Option<std::sync::Arc<std::sync::Mutex<f32>>>,
    }

    impl NoiseGate {
        pub fn new(threshold_db: f32) -> Self {
            let sr = 48000.0f32;
            let mut ng = Self {
                threshold_db,
                attack_ms:    5.0,
                decay_ms:     200.0,
                sample_rate:  sr,
                threshold_lin: 0.0,
                attack_coef:   0.0,
                decay_coef:    0.0,
                envelope:      0.0,
                input_rms:     None,
            };
            ng.rebuild();
            ng
        }

        fn rebuild(&mut self) {
            // dB → linear conversion
            self.threshold_lin = 10.0f32.powf(self.threshold_db / 20.0);
            // ms → sample coefficient
            self.attack_coef = ms_to_coef(self.attack_ms, self.sample_rate);
            self.decay_coef  = ms_to_coef(self.decay_ms,  self.sample_rate);
        }
    }

    impl Effect for NoiseGate {
        fn process_sample(&mut self, l: f32, r: f32) -> (f32, f32) {
            let level = if let Some(ref rms) = self.input_rms {
                *rms.lock().unwrap()
            } else {
                (l * l + r * r).sqrt()
            };
            let open_threshold  = self.threshold_lin;
            let close_threshold = self.threshold_lin * 0.5;
            if level > open_threshold {
                self.envelope = (self.envelope + self.attack_coef).min(1.0);
            } else if level < close_threshold {
                self.envelope = (self.envelope - self.decay_coef).max(0.0);
            }
            (l * self.envelope, r * self.envelope)
        }

        fn name(&self) -> &str { "NoiseGate" }
        fn init(&mut self, sample_rate: f64) {
            self.sample_rate = sample_rate as f32;
            self.rebuild();
            self.envelope = 0.0;
        }

        fn params(&self) -> Vec<ParamDef> {
            vec![
                ParamDef::with_unit("threshold", -60.0, 0.0,    self.threshold_db, "dB"),
                ParamDef::with_unit("attack",    0.1,   100.0,  self.attack_ms,    "ms"),
                ParamDef::with_unit("decay",     1.0,   2000.0, self.decay_ms,     "ms"),
            ]
        }

        fn set_param(&mut self, index: usize, value: f32) {
            match index {
                0 => { self.threshold_db = value; self.rebuild(); }
                1 => { self.attack_ms    = value; self.rebuild(); }
                2 => { self.decay_ms     = value; self.rebuild(); }
                _ => {}
            }
        }

        fn inject_input_rms(&mut self, rms: std::sync::Arc<std::sync::Mutex<f32>>) {
            self.input_rms = Some(rms);
        }
    }

    // ── Auto Wah ─────────────────────────────────────────────────
    pub struct AutoWah {
        filter_l:      Box<dyn AudioUnit>,
        filter_r:      Box<dyn AudioUnit>,
        follower_l:    Box<dyn AudioUnit>,  // afollow() L ch
        follower_r:    Box<dyn AudioUnit>,  // afollow() R ch
        smooth_freq_l: f32,
        smooth_freq_r: f32,
        update_counter: usize,
        // User-facing parameters
        min_freq:   f32,
        max_freq:   f32,
        attack_ms:  f32,
        release_ms: f32,
        q:          f32,
    }

    impl AutoWah {
        pub fn new(min_freq: f32, max_freq: f32, attack_ms: f32, release_ms: f32, q: f32) -> Self {
            let a = attack_ms  / 1000.0;
            let r = release_ms / 1000.0;
            Self {
                filter_l:      Box::new(bandpass_hz(min_freq, q)),
                filter_r:      Box::new(bandpass_hz(min_freq, q)),
                follower_l:    Box::new(afollow(a, r)),
                follower_r:    Box::new(afollow(a, r)),
                smooth_freq_l: min_freq,
                smooth_freq_r: min_freq,
                update_counter: 0,
                min_freq,
                max_freq,
                attack_ms,
                release_ms,
                q,
            }
        }
    }

    impl Effect for AutoWah {
        fn process_sample(&mut self, l: f32, r: f32) -> (f32, f32) {
            // Envelope tracking via afollow() (absolute value input)
            let mut env_l = [0.0f32; 1];
            let mut env_r = [0.0f32; 1];
            self.follower_l.tick(&[l.abs()], &mut env_l);
            self.follower_r.tick(&[r.abs()], &mut env_r);

            let freq_l = self.min_freq + (self.max_freq - self.min_freq) * (env_l[0] * 5.0).clamp(0.0, 1.0);
            let freq_r = self.min_freq + (self.max_freq - self.min_freq) * (env_r[0] * 5.0).clamp(0.0, 1.0);

            let smooth = 0.002f32;
            self.smooth_freq_l += smooth * (freq_l - self.smooth_freq_l);
            self.smooth_freq_r += smooth * (freq_r - self.smooth_freq_r);

            self.update_counter += 1;
            if self.update_counter >= 32 {
                self.update_counter = 0;
                self.filter_l.set(Setting::center(self.smooth_freq_l));
                self.filter_r.set(Setting::center(self.smooth_freq_r));
            }

            let mut wl = [0.0f32; 1];
            let mut wr = [0.0f32; 1];
            self.filter_l.tick(&[l], &mut wl);
            self.filter_r.tick(&[r], &mut wr);
            (wl[0] * 1.7, wr[0] * 1.7)
        }

        fn name(&self) -> &str { "AutoWah" }

        fn init(&mut self, sample_rate: f64) {
            self.filter_l.set_sample_rate(sample_rate);
            self.filter_r.set_sample_rate(sample_rate);
            self.follower_l.set_sample_rate(sample_rate);
            self.follower_r.set_sample_rate(sample_rate);
            self.filter_l.reset();
            self.filter_r.reset();
            self.follower_l.reset();
            self.follower_r.reset();
            self.smooth_freq_l = self.min_freq;
            self.smooth_freq_r = self.min_freq;
            self.update_counter = 0;
        }

        fn params(&self) -> Vec<ParamDef> {
            vec![
                ParamDef::with_unit("min_freq",   50.0,  1000.0, self.min_freq,   "Hz"),
                ParamDef::with_unit("max_freq",   500.0, 8000.0, self.max_freq,   "Hz"),
                ParamDef::with_unit("attack_ms",  1.0,   200.0,  self.attack_ms,  "ms"),
                ParamDef::with_unit("release_ms", 10.0,  500.0,  self.release_ms, "ms"),
                ParamDef::new(      "q",          0.3,   5.0,    self.q),
            ]
        }

        fn set_param(&mut self, index: usize, value: f32) {
            match index {
                0 => self.min_freq   = value,
                1 => self.max_freq   = value,
                2 => {
                    self.attack_ms = value;
                    self.follower_l.set(Setting::attack_release(value / 1000.0, self.release_ms / 1000.0));
                    self.follower_r.set(Setting::attack_release(value / 1000.0, self.release_ms / 1000.0));
                }
                3 => {
                    self.release_ms = value;
                    self.follower_l.set(Setting::attack_release(self.attack_ms / 1000.0, value / 1000.0));
                    self.follower_r.set(Setting::attack_release(self.attack_ms / 1000.0, value / 1000.0));
                }
                4 => self.q = value,
                _ => {}
            }
        }
    }

    // ── Overdrive / Distortion ───────────────────────────────────
    // Promoted to dedicated struct to support params
    pub struct Overdrive {
        gain: f32,
        unit_l: Box<dyn AudioUnit>,
        unit_r: Box<dyn AudioUnit>,
    }

    impl Overdrive {
        pub fn new(gain: f32) -> Self {
            Self {
                gain,
                unit_l: Box::new(shape_fn(move |x: f32| {
                    let v = 1.4 / gain.sqrt();
                    let xg = x * gain;
                    if xg > 0.0 { xg / (1.0 + xg.abs()) * v }
                    else { xg / (1.0 + xg.abs() * 0.5) * v }
                })),
                unit_r: Box::new(shape_fn(move |x: f32| {
                    let v = 1.4 / gain.sqrt();
                    let xg = x * gain;
                    if xg > 0.0 { xg / (1.0 + xg.abs()) * v }
                    else { xg / (1.0 + xg.abs() * 0.5) * v }
                })),
            }
        }

        fn rebuild(&mut self) {
            let gain = self.gain;
            self.unit_l = Box::new(shape_fn(move |x: f32| {
                let v = 1.4 / gain.sqrt();
                let xg = x * gain;
                if xg > 0.0 { xg / (1.0 + xg.abs()) * v }
                else { xg / (1.0 + xg.abs() * 0.5) * v }
            }));
            self.unit_r = Box::new(shape_fn(move |x: f32| {
                let v = 1.4 / gain.sqrt();
                let xg = x * gain;
                if xg > 0.0 { xg / (1.0 + xg.abs()) * v }
                else { xg / (1.0 + xg.abs() * 0.5) * v }
            }));
        }
    }

    impl Effect for Overdrive {
        fn process_sample(&mut self, l: f32, r: f32) -> (f32, f32) {
            let mut ol = [0.0f32; 1];
            let mut or_ = [0.0f32; 1];
            self.unit_l.tick(&[l], &mut ol);
            self.unit_r.tick(&[r], &mut or_);
            (ol[0], or_[0])
        }
        fn name(&self) -> &str {
            if self.gain >= 20.0 { "Distortion" } else { "Overdrive" }
        }
        fn init(&mut self, sample_rate: f64) {
            self.unit_l.set_sample_rate(sample_rate);
            self.unit_r.set_sample_rate(sample_rate);
        }
        fn params(&self) -> Vec<ParamDef> {
            vec![ParamDef::new("gain", 1.0, 50.0, self.gain)]
        }
        fn set_param(&mut self, index: usize, value: f32) {
            if index == 0 {
                self.gain = value;
                self.rebuild();
            }
        }
    }

    // ── Reverb ───────────────────────────────────────────────────
    pub struct Reverb {
        room_size: f64,
        time:      f64,
        damping:   f64,
        dry:       f32,
        wet:       f32,
        unit: Box<dyn AudioUnit>,
    }

    impl Reverb {
        pub fn new(room_size: f64, time: f64, damping: f64) -> Self {
            Self {
                room_size, time, damping,
                dry: 0.9, wet: 0.3,
                unit: Box::new(reverb_stereo(room_size, time, damping)),
            }
        }
        fn rebuild(&mut self) {
            self.unit = Box::new(reverb_stereo(self.room_size, self.time, self.damping));
        }
    }

    impl Effect for Reverb {
        fn process_sample(&mut self, l: f32, r: f32) -> (f32, f32) {
            let mut out = [0.0f32; 2];
            self.unit.tick(&[l, r], &mut out);
            (l * self.dry + out[0] * self.wet,
             r * self.dry + out[1] * self.wet)
        }
        fn name(&self) -> &str { "Reverb" }
        fn init(&mut self, sample_rate: f64) {
            self.unit.set_sample_rate(sample_rate);
            self.unit.reset();
        }
        fn params(&self) -> Vec<ParamDef> {
            vec![
                ParamDef::new("room_size", 0.5,  50.0, self.room_size as f32),
                ParamDef::new("time",      0.1,  5.0,  self.time      as f32),
                ParamDef::new("damping",   0.0,  1.0,  self.damping   as f32),
                ParamDef::new("dry",       0.0,  1.0,  self.dry),
                ParamDef::new("wet",       0.0,  1.0,  self.wet),
            ]
        }
        fn set_param(&mut self, index: usize, value: f32) {
            match index {
                0 => { self.room_size = value as f64; self.rebuild(); }
                1 => { self.time      = value as f64; self.rebuild(); }
                2 => { self.damping   = value as f64; self.rebuild(); }
                3 => self.dry = value,
                4 => self.wet = value,
                _ => {}
            }
        }
    }

    // ── EQ 3-Band ─────────────────────────────────────────────────
    pub struct Eq3Band {
        low_freq:  f32, low_db:  f32, low_q:  f32,
        mid_freq:  f32, mid_db:  f32, mid_q:  f32,
        high_freq: f32, high_db: f32, high_q: f32,
        gain_db:   f32, // Total gain
        unit_l: Box<dyn AudioUnit>,
        unit_r: Box<dyn AudioUnit>,
    }

    impl Eq3Band {
        pub fn new(
            low_freq: f32,  low_db: f32,  low_q: f32,
            mid_freq: f32,  mid_db: f32,  mid_q: f32,
            high_freq: f32, high_db: f32, high_q: f32,
        ) -> Self {
            let (ll, ml, hl) = (db_to_linear(low_db), db_to_linear(mid_db), db_to_linear(high_db));
            Self {
                low_freq, low_db, low_q,
                mid_freq, mid_db, mid_q,
                high_freq, high_db, high_q,
                gain_db: 0.0,
                unit_l: Box::new(lowshelf_hz(low_freq, low_q, ll) >> bell_hz(mid_freq, mid_q, ml) >> highshelf_hz(high_freq, high_q, hl)),
                unit_r: Box::new(lowshelf_hz(low_freq, low_q, ll) >> bell_hz(mid_freq, mid_q, ml) >> highshelf_hz(high_freq, high_q, hl)),
            }
        }
        fn rebuild(&mut self) {
            let (ll, ml, hl) = (db_to_linear(self.low_db), db_to_linear(self.mid_db), db_to_linear(self.high_db));
            self.unit_l = Box::new(lowshelf_hz(self.low_freq, self.low_q, ll) >> bell_hz(self.mid_freq, self.mid_q, ml) >> highshelf_hz(self.high_freq, self.high_q, hl));
            self.unit_r = Box::new(lowshelf_hz(self.low_freq, self.low_q, ll) >> bell_hz(self.mid_freq, self.mid_q, ml) >> highshelf_hz(self.high_freq, self.high_q, hl));
        }
    }

    impl Effect for Eq3Band {
        fn process_sample(&mut self, l: f32, r: f32) -> (f32, f32) {
            let mut ol = [0.0f32; 1];
            let mut or_ = [0.0f32; 1];
            self.unit_l.tick(&[l], &mut ol);
            self.unit_r.tick(&[r], &mut or_);
            let gain = db_to_linear(self.gain_db);
            (ol[0] * gain, or_[0] * gain)
        }
        fn name(&self) -> &str { "EQ3Band" }
        fn init(&mut self, sample_rate: f64) {
            self.unit_l.set_sample_rate(sample_rate);
            self.unit_r.set_sample_rate(sample_rate);
            self.unit_l.reset();
            self.unit_r.reset();
        }
        fn params(&self) -> Vec<ParamDef> {
            vec![
                ParamDef::new("low_freq",  20.0,   800.0,  self.low_freq),
                ParamDef::new("low_db",   -12.0,   12.0,   self.low_db),
                ParamDef::new("mid_freq",  200.0,  4000.0, self.mid_freq),
                ParamDef::new("mid_db",   -12.0,   12.0,   self.mid_db),
                ParamDef::new("high_freq", 1000.0, 16000.0,self.high_freq),
                ParamDef::new("high_db",  -12.0,   12.0,   self.high_db),
                ParamDef::new("gain_db",  -12.0,   12.0,   self.gain_db),
            ]
        }
        fn set_param(&mut self, index: usize, value: f32) {
            match index {
                0 => self.low_freq  = value,
                1 => self.low_db    = value,
                2 => self.mid_freq  = value,
                3 => self.mid_db    = value,
                4 => self.high_freq = value,
                5 => self.high_db   = value,
                6 => self.gain_db   = value,
                _ => {}
            }
            self.rebuild();
        }
    }

    // ── Echo ──────────────────────────────────────────────────────
    pub struct Echo {
        delay_time: f64,
        feedback:   f32,
        wet:        f32,
        unit: Box<dyn AudioUnit>,
    }

    impl Echo {
        pub fn new(delay_time: f64, feedback: f32) -> Self {
            Self {
                delay_time, feedback, wet: 0.8,
                unit: Box::new(
                    (pass() & feedback2(delay(delay_time), pass() * feedback))
                    | (pass() & feedback2(delay(delay_time), pass() * feedback))
                ),
            }
        }
        fn rebuild(&mut self) {
            let (dt, fb) = (self.delay_time, self.feedback);
            self.unit = Box::new(
                (pass() & feedback2(delay(dt), pass() * fb))
                | (pass() & feedback2(delay(dt), pass() * fb))
            );
        }
    }

    impl Effect for Echo {
        fn process_sample(&mut self, l: f32, r: f32) -> (f32, f32) {
            let mut out = [0.0f32; 2];
            self.unit.tick(&[l, r], &mut out);
            (l * (1.0 - self.wet) + out[0] * self.wet,
             r * (1.0 - self.wet) + out[1] * self.wet)
        }
        fn name(&self) -> &str { "Echo" }
        fn init(&mut self, sample_rate: f64) {
            self.unit.set_sample_rate(sample_rate);
            self.unit.reset();
        }
        fn params(&self) -> Vec<ParamDef> {
            vec![
                ParamDef::new("delay_time", 0.05, 1.0,  self.delay_time as f32),
                ParamDef::new("feedback",   0.0,  0.95, self.feedback),
                ParamDef::new("wet",        0.0,  1.0,  self.wet),
            ]
        }
        fn set_param(&mut self, index: usize, value: f32) {
            match index {
                0 => { self.delay_time = value as f64; self.rebuild(); }
                1 => { self.feedback   = value;        self.rebuild(); }
                2 =>   self.wet        = value,
                _ => {}
            }
        }
    }

    // ── Flanger ───────────────────────────────────────────────────
    pub struct Flanger {
        rate:     f32,
        feedback: f32,
        depth:    f32,  // LFO depth (0.0-1.0, ratio relative to max_delay)
        wet:      f32,
        volume:   f32,  // Overall output gain
        unit: Box<dyn AudioUnit>,
    }

    impl Flanger {
        pub fn new(rate: f32, feedback: f32) -> Self {
            Self {
                rate, feedback, depth: 0.5, wet: 0.5, volume: 0.7,
                unit: Box::new(
                    flanger(feedback, 0.001, 0.020, move |t: f32| lerp(0.001, 0.020, sin_hz(rate, t) * 0.5 + 0.5))
                    | flanger(feedback, 0.001, 0.020, move |t: f32| lerp(0.001, 0.020, sin_hz(rate, t) * 0.5 + 0.5))
                ),
            }
        }
        fn rebuild(&mut self) {
            let (r, fb, d) = (self.rate, self.feedback, self.depth);
            let min_d = 0.001f32;
            let max_d = 0.001 + 0.019 * d;
            self.unit = Box::new(
                flanger(fb, min_d, max_d, move |t: f32| lerp(min_d, max_d, sin_hz(r, t) * 0.5 + 0.5))
                | flanger(fb, min_d, max_d, move |t: f32| lerp(min_d, max_d, sin_hz(r, t) * 0.5 + 0.5))
            );
        }
    }

    impl Effect for Flanger {
        fn process_sample(&mut self, l: f32, r: f32) -> (f32, f32) {
            let mut out = [0.0f32; 2];
            self.unit.tick(&[l, r], &mut out);
            let ol = l * (1.0 - self.wet) + out[0] * self.wet;
            let or_ = r * (1.0 - self.wet) + out[1] * self.wet;
            (ol * self.volume, or_ * self.volume)
        }
        fn name(&self) -> &str { "Flanger" }
        fn init(&mut self, sample_rate: f64) {
            self.unit.set_sample_rate(sample_rate);
            self.unit.reset();
        }
        fn params(&self) -> Vec<ParamDef> {
            vec![
                ParamDef::new("rate",     0.05, 5.0, self.rate),
                ParamDef::new("depth",    0.0,  1.0, self.depth),
                ParamDef::new("feedback", 0.0,  0.9, self.feedback),
                ParamDef::new("wet",      0.0,  1.0, self.wet),
            ]
        }
        fn set_param(&mut self, index: usize, value: f32) {
            match index {
                0 => { self.rate     = value; self.rebuild(); }
                1 => { self.depth    = value; self.rebuild(); }
                2 => { self.feedback = value; self.rebuild(); }
                3 =>   self.wet      = value,
                _ => {}
            }
        }
    }

    // ── Phaser ───────────────────────────────────────────────────
    pub struct Phaser {
        rate:  f32,
        depth: f32,  // magnitude (0.0-1.0)
        wet:   f32,
        unit: Box<dyn AudioUnit>,
    }

    impl Phaser {
        pub fn new(rate: f32) -> Self {
            Self {
                rate, depth: 0.5, wet: 0.8,
                unit: Box::new(
                    phaser(0.5, move |t| lerp(0.0, 1.0, sin_hz(rate, t) * 0.5 + 0.5))
                    | phaser(0.5, move |t| lerp(0.0, 1.0, sin_hz(rate, t) * 0.5 + 0.5))
                ),
            }
        }
        fn rebuild(&mut self) {
            let (r, d) = (self.rate, self.depth);
            self.unit = Box::new(
                phaser(d, move |t| lerp(0.0, 1.0, sin_hz(r, t) * 0.5 + 0.5))
                | phaser(d, move |t| lerp(0.0, 1.0, sin_hz(r, t) * 0.5 + 0.5))
            );
        }
    }

    impl Effect for Phaser {
        fn process_sample(&mut self, l: f32, r: f32) -> (f32, f32) {
            let mut out = [0.0f32; 2];
            self.unit.tick(&[l, r], &mut out);
            (l * (1.0 - self.wet) + out[0] * self.wet,
             r * (1.0 - self.wet) + out[1] * self.wet)
        }
        fn name(&self) -> &str { "Phaser" }
        fn init(&mut self, sample_rate: f64) {
            self.unit.set_sample_rate(sample_rate);
            self.unit.reset();
        }
        fn params(&self) -> Vec<ParamDef> {
            vec![
                ParamDef::new("rate",  0.05, 5.0, self.rate),
                ParamDef::new("depth", 0.0,  1.0, self.depth),
                ParamDef::new("wet",   0.0,  1.0, self.wet),
            ]
        }
        fn set_param(&mut self, index: usize, value: f32) {
            match index {
                0 => { self.rate  = value; self.rebuild(); }
                1 => { self.depth = value; self.rebuild(); }
                2 =>   self.wet   = value,
                _ => {}
            }
        }
    }

    // ── RingMod ──────────────────────────────────────────────────
    pub struct RingMod {
        carrier_freq: f32,
        mix: f32,  // 0.0=dry only, 1.0=wet only
        unit: Box<dyn AudioUnit>,
    }

    impl RingMod {
        pub fn new(carrier_freq: f32) -> Self {
            Self {
                carrier_freq,
                mix: 1.0,
                unit: Box::new(
                    (pass() * sine_hz::<f32>(carrier_freq))
                    | (pass() * sine_hz::<f32>(carrier_freq))
                ),
            }
        }
        fn rebuild(&mut self) {
            let cf = self.carrier_freq;
            self.unit = Box::new(
                (pass() * sine_hz::<f32>(cf))
                | (pass() * sine_hz::<f32>(cf))
            );
        }
    }

    impl Effect for RingMod {
        fn process_sample(&mut self, l: f32, r: f32) -> (f32, f32) {
            let mut out = [0.0f32; 2];
            self.unit.tick(&[l, r], &mut out);
            // Normalize volume by dividing by RMS of sine wave (1/√2)
            // At wet=1.0, output volume equals dry input volume
            const NORM: f32 = std::f32::consts::SQRT_2; // ≈ 1.414
            let wet_l = out[0] * NORM;
            let wet_r = out[1] * NORM;
            (l * (1.0 - self.mix) + wet_l * self.mix,
             r * (1.0 - self.mix) + wet_r * self.mix)
        }
        fn name(&self) -> &str { "RingMod" }
        fn init(&mut self, sample_rate: f64) {
            self.unit.set_sample_rate(sample_rate);
            self.unit.reset();
        }
        fn params(&self) -> Vec<ParamDef> {
            vec![
                ParamDef::new("carrier_freq", 50.0, 2000.0, self.carrier_freq),
                ParamDef::new("mix",          0.0,  1.0,    self.mix),
            ]
        }
        fn set_param(&mut self, index: usize, value: f32) {
            match index {
                0 => { self.carrier_freq = value; self.rebuild(); }
                1 =>   self.mix = value,
                _ => {}
            }
        }
    }

    // ── OctaveUp ─────────────────────────────────────────────────
    // No parameters (FxUnit as-is is fine)
    pub fn octave_up() -> Box<dyn Effect> {
        Box::new(FxUnit::new(
            Box::new(
                (shape_fn(|x: f32| x.abs()) >> highpass_hz(20.0, 0.7))
                | (shape_fn(|x: f32| x.abs()) >> highpass_hz(20.0, 0.7))
            ),
            0.0, 1.3, "OctaveUp"
        ))
    }

    // ── Chorus ───────────────────────────────────────────────────
    pub struct Chorus {
        rate:  f32,  // variation (LFO rate equivalent)
        depth: f32,  // mod_depth
        wet:   f32,
        unit: Box<dyn AudioUnit>,
    }

    impl Chorus {
        pub fn new(rate: f32, depth: f32) -> Self {
            Self {
                rate, depth, wet: 1.0,
                unit: Box::new(
                    (chorus(0, 0.02, rate, depth) >> mul(1.9f32))
                    | (chorus(1, 0.02, rate, depth) >> mul(1.9f32))
                ),
            }
        }
        fn rebuild(&mut self) {
            let (r, d) = (self.rate, self.depth);
            self.unit = Box::new(
                (chorus(0, 0.02, r, d) >> mul(1.9f32))
                | (chorus(1, 0.02, r, d) >> mul(1.9f32))
            );
        }
    }

    impl Effect for Chorus {
        fn process_sample(&mut self, l: f32, r: f32) -> (f32, f32) {
            let mut out = [0.0f32; 2];
            self.unit.tick(&[l, r], &mut out);
            (l * (1.0 - self.wet) + out[0] * self.wet,
             r * (1.0 - self.wet) + out[1] * self.wet)
        }
        fn name(&self) -> &str { "Chorus" }
        fn init(&mut self, sample_rate: f64) {
            self.unit.set_sample_rate(sample_rate);
            self.unit.reset();
        }
        fn params(&self) -> Vec<ParamDef> {
            vec![
                ParamDef::new("rate",  0.01, 0.1,  self.rate),
                ParamDef::new("depth", 0.01, 0.5,  self.depth),
                ParamDef::new("wet",   0.0,  1.0,  self.wet),
            ]
        }
        fn set_param(&mut self, index: usize, value: f32) {
            match index {
                0 => { self.rate  = value; self.rebuild(); }
                1 => { self.depth = value; self.rebuild(); }
                2 =>   self.wet   = value,
                _ => {}
            }
        }
    }

    // ── Guitar Synth (FunDSP square wave oscillator) ───────────────────
    pub struct GuitarSynth {
        sample_rate:  f32,
        volume:       f32,
        attack_ms:    f32,
        release_ms:   f32,
        freq_smooth:  f32,

        // Pre-stage LPF (removes harmonics to improve pitch detection)
        lpf: Box<dyn AudioUnit>,

        // FunDSP square wave oscillator (frequency controlled via input port)
        osc_l: Box<dyn AudioUnit>,
        osc_r: Box<dyn AudioUnit>,

        // Autocorrelation buffer (LPF-filtered signal)
        buf:     Vec<f32>,
        buf_pos: usize,

        // Envelope follower (afollow)
        follower: Box<dyn AudioUnit>,
    }

    impl GuitarSynth {
        pub fn new() -> Self {
            Self {
                sample_rate:  48000.0,
                volume:       0.8,
                attack_ms:    20.0,
                release_ms:   80.0,
                freq_smooth:  0.0,
                lpf:      Box::new(lowpass_hz(440.0, 0.9)),
                osc_l:    Box::new(square()),
                osc_r:    Box::new(square()),
                buf:      vec![0.0f32; 2048],
                buf_pos:  0,
                follower: Box::new(afollow(0.020, 0.080)), // attack 20ms, release 80ms
            }
        }

        fn detect_pitch(&self) -> f32 {
            let n  = self.buf.len();
            let sr = self.sample_rate;
            let lag_min = (sr / 880.0) as usize; // 880Hz upper limit
            let lag_max = std::cmp::Ord::min((sr / 40.0) as usize, n / 2 - 1); // 40Hz lower limit
            if lag_min >= lag_max { return 0.0; }

            let rms = (self.buf.iter().map(|s| s * s).sum::<f32>() / n as f32).sqrt();
            if rms < 0.01 { return 0.0; }

            let r0: f32 = self.buf.iter().map(|s| s * s).sum();
            if r0 < 1e-6 { return 0.0; }

            let mut best_lag = 0usize;
            let mut best_val = 0.0f32;
            for lag in lag_min..=lag_max {
                let r: f32 = (0..(n - lag)).map(|i| self.buf[i] * self.buf[i + lag]).sum();
                let normalized = r / r0;
                if normalized > best_val {
                    best_val = normalized;
                    best_lag = lag;
                }
            }
            if best_lag == 0 || best_val < 0.25 { return 0.0; }
            sr / best_lag as f32
        }
    }

    impl Effect for GuitarSynth {
        fn process_sample(&mut self, l: f32, _r: f32) -> (f32, f32) {
            // Accumulate LPF-filtered signal (harmonics removed) into buffer
            let mut lpf_out = [0.0f32; 1];
            self.lpf.tick(&[l], &mut lpf_out);
            self.buf[self.buf_pos] = lpf_out[0];
            self.buf_pos = (self.buf_pos + 1) % self.buf.len();

            // Envelope tracking via afollow()
            let mut env_out = [0.0f32; 1];
            self.follower.tick(&[l.abs()], &mut env_out);
            let env = env_out[0];

            // Pitch detection every buffer cycle (2048 samples ≈ 43ms)
            if self.buf_pos == 0 {
                let detected = self.detect_pitch();
                if detected > 20.0 {
                    self.freq_smooth += 0.8 * (detected - self.freq_smooth);
                } else {
                    // self.freq_smooth *= 0.95;
                }
            }

            if env < 0.002 || self.freq_smooth < 20.0 {
                return (0.0, 0.0);
            }

            let mut out_l = [0.0f32; 1];
            let mut out_r = [0.0f32; 1];
            self.osc_l.tick(&[self.freq_smooth], &mut out_l);
            self.osc_r.tick(&[self.freq_smooth], &mut out_r);

            let amp = (env * 4.0).clamp(0.0, 1.0) * self.volume;
            (out_l[0] * amp, out_r[0] * amp)
        }

        fn name(&self) -> &str { "GuitarSynth" }

        fn init(&mut self, sample_rate: f64) {
            self.sample_rate = sample_rate as f32;
            self.lpf         = Box::new(lowpass_hz(440.0, 0.9));
            self.osc_l       = Box::new(square());
            self.osc_r       = Box::new(square());
            self.follower    = Box::new(afollow(0.020, 0.080));
            self.lpf.set_sample_rate(sample_rate);
            self.osc_l.set_sample_rate(sample_rate);
            self.osc_r.set_sample_rate(sample_rate);
            self.follower.set_sample_rate(sample_rate);
            self.freq_smooth = 0.0;
            self.buf_pos     = 0;
            self.buf         = vec![0.0f32; 2048];
        }

        fn params(&self) -> Vec<ParamDef> {
            vec![
                ParamDef::new(      "volume",     0.1,   1.0,    self.volume),
                ParamDef::with_unit("attack_ms",  1.0,   200.0,  self.attack_ms,  "ms"),
                ParamDef::with_unit("release_ms", 10.0,  500.0,  self.release_ms, "ms"),
            ]
        }

        fn set_param(&mut self, index: usize, value: f32) {
            match index {
                0 => self.volume = value,
                1 => {
                    self.attack_ms = value;
                    self.follower.set(Setting::attack_release(value / 1000.0, self.release_ms / 1000.0));
                }
                2 => {
                    self.release_ms = value;
                    self.follower.set(Setting::attack_release(self.attack_ms / 1000.0, value / 1000.0));
                }
                _ => {}
            }
        }
    }

    // ── Compressor / Limiter ─────────────────────────────────────
    // Acts as a limiter when ratio is large (e.g. 100)
    pub struct Compressor {
        // User-facing parameters
        threshold_db: f32,
        ratio:        f32,
        attack_ms:    f32,
        release_ms:   f32,
        gain_db:      f32,   // Makeup gain
        // Internally converted
        threshold_lin: f32,
        gain_lin:      f32,
        // Envelope follower (afollow)
        follower: Box<dyn AudioUnit>,
    }

    impl Compressor {
        pub fn new(threshold_db: f32, ratio: f32) -> Self {
            let attack_ms  = 5.0f32;
            let release_ms = 50.0f32;
            Self {
                threshold_db,
                ratio,
                attack_ms,
                release_ms,
                gain_db:       0.0,
                threshold_lin: 10.0f32.powf(threshold_db / 20.0),
                gain_lin:      1.0,
                follower:      Box::new(afollow(attack_ms / 1000.0, release_ms / 1000.0)),
            }
        }
    }

    impl Effect for Compressor {
        fn process_sample(&mut self, l: f32, r: f32) -> (f32, f32) {
            // Track peak level via afollow() (max of L/R)
            let level = l.abs().max(r.abs());
            let mut env_out = [0.0f32; 1];
            self.follower.tick(&[level], &mut env_out);
            let envelope = env_out[0];

            // Gain computer (calculated in dB domain)
            let gain = if envelope > self.threshold_lin && envelope > 1e-6 {
                let env_db  = 20.0 * envelope.log10();
                let excess  = env_db - self.threshold_db;
                let reduced = excess / self.ratio;
                10.0f32.powf(-(excess - reduced) / 20.0)
            } else {
                1.0
            };

            let out = gain * self.gain_lin;
            (l * out, r * out)
        }

        fn name(&self) -> &str {
            if self.ratio >= 20.0 { "Limiter" } else { "Compressor" }
        }

        fn init(&mut self, sample_rate: f64) {
            self.follower = Box::new(afollow(self.attack_ms / 1000.0, self.release_ms / 1000.0));
            self.follower.set_sample_rate(sample_rate);
            self.follower.reset();
        }

        fn params(&self) -> Vec<ParamDef> {
            vec![
                ParamDef::with_unit("threshold", -60.0, 0.0,   self.threshold_db, "dB"),
                ParamDef::new(      "ratio",        1.0, 100.0, self.ratio),
                ParamDef::with_unit("attack",       0.1, 100.0, self.attack_ms,    "ms"),
                ParamDef::with_unit("release",      1.0, 500.0, self.release_ms,   "ms"),
                ParamDef::with_unit("gain",        -12.0, 24.0, self.gain_db,      "dB"),
            ]
        }

        fn set_param(&mut self, index: usize, value: f32) {
            match index {
                0 => { self.threshold_db  = value; self.threshold_lin = 10.0f32.powf(value / 20.0); }
                1 =>   self.ratio         = value,
                2 => { self.attack_ms     = value;
                       self.follower.set(Setting::attack_release(value / 1000.0, self.release_ms / 1000.0)); }
                3 => { self.release_ms    = value;
                       self.follower.set(Setting::attack_release(self.attack_ms / 1000.0, value / 1000.0)); }
                4 => { self.gain_db       = value; self.gain_lin = 10.0f32.powf(value / 20.0); }
                _ => {}
            }
        }
    }

    // ── Bypass ───────────────────────────────────────────────────
    pub fn bypass() -> Box<dyn Effect> {
        Box::new(FxUnit::new(
            Box::new(mul(1.0f32) | mul(1.0f32)),
            1.0, 0.0, "Bypass"
        ))
    }

    // ── EffectChain construction helper ──────────────────────────────────
    // Wraps single effect in EffectChain for unified interface
    pub fn make_chain(name: &str, effects: Vec<Box<dyn Effect>>) -> Box<dyn Effect> {
        Box::new(EffectChain { effects, name: name.to_string() })
    }

    // ── Preset construction ────────────────────────────────────────────
    // make_preset() returns preset name and key
    pub fn make_effect(key: &str) -> Option<Box<dyn Effect>> {
        match key {
            "b" => Some(make_chain("Bypass",     vec![bypass()])),
            "d" => Some(make_chain("Overdrive",  vec![Box::new(Overdrive::new(8.0))])),
            "D" => Some(make_chain("Distortion", vec![Box::new(Overdrive::new(25.0))])),
            "a" => Some(make_chain("AutoWah",    vec![Box::new(AutoWah::new(250.0, 3000.0, 5.0, 100.0, 1.5))])),
            "o" => Some(make_chain("OctaveUp",   vec![octave_up()])),
            "x" => Some(make_chain("RingMod",    vec![Box::new(RingMod::new(440.0))])),
            "q" => Some(make_chain("EQ3Band",    vec![Box::new(Eq3Band::new(400.0,-6.0,0.7, 800.0,3.0,0.7, 8000.0,-6.0,0.7))])),
            "g" => Some(make_chain("NoiseGate",  vec![Box::new(NoiseGate::new(-40.0))])),
            "e" => Some(make_chain("Echo",       vec![Box::new(Echo::new(0.35, 0.2))])),
            "r" => Some(make_chain("Reverb",     vec![Box::new(Reverb::new(20.0, 2.0, 0.5))])),
            "c" => Some(make_chain("Chorus",     vec![Box::new(Chorus::new(0.03, 0.3))])),
            "f" => Some(make_chain("Flanger",    vec![Box::new(Flanger::new(0.3, 0.5))])),
            "p" => Some(make_chain("Phaser",     vec![Box::new(Phaser::new(0.3))])),
            // Chain presets
            "1" => Some(make_chain("OD>EQ>Reverb", vec![
                Box::new(Overdrive::new(15.0)),
                Box::new(Eq3Band::new(400.0,-6.0,0.7, 800.0,3.0,0.5, 9000.0,-6.0,0.7)),
                Box::new(Reverb::new(3.0, 0.3, 0.2)),
            ])),
            "2" => Some(make_chain("OD>EQ>Phaser", vec![
                Box::new(Overdrive::new(15.0)),
                Box::new(Eq3Band::new(400.0,-6.0,0.7, 800.0,3.0,0.5, 9000.0,-6.0,0.7)),
                Box::new(Phaser::new(0.3)),
            ])),
            "3" => Some(make_chain("Wah>OD>Reverb", vec![
                Box::new(AutoWah::new(250.0, 3000.0, 5.0, 100.0, 1.5)),
                Box::new(Overdrive::new(10.0)),
                Box::new(Reverb::new(3.0, 0.3, 0.3)),
            ])),
            _ => None,
        }
    }

    // ── Preset management ────────────────────────────────────────────
    // Save/load presets in TOML format
    //
    // presets.toml example:
    //
    // [[presets]]
    // name = "Crunch"
    // chain = ["overdrive", "eq3band", "reverb"]
    //
    // [presets.params.overdrive]
    // gain = 15.0
    //
    // [presets.params.eq3band]
    // low_freq = 400.0
    // low_db   = -6.0
    // mid_freq = 800.0
    // mid_db   = 3.0
    // high_freq = 9000.0
    // high_db  = -6.0
    //
    // [presets.params.reverb]
    // room_size = 3.0
    // time      = 0.3
    // damping   = 0.2

    use std::collections::HashMap;
    use serde::{Deserialize, Serialize};

    // Raw data structure for TOML serialization/deserialization
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct PresetDef {
        pub name:   String,
        // Effect type keys in chain order (e.g. ["overdrive", "eq3band", "reverb"])
        pub chain:  Vec<String>,
        // Effect type key → (parameter name → value)
        #[serde(default)]
        pub params: HashMap<String, HashMap<String, f32>>,
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct PresetsFile {
        presets: Vec<PresetDef>,
    }

    // Builds an effect with default parameters from effect type key
    pub fn make_single_effect(kind: &str) -> Option<Box<dyn Effect>> {
        match kind {
            "bypass"     => Some(bypass()),
            "overdrive"  => Some(Box::new(Overdrive::new(8.0))),
            "distortion" => Some(Box::new(Overdrive::new(25.0))),
            "autowah"    => Some(Box::new(AutoWah::new(250.0, 3000.0, 5.0, 100.0, 1.5))),
            "octaveup"   => Some(octave_up()),
            "ringmod"    => Some(Box::new(RingMod::new(440.0))),
            "eq3band"    => Some(Box::new(Eq3Band::new(400.0,-6.0,0.7, 800.0,3.0,0.7, 8000.0,-6.0,0.7))),
            "noisegate"  => Some(Box::new(NoiseGate::new(-40.0))), // -40dB
            "echo"       => Some(Box::new(Echo::new(0.35, 0.2))),
            "reverb"     => Some(Box::new(Reverb::new(20.0, 2.0, 0.5))),
            "chorus"     => Some(Box::new(Chorus::new(0.03, 0.3))),
            "flanger"    => Some(Box::new(Flanger::new(0.3, 0.5))),
            "phaser"     => Some(Box::new(Phaser::new(0.3))),
            "guitarsynth"  => Some(Box::new(GuitarSynth::new())),
            "compressor"   => Some(Box::new(Compressor::new(-20.0, 4.0))),
            "limiter"      => Some(Box::new(Compressor::new(-6.0, 100.0))),
            _              => None,
        }
    }

    // Converts PresetDef to Box<dyn Effect> (EffectChain)
    // Overrides default values with params if present
    pub fn build_preset(def: &PresetDef) -> Option<Box<dyn Effect>> {
        let mut effects: Vec<Box<dyn Effect>> = Vec::new();
        for kind in &def.chain {
            let mut e = make_single_effect(kind)?;
            // Override parameters
            if let Some(pmap) = def.params.get(kind) {
                let pdefs = e.params();
                for (i, pd) in pdefs.iter().enumerate() {
                    if let Some(&v) = pmap.get(pd.name) {
                        e.set_param(i, v);
                    }
                }
            }
            effects.push(e);
        }
        Some(make_chain(&def.name, effects))
    }

    // Path to presets file
    pub fn presets_path() -> std::path::PathBuf {
        // ~/.config/funpedals/presets.toml
        let base = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        std::path::PathBuf::from(base).join(".config").join("funpedals").join("presets.toml")
    }

    // Loads PresetDef list from TOML file
    // Returns built-in defaults if file does not exist
    pub fn load_presets() -> Vec<PresetDef> {
        let path = presets_path();
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(s) => match toml::from_str::<PresetsFile>(&s) {
                    Ok(pf) => {
                        println!("Presets loaded: {} preset(s) from{})", pf.presets.len(), path.display());
                        return pf.presets;
                    }
                    Err(e) => eprintln!("presets.toml parse error: {e}"),
                },
                Err(e) => eprintln!("presets.toml read error: {e}"),
            }
        }
        // Fallback: built-in default presets
        default_presets()
    }

    // Converts current effect (EffectChain) state to PresetDef
    #[allow(dead_code)]
    pub fn effect_to_preset_def(name: &str, effect: &Box<dyn Effect>) -> PresetDef {
        // Treat as single effect if not EffectChain
        let params_flat = effect.params();
        // Ideally restore chain from EffectChain.effects directly,
        // but difficult via trait objects, so infer from name.
        // → Save params only; chain is preserved as-is.
        let mut params_map: HashMap<String, HashMap<String, f32>> = HashMap::new();
        // Group by each effect in the chain
        // ParamDef has no effect name, so save flat under "effect" key
        let mut flat: HashMap<String, f32> = HashMap::new();
        for pd in &params_flat {
            flat.insert(pd.name.to_string(), pd.value);
        }
        if !flat.is_empty() {
            params_map.insert("_params".to_string(), flat);
        }
        PresetDef {
            name: name.to_string(),
            chain: vec![], // chain restoration not supported yet (edit TOML directly)
            params: params_map,
        }
    }

    // Writes preset list to TOML file
    // Overwrites existing preset with same name, or appends
    pub fn save_preset(def: &PresetDef) -> Result<(), String> {
        let path = presets_path();
        // Create directory
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }

        // Load existing file
        let mut pf = if path.exists() {
            let s = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
            toml::from_str::<PresetsFile>(&s).unwrap_or(PresetsFile { presets: vec![] })
        } else {
            PresetsFile { presets: vec![] }
        };

        // Overwrite or append preset with same name
        if let Some(existing) = pf.presets.iter_mut().find(|p| p.name == def.name) {
            *existing = def.clone();
        } else {
            pf.presets.push(def.clone());
        }

        let s = toml::to_string_pretty(&pf).map_err(|e| e.to_string())?;
        std::fs::write(&path, s).map_err(|e| e.to_string())?;
        Ok(())
    }

    // Built-in default presets (fallback when TOML file is absent)
    fn default_presets_toml() -> &'static str {
        r#"
[[presets]]
name = "Bypass"
chain = ["bypass"]

[presets.params]

[[presets]]
name = "OD>EQ>Reverb"
chain = ["overdrive", "eq3band", "reverb"]

[presets.params.overdrive]
gain = 15.0

[presets.params.eq3band]
low_freq = 400.0
low_db = -8.1
mid_freq = 800.0
mid_db = 3.0
high_freq = 9000.0
high_db = -6.0
gain_db = 1.8

[presets.params.reverb]
room_size = 3.0
time = 0.3
damping = 0.56
dry = 0.9
wet = 0.3

[[presets]]
name = "OD>EQ>Phaser"
chain = ["overdrive", "eq3band", "phaser"]

[presets.params.overdrive]
gain = 15.0

[presets.params.eq3band]
low_freq = 400.0
low_db = -6.0
mid_freq = 800.0
mid_db = 3.0
high_freq = 9000.0
high_db = -6.0
gain_db = 0.74

[presets.params.phaser]
rate = 0.3
depth = 0.36
wet = 1.0

[[presets]]
name = "Wah>OD>Reverb"
chain = ["autowah", "overdrive", "reverb"]

[presets.params.autowah]
min_freq = 195.8
max_freq = 2612.7
attack_ms = 87.3
release_ms = 100.0
q = 2.36

[presets.params.overdrive]
gain = 6.1

[presets.params.reverb]
room_size = 3.0
time = 0.3
damping = 0.3
dry = 1.0
wet = 0.58

[[presets]]
name = "OD>Echo>Reverb"
chain = ["overdrive", "echo", "reverb"]

[presets.params.overdrive]
gain = 5.7

[presets.params.echo]
delay_time = 0.35
feedback = 0.2
wet = 0.55

[presets.params.reverb]
room_size = 3.0
time = 0.3
damping = 0.2
dry = 0.9
wet = 0.3

[[presets]]
name = "EQ>Chorus"
chain = ["eq3band", "chorus"]

[presets.params.eq3band]
low_freq = 400.0
low_db = -0.2
mid_freq = 783.4
mid_db = 0.1
high_freq = 9000.0
high_db = 0.0
gain_db = 1.6

[presets.params.chorus]
rate = 0.047
depth = 0.42
wet = 0.78

[[presets]]
name = "OD>NoiseGate"
chain = ["overdrive", "noisegate"]

[presets.params.overdrive]
gain = 42.8

[presets.params.noisegate]
threshold = -41.2
attack = 8.4
decay = 122.1

[[presets]]
name = "OctaveUp"
chain = ["octaveup"]

[presets.params]

[[presets]]
name = "RingMod"
chain = ["ringmod"]

[presets.params.ringmod]
carrier_freq = 1404.0
mix = 0.55

[[presets]]
name = "Flanger"
chain = ["flanger"]

[presets.params.flanger]
rate = 0.66
depth = 0.41
feedback = 0.48
wet = 1.0

[[presets]]
name = "GuitarSynth"
chain = ["guitarsynth"]

[presets.params.guitarsynth]
volume = 0.34
attack_ms = 1.0
release_ms = 59.7

[[presets]]
name = "EQ3Band"
chain = ["eq3band"]

[presets.params.eq3band]
low_freq = 290.3
low_db = -7.9
mid_freq = 1195.5
mid_db = 1.8
high_freq = 6387.3
high_db = -8.6
gain_db = 2.5

[[presets]]
name = "Echo"
chain = ["echo"]

[presets.params.echo]
delay_time = 0.35
feedback = 0.2
wet = 0.78

[[presets]]
name = "Reverb"
chain = ["reverb"]

[presets.params.reverb]
room_size = 4.8
time = 2.0
damping = 0.5
dry = 0.9
wet = 0.71

[[presets]]
name = "Phaser"
chain = ["phaser"]

[presets.params.phaser]
rate = 0.3
depth = 0.5
wet = 1.0

[[presets]]
name = "Overdrive"
chain = ["overdrive"]

[presets.params.overdrive]
gain = 6.1

[[presets]]
name = "Distortion"
chain = ["overdrive"]

[presets.params.overdrive]
gain = 16.9

[[presets]]
name = "AutoWah"
chain = ["autowah"]

[presets.params.autowah]
min_freq = 250.0
max_freq = 3000.0
attack_ms = 160.2
release_ms = 100.0
q = 3.18

[[presets]]
name = "Compressor"
chain = ["compressor"]

[presets.params.compressor]
threshold = -26.6
ratio = 16.9
attack = 9.1
release = 50.0
gain = 10.2

[[presets]]
name = "Limiter"
chain = ["limiter"]

[presets.params.limiter]
threshold = -6.0
ratio = 100.0
attack = 5.0
release = 50.0
gain = 0.0
"#
    }

    fn default_presets() -> Vec<PresetDef> {
        let pf: PresetsFile = toml::from_str(default_presets_toml())
            .expect("Failed to parse default presets");
        pf.presets
    }

    // Writes default presets.toml on first launch
    pub fn init_presets_file() {
        let path = presets_path();
        if !path.exists() {
            let defaults = default_presets();
            let pf = PresetsFile { presets: defaults };
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(s) = toml::to_string_pretty(&pf) {
                match std::fs::write(&path, s) {
                    Ok(_)  => println!("Default presets written: {}", path.display()),
                    Err(e) => eprintln!("presets.toml write error: {e}"),
                }
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::common::*;
    use alsa::pcm::*;
    use alsa::*;
    use ringbuf::HeapRb;
    use ringbuf::traits::{Consumer, Producer, Split, Observer};
    use std::thread;
    use std::sync::{Arc, Mutex};
    use std::io::Write;
    use sdl2::pixels::Color;
    use sdl2::event::Event;
    use sdl2::keyboard::Keycode;
    use sdl2::rect::{Point, Rect};
    use sdl2::ttf;

    const BLOCK_SIZE: usize = 64;
    const SAMPLE_RATE: f64 = 48000.0;
    const SNOOP_SIZE: usize = 4096;
    const FFT_SIZE: usize = 4096;
    const NUM_BARS: usize = 64;
    const WIDTH: u32 = 800;
    const HEIGHT: u32 = 480;
    const BTN_H: u32 = 36; // [PARAM][RELOAD] button height

    pub fn run() {
        // Check --gui flag
        let gui_mode = std::env::args().any(|a| a == "--gui");

        let rb = HeapRb::<i16>::new(BLOCK_SIZE * 8);
        let (mut prod, mut cons) = rb.split();

        let snoop_buf: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(vec![0.0f32; SNOOP_SIZE]));
        let snoop_buf_writer = Arc::clone(&snoop_buf);
        // Input RMS (referenced by NoiseGate)
        let input_rms: Arc<Mutex<f32>> = Arc::new(Mutex::new(0.0f32));
        let input_rms_main   = Arc::clone(&input_rms);
        let input_rms_menu   = Arc::clone(&input_rms);

        let mut initial = make_effect("b").unwrap();
        initial.init(SAMPLE_RATE);
        initial.inject_input_rms(Arc::clone(&input_rms));
        let effect_shared: Arc<Mutex<Box<dyn Effect>>> = Arc::new(Mutex::new(initial));

        // ── Input thread ──────────────────────────────────────────
        thread::spawn(move || {
            // Function to open ALSA capture device
            let open_capture = || -> Option<PCM> {
                for retry in 0..10 {
                    if retry > 0 {
                        eprintln!("Retrying input device reopen {}...", retry);
                        std::thread::sleep(std::time::Duration::from_millis(500));
                    }
                    let Ok(capture) = PCM::new("hw:1,0", Direction::Capture, false) else { continue };
                    {
                        let Ok(hwp) = HwParams::any(&capture) else { continue };
                        if hwp.set_channels(2).is_err() { continue }
                        if hwp.set_rate(48000, ValueOr::Nearest).is_err() { continue }
                        if hwp.set_format(pcm::Format::s16()).is_err() { continue }
                        if hwp.set_access(Access::RWInterleaved).is_err() { continue }
                        hwp.set_period_size(BLOCK_SIZE as alsa::pcm::Frames, ValueOr::Nearest).ok();
                        hwp.set_buffer_size((BLOCK_SIZE * 8) as alsa::pcm::Frames).ok();
                        if capture.hw_params(&hwp).is_err() { continue }
                    }
                    eprintln!("Input device opened successfully");
                    return Some(capture);
                }
                None
            };

            let Some(capture) = open_capture() else {
                eprintln!("Failed to open input device");
                return;
            };
            #[allow(unused_mut)]
            let mut capture = capture;
            let mut io = capture.io_i16().unwrap();

            loop {
                let mut buf = vec![0i16; BLOCK_SIZE * 2];
                match io.readi(&mut buf) {
                    Ok(_) => {
                        for s in buf.iter() {
                            while prod.try_push(*s).is_err() {}
                        }
                    },
                    Err(e) => {
                        eprintln!("Input error: {}", e);
                        if capture.recover(e.errno() as std::ffi::c_int, true).is_ok() {
                            capture.prepare().ok();
                        } else {
                            eprintln!("Reopening input device...");
                            if let Some(new_cap) = open_capture() {
                                drop(io);
                                capture = new_cap;
                                io = capture.io_i16().unwrap();
                            }
                        }
                    }
                }
            }
        });

        // ── Output device ──────────────────────────────────────────
        // Function to open ALSA playback device
        let open_playback = || -> Option<PCM> {
            for retry in 0..10 {
                if retry > 0 {
                    eprintln!("Retrying output device reopen {}...", retry);
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
                let Ok(playback) = PCM::new("hw:1,0", Direction::Playback, false) else { continue };
                {
                    let Ok(hwp) = HwParams::any(&playback) else { continue };
                    if hwp.set_channels(2).is_err() { continue }
                    if hwp.set_rate(48000, ValueOr::Nearest).is_err() { continue }
                    if hwp.set_format(pcm::Format::s16()).is_err() { continue }
                    if hwp.set_access(Access::RWInterleaved).is_err() { continue }
                    hwp.set_period_size(BLOCK_SIZE as alsa::pcm::Frames, ValueOr::Nearest).ok();
                    hwp.set_buffer_size((BLOCK_SIZE * 8) as alsa::pcm::Frames).ok();
                    if playback.hw_params(&hwp).is_err() { continue }
                }
                eprintln!("Output device opened successfully");
                return Some(playback);
            }
            None
        };

        #[allow(unused_mut)]
        let Some(mut playback) = open_playback() else {
            eprintln!("Failed to open output device");
            return;
        };
        let mut io_play = playback.io_i16().unwrap();

        // Initialize presets.toml on startup (write defaults if missing)
        init_presets_file();
        // Share preset list
        let presets_shared: Arc<Mutex<Vec<PresetDef>>> =
            Arc::new(Mutex::new(load_presets()));

        println!("FunPedals starting... Press Ctrl+C to stop");
        if gui_mode { println!("GUI mode"); } else { println!("Terminal mode"); }
        println!("Filling buffer...");
        while cons.occupied_len() < BLOCK_SIZE * 4 {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        println!("Ready!");

        // ── Terminal menu thread (always running)──────────────────
        let effect_menu   = Arc::clone(&effect_shared);
        let presets_menu  = Arc::clone(&presets_shared);
        thread::spawn(move || {
            loop {
                // Display preset list
                {
                    let presets = presets_menu.lock().unwrap();
                    println!("\n=== Preset selection ===");
                    for (i, p) in presets.iter().enumerate() {
                        println!("  {:2}: {}", i + 1, p.name);
                    }
                }
                println!("  number  : select preset");
                println!("  P       : show current parameters");
                println!("  S <name>: save current state as preset");
                println!("  R       : reload presets.toml");
                print!("select > ");
                std::io::stdout().flush().unwrap();

                let mut input = String::new();
                std::io::stdin().read_line(&mut input).unwrap();
                let line = input.trim().to_string();

                if line == "P" {
                    // Display parameters
                    let effect = effect_menu.lock().unwrap();
                    let params = effect.params();
                    if params.is_empty() {
                        println!("(no parameters)");
                    } else {
                        println!("--- {} parameters ---", effect.name());
                        for (i, p) in params.iter().enumerate() {
                            println!("  [{i}] {}: {:.3}  (min={:.3}, max={:.3})",
                                p.name, p.value, p.min, p.max);
                        }
                    }

                } else if line == "R" {
                    // Reload TOML
                    let new_presets = load_presets();
                    println!("{} presets loaded", new_presets.len());
                    *presets_menu.lock().unwrap() = new_presets;

                } else if let Some(rest) = line.strip_prefix("S ") {
                    // Save current state
                    let preset_name = rest.trim().to_string();
                    let current_name = effect_menu.lock().unwrap().name().to_string();

                    let chain = {
                        let presets = presets_menu.lock().unwrap();
                        presets.iter()
                            .find(|p| p.name == current_name)
                            .map(|p| p.chain.clone())
                            .unwrap_or_default()
                    };

                    // Group parameters by chain order and save
                    // Group by effect type within the chain
                    let effect = effect_menu.lock().unwrap();
                    let params = effect.params();
                    drop(effect);

                    let mut params_map: std::collections::HashMap<String, std::collections::HashMap<String, f32>> = std::collections::HashMap::new();
                    // Distribute parameters per effect in the chain
                    // Simply: split by default param count of each kind
                    let mut offset = 0usize;
                    for kind in &chain {
                        if let Some(e) = make_single_effect(kind) {
                            let pdefs = e.params();
                            let n = pdefs.len();
                            let mut pmap: std::collections::HashMap<String, f32> = std::collections::HashMap::new();
                            for (i, pd) in pdefs.iter().enumerate() {
                                let idx = offset + i;
                                if idx < params.len() {
                                    pmap.insert(pd.name.to_string(), params[idx].value);
                                }
                            }
                            if !pmap.is_empty() {
                                params_map.insert(kind.clone(), pmap);
                            }
                            offset += n;
                        }
                    }

                    let def = PresetDef {
                        name:   preset_name.clone(),
                        chain:  chain,
                        params: params_map,
                    };
                    match save_preset(&def) {
                        Ok(_) => {
                            println!("→ Preset \"{}\" saved", preset_name);
                            // Update preset list
                            *presets_menu.lock().unwrap() = load_presets();
                        }
                        Err(e) => eprintln!("Save error: {e}"),
                    }

                } else if let Ok(idx) = line.parse::<usize>() {
                    // Select preset by number (1-based)
                    let def = {
                        let presets = presets_menu.lock().unwrap();
                        if idx == 0 { None } else { presets.get(idx - 1).cloned() }
                    };
                    match def {
                        Some(d) => {
                            if let Some(mut new_effect) = build_preset(&d) {
                                new_effect.init(SAMPLE_RATE);
                                new_effect.inject_input_rms(Arc::clone(&input_rms_menu));
                                let name = new_effect.name().to_string();
                                *effect_menu.lock().unwrap() = new_effect;
                                println!("→ {} selected!", name);
                            } else {
                                println!("Failed to build preset");
                            }
                        }
                        None => println!("Number out of range"),
                    }
                } else {
                    println!("Invalid input (number / P / S <name> / R)");
                }
            }
        });

        // ── SDL2 display thread (launched only with --gui)────────────────
        if gui_mode {
            let snoop_buf_reader = Arc::clone(&snoop_buf);
            let effect_display   = Arc::clone(&effect_shared);
            let presets_display  = Arc::clone(&presets_shared);
            let effect_gui       = Arc::clone(&effect_shared);
            let input_rms_gui    = Arc::clone(&input_rms);

            thread::spawn(move || {
                let sdl_context     = sdl2::init().unwrap();
                let video_subsystem = sdl_context.video().unwrap();
                let ttf_context     = ttf::init().unwrap();

                let window = video_subsystem
                    .window("FunPedals", WIDTH, HEIGHT)
                    .fullscreen_desktop()
                    .build()
                    .unwrap();

                let mut canvas     = window.into_canvas().build().unwrap();
                let mut event_pump = sdl_context.event_pump().unwrap();

                // Fonts
                let font_sm = ttf_context.load_font(
                    "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", 20
                ).ok();
                let font_md = ttf_context.load_font(
                    "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", 24
                ).ok();
                let _font_lg = ttf_context.load_font(
                    "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", 30
                ).ok();

                // ── Layout constants ──────────────────────────────────
                // Left area (waveform + spectrum)
                let left_w    = 580u32;
                let right_w   = WIDTH - left_w;           // 220px
                let title_h   = 30u32;                    // "FunPedals x.x" title bar
                let label_h   = 22u32;                    // Frequency scale label area
                let wave_h    = (HEIGHT - title_h - label_h) / 2; // Waveform area height
                let spec_h    = HEIGHT - title_h - wave_h - label_h; // Spectrum area height

                // Right column width only (scroll calc done inline in event handler)

                // ── Screen mode ──────────────────────────────────────
                // false = main screen, true = PARAM screen
                let mut param_mode = false;
                let mut param_tab  = 0usize;  // Currently selected tab (per effect)
                // Slider drag state
                let mut drag_param: Option<usize> = None; // Parameter index being dragged
                // Snapshot for CANCEL: parameter values when entering PARAM screen
                let mut param_snapshot: Vec<f32> = Vec::new();

                // Selected preset index (highlight first on startup)
                let mut selected: Option<usize> = Some(0);
                // Preset page (0-based, 10 presets per page)
                let mut preset_page: usize = 0;

                let mut peaks      = vec![0.0f32; NUM_BARS];
                let peak_decay     = 0.003f32;

                // Text rendering helper (written inline, not as closure)
                'display: loop {
                    canvas.set_draw_color(Color::RGB(15, 15, 15));
                    canvas.clear();

                    if param_mode {
                        // ══════════════════════════════════════════════
                        // PARAM screen
                        // ══════════════════════════════════════════════
                        let units = effect_display.lock().unwrap().params_by_unit();

                        // Tab bar height / slider area / button bar
                        let tab_h  = 44u32;
                        let bot_h  = 44u32;
                        let _area_h = HEIGHT - tab_h - bot_h;
                        let slider_h = 56u32; // Height per slider

                        // ── Tab bar ──────────────────────────────────
                        canvas.set_draw_color(Color::RGB(25, 25, 25));
                        canvas.fill_rect(Rect::new(0, 0, WIDTH, tab_h)).unwrap();
                        canvas.set_draw_color(Color::RGB(70, 70, 70));
                        canvas.draw_line(Point::new(0, tab_h as i32), Point::new(WIDTH as i32, tab_h as i32)).unwrap();

                        // Tabs (effect names)
                        let n_tabs  = units.len();
                        let tab_w   = if n_tabs > 0 { (WIDTH - 80) / n_tabs as u32 } else { 100 };
                        for (i, (ename, _)) in units.iter().enumerate() {
                            let tx   = i as i32 * tab_w as i32;
                            let active = param_tab == i;
                            canvas.set_draw_color(if active { Color::RGB(40, 80, 150) } else { Color::RGB(30, 30, 30) });
                            canvas.fill_rect(Rect::new(tx + 1, 1, tab_w - 2, tab_h - 1)).unwrap();
                            if active {
                                canvas.set_draw_color(Color::RGB(80, 150, 255));
                                canvas.fill_rect(Rect::new(tx + 1, tab_h as i32 - 3, tab_w - 2, 3)).unwrap();
                            }
                            if let Some(ref f) = font_sm {
                                let tc = canvas.texture_creator();
                                    let label = ename.as_str();
                                let col = if active { Color::RGB(255,255,255) } else { Color::RGB(150,150,150) };
                                if let Ok(surf) = f.render(label).blended(col) {
                                    let tw = surf.width().min(tab_w - 4);
                                    let tty = (tab_h as i32 - surf.height() as i32) / 2;
                                    if let Ok(tex) = tc.create_texture_from_surface(&surf) {
                                        canvas.copy(&tex, None, Some(Rect::new(tx + 4, tty, tw, surf.height()))).unwrap();
                                    }
                                }
                            }
                        }

                        // [CLOSE] removed → right end of tab bar is empty

                        // ── Slider area ──────────────────────────
                        if param_tab < units.len() {
                            let (_, ref params) = units[param_tab];
                            // Calculate sequential index of first param in this tab
                            let offset: usize = units[..param_tab].iter().map(|(_, p)| p.len()).sum();

                            for (i, pd) in params.iter().enumerate() {
                                let sy = tab_h as i32 + i as i32 * slider_h as i32;
                                if sy + slider_h as i32 > (HEIGHT - bot_h) as i32 { break; }

                                // Parameter name
                                if let Some(ref f) = font_sm {
                                    let tc = canvas.texture_creator();
                                    let label = format!("{}", pd.name);
                                    if let Ok(surf) = f.render(&label).blended(Color::RGB(160, 160, 160)) {
                                        if let Ok(tex) = tc.create_texture_from_surface(&surf) {
                                            canvas.copy(&tex, None, Some(Rect::new(12, sy + 4, surf.width(), surf.height()))).unwrap();
                                        }
                                    }
                                }

                                // Value text (right edge)
                                if let Some(ref f) = font_md {
                                    let tc = canvas.texture_creator();
                                    let val_str = match pd.unit {
                                        "dB" => format!("{:.1}dB", pd.value),
                                        "ms" => format!("{:.0}ms", pd.value),
                                        "Hz" => format!("{:.0}Hz", pd.value),
                                        _    => format!("{:.2}", pd.value),
                                    };
                                    if let Ok(surf) = f.render(&val_str).blended(Color::RGB(100, 220, 180)) {
                                        let vx = WIDTH as i32 - surf.width() as i32 - 10;
                                        if let Ok(tex) = tc.create_texture_from_surface(&surf) {
                                            canvas.copy(&tex, None, Some(Rect::new(vx, sy + 4, surf.width(), surf.height()))).unwrap();
                                        }
                                    }
                                }

                                // Slider track
                                let sl_x  = 12i32;
                                let sl_w  = WIDTH as i32 - 90;
                                let sl_y  = sy + 30;
                                let sl_h  = 8i32;
                                canvas.set_draw_color(Color::RGB(50, 50, 50));
                                canvas.fill_rect(Rect::new(sl_x, sl_y, sl_w as u32, sl_h as u32)).unwrap();

                                // Slider knob position
                                let ratio = if pd.max > pd.min {
                                    ((pd.value - pd.min) / (pd.max - pd.min)).clamp(0.0, 1.0)
                                } else { 0.0 };
                                let knob_x = sl_x + (ratio * sl_w as f32) as i32;
                                // Active portion (left side)
                                canvas.set_draw_color(Color::RGB(40, 100, 200));
                                if knob_x > sl_x {
                                    canvas.fill_rect(Rect::new(sl_x, sl_y, (knob_x - sl_x) as u32, sl_h as u32)).unwrap();
                                }
                                // Knob
                                canvas.set_draw_color(Color::RGB(100, 180, 255));
                                canvas.fill_rect(Rect::new(knob_x - 6, sl_y - 4, 12, (sl_h + 8) as u32)).unwrap();

                                // Separator line
                                canvas.set_draw_color(Color::RGB(35, 35, 35));
                                canvas.draw_line(
                                    Point::new(0, sy + slider_h as i32 - 1),
                                    Point::new(WIDTH as i32, sy + slider_h as i32 - 1)
                                ).unwrap();

                                // Remember slider index for drag
                                let _ = (offset + i, sl_x, sl_w, sl_y, sl_h);
                            }
                        }

                        // ── [SAVE] [CANCEL] buttons ────────────────────
                        let save_y = (HEIGHT - bot_h) as i32;
                        let half_w = WIDTH / 2;
                        canvas.set_draw_color(Color::RGB(70, 70, 30));
                        canvas.draw_line(Point::new(0, save_y), Point::new(WIDTH as i32, save_y)).unwrap();
                        // [SAVE] left half
                        canvas.set_draw_color(Color::RGB(50, 60, 20));
                        canvas.fill_rect(Rect::new(0, save_y + 1, half_w, bot_h - 1)).unwrap();
                        // [CANCEL] right half
                        canvas.set_draw_color(Color::RGB(60, 25, 25));
                        canvas.fill_rect(Rect::new(half_w as i32, save_y + 1, WIDTH - half_w, bot_h - 1)).unwrap();
                        // Center separator line
                        canvas.set_draw_color(Color::RGB(80, 80, 80));
                        canvas.draw_line(Point::new(half_w as i32, save_y), Point::new(half_w as i32, HEIGHT as i32)).unwrap();
                        if let Some(ref f) = font_md {
                            let tc = canvas.texture_creator();
                            for (label, col, lx) in [
                                ("SAVE",   Color::RGB(200, 200, 100), 0i32),
                                ("CANCEL", Color::RGB(220, 140, 140), half_w as i32),
                            ] {
                                if let Ok(surf) = f.render(label).blended(col) {
                                    let tx = lx + (half_w as i32 - surf.width() as i32) / 2;
                                    let ty = save_y + (bot_h as i32 - surf.height() as i32) / 2;
                                    if let Ok(tex) = tc.create_texture_from_surface(&surf) {
                                        canvas.copy(&tex, None, Some(Rect::new(tx, ty, surf.width(), surf.height()))).unwrap();
                                    }
                                }
                            }
                        }

                        // ── PARAM screen events ─────────────────────────
                        for event in event_pump.poll_iter() {
                            match event {
                                Event::Quit { .. }
                                | Event::KeyDown { keycode: Some(Keycode::Escape), .. } => {
                                    break 'display;
                                }

                                Event::MouseButtonDown { x, y, .. } => {
                                    let (tx, ty) = (x, y);
                                    // Tab switch (inside tab bar)
                                    if ty < tab_h as i32 {
                                        let idx = (tx / tab_w as i32) as usize;
                                        if idx < units.len() {
                                            param_tab = idx;
                                            drag_param = None;
                                        }
                                        continue;
                                    }
                                    // [SAVE] / [CANCEL]
                                    let save_y = (HEIGHT - bot_h) as i32;
                                    if ty >= save_y {
                                        let half_w = WIDTH / 2;
                                        if tx < half_w as i32 {
                                            // SAVE: write to TOML and return to main screen
                                            let chain_name = effect_display.lock().unwrap().name().to_string();
                                            let params_flat = effect_display.lock().unwrap().params();
                                            let chain = presets_display.lock().unwrap()
                                                .iter().find(|p| p.name == chain_name)
                                                .map(|p| p.chain.clone())
                                                .unwrap_or_default();
                                            // Distribute parameters in chain key order
                                            let mut params_map = std::collections::HashMap::new();
                                            let mut offset = 0usize;
                                            for kind in &chain {
                                                if let Some(e) = make_single_effect(kind) {
                                                    let pdefs = e.params();
                                                    let mut pm = std::collections::HashMap::new();
                                                    for (i, pd) in pdefs.iter().enumerate() {
                                                        if offset + i < params_flat.len() {
                                                            pm.insert(pd.name.to_string(), params_flat[offset + i].value);
                                                        }
                                                    }
                                                    if !pm.is_empty() { params_map.insert(kind.clone(), pm); }
                                                    offset += pdefs.len();
                                                }
                                            }
                                            let def = PresetDef { name: chain_name.clone(), chain, params: params_map };
                                            match save_preset(&def) {
                                                Ok(_) => {
                                                    println!("Saved: {}", chain_name);
                                                    *presets_display.lock().unwrap() = load_presets();
                                                }
                                                Err(e) => eprintln!("Save error: {e}"),
                                            }
                                        } else {
                                            // CANCEL: restore snapshot and return to main screen
                                            let mut effect = effect_gui.lock().unwrap();
                                            for (i, &v) in param_snapshot.iter().enumerate() {
                                                effect.set_param(i, v);
                                            }
                                        }
                                        param_mode = false;
                                        drag_param = None;
                                        continue;
                                    }
                                    // Slider operation start
                                    if param_tab < units.len() {
                                        let (_, ref ps) = units[param_tab];
                                        let offset: usize = units[..param_tab].iter().map(|(_, p)| p.len()).sum();
                                        let sl_x = 12i32;
                                        let sl_w = WIDTH as i32 - 90;
                                        for (i, pd) in ps.iter().enumerate() {
                                            let sl_y = tab_h as i32 + i as i32 * slider_h as i32 + 30;
                                            if (ty - sl_y).abs() < 16 {
                                                drag_param = Some(offset + i);
                                                let ratio = ((tx - sl_x) as f32 / sl_w as f32).clamp(0.0, 1.0);
                                                let val = pd.min + ratio * (pd.max - pd.min);
                                                effect_gui.lock().unwrap().set_param(offset + i, val);
                                                break;
                                            }
                                        }
                                    }
                                }

                                Event::MouseMotion { x, y, mousestate, .. }
                                    if mousestate.left() =>
                                {
                                    let (tx, ty) = (x, y);
                                    if param_tab < units.len() {
                                        let (_, ref ps) = units[param_tab];
                                        let offset: usize = units[..param_tab].iter().map(|(_, p)| p.len()).sum();
                                        let sl_x = 12i32;
                                        let sl_w = WIDTH as i32 - 90;
                                        for (i, pd) in ps.iter().enumerate() {
                                            let sl_y = tab_h as i32 + i as i32 * slider_h as i32 + 30;
                                            if drag_param == Some(offset + i) || (ty - sl_y).abs() < 16 {
                                                drag_param = Some(offset + i);
                                                let ratio = ((tx - sl_x) as f32 / sl_w as f32).clamp(0.0, 1.0);
                                                let val = pd.min + ratio * (pd.max - pd.min);
                                                effect_gui.lock().unwrap().set_param(offset + i, val);
                                                break;
                                            }
                                        }
                                    }
                                }

                                Event::FingerDown { x, y, .. } | Event::FingerMotion { x, y, .. } => {
                                    let tx = (x * WIDTH as f32) as i32;
                                    let ty = (y * HEIGHT as f32) as i32;
                                    // Tab switch
                                    if ty < tab_h as i32 {
                                        let idx = (tx / tab_w as i32) as usize;
                                        if idx < units.len() { param_tab = idx; }
                                        continue;
                                    }
                                    // [SAVE] / [CANCEL]
                                    let save_y = (HEIGHT - bot_h) as i32;
                                    if ty >= save_y {
                                        let half_w = WIDTH / 2;
                                        if tx < half_w as i32 {
                                            // SAVE
                                            let chain_name = effect_display.lock().unwrap().name().to_string();
                                            let params_flat = effect_display.lock().unwrap().params();
                                            let chain = presets_display.lock().unwrap()
                                                .iter().find(|p| p.name == chain_name)
                                                .map(|p| p.chain.clone())
                                                .unwrap_or_default();
                                            let mut params_map = std::collections::HashMap::new();
                                            let mut offset = 0usize;
                                            for kind in &chain {
                                                if let Some(e) = make_single_effect(kind) {
                                                    let pdefs = e.params();
                                                    let mut pm = std::collections::HashMap::new();
                                                    for (i, pd) in pdefs.iter().enumerate() {
                                                        if offset + i < params_flat.len() {
                                                            pm.insert(pd.name.to_string(), params_flat[offset + i].value);
                                                        }
                                                    }
                                                    if !pm.is_empty() { params_map.insert(kind.clone(), pm); }
                                                    offset += pdefs.len();
                                                }
                                            }
                                            let def = PresetDef { name: chain_name.clone(), chain, params: params_map };
                                            match save_preset(&def) {
                                                Ok(_) => {
                                                    println!("Saved: {}", chain_name);
                                                    *presets_display.lock().unwrap() = load_presets();
                                                }
                                                Err(e) => eprintln!("Save error: {e}"),
                                            }
                                        } else {
                                            // CANCEL: restore snapshot
                                            let mut effect = effect_gui.lock().unwrap();
                                            for (i, &v) in param_snapshot.iter().enumerate() {
                                                effect.set_param(i, v);
                                            }
                                        }
                                        param_mode = false;
                                        drag_param = None;
                                        continue;
                                    }
                                    if param_tab < units.len() {
                                        let (_, ref ps) = units[param_tab];
                                        let offset: usize = units[..param_tab].iter().map(|(_, p)| p.len()).sum();
                                        let sl_x = 12i32;
                                        let sl_w = WIDTH as i32 - 90;
                                        for (i, pd) in ps.iter().enumerate() {
                                            let sl_y = tab_h as i32 + i as i32 * slider_h as i32 + 30;
                                            if (ty - sl_y).abs() < 20 {
                                                let ratio = ((tx - sl_x) as f32 / sl_w as f32).clamp(0.0, 1.0);
                                                let val = pd.min + ratio * (pd.max - pd.min);
                                                effect_gui.lock().unwrap().set_param(offset + i, val);
                                                break;
                                            }
                                        }
                                    }
                                }

                                _ => {}
                            }
                        }

                    } else {
                        // ══════════════════════════════════════════════
                        // Main screen
                        // ══════════════════════════════════════════════
                        let samples = snoop_buf_reader.lock().unwrap().clone();

                    // ── Left area border ─────────────────────────────────
                    canvas.set_draw_color(Color::RGB(80, 80, 80));
                    canvas.draw_rect(Rect::new(0, 0, left_w, HEIGHT)).unwrap();

                    // ── Title bar ──────────────────────────────────
                    canvas.set_draw_color(Color::RGB(30, 30, 30));
                    canvas.fill_rect(Rect::new(0, 0, left_w, title_h)).unwrap();
                    // [≡] Hamburger icon (left edge)
                    let ham_w = 40i32;
                    canvas.set_draw_color(Color::RGB(55, 55, 75));
                    canvas.fill_rect(Rect::new(2, 2, ham_w as u32, title_h - 4)).unwrap();
                    if let Some(ref f) = font_md {
                        let tc = canvas.texture_creator();
                        if let Ok(surf) = f.render("≡").blended(Color::RGB(160, 160, 220)) {
                            let tx = 2 + (ham_w - surf.width() as i32) / 2;
                            let ty = (title_h as i32 - surf.height() as i32) / 2;
                            if let Ok(tex) = tc.create_texture_from_surface(&surf) {
                                canvas.copy(&tex, None, Some(Rect::new(tx, ty, surf.width(), surf.height()))).unwrap();
                            }
                        }
                    }
                    // Title text (right of hamburger icon)
                    if let Some(ref f) = font_md {
                        let tc = canvas.texture_creator();
                        if let Ok(surf) = f.render(&format!("FunPedals {}", env!("CARGO_PKG_VERSION"))).blended(Color::RGB(180, 180, 180)) {
                            if let Ok(tex) = tc.create_texture_from_surface(&surf) {
                                canvas.copy(&tex, None, Some(Rect::new(
                                    ham_w + 8,
                                    (title_h as i32 - surf.height() as i32) / 2,
                                    surf.width(), surf.height()
                                ))).unwrap();
                            }
                        }
                    }
                    // Title bar bottom separator line
                    canvas.set_draw_color(Color::RGB(80, 80, 80));
                    canvas.draw_line(
                        Point::new(0, title_h as i32),
                        Point::new(left_w as i32, title_h as i32)
                    ).unwrap();

                    // ── Waveform area ────────────────────────────────────
                    let wave_top = title_h as i32;
                    let wave_mid = wave_top + wave_h as i32 / 2;

                    // Zero line
                    canvas.set_draw_color(Color::RGB(50, 50, 50));
                    canvas.draw_line(
                        Point::new(0, wave_mid),
                        Point::new(left_w as i32, wave_mid)
                    ).unwrap();

                    // Waveform rendering
                    canvas.set_draw_color(Color::RGB(0, 200, 80));
                    let display_size = SNOOP_SIZE / 4;
                    let wpts: Vec<Point> = (0..left_w).map(|x| {
                        let idx = (x as usize * display_size / left_w as usize).min(display_size - 1);
                        let y = wave_mid + (-(samples[idx] * wave_h as f32 / 2.5)) as i32;
                        Point::new(x as i32, y.clamp(wave_top, wave_top + wave_h as i32 - 1))
                    }).collect();
                    canvas.draw_lines(wpts.as_slice()).unwrap();

                    // Waveform/spectrum separator line
                    let spec_top = wave_top + wave_h as i32;
                    canvas.set_draw_color(Color::RGB(80, 80, 80));
                    canvas.draw_line(
                        Point::new(0, spec_top),
                        Point::new(left_w as i32, spec_top)
                    ).unwrap();

                    // ── Spectrum area ─────────────────────────────
                    // FFT
                    let mut fft_buf = vec![0.0f32; FFT_SIZE];
                    for i in 0..FFT_SIZE {
                        fft_buf[i] = samples[i.min(SNOOP_SIZE - 1)];
                    }
                    let mean = fft_buf.iter().sum::<f32>() / FFT_SIZE as f32;
                    for s in fft_buf.iter_mut() { *s -= mean; }
                    for i in 0..FFT_SIZE {
                        let w = 0.54 - 0.46 * (2.0 * std::f32::consts::PI * i as f32 / FFT_SIZE as f32).cos();
                        fft_buf[i] *= w;
                    }
                    let spectrum = fundsp::fft::real_fft(&mut fft_buf);

                    for b in 0..NUM_BARS {
                        let min_freq  = 20.0f32;
                        let max_freq  = 20000.0f32;
                        let freq_hz   = min_freq * (max_freq / min_freq).powf(b as f32 / NUM_BARS as f32);
                        let freq_next = min_freq * (max_freq / min_freq).powf((b + 1) as f32 / NUM_BARS as f32);
                        let bin_size  = 48000.0 / FFT_SIZE as f32;
                        let fs = (freq_hz   / bin_size) as usize;
                        let fe = ((freq_next / bin_size) as usize).min(spectrum.len() - 1);
                        if fe <= fs { continue; }

                        let power: f32 = spectrum[fs..fe].iter()
                            .map(|c| (c.re * c.re + c.im * c.im).sqrt())
                            .sum::<f32>() / (fe - fs) as f32 / FFT_SIZE as f32;
                        let db = (power + 1e-10).log10() * 20.0;
                        let norm = ((db + 090.0) / 100.0).clamp(0.0, 1.0); // dB display range adjustment

                        if norm > peaks[b] { peaks[b] = norm; }
                        else { peaks[b] = (peaks[b] - peak_decay).max(0.0); }

                        let ratio      = (freq_hz   / min_freq).ln() / (max_freq / min_freq).ln();
                        let ratio_next = (freq_next / min_freq).ln() / (max_freq / min_freq).ln();
                        let bx = (ratio      * left_w as f32) as i32;
                        let bw = ((ratio_next - ratio) * left_w as f32) as u32;
                        if bw == 0 { continue; }

                        let bar_h  = (norm * spec_h as f32) as u32;
                        let peak_y = spec_top + spec_h as i32 - (peaks[b] * spec_h as f32) as i32;

                        canvas.set_draw_color(Color::RGB(
                            (norm * 255.0) as u8,
                            ((1.0 - norm) * 128.0) as u8, 30
                        ));
                        if bar_h > 0 {
                            canvas.fill_rect(Rect::new(
                                bx + 1,
                                spec_top + spec_h as i32 - bar_h as i32,
                                (bw - 1).max(1), bar_h,
                            )).unwrap();
                        }
                        canvas.set_draw_color(Color::RGB(255, 255, 200));
                        canvas.fill_rect(Rect::new(bx + 1, peak_y, (bw - 1).max(1), 2)).unwrap();
                    }

                    // Frequency scale labels (displayed below spectrum)
                    if let Some(ref f) = font_sm {
                        let tc = canvas.texture_creator();
                        let labels = [
                            (20.0f32,"20"), (50.0,"50"), (100.0,"100"),
                            (200.0,"200"), (500.0,"500"), (1000.0,"1k"),
                            (2000.0,"2k"), (5000.0,"5k"), (10000.0,"10k"),
                        ];
                        let (min_f, max_f) = (20.0f32, 20000.0f32);
                        let label_top = spec_top + spec_h as i32; // Label area top edge
                        for (freq, label) in labels.iter() {
                            let ratio = (freq / min_f).ln() / (max_f / min_f).ln();
                            let lx = (ratio * left_w as f32) as i32;
                            // Vertical line inside spectrum (faint)
                            canvas.set_draw_color(Color::RGB(50, 50, 50));
                            canvas.draw_line(
                                Point::new(lx, spec_top),
                                Point::new(lx, spec_top + spec_h as i32)
                            ).unwrap();
                            // Render text in label area
                            if let Ok(surf) = f.render(label).blended(Color::RGB(130, 130, 130)) {
                                if let Ok(tex) = tc.create_texture_from_surface(&surf) {
                                    canvas.copy(&tex, None, Some(Rect::new(
                                        lx + 2,
                                        label_top + 0,
                                        surf.width().min(38), surf.height()
                                    ))).unwrap();
                                }
                            }
                        }
                    }

                    // ── Right column ──────────────────────────────────────
                    let rx = left_w as i32;

                    canvas.set_draw_color(Color::RGB(20, 20, 20));
                    canvas.fill_rect(Rect::new(rx, 0, right_w, HEIGHT)).unwrap();
                    canvas.set_draw_color(Color::RGB(80, 80, 80));
                    canvas.draw_line(Point::new(rx, 0), Point::new(rx, HEIGHT as i32)).unwrap();

                    let presets = presets_display.lock().unwrap();
                    let n_presets = presets.len();
                    let n_pages = ((n_presets + 9) / 10).max(1); // Round up, minimum 1
                    if preset_page >= n_pages { preset_page = n_pages - 1; }

                    // ── Page tab ────────────────────────────────────
                    let tab_bar_h = 30u32;
                    let tab_w = right_w / n_pages as u32;
                    for p in 0..n_pages {
                        let tx = rx + p as i32 * tab_w as i32;
                        let is_active = preset_page == p;
                        canvas.set_draw_color(if is_active { Color::RGB(40, 80, 140) } else { Color::RGB(25, 25, 35) });
                        canvas.fill_rect(Rect::new(tx + 1, 0, tab_w - 1, tab_bar_h)).unwrap();
                        // Active tab bottom accent
                        if is_active {
                            canvas.set_draw_color(Color::RGB(80, 150, 255));
                            canvas.fill_rect(Rect::new(tx + 1, tab_bar_h as i32 - 3, tab_w - 1, 3)).unwrap();
                        }
                        // Tab separator line
                        canvas.set_draw_color(Color::RGB(60, 60, 60));
                        canvas.draw_line(Point::new(tx, 0), Point::new(tx, tab_bar_h as i32)).unwrap();
                        // Tab label (e.g. "1-10", "11-20")
                        if let Some(ref f) = font_sm {
                            let tc = canvas.texture_creator();
                            let start = p * 10 + 1;
                            let end   = ((p + 1) * 10).min(n_presets);
                            let label = format!("{}-{}", start, end);
                            let col = if is_active { Color::RGB(220, 220, 255) } else { Color::RGB(120, 120, 140) };
                            if let Ok(surf) = f.render(&label).blended(col) {
                                let lx = tx + (tab_w as i32 - surf.width() as i32) / 2;
                                let ly = (tab_bar_h as i32 - surf.height() as i32) / 2;
                                if let Ok(tex) = tc.create_texture_from_surface(&surf) {
                                    canvas.copy(&tex, None, Some(Rect::new(lx, ly, surf.width(), surf.height()))).unwrap();
                                }
                            }
                        }
                    }
                    // Tab bar underline
                    canvas.set_draw_color(Color::RGB(60, 60, 60));
                    canvas.draw_line(
                        Point::new(rx, tab_bar_h as i32),
                        Point::new(rx + right_w as i32, tab_bar_h as i32)
                    ).unwrap();

                    // ── Preset buttons (fixed 10) ───────────────────
                    let list_top  = tab_bar_h;
                    let list_h    = HEIGHT - BTN_H - tab_bar_h;
                    let pbh       = list_h / 10;

                    for i in 0..10 {
                        let preset_idx = preset_page * 10 + i;
                        let by = list_top as i32 + i as i32 * pbh as i32;
                        let is_selected = selected == Some(preset_idx);
                        let has_preset  = preset_idx < n_presets;
                        let bg = if is_selected { Color::RGB(40, 80, 140) }
                                 else if has_preset { Color::RGB(28, 28, 28) }
                                 else { Color::RGB(18, 18, 18) };
                        canvas.set_draw_color(bg);
                        canvas.fill_rect(Rect::new(rx + 1, by, right_w - 1, pbh)).unwrap();
                        // Separator line
                        canvas.set_draw_color(Color::RGB(60, 60, 60));
                        canvas.draw_line(
                            Point::new(rx, by + pbh as i32 - 1),
                            Point::new(rx + right_w as i32 - 1, by + pbh as i32 - 1)
                        ).unwrap();
                        // Preset name
                        if has_preset {
                            if let Some(ref f) = font_sm {
                                let tc = canvas.texture_creator();
                                let label = presets[preset_idx].name.clone();
                                let col = if is_selected { Color::RGB(255,255,255) } else { Color::RGB(180,180,180) };
                                if let Ok(surf) = f.render(&label).blended(col) {
                                    let tw = surf.width().min(right_w - 12);
                                    let th = surf.height();
                                    let ty = by + (pbh as i32 - th as i32) / 2;
                                    if let Ok(tex) = tc.create_texture_from_surface(&surf) {
                                        canvas.copy(&tex, None, Some(Rect::new(rx + 8, ty, tw, th))).unwrap();
                                    }
                                }
                            }
                        }
                        // Selected accent bar
                        if is_selected {
                            canvas.set_draw_color(Color::RGB(80, 160, 255));
                            canvas.fill_rect(Rect::new(rx + 1, by, 3, pbh)).unwrap();
                        }
                    }
                    drop(presets);

                    // [PARAM] [RELOAD]
                    let btn_y = (HEIGHT - BTN_H) as i32;
                    let half  = right_w / 2;
                    canvas.set_draw_color(Color::RGB(50, 50, 70));
                    canvas.fill_rect(Rect::new(rx + 1, btn_y, half - 1, BTN_H)).unwrap();
                    canvas.set_draw_color(Color::RGB(50, 70, 50));
                    canvas.fill_rect(Rect::new(rx + half as i32, btn_y, right_w - half, BTN_H)).unwrap();
                    canvas.set_draw_color(Color::RGB(80, 80, 80));
                    canvas.draw_line(Point::new(rx, btn_y), Point::new(rx + right_w as i32, btn_y)).unwrap();
                    canvas.draw_line(Point::new(rx + half as i32, btn_y), Point::new(rx + half as i32, HEIGHT as i32)).unwrap();
                    if let Some(ref f) = font_sm {
                        let tc = canvas.texture_creator();
                        for (label, lx) in [("PARAM", rx + 4), ("RELOAD", rx + half as i32 + 4)] {
                            if let Ok(surf) = f.render(label).blended(Color::RGB(160, 160, 200)) {
                                let ty = btn_y + (BTN_H as i32 - surf.height() as i32) / 2;
                                if let Ok(tex) = tc.create_texture_from_surface(&surf) {
                                    canvas.copy(&tex, None, Some(Rect::new(lx, ty, surf.width(), surf.height()))).unwrap();
                                }
                            }
                        }
                    }

                    // ── Main screen events ────────────────────────────
                    for event in event_pump.poll_iter() {
                        match event {
                            Event::Quit { .. }
                            | Event::KeyDown { keycode: Some(Keycode::Escape), .. } => {
                                break 'display;
                            }
                            Event::MouseButtonDown { x, y, .. } => {
                                let (tx, ty) = (x, y);
                                // [≡] Hamburger icon (left edge)
                                if ty < title_h as i32 && tx >= 2 && tx < 42 {
                                    canvas.window_mut().minimize();
                                    continue;
                                }
                                if tx >= rx {
                                    let n = presets_display.lock().unwrap().len();
                                    let n_pages2 = ((n + 9) / 10).max(1);
                                    let tab_w2 = right_w as i32 / n_pages2 as i32;
                                    let tab_bar_h2 = 30i32;
                                    let list_top2  = tab_bar_h2;
                                    let list_h2    = HEIGHT as i32 - BTN_H as i32 - tab_bar_h2;
                                    let pbh2       = list_h2 / 10;
                                    let btn_y2     = (HEIGHT - BTN_H) as i32;

                                    if ty < tab_bar_h2 {
                                        // Page tab
                                        let p = ((tx - rx) / tab_w2) as usize;
                                        if p < n_pages2 { preset_page = p; }
                                    } else if ty >= list_top2 && ty < list_top2 + list_h2 {
                                        // Preset selection
                                        let i = ((ty - list_top2) / pbh2) as usize;
                                        let preset_idx = preset_page * 10 + i;
                                        if preset_idx < n {
                                            selected = Some(preset_idx);
                                            let def = presets_display.lock().unwrap().get(preset_idx).cloned();
                                            if let Some(d) = def {
                                                if let Some(mut e) = build_preset(&d) {
                                                    e.init(SAMPLE_RATE);
                                                    e.inject_input_rms(Arc::clone(&input_rms_gui));
                                                    *effect_gui.lock().unwrap() = e;
                                                }
                                            }
                                        }
                                    } else if ty >= btn_y2 {
                                        let half = right_w as i32 / 2;
                                        if tx < rx + half {
                                            param_snapshot = effect_gui.lock().unwrap().params()
                                                .iter().map(|p| p.value).collect();
                                            param_mode = true;
                                            param_tab  = 0;
                                        } else {
                                            let new_presets = load_presets();
                                            println!("Reload: {} presets", new_presets.len());
                                            *presets_display.lock().unwrap() = new_presets;
                                        }
                                    }
                                }
                            }
                            Event::FingerDown { x, y, .. } => {
                                let tx = (x * WIDTH as f32) as i32;
                                let ty = (y * HEIGHT as f32) as i32;
                                // [≡] Hamburger icon (left edge)
                                if ty < title_h as i32 && tx >= 2 && tx < 42 {
                                    canvas.window_mut().minimize();
                                    continue;
                                }
                                if tx >= rx {
                                    let n = presets_display.lock().unwrap().len();
                                    let n_pages2 = ((n + 9) / 10).max(1);
                                    let tab_w2 = right_w as i32 / n_pages2 as i32;
                                    let tab_bar_h2 = 30i32;
                                    let list_top2  = tab_bar_h2;
                                    let list_h2    = HEIGHT as i32 - BTN_H as i32 - tab_bar_h2;
                                    let pbh2       = list_h2 / 10;
                                    let btn_y2     = (HEIGHT - BTN_H) as i32;

                                    if ty < tab_bar_h2 {
                                        let p = ((tx - rx) / tab_w2) as usize;
                                        if p < n_pages2 { preset_page = p; }
                                    } else if ty >= list_top2 && ty < list_top2 + list_h2 {
                                        let i = ((ty - list_top2) / pbh2) as usize;
                                        let preset_idx = preset_page * 10 + i;
                                        if preset_idx < n {
                                            selected = Some(preset_idx);
                                            let def = presets_display.lock().unwrap().get(preset_idx).cloned();
                                            if let Some(d) = def {
                                                if let Some(mut e) = build_preset(&d) {
                                                    e.init(SAMPLE_RATE);
                                                    e.inject_input_rms(Arc::clone(&input_rms_gui));
                                                    *effect_gui.lock().unwrap() = e;
                                                }
                                            }
                                        }
                                    } else if ty >= btn_y2 {
                                        let half = right_w as i32 / 2;
                                        if tx < rx + half {
                                            param_snapshot = effect_gui.lock().unwrap().params()
                                                .iter().map(|p| p.value).collect();
                                            param_mode = true;
                                            param_tab  = 0;
                                        } else {
                                            let new_presets = load_presets();
                                            println!("Reload: {} presets", new_presets.len());
                                            *presets_display.lock().unwrap() = new_presets;
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    } // end else (main screen)

                    canvas.present();
                    std::thread::sleep(std::time::Duration::from_millis(33));
                }
            });
        }

        // ── Main audio processing loop ────────────────────────────
        let mut snoop_pos = 0usize;
        loop {
            let mut out_buf  = vec![0i16; BLOCK_SIZE * 2];
            let mut in_buf   = vec![0.0f32; BLOCK_SIZE]; // Input signal (L channel)
            while cons.occupied_len() < BLOCK_SIZE * 4 {
                std::thread::sleep(std::time::Duration::from_micros(10));
            }
            {
                let mut effect = effect_shared.lock().unwrap();
                for j in 0..BLOCK_SIZE {
                    let l = cons.try_pop().unwrap_or(0) as f32 / 32767.0;
                    let r = cons.try_pop().unwrap_or(0) as f32 / 32767.0;
                    in_buf[j] = l; // Record input before effect processing
                    let (out_l, out_r) = effect.process_sample(l, r);
                    out_buf[j * 2]     = (out_l * 32767.0).clamp(-32767.0, 32767.0) as i16;
                    out_buf[j * 2 + 1] = (out_r * 32767.0).clamp(-32767.0, 32767.0) as i16;
                }
            }
            // Update input RMS (used by NoiseGate)
            {
                let rms = (in_buf.iter().map(|s| s * s).sum::<f32>() / BLOCK_SIZE as f32).sqrt();
                *input_rms_main.lock().unwrap() = rms;
            }
            // Write output signal to snoop_buf (for waveform/spectrum display)
            {
                let mut buf = snoop_buf_writer.lock().unwrap();
                for j in 0..BLOCK_SIZE {
                    buf[snoop_pos] = out_buf[j * 2] as f32 / 32767.0;
                    snoop_pos = (snoop_pos + 1) % SNOOP_SIZE;
                }
            }
            match io_play.writei(&out_buf) {
                Ok(_) => {},
                Err(e) => {
                    eprintln!("Output error: {}", e);
                    if playback.recover(e.errno() as std::ffi::c_int, true).is_ok() {
                        playback.prepare().ok();
                        playback.start().ok();
                    } else {
                        eprintln!("Reopening output device...");
                        if let Some(new_pb) = open_playback() {
                            drop(io_play);
                            playback = new_pb;
                            io_play  = playback.io_i16().unwrap();
                        }
                    }
                }
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::common::*;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::io::Write;

    pub fn run() {
        let host   = cpal::default_host();
        let device = host.default_output_device().unwrap();
        let config = device.default_output_config().unwrap();
        let sample_rate = config.sample_rate().0 as f64;

        let mut initial = make_effect("b").unwrap();
        initial.init(sample_rate);
        let effect_shared: Arc<Mutex<Box<dyn Effect>>> = Arc::new(Mutex::new(initial));

        init_presets_file();
        let presets_shared: Arc<Mutex<Vec<PresetDef>>> =
            Arc::new(Mutex::new(load_presets()));

        let effect_menu  = Arc::clone(&effect_shared);
        let presets_menu = Arc::clone(&presets_shared);
        thread::spawn(move || {
            loop {
                {
                    let presets = presets_menu.lock().unwrap();
                    println!("\n=== Preset selection ===");
                    for (i, p) in presets.iter().enumerate() {
                        println!("  {:2}: {}", i + 1, p.name);
                    }
                }
                println!("  P: params  S <n>: save  R: reload");
                print!("select > ");
                std::io::stdout().flush().unwrap();

                let mut input = String::new();
                std::io::stdin().read_line(&mut input).unwrap();
                let line = input.trim().to_string();

                if line == "P" {
                    let effect = effect_menu.lock().unwrap();
                    let params = effect.params();
                    if params.is_empty() {
                        println!("(no parameters)");
                    } else {
                        println!("--- {} parameters ---", effect.name());
                        for (i, p) in params.iter().enumerate() {
                            println!("  [{i}] {}: {:.3}  (min={:.3}, max={:.3})",
                                p.name, p.value, p.min, p.max);
                        }
                    }
                } else if line == "R" {
                    let new_presets = load_presets();
                    println!("{} presets loaded", new_presets.len());
                    *presets_menu.lock().unwrap() = new_presets;
                } else if let Ok(idx) = line.parse::<usize>() {
                    let def = {
                        let presets = presets_menu.lock().unwrap();
                        if idx == 0 { None } else { presets.get(idx - 1).cloned() }
                    };
                    match def {
                        Some(d) => {
                            if let Some(mut new_effect) = build_preset(&d) {
                                new_effect.init(sample_rate);
                                let name = new_effect.name().to_string();
                                *effect_menu.lock().unwrap() = new_effect;
                                println!("→ {} selected!", name);
                            }
                        }
                        None => println!("Number out of range"),
                    }
                } else {
                    println!("Invalid input");
                }
            }
        });

        let stream = device.build_output_stream(
            &config.into(),
            move |data: &mut [f32], _| {
                let mut effect = effect_shared.lock().unwrap();
                for frame in data.chunks_mut(2) {
                    let (out_l, out_r) = effect.process_sample(frame[0], frame[1]);
                    frame[0] = out_l;
                    frame[1] = out_r;
                }
            },
            |err| eprintln!("Error: {}", err),
            None,
        ).unwrap();

        stream.play().unwrap();
        println!("Playing... Press Ctrl+C to stop");
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
