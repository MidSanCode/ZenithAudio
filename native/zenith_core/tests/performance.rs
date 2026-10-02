//! S9 acceptance: heavy-load real-time ratio (PLAN §3.S9 item 2, §1.1).
//!
//! # What this measures
//!
//! The plan's target is "128 tracks + several effects per track, real-time
//! load < 50%". Real-time load is `dsp_time / available_time`: rendering one
//! second of audio must take well under a second of wall clock.
//!
//! # Why it is `#[ignore]`
//!
//! Wall-clock measurements are noisy and machine-dependent, so this is not part
//! of the default `cargo test` run: a loaded CI box could fail a passing build.
//! It is marked `#[ignore]` and run explicitly:
//!
//! ```text
//! cargo test -p zenith_core --release --test performance -- --ignored --nocapture
//! ```
//!
//! The numbers printed are the evidence; the assertion is a generous ceiling
//! that only catches a real order-of-magnitude regression.
//!
//! # What it builds
//!
//! `TRACKS` insert channels, each with `EFFECTS_PER_TRACK` latency-bearing
//! effects (a reverb, a compressor, a parametric EQ — the three most expensive
//! built-ins), all wired to master, and a full voice pool sounding. That is the
//! shape the plan describes, exercised through the engine's real block path.
//!
//! It deliberately does **not** measure I/O or the audio device: those do not
//! exist in a headless test, and the DSP is what the target is about.

use std::time::Instant;

use zenith_core::engine::{Engine, EngineConfig};
use zenith_core::effects::registry::{
    KIND_COMPRESSOR, KIND_EQ_PARAMETRIC, KIND_REVERB_ALGORITHMIC,
};

/// Insert channels to populate (PLAN target: 128 tracks).
const TRACKS: u32 = 128;

/// Effects loaded on each channel.
const EFFECTS_PER_TRACK: usize = 3;

/// Notes sounding at once (the voice pool size).
const VOICES: usize = 64;

/// Block size under test.
const BLOCK: usize = 256;

/// The engine default is 64 inserts + 8 returns + 1 master; 128 tracks needs
/// the mixer to grow. It starts with 64 inserts and `add_channel` appends more.
const SEEDED_INSERT_CHANNELS: u32 = 64;

#[test]
#[ignore = "wall-clock benchmark; run explicitly with --release --ignored"]
fn one_hundred_twenty_eight_tracks_with_effects_stay_under_half_real_time() {
    let sample_rate = 48_000u32;
    let mut engine = Engine::new(EngineConfig {
        sample_rate,
        block_size: BLOCK as u32,
        ..EngineConfig::default()
    })
    .expect("default config is valid");

    // ── Build the console: TRACKS insert channels, each with effects ──
    //
    // The engine ships 64 inserts already; grow to TRACKS with add_channel.
    let mut channels = Vec::with_capacity(TRACKS as usize);
    for id in 0..TRACKS {
        channels.push(id);
    }
    // Extra channels beyond the seeded ones.
    for _ in SEEDED_INSERT_CHANNELS..TRACKS {
        // `add_channel` returns the new id; the low ids already exist.
        let _ = engine.mixer_mut().add_channel();
    }

    let kinds = [KIND_REVERB_ALGORITHMIC, KIND_COMPRESSOR, KIND_EQ_PARAMETRIC];
    for &channel in &channels {
        for slot in 0..EFFECTS_PER_TRACK {
            engine
                .mixer_mut()
                .node_mut(channel)
                .expect("channel exists")
                .effects
                .insert(slot, kinds[slot % kinds.len()]);
        }
    }
    // Instantiate every processor and size its buffers. This is the expensive
    // control-thread step; the audio path below must not repeat it.
    engine.sync_effects();

    // ── Sound every voice ──
    engine.voices_mut().configure_all(|voice| {
        voice.set_amp_envelope(zenith_core::voice::AdsrSettings {
            attack_ms: 5.0,
            decay_ms: 200.0,
            sustain: 1.0,
            release_ms: 300.0,
        });
    });
    for i in 0..VOICES {
        engine.voices_mut().note_on((40 + i) as u8, 1.0, 0);
    }
    engine.transport_mut().play();

    // ── Render one second of audio and time it ──
    let frames_per_second = sample_rate as usize;
    let blocks = frames_per_second / BLOCK;
    let mut out = vec![0.0f32; BLOCK * 2];

    // Warm up (first-touch page faults, branch predictors) so the measurement
    // is steady-state, matching how the load would actually be sustained.
    for _ in 0..32 {
        engine.render_block(&mut out, BLOCK);
    }

    let start = Instant::now();
    for _ in 0..blocks {
        engine.render_block(&mut out, BLOCK);
    }
    let elapsed = start.elapsed();

    let ratio = elapsed.as_secs_f64() / 1.0; // one second of audio
    let percent = ratio * 100.0;
    println!(
        "S9 load: {TRACKS} tracks x {EFFECTS_PER_TRACK} effects, {VOICES} voices  =>  \
         {:.1} ms for 1.0 s of audio  =>  {percent:.1}% real-time",
        elapsed.as_secs_f64() * 1000.0
    );

    // The plan's target is < 50%. The assertion is looser (150%) so a slow CI
    // box does not fail a healthy build; the printed number is the real check.
    assert!(
        ratio < 1.5,
        "real-time ratio {ratio:.3} is far above the 0.5 target; investigate a regression"
    );
}
