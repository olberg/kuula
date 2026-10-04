//! Audio output: a 44.1 kHz stereo i16 device in SDL's callback mode and a
//! ring buffer of a few frames between the main loop and the callback.
//! The loop pushes each `FrameOutput.audio` (interleaved, left first) and
//! the callback drains it.
//!
//! The ring is a small jitter buffer. The callback gives the device
//! silence until the ring holds [`PRIME_FRAMES`] frames, then plays; when
//! it runs dry it gives silence again until the ring has refilled, so a
//! slow host makes one gap rather than a train of clicks, and glitches
//! rather than drifts. A rebuild flushes it.
//!
//! A device may ask for audio faster than it plays it while it fills a
//! queue of its own. No host can feed that in real time, so
//! [`Output::settle`] lets the device run on silence until it stops getting
//! further ahead of playback, before the first frame is stepped. How long
//! that takes depends on the size of buffer the device is asked to take as
//! much as on the device, so the size comes from the host's profile
//! ([`crate::device::Profile::audio_buffer`]).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use kuula_core::audio::{OUTPUT_CHANNELS, SAMPLE_RATE, VALUES_PER_FRAME};
use sdl2::audio::{AudioCallback, AudioDevice, AudioSpecDesired};
use sdl2::AudioSubsystem;

/// Frames the ring holds before the oldest are dropped: room for the
/// burst of frames a host steps to catch up after a slow one.
pub const RING_FRAMES: usize = 12;

/// Frames the ring must hold before the callback takes any.
pub const PRIME_FRAMES: usize = 3;

/// How long [`Output::settle`] waits at most.
pub const SETTLE_LIMIT: Duration = Duration::from_millis(3000);

/// How long a device must go without getting further ahead of playback to
/// have settled.
pub const SETTLE_QUIET: Duration = Duration::from_millis(250);

/// What the ring did, for the host's line at exit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// Times a playing ring ran dry inside a callback.
    pub underruns: u64,
    /// Values of silence given in those callbacks.
    pub silent_values: u64,
    /// Values dropped because the ring was full.
    pub dropped_values: u64,
}

/// Events kept for the diagnostic log; later ones are not recorded.
const EVENT_LIMIT: usize = 64;

