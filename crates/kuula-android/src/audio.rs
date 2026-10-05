//! Audio output: a stereo `i16` AAudio stream on the default device, fed
//! from the ring that `kuula_host_common` shares with the SDL host. The
//! loop pushes each frame's PCM; the stream's callback drains it, with
//! silence until the ring has primed and again when it runs dry. A device
//! that is missing, or goes away, is not fatal: the console still renders
//! its PCM and the loop carries on without it.
//!
//! What keeps the sound close to the picture:
//!
//! - The stream names no rate and asks for the low-latency path, which a
//!   stream at any rate but the device's own is not given. At 44.1 kHz on a
//!   48 kHz phone the sound was written 124 ms ahead of the speaker and a
//!   cart's picture led its music. The console's 44.1 kHz is brought to the
//!   device's rate in the callback (`resample`).
//! - The stream is told to hold two of the device's bursts, not all it has
//!   room for (40 ms and more), and a burst more each time the device runs
//!   out.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use kuula_core::audio::{OUTPUT_CHANNELS, SAMPLE_RATE};
use kuula_host_common::audio::{Pace, Ring};
use ndk::audio::{
    AudioCallbackResult, AudioDirection, AudioFormat, AudioPerformanceMode, AudioStream,
    AudioStreamBuilder, Clockid,
};

use crate::resample::{Resampler, MAX_STEP};

/// Output sample frames resampled at a time, so the callback has its
/// source frames in a buffer made before the stream started.
const PIECE_FRAMES: usize = 512;

/// Bursts the stream holds to begin with.
const START_BURSTS: i32 = 2;

/// The delay to the speaker is asked once in this many callbacks.
const STAMP_EVERY: u32 = 64;

/// What the stream's callback last saw, for the log.
#[derive(Default)]
pub struct Latency {
    frames: AtomicU32,
    held: AtomicU32,
    millis: AtomicU32,
}

impl Latency {
    /// Sample frames a callback, sample frames the stream holds at most,
    /// and milliseconds from a callback to the speaker (0 when the device
    /// does not say).
    pub fn read(&self) -> (u32, u32, u32) {
        (
            self.frames.load(Ordering::Relaxed),
            self.held.load(Ordering::Relaxed),
            self.millis.load(Ordering::Relaxed),
        )
    }
}

/// The open stream, kept for the run. Dropping it closes the device.
pub struct Audio {
    stream: AudioStream,
    pub ring: Ring,
    pub pace: Arc<Pace>,
    pub latency: Arc<Latency>,
    /// The rate the stream plays at, in Hz.
    pub rate: u32,
    /// Set by the stream's error callback: the device went away (headphones
    /// pulled, a route change) and the stream is dead.
    failed: Arc<AtomicBool>,
}

/// Nanoseconds on the clock the stream's timestamps are read on.
fn monotonic_nanos() -> Option<i64> {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid place for the one value the call writes.
    let status = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    (status == 0).then(|| ts.tv_sec as i64 * 1_000_000_000 + ts.tv_nsec as i64)
}

/// Milliseconds until what the stream is given now reaches the speaker:
/// the device says when a frame it names was played, and the frames
/// written since are still on their way.
fn ahead_millis(stream: &AudioStream, rate: u32) -> Option<u32> {
    let stamp = stream.timestamp(Clockid::Monotonic).ok()?;
    let waiting = stream.frames_written() - stamp.frame_position;
    let plays_at = stamp.time_nanoseconds + waiting * 1_000_000_000 / rate as i64;
    let ahead = plays_at - monotonic_nanos()?;
    Some((ahead.max(0) / 1_000_000) as u32)
}

