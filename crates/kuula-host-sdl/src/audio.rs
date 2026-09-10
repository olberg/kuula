//! Audio output: a 44.1 kHz mono i16 device in SDL's callback mode and a
//! ring buffer of a few frames between the main loop and the callback
//!. The loop pushes each `FrameOutput.audio`; the
//! callback drains it and outputs silence on underrun, so a slow host
//! glitches rather than drifts. A rebuild flushes it.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use kuula_core::audio::{SAMPLES_PER_FRAME, SAMPLE_RATE};
use sdl2::audio::{AudioCallback, AudioDevice, AudioSpecDesired};
use sdl2::AudioSubsystem;

/// Frames the ring holds before the oldest are dropped.
pub const RING_FRAMES: usize = 4;

/// Samples the device asks for per callback.
const DEVICE_BUFFER: u16 = 1024;

/// The shared ring; `push` from the loop, `drain` from the callback.
#[derive(Clone, Default)]
pub struct Ring(Arc<Mutex<VecDeque<i16>>>);

impl Ring {
    pub fn capacity() -> usize {
        RING_FRAMES * SAMPLES_PER_FRAME
    }

    /// Append a frame; the oldest samples go when the ring is full.
    pub fn push(&self, samples: &[i16]) {
        let mut q = self.0.lock().unwrap_or_else(|e| e.into_inner());
        q.extend(samples.iter().copied());
        let over = q.len().saturating_sub(Ring::capacity());
        if over > 0 {
            q.drain(..over);
        }
    }

    /// Fill `out`, padding with silence when the ring runs dry.
    pub fn drain(&self, out: &mut [i16]) -> usize {
        let mut q = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let n = out.len().min(q.len());
        for (slot, s) in out.iter_mut().zip(q.drain(..n)) {
            *slot = s;
        }
        for slot in &mut out[n..] {
            *slot = 0;
        }
        n
    }

    pub fn clear(&self) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    pub fn len(&self) -> usize {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

struct Callback(Ring);

impl AudioCallback for Callback {
    type Channel = i16;

    fn callback(&mut self, out: &mut [i16]) {
        self.0.drain(out);
    }
}

/// The open device, kept alive for the run. Dropping it closes the
/// device.
pub struct Output {
    _device: AudioDevice<Callback>,
    pub ring: Ring,
}

/// Open the default playback device. `None` (after a line on stderr)
/// when there is no usable device, in which case the host runs silent.
pub fn open(audio: &AudioSubsystem) -> Option<Output> {
    let ring = Ring::default();
    let spec = AudioSpecDesired {
        freq: Some(SAMPLE_RATE as i32),
        channels: Some(1),
        samples: Some(DEVICE_BUFFER),
    };
    let device = match audio.open_playback(None, &spec, |got| {
        if got.freq != SAMPLE_RATE as i32 || got.channels != 1 {
            eprintln!(
                "audio: device gave {} Hz x{}, wanted {SAMPLE_RATE} mono; playing anyway",
                got.freq, got.channels
            );
        }
        Callback(ring.clone())
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_is_bounded_and_pads_with_silence() {
        let ring = Ring::default();
        let frame = vec![7i16; SAMPLES_PER_FRAME];
        for _ in 0..RING_FRAMES + 2 {
            ring.push(&frame);
        }
        assert_eq!(ring.len(), Ring::capacity());
        let mut out = vec![1i16; 10];
        assert_eq!(ring.drain(&mut out), 10);
        assert!(out.iter().all(|&s| s == 7));
        ring.clear();
        assert!(ring.is_empty());
        ring.push(&[3, 4]);
        let mut out = vec![1i16; 4];
        assert_eq!(ring.drain(&mut out), 2);
        assert_eq!(out, [3, 4, 0, 0]);
    }
}
