//! Small in-memory songs for the tests: a builder for the OMT payload and
//! the container around it (written by the engine's own `omc::write_song`).
//! Every channel holds one pulse note, at a quarter of full scale, for the
//! whole order row, so a song of `n` channels sounds on `n` channels for
//! `ticks` ticks.

use omt_engine::omc::{self, ResourceOut};
use serde_json::{json, Value};

/// A song under construction; see [`omc`] for the file.
#[derive(Clone)]
pub struct Song {
    channels: Vec<Value>,
    /// The channels with a track: the others have none.
    silent: Vec<bool>,
    tick: (u64, u64),
    ticks: u64,
    looping: bool,
    profile: Option<String>,
    samples: Vec<Value>,
    sampler: bool,
    arrangements: usize,
    subsong: Option<u64>,
    volume: Option<u64>,
    padding: usize,
}

impl Song {
    /// `n` music channels, a tick of 1/60 s (one per frame) and an order row
    /// of 6 ticks, ending after it.
    pub fn new(n: usize) -> Song {
        Song {
            channels: vec![json!({}); n],
            silent: vec![false; n],
            tick: (1, 60),
            ticks: 6,
            looping: false,
            profile: Some("kuula".into()),
            samples: Vec::new(),
            sampler: false,
            arrangements: 1,
            subsong: None,
            volume: None,
            padding: 0,
        }
    }

    pub fn ticks(mut self, ticks: u64) -> Song {
        self.ticks = ticks;
        self
    }

    pub fn tick(mut self, n: u64, d: u64) -> Song {
        self.tick = (n, d);
        self
    }

    /// The arrangement goes back to its first order row.
    pub fn looping(mut self) -> Song {
        self.looping = true;
        self
    }

    /// Channel `c` has role `reserved`, and with it no track.
    pub fn reserved(mut self, c: usize) -> Song {
        self.channels[c]["role"] = json!("reserved");
        self.silent[c] = true;
        self
    }

    /// Channel `c` has no track (but keeps its role).
    pub fn silent(mut self, c: usize) -> Song {
        self.silent[c] = true;
        self
    }

    pub fn pan(mut self, c: usize, pan: i32) -> Song {
        self.channels[c]["pan"] = json!(pan);
        self
    }

    pub fn profile(mut self, profile: Option<&str>) -> Song {
        self.profile = profile.map(str::to_string);
        self
    }

    /// A sample record of `frames` frames, and its `sampler` instrument 2.
    pub fn sample(mut self, frames: u64, channels: u64) -> Song {
        let resource = self.samples.len();
        self.samples.push(json!({
            "resource": resource, "encoding": "pcm16", "rate": 44100,
            "channels": channels, "frames": frames, "root": 15360,
        }));
        self
    }

    /// Every channel plays instrument 2, the sampler, instead of the pulse.
    pub fn sampler(mut self) -> Song {
        self.sampler = true;
        self
    }

    /// The song's `volume`, 0 to 1024 with 256 as unity.
    pub fn volume(mut self, volume: u64) -> Song {
        self.volume = Some(volume);
        self
    }

    /// A title of `n` bytes, to make the song's payload larger.
    pub fn padded(mut self, n: usize) -> Song {
        self.padding = n;
        self
    }

    /// `n` arrangements, arrangement `i` lasting `ticks * (i + 1)` ticks, and
    /// the `subsong` that selects one.
    pub fn arrangements(mut self, n: usize, subsong: Option<u64>) -> Song {
        self.arrangements = n;
        self.subsong = subsong;
        self
    }