/// One thing worth knowing when the audio misbehaves: when (milliseconds
/// after the ring was first used), what, and how much.
pub type Event = (u64, &'static str, u64);

#[derive(Default)]
struct Inner {
    queue: VecDeque<i16>,
    /// The callback is taking values; false until the ring has filled to
    /// the priming level, and again after it ran dry.
    primed: bool,
    stats: Stats,
    /// When the ring was first pushed to or drained.
    origin: Option<Instant>,
    /// When the callback last came.
    last_drain: Option<Instant>,
    events: Vec<Event>,
}

impl Inner {
    fn note(&mut self, what: &'static str, n: u64) {
        let at = self.origin.get_or_insert_with(Instant::now).elapsed();
        if self.events.len() < EVENT_LIMIT {
            self.events.push((at.as_millis() as u64, what, n));
        }
    }
}

/// The shared ring of interleaved values; `push` from the loop, `drain`
/// from the callback.
#[derive(Clone, Default)]
pub struct Ring(Arc<Mutex<Inner>>);

impl Ring {
    pub fn capacity() -> usize {
        RING_FRAMES * VALUES_PER_FRAME
    }

    /// Values the ring must hold before the callback takes any.
    pub fn prime_level() -> usize {
        PRIME_FRAMES * VALUES_PER_FRAME
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Append a frame; the oldest values go when the ring is full.
    pub fn push(&self, samples: &[i16]) {
        let mut inner = self.lock();
        inner.queue.extend(samples.iter().copied());
        let over = inner.queue.len().saturating_sub(Ring::capacity());
        if over > 0 {
            inner.queue.drain(..over);
            inner.stats.dropped_values += over as u64;
            inner.note("values dropped, ring full", over as u64);
        }
    }

    /// Record something for the diagnostic log.
    pub fn note(&self, what: &'static str, n: u64) {
        self.lock().note(what, n);
    }

    /// The diagnostic log so far.
    pub fn events(&self) -> Vec<Event> {
        self.lock().events.clone()
    }

    /// Fill `out`: silence until the ring is primed, then its values,
    /// padded with silence when it runs dry. Returns the values taken.
    pub fn drain(&self, out: &mut [i16]) -> usize {
        let mut inner = self.lock();
        let now = Instant::now();
        if let Some(before) = inner.last_drain.replace(now) {
            let gap = now.duration_since(before).as_millis() as u64;
            if gap > 60 {
                inner.note("ms between callbacks", gap);
            }
        }
        if !inner.primed {
            if inner.queue.len() < Ring::prime_level() {
                out.fill(0);
                return 0;
            }
            inner.primed = true;
            let held = inner.queue.len() as u64;
            inner.note("playing, values held", held);
        }
        let n = out.len().min(inner.queue.len());
        for (slot, s) in out.iter_mut().zip(inner.queue.drain(..n)) {
            *slot = s;
        }
        out[n..].fill(0);
        if n < out.len() {
            inner.primed = false;
            inner.stats.underruns += 1;
            inner.stats.silent_values += (out.len() - n) as u64;
            inner.note("ran dry, values short", (out.len() - n) as u64);
        }
        n
    }

    /// Empty the ring; it primes again before it plays.
    pub fn clear(&self) {
        let mut inner = self.lock();
        inner.queue.clear();
        inner.primed = false;
    }

    pub fn len(&self) -> usize {
        self.lock().queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn stats(&self) -> Stats {
        self.lock().stats
    }

    /// When the diagnostic log began.
    pub fn origin(&self) -> Option<Instant> {
        self.lock().origin
    }

    /// The values held while the callback is taking them, `None` while it
    /// is not (before the ring has primed, and after it ran dry).
    pub fn playing_level(&self) -> Option<usize> {
        let inner = self.lock();
        inner.primed.then_some(inner.queue.len())
    }
}

/// Below this many values in a playing ring the host steps a frame more.
pub const LOW_LEVEL: usize = VALUES_PER_FRAME * 3 / 2;

/// Above this many it steps a frame fewer.
pub const HIGH_LEVEL: usize = VALUES_PER_FRAME * 5;

/// The level is acted on once in this many shown frames.
pub const TRIM_EVERY: u64 = 8;

/// How many frames to step given what the schedule asks for and what the
/// ring holds. The device's clock, not the host's, decides how fast audio is
/// used, and the two differ a little, so a schedule kept exactly lets the
/// ring creep to empty or to full. One frame more or fewer, at most once in
/// [`TRIM_EVERY`] shown frames (`turn` counts them), holds it between
/// [`LOW_LEVEL`] and [`HIGH_LEVEL`]; the limit keeps a device that stops
/// taking audio, or takes it without end, from stalling or racing the cart.
pub fn trim_steps(steps: u32, level: Option<usize>, turn: u64) -> u32 {
    match level {
        Some(held) if turn.is_multiple_of(TRIM_EVERY) && held < LOW_LEVEL => steps + 1,
        Some(held) if turn.is_multiple_of(TRIM_EVERY) && held > HIGH_LEVEL => {
            steps.saturating_sub(1)
        }
        _ => steps,
    }
}

/// Callback times kept for the diagnostic log.
const TRACE_LIMIT: usize = 200;

/// The diagnostic log keeps how far ahead the device got in each stretch of
/// this many seconds, for at most [`STRETCH_LIMIT`] of them.
const STRETCH_SECONDS: u64 = 10;
const STRETCH_LIMIT: usize = 64;

/// Callbacks back that the device is compared with to see whether it is
/// still gaining on playback.
const LOOKBACK: usize = 8;

/// How far ahead of playback the device is: the audio it has taken, less
/// the time since it first asked. A device filling a queue of its own gets
/// further ahead with every callback; one that takes what it plays stays
/// where it is. It has settled when it has gone [`SETTLE_QUIET`] without
/// getting further ahead than it has ever been, and is not on its way
/// there either: a callback that came late while the device was filling
/// set it back by the delay, and it is filling still, which shows as being
/// further ahead than [`LOOKBACK`] callbacks ago. A quarter of a buffer is
/// allowed in both for callbacks that do not come evenly.
#[derive(Default)]
struct Pace {
    lead: Mutex<Lead>,
    settled: AtomicBool,
}

#[derive(Default)]
struct Lead {
    /// When the first callback came.
    first: Option<Instant>,
    /// The audio taken before the callback being judged.
    taken: Duration,
    callbacks: u64,
    /// The furthest ahead so far, in microseconds, and when it got there.
    peak: i64,
    peak_at: Option<Instant>,
    /// How far ahead it was at each of the last callbacks, oldest first.
    recent: VecDeque<i64>,
    /// For the diagnostic log: the furthest ahead in the stretch that is
    /// running and in each one before it, and when the first callbacks
    /// came.
    stretch_peak: i64,
    stretches: Vec<i64>,
    trace: Vec<Instant>,
}

impl Pace {
    fn callback(&self, now: Instant, buffer: Duration) {
        let mut lead = self.lead.lock().unwrap_or_else(|e| e.into_inner());
        let first = *lead.first.get_or_insert(now);
        let elapsed = now.duration_since(first);
        let ahead = lead.taken.as_micros() as i64 - elapsed.as_micros() as i64;
        lead.taken += buffer;
        lead.callbacks += 1;
        let slack = buffer.as_micros() as i64 / 4;
        lead.recent.push_back(ahead);
        if lead.recent.len() > LOOKBACK + 1 {
            lead.recent.pop_front();
        }
        let gaining = ahead > lead.recent[0] + slack;
        match lead.peak_at {
            Some(at) if ahead <= lead.peak + slack => {
                if !gaining && now.duration_since(at) >= SETTLE_QUIET {
                    self.settled.store(true, Ordering::Relaxed);
                }
            }
            _ => {
                lead.peak = ahead;
                lead.peak_at = Some(now);
            }
        }
        if lead.trace.len() < TRACE_LIMIT {
            lead.trace.push(now);
        }
        let stretch = (elapsed.as_secs() / STRETCH_SECONDS) as usize;
        while stretch > lead.stretches.len() && lead.stretches.len() < STRETCH_LIMIT {
            let peak = lead.stretch_peak;
            lead.stretches.push(peak);
            lead.stretch_peak = i64::MIN;
        }
        lead.stretch_peak = lead.stretch_peak.max(ahead);
    }

    fn settled(&self) -> bool {
        self.settled.load(Ordering::Relaxed)
    }
}

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
        let began = Instant::now();
        while !self.pace.settled() && began.elapsed() < SETTLE_LIMIT {
            std::thread::sleep(Duration::from_millis(2));
        }
        let what = if self.pace.settled() {
            "ms until the device settled"
        } else {
            "ms waited, the device did not settle"
        };
        self.ring.note(what, began.elapsed().as_millis() as u64);
    }

    /// A line on stderr when the ring ran dry or overflowed during the
    /// run; nothing when it never did. With `log`, every recorded event
    /// too, one a line, and what the device's callbacks did: how much
    /// audio they took in how long, how far ahead of playback that put the
    /// device in each ten seconds (a device's own queue filling shows as a
    /// rise, a clock that differs from the host's as a slope), and how far
    /// apart the first of them came.
    pub fn report(&self, log: bool) {
        let s = self.ring.stats();
        if s.underruns > 0 || s.dropped_values > 0 {
            let ms = |values: u64| values * 1000 / (SAMPLE_RATE as u64 * OUTPUT_CHANNELS as u64);
            eprintln!(
                "audio: the ring ran dry {} times ({} ms of silence) and dropped {} ms",
                s.underruns,
                ms(s.silent_values),
                ms(s.dropped_values)
            );
        }
        if !log {
            return;
        }
        for (at, what, n) in self.ring.events() {
            eprintln!("audio: {at:>6} ms  {n:>6} {what}");
        }
        let lead = self.pace.lead.lock().unwrap_or_else(|e| e.into_inner());
        let Some(first) = lead.first else {
            return;
        };
        eprintln!(
            "audio: {} callbacks took {} ms of audio in {} ms",
            lead.callbacks,
            lead.taken.as_millis(),
            first.elapsed().as_millis()
        );
        let ahead: Vec<String> = lead
            .stretches
            .iter()
            .chain(std::iter::once(&lead.stretch_peak))
            .map(|&us| match us {
                i64::MIN => "-".to_string(),
                us => (us / 1000).to_string(),
            })
            .collect();
        eprintln!(
            "audio: most ms ahead of playback in each {STRETCH_SECONDS} s: {}",
            ahead.join(" ")
        );
        let before = match self.ring.origin() {
            Some(origin) => origin.saturating_duration_since(first).as_millis(),
            None => 0,
        };
        eprintln!(
            "audio: the first callback came {before} ms before the log began; ms from each to the next:"
        );
        let at = |t: &Instant| t.duration_since(first).as_millis();
        let apart: Vec<String> = lead
            .trace
            .iter()
            .zip(lead.trace.iter().skip(1))
            .map(|(a, b)| (at(b) - at(a)).to_string())
            .collect();
        for row in apart.chunks(25) {
            eprintln!("audio:   {}", row.join(" "));
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_is_bounded_and_counts_what_it_drops() {
        let ring = Ring::default();
        let frame = vec![7i16; VALUES_PER_FRAME];
        for _ in 0..RING_FRAMES + 2 {
            ring.push(&frame);
        }
        assert_eq!(ring.len(), Ring::capacity());
        assert_eq!(ring.stats().dropped_values, 2 * VALUES_PER_FRAME as u64);
        let mut out = vec![1i16; 10];
        assert_eq!(ring.drain(&mut out), 10);
        assert!(out.iter().all(|&s| s == 7));
        ring.clear();
        assert!(ring.is_empty());
    }

    #[test]
    fn the_callback_gets_silence_until_the_ring_is_primed() {
        let ring = Ring::default();
        let frame = vec![5i16; VALUES_PER_FRAME];
        let mut out = vec![1i16; 2048];
        // Two frames are not enough: silence, and nothing taken.
        ring.push(&frame);
        ring.push(&frame);
        assert_eq!(ring.drain(&mut out), 0);
        assert!(out.iter().all(|&s| s == 0));
        assert_eq!(ring.len(), 2 * VALUES_PER_FRAME);
        assert_eq!(
            ring.stats(),
            Stats::default(),
            "waiting to prime is not an underrun"
        );
        // With the third it plays.
        ring.push(&frame);
        assert_eq!(ring.drain(&mut out), 2048);
        assert!(out.iter().all(|&s| s == 5));
        // Below the priming level it keeps playing: priming is for starts.
        assert_eq!(ring.drain(&mut out), 2048);
        assert_eq!(ring.len(), 3 * VALUES_PER_FRAME - 4096);
    }

    #[test]
    fn a_dry_ring_is_one_gap_and_primes_again() {
        let ring = Ring::default();
        let frame = vec![9i16; VALUES_PER_FRAME];
        for _ in 0..PRIME_FRAMES {
            ring.push(&frame);
        }
        let mut out = vec![1i16; Ring::prime_level() + 6];
        assert_eq!(ring.drain(&mut out), Ring::prime_level());
        assert_eq!(&out[Ring::prime_level()..], &[0; 6]);
        let s = ring.stats();
        assert_eq!((s.underruns, s.silent_values), (1, 6));
        // One frame back is not enough to start again: no click train.
        ring.push(&frame);
        let mut out = vec![1i16; 64];
        assert_eq!(ring.drain(&mut out), 0);
        assert!(out.iter().all(|&s| s == 0));
        assert_eq!(ring.stats().underruns, 1);
        ring.push(&frame);
        ring.push(&frame);
        assert_eq!(ring.drain(&mut out), 64);
        // A flush leaves it unprimed too.
        ring.clear();
        ring.push(&frame);
        assert_eq!(ring.drain(&mut out), 0);
    }

    /// A device whose clock runs one percent fast against a host that keeps
    /// its schedule exactly: without the trim the ring runs dry again and
    /// again; with it, never, and the cart runs the one percent faster.
    #[test]
    fn the_ring_level_trims_the_frame_count_to_the_devices_clock() {
        let run = |trim: bool| {
            let ring = Ring::default();
            let frame = vec![1i16; VALUES_PER_FRAME];
            let mut out = vec![0i16; 2048];
            // Time in microseconds: a frame every 16 667, a buffer of 1024
            // frames every 22 990 (44 544 Hz instead of 44 100).
            let (mut next_frame, mut next_buffer, mut stepped, mut turn) = (0u64, 0u64, 0u64, 0u64);
            while next_frame < 60_000_000 {
                while next_buffer <= next_frame {
                    ring.drain(&mut out);
                    next_buffer += 22_990;
                }
                let steps = if trim {
                    trim_steps(1, ring.playing_level(), turn)
                } else {
                    1
                };
                for _ in 0..steps {
                    ring.push(&frame);
                    stepped += 1;
                }
                turn += 1;
                next_frame += 16_667;
            }
            (ring.stats(), stepped)
        };
        let (plain, frames) = run(false);
        assert!(plain.underruns > 5, "{plain:?}");
        assert_eq!(frames, 3600);
        let (trimmed, frames) = run(true);
        assert_eq!((trimmed.underruns, trimmed.dropped_values), (0, 0));
        assert!((3630..=3645).contains(&frames), "{frames}");
    }

    #[test]
    fn the_trim_is_one_frame_at_most_once_in_eight_and_only_while_playing() {
        assert_eq!(trim_steps(1, None, 0), 1, "not playing: the schedule alone");
        assert_eq!(trim_steps(1, Some(LOW_LEVEL - 1), 0), 2);
        assert_eq!(trim_steps(1, Some(LOW_LEVEL - 1), 1), 1, "not this turn");
        assert_eq!(trim_steps(1, Some(LOW_LEVEL), 0), 1);
        assert_eq!(trim_steps(1, Some(HIGH_LEVEL + 1), 8), 0);
        assert_eq!(trim_steps(0, Some(HIGH_LEVEL + 1), 8), 0);
        assert_eq!(trim_steps(3, Some(HIGH_LEVEL), 8), 3);
    }

    /// Feed callbacks the given milliseconds apart, the first at zero, and
    /// say how many milliseconds after the first the pace had settled.
    fn settled_after(buffer: Duration, apart: impl IntoIterator<Item = u64>) -> Option<u64> {
        let pace = Pace::default();
        let t0 = Instant::now();
        let mut at = 0;
        pace.callback(t0, buffer);
        for gap in apart {
            if pace.settled() {
                break;
            }
            at += gap;
            pace.callback(t0 + Duration::from_millis(at), buffer);
        }
        pace.settled().then_some(at)
    }

    /// The Miyoo Mini's driver with the buffer most hosts ask for, as traced
    /// on the device: its 13 ms sleep between buffers lasts two of the
    /// kernel's 10 ms ticks, so it takes 23.2 ms of audio every 20 ms, for
    /// 108 callbacks, until the 350 ms it can queue are queued. From then
    /// on a send blocks once in nine or ten.
    #[test]
    fn a_device_filling_its_queue_has_not_settled_until_the_queue_is_full() {
        let buffer = Duration::from_micros(23_220); // 1024 frames at 44.1 kHz
        let filling = std::iter::once(18).chain(std::iter::repeat_n(20, 107));
        let full = [8usize, 9]
            .into_iter()
            .cycle()
            .flat_map(|n| std::iter::once(50).chain(std::iter::repeat_n(20, n)));
        let filled_at = 18 + 107 * 20;
        assert_eq!(
            settled_after(buffer, filling.clone()),
            None,
            "taking audio faster than it plays: still filling"
        );
        let at = settled_after(buffer, filling.clone().chain(full.clone().take(100)))
            .expect("settles once it is full");
        assert!(
            (filled_at..filled_at + 400).contains(&at),
            "settled at {at} ms, full at {filled_at}"
        );

        // A callback 40 ms late in the middle of the filling sets the device
        // back, and it is still filling: the wait goes on to the same end.
        let stalled = std::iter::repeat_n(20, 50)
            .chain(std::iter::once(60))
            .chain(std::iter::repeat_n(20, 64));
        assert_eq!(settled_after(buffer, stalled.clone()), None);
        let at = settled_after(buffer, stalled.chain(full.take(100))).expect("settles");
        assert!((2300..2800).contains(&at), "settled at {at} ms");
    }

    #[test]
    fn a_device_in_step_settles_after_the_quiet_wait() {
        let quiet = SETTLE_QUIET.as_millis() as u64;
        // The Miyoo Mini's driver with 441 frames, as traced on the device:
        // 10 ms of audio a tick from the start.
        let buffer = Duration::from_millis(10);
        let traced = [3, 10, 10, 10, 10, 10, 11, 9];
        let at = settled_after(
            buffer,
            traced.into_iter().chain(std::iter::repeat_n(10, 60)),
        )
        .expect("settles");
        assert!((quiet..quiet + 30).contains(&at), "settled at {at} ms");

        // A desktop device that takes three buffers at once to fill its own
        // and is in step from then on.
        let buffer = Duration::from_micros(23_220);
        let at = settled_after(
            buffer,
            [0, 0].into_iter().chain(std::iter::repeat_n(23, 30)),
        )
        .expect("settles");
        assert!((quiet..quiet + 50).contains(&at), "settled at {at} ms");

        // One that asks for two buffers at a time, twice as far apart.
        let pairs = (0..40).map(|i| if i % 2 == 0 { 0 } else { 46 });
        let at = settled_after(buffer, pairs).expect("settles");
        assert!((quiet..quiet + 100).contains(&at), "settled at {at} ms");
    }
}
