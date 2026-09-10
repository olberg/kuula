//! The cart-facing audio calls on [`DrawState`]: they resolve names to
//! cart paths, load and cache the assets through the `CartSource` like
//! `load_sheet` does, and hand the decoded data to the mixer.

use std::rc::Rc;

use super::tracker::{self, Track};
use super::AudioError;
use crate::draw::DrawState;
use crate::manifest::valid_asset_name;

pub fn sfx_path(name: &str) -> String {
    format!("sfx/{name}.trk")
}

pub fn music_path(name: &str) -> String {
    format!("music/{name}.trk")
}

pub fn sample_path(name: &str) -> String {
    format!("samples/{name}.wav")
}

impl DrawState {
    fn read_asset(&self, path: &str, name: &str) -> Result<Vec<u8>, AudioError> {
        if !valid_asset_name(name) {
            return Err(AudioError::AssetInvalid {
                path: path.to_string(),
                why: "bad asset name".to_string(),
            });
        }
        self.cart.read(path).map_err(|_| AudioError::AssetNotFound {
            path: path.to_string(),
        })
    }

    /// Decode `samples/<name>.wav` into the bank, or return the cached one.
    pub fn load_sample(&mut self, name: &str) -> Result<Rc<super::sample::Sample>, AudioError> {
        let path = sample_path(name);
        if let Some(s) = self.audio.samples().get(&path) {
            return Ok(s);
        }
        let bytes = self.read_asset(&path, name)?;
        self.audio.samples_mut().insert(&path, &bytes)
    }

    /// Parse a track at `path` and load every sample it names, or return
    /// the cached one.
    fn load_track(&mut self, path: &str, name: &str) -> Result<Rc<Track>, AudioError> {
        if let Some(t) = self.audio.cached_track(path) {
            return Ok(t);
        }
        let bytes = self.read_asset(path, name)?;
        let track = tracker::parse(path, &bytes)?;
        let samples: Vec<String> = track.sample_names().map(str::to_string).collect();
        for s in samples {
            self.load_sample(&s)?;
        }
        Ok(self.audio.cache_track(path, track))
    }

    /// `sfx(name, channel?)`: play `sfx/<name>.trk`. Returns the channel.
    pub fn sfx(&mut self, name: &str, channel: Option<i64>) -> Result<usize, AudioError> {
        let path = sfx_path(name);
        let track = self.load_track(&path, name)?;
        self.audio.play_sfx(&path, track, channel)
    }

    /// `music(name, fade?)`: play `music/<name>.trk` on the low channels;
    /// `None` fades the current music out over `fade` frames.
    pub fn music(&mut self, name: Option<&str>, fade: u32) -> Result<(), AudioError> {
        match name {
            Some(name) => {
                let track = self.load_track(&music_path(name), name)?;
                self.audio.play_music(track, fade);
            }
            None => self.audio.stop_music(fade),
        }
        Ok(())
    }

    /// `sample(name, channel?, pitch?)` with `pitch` in 16.16.
    pub fn sample(
        &mut self,
        name: &str,
        channel: Option<i64>,
        pitch: u32,
    ) -> Result<usize, AudioError> {
        let sample = self.load_sample(name)?;
        self.audio.play_sample(sample, channel, pitch)
    }

    /// `volume(channel, v)` with `v` already scaled to 0..=255.
    pub fn volume(&mut self, channel: i64, v: u8) -> Result<(), AudioError> {
        self.audio.set_volume(channel, v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::sample::encode_wav;
    use crate::snapshot::{Snapshot, SnapshotLimits};
    use crate::source::CartSource;

    fn state(entries: Vec<(&str, Vec<u8>)>) -> DrawState {
        let cart: Rc<dyn CartSource> =
            Rc::new(Snapshot::from_entries(entries, SnapshotLimits::default()).unwrap());
        DrawState::new(8, 8, cart)
    }

    #[test]
    fn loads_tracks_and_samples_by_name_with_caching() {
        let mut d = state(vec![
            (
                "sfx/hit.trk",
                b"tempo 2\ninst 1 pulse\ninst 2 sample=kick\nC-4 1\nC-4 2\n".to_vec(),
            ),
            (
                "music/song.trk",
                b"loop 0\ninst 1 saw\nC-3 1|E-3 1\n".to_vec(),
            ),
            (
                "samples/kick.wav",
                encode_wav(22050, 8, 1, &[0, 255, 0, 255]),
            ),
            ("sfx/bad.trk", b"inst 1 sample=nope\nC-4 1\n".to_vec()),
        ]);
        assert_eq!(d.sfx("hit", None).unwrap(), 7);
        assert_eq!(d.audio.samples().used(), 4);
        assert_eq!(d.sfx("hit", Some(1)).unwrap(), 1);
        assert_eq!(d.sfx("nope", None).unwrap_err().code(), "asset_not_found");
        assert_eq!(d.sfx("../x", None).unwrap_err().code(), "asset_invalid");
        assert_eq!(d.sfx("bad", None).unwrap_err().code(), "asset_not_found");
        d.music(Some("song"), 0).unwrap();
        assert!(d.audio.music_playing());
        d.music(None, 0).unwrap();
        assert!(!d.audio.music_playing());
        d.audio.stop_all();
        assert_eq!(d.sample("kick", Some(3), 1 << 16).unwrap(), 3);
        assert_eq!(
            d.sample("kick", Some(9), 1 << 16).unwrap_err().code(),
            "audio_bad_channel"
        );
        d.volume(3, 0).unwrap();
        let out = d.audio.render().to_vec();
        assert!(out.iter().all(|&s| s == 0));
    }
}