    /// The payload as JSON, to tweak before it is written.
    pub fn json(&self) -> Value {
        let n = self.channels.len();
        let ins = if self.sampler { "02" } else { "01" };
        // One note held for the whole order row: a track of speed at most
        // 255 and as many rows as the row's ticks need.
        let speed = self.ticks.clamp(1, 255);
        let rows = self.ticks.div_ceil(speed).max(1);
        let tracks: Vec<Value> = (0..n)
            .map(|_| json!({"rows": rows, "speed": speed, "cells": [[0, format!("C-4 {ins}")]]}))
            .collect();
        let order: Vec<Value> = (0..n)
            .map(|c| {
                if self.silent[c] {
                    Value::Null
                } else {
                    json!(c)
                }
            })
            .collect();
        let row = |ticks: u64| {
            let mut row = json!({"tracks": order, "ticks": ticks});
            if self.looping {
                row["next"] = json!(0);
            }
            row
        };
        let mut song = json!({
            "omt": "0.3",
            "rate": 44100,
            "tick": [self.tick.0, self.tick.1],
            "channels": self.channels,
            "instruments": [
                {"number": 1, "volume": 32, "engine": {"kind": "wave", "waveform": "pulse"}},
                {"number": 2, "volume": 32, "engine": {"kind": "sampler", "sample": 0}},
            ],
            "tracks": tracks,
            "arrangements": (0..self.arrangements as u64)
                .map(|i| json!({"orders": [row(self.ticks * (i + 1))]}))
                .collect::<Vec<_>>(),
        });
        if !self.samples.is_empty() {
            song["samples"] = json!(self.samples);
        }
        if let Some(v) = self.volume {
            song["volume"] = json!(v);
        }
        if self.padding > 0 {
            song["title"] = json!("x".repeat(self.padding));
        }
        if let Some(p) = &self.profile {
            song["profile"] = json!(p);
        }
        // The sampler needs a sample; without one it is not in the song.
        if self.samples.is_empty() {
            song["instruments"].as_array_mut().unwrap().pop();
        }
        song
    }

    /// The resources the records describe: constant PCM of the right length.
    pub fn resources(&self) -> Vec<Vec<u8>> {
        self.samples
            .iter()
            .map(|s| {
                let n = s["frames"].as_u64().unwrap() * s["channels"].as_u64().unwrap();
                1000i16.to_le_bytes().repeat(n as usize)
            })
            .collect()
    }
}

/// The OMC file of `song`, with its resources.
pub fn omc(song: &Song) -> Vec<u8> {
    let payload = serde_json::to_vec(&song.json()).unwrap();
    let resources = song.resources();
    let mut file = file(&payload, &resources);
    if let Some(sub) = song.subsong {
        file = with_subsong(&file, sub);
    }
    file
}

/// The container around `payload` and `resources`, written by the engine.
pub fn file(payload: &[u8], resources: &[Vec<u8>]) -> Vec<u8> {
    let out: Vec<ResourceOut> = resources
        .iter()
        .map(|r| ResourceOut {
            data: r,
            media_type: None,
        })
        .collect();
    omc::write_song(payload, &out, None, "kuula tests")
}

/// The same file with `"subsong": n` in its song entry: the manifest is
/// rewritten (the engine's writer has no such option).
fn with_subsong(file: &[u8], subsong: u64) -> Vec<u8> {
    let chunks = omc::chunks(file).unwrap();
    let mut manifest: Value = serde_json::from_slice(chunks[0].data.unwrap()).unwrap();
    manifest["songs"][0]["subsong"] = json!(subsong);
    let rest: Vec<(&[u8; 4], &[u8])> = chunks[1..]
        .iter()
        .map(|c| (&c.kind, c.data.unwrap()))
        .collect();
    container(&serde_json::to_vec(&manifest).unwrap(), &rest)
}

/// A container of `manifest` as a `JSON` chunk and then `chunks`, as they
/// are: for files the engine's writer would not make.
pub fn container(manifest: &[u8], chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
    container_of(b"JSON", manifest, chunks)
}

/// As [`container`], with the first chunk of type `kind` (`JSNZ` for a
/// compressed manifest).
pub fn container_of(kind: &[u8; 4], manifest: &[u8], chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
    let mut out = b"\x89OMC".to_vec();
    out.extend_from_slice(&[0, 0, 8, 0, 0, 0, 0, 0]);
    put(&mut out, kind, manifest);
    for (kind, data) in chunks {
        put(&mut out, kind, data);
    }
    let len = out.len() as u32;
    out[8..12].copy_from_slice(&len.to_le_bytes());
    out
}

/// A chunk: length, type, data, padding, CRC-32 of type and data.
fn put(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.resize(out.len() + (4 - data.len() % 4) % 4, 0);
    let crc = crc32(kind.iter().chain(data));
    out.extend_from_slice(&crc.to_le_bytes());
}

/// CRC-32 (IEEE, reflected), bit by bit.
fn crc32<'a>(bytes: impl Iterator<Item = &'a u8>) -> u32 {
    let mut crc = !0u32;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}
