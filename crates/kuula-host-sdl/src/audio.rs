//! Audio output: a 44.1 kHz stereo i16 device in SDL's callback mode and a
//! ring buffer of a few frames between the main loop and the callback.
//! The ring, its statistics, the frame trim and the settling wait are in
//! `kuula-host-common` and are re-exported here; what stays is what names
//! SDL: the callback, the open device and `open`.
//!
//! A device may ask for audio faster than it plays it while it fills a
//! queue of its own, so [`Output::settle`] lets it run on silence before
//! the first frame is stepped. The size of buffer the device is asked to
//! take comes from the host's profile
//! ([`crate::device::Profile::audio_buffer`]).

use std::sync::Arc;
use std::time::{Duration, Instant};

use kuula_core::audio::{OUTPUT_CHANNELS, SAMPLE_RATE};
use sdl2::audio::{AudioCallback, AudioDevice, AudioSpecDesired};
use sdl2::AudioSubsystem;

pub use kuula_host_common::audio::*;

struct Callback {
    ring: Ring,
    pace: Arc<Pace>,
    /// Device frames a second, for a buffer's duration.
    rate: u32,
}

impl AudioCallback for Callback {
    type Channel = i16;

    fn callback(&mut self, out: &mut [i16]) {
        let frames = (out.len() / OUTPUT_CHANNELS) as u64;
        let buffer = Duration::from_nanos(frames * 1_000_000_000 / self.rate.max(1) as u64);
        self.pace.callback(Instant::now(), buffer);
        self.ring.drain(out);
    }
}

/// The open device, kept alive for the run. Dropping it closes the
/// device.
pub struct Output {
    _device: AudioDevice<Callback>,
    pub ring: Ring,
    pace: Arc<Pace>,
}

impl Output {
    /// Wait, at most [`SETTLE_LIMIT`], until the device asks for audio at
    /// the pace it plays it. Until then it is given silence, and nothing
    /// should be pushed: call this before the first frame.
    pub fn settle(&self) {
        self.pace.settle(&self.ring);
    }

    /// A line on stderr when the ring ran dry or overflowed during the
    /// run; nothing when it never did. With `log`, the events and the
    /// device's callbacks too ([`kuula_host_common::audio::report`]).
    pub fn report(&self, log: bool) {
        report(&self.ring, &self.pace, log);
    }
}

/// Open the default playback device, asking it to take `buffer` sample
/// frames at a time (`KUULA_AUDIO_BUFFER` names another number, for
/// finding out what a device does with it). `None` (after a line on
/// stderr) when there is no usable device, in which case the host runs
/// silent.
pub fn open(audio: &AudioSubsystem, buffer: u16) -> Option<Output> {
    let ring = Ring::default();
    let pace = Arc::new(Pace::default());
    let spec = AudioSpecDesired {
        freq: Some(SAMPLE_RATE as i32),
        channels: Some(OUTPUT_CHANNELS as u8),
        samples: Some(
            std::env::var("KUULA_AUDIO_BUFFER")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(buffer),
        ),
    };
    let device = match audio.open_playback(None, &spec, |got| {
        if got.freq != SAMPLE_RATE as i32 || got.channels != OUTPUT_CHANNELS as u8 {
            eprintln!(
                "audio: device gave {} Hz x{}, wanted {SAMPLE_RATE} stereo; playing anyway",
                got.freq, got.channels
            );
        }
        Callback {
            ring: ring.clone(),
            pace: pace.clone(),
            rate: got.freq.max(1) as u32,
        }
    }) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("audio: cannot open a playback device ({e}); running silent");
            return None;
        }
    };
    device.resume();
    Some(Output {
        _device: device,
        ring,
        pace,
    })
}
