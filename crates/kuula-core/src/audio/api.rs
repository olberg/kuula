//! The cart-facing audio calls on [`DrawState`]: they resolve names to
//! cart paths, load and cache the assets through the `CartSource` like
//! `load_sheet` does, and hand the decoded data to the mixer.

use std::rc::Rc;
use std::sync::Arc;

use omt_engine::omq::{Bank, Trigger};

use super::songs::{self, PlayableSong};
use super::{cues, AudioError, Mixer};
use crate::draw::DrawState;
use crate::manifest::valid_asset_name;

pub fn sfx_path(name: &str) -> String {
    format!("sfx/{name}.omc")
}

pub fn music_path(name: &str) -> String {
    format!("music/{name}.omc")
}

pub fn sample_path(name: &str) -> String {
    format!("samples/{name}.wav")
}

pub fn cues_path(bank: &str) -> String {
    format!("cues/{bank}.omc")
}

/// The version of a song a `music` call names: one of its arrangements,
/// by its name or by its number, 0 being the first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Version<'a> {
    Name(&'a str),
    Number(i64),
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

    /// Decode `samples/<name>.wav` into the bank, or return the cached
    /// one. A file that was refused is not read again.
    pub fn load_sample(&mut self, name: &str) -> Result<Rc<super::sample::Sample>, AudioError> {
        let path = sample_path(name);
        if let Some(s) = self.audio.samples().get(&path) {
            return Ok(s);
        }
        if let Some(e) = self.audio.refused(&path) {
            return Err(e);
        }
        let bytes = self.read_asset(&path, name)?;
        match self.audio.samples_mut().insert(&path, &bytes) {
            Ok(sample) => Ok(sample),
            Err(e) => Err(self.audio.refuse(&path, e)),
        }
    }

    /// Read the song at `path`, or return the cached one. A refused load
    /// changes nothing but that it is remembered: both budgets are charged
    /// only once it is accepted, and the file is not read again.
    fn load_song(&mut self, path: &str, name: &str) -> Result<PlayableSong, AudioError> {
        if let Some(s) = self.audio.cached_song(path) {
            return Ok(s);
        }
        if let Some(e) = self.audio.refused(path) {
            return Err(e);
        }
        let bytes = self.read_asset(path, name)?;
        match songs::load(
            path,
            &bytes,
            self.audio.song_room(),
            self.audio.samples().room(),
        ) {
            Ok(loaded) => Ok(self.audio.add_song(path, &loaded)),
            Err(e) => Err(self.audio.refuse(path, e)),
        }
    }

    /// Read the cue bank at `path`, or return the cached one, as
    /// [`DrawState::load_song`] does a song.
    fn load_cues(&mut self, path: &str, name: &str) -> Result<Arc<Bank>, AudioError> {
        if let Some(b) = self.audio.cached_cues(path) {
            return Ok(b);
        }
        if let Some(e) = self.audio.refused(path) {
            return Err(e);
        }
        let bytes = self.read_asset(path, name)?;
        match cues::load(
            path,
            &bytes,
            self.audio.song_room(),
            self.audio.samples().room(),
        ) {
            Ok(loaded) => Ok(self.audio.add_cues(path, &loaded)),
            Err(e) => Err(self.audio.refuse(path, e)),
        }
    }

    /// `cue(bank, name, transpose?, gain?, channel?)`: trigger the cue
    /// `name` of `cues/<bank>.omc`. `trigger` is the transposition in
    /// pitch units and the gain (256 is unity) when the cart gave either;
    /// without one the console varies the cue. Returns the first channel
    /// it took, or `None` for a cue that named no channel and was not
    /// played.
    pub fn cue(
        &mut self,
        bank: &str,
        name: &str,
        trigger: Option<Trigger>,
        channel: Option<i64>,
    ) -> Result<Option<usize>, AudioError> {
        let path = cues_path(bank);
        let bank = self.load_cues(&path, bank)?;
        let Some(cue) = bank.cue_named(name) else {
            return Err(Mixer::no_cue(&path, name));
        };
        self.audio.play_cue(&path, &bank, cue, trigger, channel)
    }

    /// `stop(channel, cut?)`: end what holds `channel`, or cut it.
    pub fn stop(&mut self, channel: i64, cut: bool) -> Result<(), AudioError> {
        self.audio.stop(channel, cut)
    }

    /// `sfx(name, channel?)`: play `sfx/<name>.omc`. Returns the first
    /// channel it took, or `None` for an effect that named no channel and
    /// was not played.
    pub fn sfx(&mut self, name: &str, channel: Option<i64>) -> Result<Option<usize>, AudioError> {
        let path = sfx_path(name);
        let song = self.load_song(&path, name)?;
        self.audio.play_effect(&path, &song, channel)
    }

    /// `music(name, fade?, version?)`: play `music/<name>.omc` on the low
    /// channels; `None` fades the current music out over `fade` frames.
    /// With a `version` the arrangement it names plays, and a song that is
    /// the music already carries on in it from where it is.
    pub fn music(
        &mut self,
        name: Option<&str>,
        fade: u32,
        version: Option<Version>,
    ) -> Result<(), AudioError> {
        let Some(name) = name else {
            self.audio.stop_music(fade);
            return Ok(());
        };
        let path = music_path(name);
        let song = self.load_song(&path, name)?;
        let Some(version) = version else {
            self.audio.play_music(&song, fade);
            return Ok(());
        };
        let arrangements = &song.song.arrangements;
        let found = match version {
            Version::Name(n) => arrangements.iter().position(|a| a.name == n),
            Version::Number(i) => usize::try_from(i).ok().filter(|&i| i < arrangements.len()),
        };
        match found {
            Some(a) => {
                self.audio.play_music_version(&song, a, fade);
                Ok(())
            }
            None => Err(Mixer::no_version(&path, &version)),
        }
    }

    /// `sample(name, channel?, pitch?)` with `pitch` in 16.16. Returns the
    /// channel, or `None` for a sample that named none and was not played.
    pub fn sample(
        &mut self,
        name: &str,
        channel: Option<i64>,
        pitch: u32,
    ) -> Result<Option<usize>, AudioError> {
        let sample = self.load_sample(name)?;
        self.audio
            .play_sample(&sample_path(name), sample, channel, pitch)
    }

    /// `music_channels(n?)`: how many of its channels the music keeps to
    /// itself; `None` is all of them.
    pub fn music_channels(&mut self, n: Option<i64>) {
        self.audio.set_music_channels(n);
    }

    /// `volume(channel, v)` with `v` already scaled to 0..=256.
    pub fn volume(&mut self, channel: i64, v: u16) -> Result<(), AudioError> {
        self.audio.set_volume(channel, v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::sample::encode_wav;
    use crate::audio::testsong::{omc, Song};
    use crate::snapshot::{Snapshot, SnapshotLimits};
    use crate::source::CartSource;

    fn state(entries: Vec<(&str, Vec<u8>)>) -> DrawState {
        let cart: Rc<dyn CartSource> =
            Rc::new(Snapshot::from_entries(entries, SnapshotLimits::default()).unwrap());
        DrawState::new(8, 8, cart)
    }

    #[test]
    fn loads_songs_and_samples_by_name_with_caching() {
        let mut d = state(vec![
            ("sfx/hit.omc", omc(&Song::new(2).ticks(6))),
            ("music/song.omc", omc(&Song::new(1).looping())),
            (
                "samples/kick.wav",
                encode_wav(22050, 8, 1, &[0, 255, 0, 255]),
            ),
            ("sfx/bad.omc", b"not a container".to_vec()),
        ]);
        assert_eq!(d.sfx("hit", None).unwrap(), Some(6));
        assert_eq!(d.sfx("hit", Some(1)).unwrap(), Some(1));
        assert!(d.audio.songs().used() > 0);
        assert_eq!(d.sfx("nope", None).unwrap_err().code(), "asset_not_found");
        assert_eq!(d.sfx("../x", None).unwrap_err().code(), "asset_invalid");
        assert_eq!(d.sfx("bad", None).unwrap_err().code(), "song_error");
        d.music(Some("song"), 0, None).unwrap();
        assert!(d.audio.music_playing());
        d.music(None, 0, None).unwrap();
        assert!(!d.audio.music_playing());
        d.audio.stop_all();
        assert_eq!(d.sample("kick", Some(3), 1 << 16).unwrap(), Some(3));
        assert_eq!(d.audio.samples().used(), 4);
        assert_eq!(
            d.sample("kick", Some(9), 1 << 16).unwrap_err().code(),
            "audio_bad_channel"
        );
        d.volume(3, 0).unwrap();
        let out = d.audio.render().to_vec();
        assert!(out.iter().all(|&s| s == 0));
    }
}