/// The stream's data callback: the ring's frames at the stream's rate.
fn callback(
    ring: Ring,
    pace: Arc<Pace>,
    latency: Arc<Latency>,
) -> impl FnMut(&AudioStream, *mut c_void, i32) -> AudioCallbackResult + Send {
    let mut resampler: Option<Resampler> = None;
    // What a piece can need at the steepest step, and the frame more that
    // the phase it is left in can ask for.
    let mut source = vec![0i16; (PIECE_FRAMES * MAX_STEP + 1) * OUTPUT_CHANNELS];
    let mut underruns = 0;
    let mut calls = 0u32;
    move |stream, data, frames| {
        let frames = usize::try_from(frames).unwrap_or(0);
        if data.is_null() || frames == 0 {
            return AudioCallbackResult::Continue;
        }
        // SAFETY: AAudio gives the callback room for `frames` frames in the
        // stream's format, and `Audio::open` starts only a stream it has
        // seen to be 16-bit with `OUTPUT_CHANNELS` values a frame.
        let out =
            unsafe { std::slice::from_raw_parts_mut(data.cast::<i16>(), frames * OUTPUT_CHANNELS) };
        let rate = stream.sample_rate().max(1) as u32;
        let buffer = Duration::from_nanos(frames as u64 * 1_000_000_000 / rate as u64);
        pace.callback(Instant::now(), buffer);

        let resampler = resampler.get_or_insert_with(|| Resampler::new(SAMPLE_RATE, rate));
        if resampler.is_identity() {
            ring.drain(out);
        } else {
            for piece in out.chunks_mut(PIECE_FRAMES * OUTPUT_CHANNELS) {
                let need = resampler.needs(piece.len() / OUTPUT_CHANNELS) * OUTPUT_CHANNELS;
                let taken = &mut source[..need];
                if !taken.is_empty() {
                    ring.drain(taken);
                }
                resampler.run(taken, piece);
            }
        }

        // The device ran out: the callbacks do not always come in time for
        // what the stream holds, so it holds a burst more from here on.
        let seen = stream.x_run_count();
        if seen > underruns {
            ring.note("device underruns", (seen - underruns) as u64);
            underruns = seen;
            let more = stream.buffer_size_in_frames() + stream.frames_per_burst().max(16);
            let _ = stream.set_buffer_size_in_frames(more.min(stream.buffer_capacity_in_frames()));
        }

        if calls.is_multiple_of(STAMP_EVERY) {
            latency.frames.store(frames as u32, Ordering::Relaxed);
            let held = stream.buffer_size_in_frames().max(0) as u32;
            latency.held.store(held, Ordering::Relaxed);
            if let Some(ahead) = ahead_millis(stream, rate) {
                latency.millis.store(ahead, Ordering::Relaxed);
            }
        }
        calls = calls.wrapping_add(1);
        AudioCallbackResult::Continue
    }
}

impl Audio {
    /// Open the default output device and start it. `None`, after a line in
    /// the log, when there is none or it cannot take this format.
    pub fn open(ring: Ring) -> Option<Audio> {
        let pace = Arc::new(Pace::default());
        let latency = Arc::new(Latency::default());
        let failed = Arc::new(AtomicBool::new(false));
        let opened = AudioStreamBuilder::new().and_then(|builder| {
            builder
                .direction(AudioDirection::Output)
                .performance_mode(AudioPerformanceMode::LowLatency)
                .channel_count(OUTPUT_CHANNELS as i32)
                .format(AudioFormat::PCM_I16)
                .data_callback(Box::new(callback(
                    ring.clone(),
                    pace.clone(),
                    latency.clone(),
                )))
                .error_callback(Box::new({
                    let failed = failed.clone();
                    move |_, e| {
                        log::warn!("audio: the stream failed: {e}");
                        failed.store(true, Ordering::Relaxed);
                    }
                }))
                .open_stream()
        });
        let stream = match opened {
            Ok(s) => s,
            Err(e) => {
                log::warn!("audio: cannot open an output stream ({e}); running silent");
                return None;
            }
        };
        let (format, channels) = (stream.format(), stream.channel_count());
        if format != AudioFormat::PCM_I16 || channels != OUTPUT_CHANNELS as i32 {
            log::warn!(
                "audio: the stream is {format:?} with {channels} channels, not what was asked; running silent"
            );
            return None;
        }
        let rate = stream.sample_rate().max(1) as u32;
        let burst = stream.frames_per_burst().max(16);
        let wanted = (burst * START_BURSTS).min(stream.buffer_capacity_in_frames());
        // What the call returns is the size it settled on, which the binding
        // takes for an error; the stream is asked instead.
        let _ = stream.set_buffer_size_in_frames(wanted);
        let held = stream.buffer_size_in_frames();
        let fast = stream.performance_mode() == AudioPerformanceMode::LowLatency;
        log::info!(
            "audio: {rate} Hz in bursts of {burst} sample frames, holding {held} of the {} it has room for, {} the low-latency path",
            stream.buffer_capacity_in_frames(),
            if fast { "on" } else { "not on" }
        );
        if let Err(e) = stream.request_start() {
            log::warn!("audio: cannot start the output stream ({e}); running silent");
            return None;
        }
        Some(Audio {
            stream,
            ring,
            pace,
            latency,
            rate,
            failed,
        })
    }

    /// Whether the stream died and needs opening again.
    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }

    /// Stop the device and forget what was queued: the app is not in front.
    pub fn pause(&self) {
        if let Err(e) = self.stream.request_pause() {
            log::warn!("audio: cannot pause: {e}");
        }
        self.ring.clear();
    }

    /// Start it again; the ring primes before it plays.
    pub fn resume(&self) {
        self.ring.clear();
        if let Err(e) = self.stream.request_start() {
            log::warn!("audio: cannot resume: {e}");
            self.failed.store(true, Ordering::Relaxed);
        }
    }
}
