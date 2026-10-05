---
title: Kuula song files
status: current; a short form of Open Module Container 0.8, Open Module Track 0.3 and Open Module Cues 0.2 as Kuula reads them, not their specification
date: 2026-10-04
related:
  - api.md (the calls that play these files, under Audio)
  - skill.md
---

## Appendix: song files

`sfx/<name>.omc` and `music/<name>.omc` each hold one song: an Open
Module Track (OMT) song, version 0.3, inside an Open Module Container
(OMC) file. `cues/<bank>.omc` holds a game's sound effects as one file: a
bank of Open Module Cues (OMQ), version 0.2, in the same container, made
of the same instruments, tracks and cells as a song
([Banks of cues](#banks-of-cues), below). The calls that play them are
under [Audio](api.md#audio) in the cart API reference. A tracker saves
such files, and a script can write them: this page is the part of the
formats Kuula reads, with the limits Kuula adds. It leaves out what Kuula
does not play (other song formats, pictures, rendered music, albums, Opus
samples, the filter) and the exact arithmetic of the sound. Where it is
silent, the formats' own specifications decide.

### The file

All integers are little-endian. The file starts with a 12-byte header:

| offset | size | field |
|---|---|---|
| 0 | 4 | magic `89 4F 4D 43` (`\x89OMC`) |
| 4 | 4 | version 0.8: `00 00 08 00` |
| 8 | 4 | length of the whole file in bytes |

Chunks follow, numbered from 0:

| size | field |
|---|---|
| 4 | length of the data, *n* |
| 4 | type, four ASCII letters |
| *n* | data |
| 0 to 3 | zero bytes, padding the data to a multiple of 4 |
| 4 | CRC-32 of the type and the data (zlib's and PNG's CRC-32) |

| type | contents |
|---|---|
| `JSON` | chunk 0, the manifest: JSON text, at most 1 MiB. `JSNZ` is the same text as one zlib stream |
| `SONG` | the song: JSON text, described below. `SONZ` is the same text as one zlib stream |
| `SMPL` | the bytes of one sample |
| `AUDI` | an audio file a bank's cue plays as it is |

Other chunks are ignored. A chunk whose CRC does not match counts as
missing.

The manifest says which chunk is the song and which hold its samples:

```json
{"songs": [{"chunk": 1, "resources": [{"chunk": 2}], "subsong": 0}]}
```

- Kuula plays the first entry of `songs` that has no `format` member.
  `chunk` is the number of its `SONG` or `SONZ` chunk.
- `resources` lists the chunks of the song's samples. A sample record
  names one by its place in this list, from 0, not by chunk number. Leave
  it out for a song without samples.
- `subsong` picks the arrangement that plays when the call names no
  version; 0 when absent. `music(name, fade, version)` names another.
- A manifest without `songs` (`{}` will do) means `[{"chunk": 1}]` when
  chunk 1 is a `SONG` or `SONZ`. Kuula ignores every other member.

A bank's manifest names the format, and lists the audio files its cues
play, if it has any:

```json
{"songs": [{"chunk": 1, "format": "omq", "resources": [{"chunk": 2}]}],
 "audio": [{"chunk": 3, "role": "cue", "name": "click"}]}
```

- `cue` plays the first entry of `songs` whose `format` is `"omq"`. Its
  `chunk` and `resources` are as a song's; its `subsong` is not read.
- An entry of `audio` with `"role": "cue"` gives the `AUDI` chunk of one
  audio file a `name`, by which a cue asks for it. It may also give
  `loopStart` and `loopEnd`, in frames, and the file then plays round that
  loop until the cue is stopped.

### The song

One JSON object. Every number in it is an integer.

| member | contents |
|---|---|
| `omt` | `"0.3"`; required |
| `rate` | `44100`; required |
| `tick` | `[n, d]`: a tick lasts *n*/*d* seconds, at least one sample; required. `[1, 60]` is one tick a frame |
| `channels` | 1 to 8 channel objects; required |
| `arrangements` | one or more; required |
| `instruments`, `samples`, `tracks` | arrays; empty when absent |
| `volume` | the song's gain, 0 to 1024; 256, the default, is unity |
| `resampling` | absent, or `"nearest"` |
| `title`, `profile`, `ticksPerBeat` | optional, never audible |

A member this table does not name is ignored. So is the value of
`profile`: Kuula holds every song to the limits below whichever profile
it names.

**Versions.** `omt` is the version of the song format, a string, and
Kuula reads exactly `"0.3"`: any other version, or a number in its place,
is refused. The version in the file's header is the container's own, a
separate number: Kuula reads 0.8, and refuses a file whose header gives
any other. A bank has a third, `omq`, of which Kuula reads `"0.2"`.

**Channels.** `{"name", "role", "volume", "pan"}`, all optional: `role`
is `"music"` (the default) or `"reserved"`, `volume` 0 to 64 (64), `pan`
-256 (left) to 256 (right). A reserved channel has no track anywhere.
When the song plays as music, that console channel is left to effects.

**Tracks.** The rows of one channel: `rows` 1 to 1024 and `speed`, ticks
a row, 1 to 255, both required; `loop`, the row to go on from after the
last one; and `cells`, `[[row, "cell"], ...]` with rows strictly
increasing. Rows with nothing in them are left out.

**Arrangements.** `{"name", "orders", "global"}`. A song's arrangements
are versions of one piece, and `name` is what `music(name, fade, version)`
asks for one by; its number, from 0, does too. Each order row is
`{"tracks", "ticks", "next"}`:

- `tracks`: one entry per channel, a track's index or `null`.
- `ticks`: the row's length, 1 to 16 777 216. Every track starts from
  its row 0 at the order row's first tick. A track that ends earlier
  releases its channel, a looping one loops until the order row ends, and
  a longer one is cut off.
- `next`: the order row to go to afterwards, or `"stop"`. Without it the
  following row plays, and after the last row the arrangement ends. A
  `next` that names the same or an earlier row loops.

`global` is optional: `[[order, tick, "event"], ...]`, sorted, where an
event is `tick n d` (a new tick length), `groove w ...` (1 to 16 tick
weights, 1 to 255 each) or `volume g` (0 to 256).

Music whose arrangement loops plays until it is stopped or replaced. An
effect ends when its arrangement has ended and its last voice is silent,
so an effect that loops plays until something cuts it.

Asking for another version of the song that is the music switches to it
at the order row and tick the music is at. Two versions stay on the same
bar and beat when they have the same order rows, each of the same number
of ticks, and the same tick lengths. The switch releases the notes that
are sounding and each part comes in with its next note, so parts written
for it have a note every few rows.

### Cells

A cell is text: `note instrument volume effects`, with one space between
fields.

| field | spelling |
|---|---|
| note | `C-4`, `C#4`: octaves 0 to 9, `C-4` is middle C and `A-4` is 440 Hz; `===` releases the voice, `^^^` cuts it, `~~~` fades it, `...` is no note |
| instrument | `01` to `99`, or `..` |
| volume | `00` to `64`, or `..` |
| effects | a name and its integers, any number of effects, each name once |

Later fields may be left off: `"C-4 01"`, `"==="`. When an effect
follows, all three fields are written, with dots: `"... .. .. vib 32 12"`.
A cell that names no instrument uses the last one named earlier in its
track, and one that gives no volume uses the last volume written earlier
in its track, or else the instrument's own.

Pitch is counted in units of 1/256 of a semitone: `C-4` is 15360.

| effect | does |
|---|---|
| `slide r` | the pitch moves *r* units a tick, -4096 to 4096 |
| `port r` | the pitch moves *r* units a tick towards the cell's note and stops there, 1 to 4096. A note with `port`, `glide` or `fglide` retargets the sounding voice instead of starting one |
| `glide d`, `fglide d` | a line to the note, there after *d* ticks (1 to 255): in pitch, or in frequency |
| `fslide r` | the pitch jumps by *r* once |
| `vib d q` | vibrato: depth *d* units (0 to 4096), period *q* ticks (1 to 256) |
| `vibw w p` | the vibrato's waveform (0 sine, 1 triangle, 2 ramp, 3 square) and phase (0 to 255) |
| `arp x y` | the note, then +*x*, then +*y* semitones, a tick each (0 to 96) |
| `arp4 a b c h` | four notes, 0, *a*, *b*, *c* semitones (-96 to 96), each held *h* ticks |
| `drop d` | the pitch falls to nothing over *d* ticks, then silence to the row's end |
| `vslide r` | the volume moves *r* a tick, -64 to 64 |
| `fvslide r` | the volume jumps by *r* once |
| `vglide v d` | a line to volume *v*, there after *d* ticks |
| `trem d q` | tremolo: depth *d* (0 to 64), period *q* ticks |
| `tremor a b` | sounds for *a* ticks, silent for *b*, round and round |
| `pan p` | the voice's pan, -256 to 256 |
| `pslide r` | the pan moves *r* a tick |
| `panbr d q` | pan sways: depth *d* (0 to 256), period *q* ticks |
| `offset f` | the sample starts at frame *f* |
| `delay d` | the cell happens *d* ticks into its row, 1 to 255 |
| `cut t` | the voice is cut on tick *t* of the row, 0 to 255 |
| `retrig r` | the voice restarts every *r* ticks |
| `retrigv r x` | as `retrig`, each restart changing the volume by rule *x* (0 to 15) |
| `nna x` | what the next note does to this voice: 0 cut, 1 continue, 2 release, 3 fade |
| `past x` | the channel's earlier voices that still sound: 0 cut, 1 release, 2 fade |

### Instruments

| member | contents |
|---|---|
| `number` | 1 to 99, unique; required. Cells name it |
| `engine` | `wave` or `sampler`, below; required |
| `volume` | 0 to 64 (64): the default note volume, and a gain |
| `transpose` | pitch units added to every note, -12288 to 12288 |
| `pan` | -256 to 256: where its voices start, instead of the channel's pan |
| `envelope` | `{"attack", "decay", "release"}` in milliseconds, 0 to 60000, and `"sustain"`, 0 to 64. Without one a note is at full level while held and silent at once when released |

**`wave`**, one oscillator: `{"kind": "wave", "waveform": ...}`.

| member | contents |
|---|---|
| `waveform` | `"pulse"`, `"triangle"`, `"saw"`, `"sine"`, `"noise"`, `"softnoise"` or `"table"`; required |
| `duty` | for `pulse`, 0 to 255 of 256; 128, a square, by default |
| `tables` | 1 to 16 wavetables, each 4, 8, ... or 256 values of -32767 to 32767; `table` plays the first |
| `second` | `{"transpose": t, "level": l}`: a second oscillator *t* pitch units away at level 0 to 64 |
| `nna` | what a new note on the channel does to the sounding one: `"cut"` (default), `"continue"` or `"release"` |
| `sequences` | step sequences that run once a tick from the note's start: `volume` (0 to 64), `arpeggio` (semitones), `pitch` (units), `duty`, `waveform` (0 to 5 for the six named above, 6 + *i* for table *i*). Each is `{"steps": [...], "loop": i, "release": i, "speed": n}` with 1 to 1024 steps |

**`sampler`**: `{"kind": "sampler", "sample": i}` plays sample *i* for
every note. `"keymap": [[low, high, sample, transpose], ...]` gives
ranges of notes (MIDI 12 to 131) their own samples instead. Optional:
`fadeout` (1 to 65536, how fast `~~~` fades), `envelopes` (`volume`,
`pan` and `pitch`, each `{"points": [[tick, value], ...], "loop": [a, b],
"sustain": [a, b]}` with 1 to 32 points), `autovibrato`
(`{"waveform", "depth", "rate", "sweep"}`), `nna` (as above, and
`"fade"`), `dct` and `dca` (the duplicate check).

### Samples

A sample record:

| member | contents |
|---|---|
| `resource` | its place in the manifest entry's `resources`; required |
| `encoding` | `"pcm16"` or `"flac"`; required |
| `rate` | the sample's own rate, 1000 to 384000 Hz; required |
| `channels` | `1`; required |
| `frames` | its length, 1 to 16 777 216; required |
| `root` | the pitch at which it plays at its own rate; 15360, `C-4`, by default |
| `gain` | 0 to 64 (64) |
| `loop` | `{"mode": "forward" or "pingpong", "start": s, "end": e}`, in frames |

A `pcm16` chunk is 16-bit signed little-endian PCM, exactly `frames` x 2
bytes. A `flac` chunk is a whole FLAC stream of 16-bit mono samples,
exactly `frames` long. Bytes that do not match their record are an error.
A resource that names a chunk the file does not have is not: the song
loads and that sample plays nothing.

### Banks of cues

A bank is one JSON object, as a song is, stored the same way. Its
instruments, samples and tracks are a song's, member for member, so what
this page says of them above holds. A bank has no `channels` and no
`arrangements`: it has cues.

| member | contents |
|---|---|
| `omq` | `"0.2"`; required |
| `rate` | `44100`; required |
| `tick` | as a song's; required. One tick length for every cue of the bank |
| `cues` | 1 to 4096 cue objects; required |
| `instruments`, `samples`, `tracks` | as a song's, shared by the cues; empty when absent |
| `volume` | the bank's gain, 0 to 1024; 256 by default |
| `resampling` | absent, or `"nearest"` |
| `title`, `profile` | optional, never audible. `"profile": "kuula"` promises the limits below, and a validator then reports a bank that breaks one; Kuula holds every bank to them whether it says so or not |

A cue is one sound, asked for by name: `cue("fx", "shot")` plays the cue
`shot` of `cues/fx.omc`.

| member | contents |
|---|---|
| `name` | 1 to 255 bytes, unique in the bank; required |
| `tracks` | a tracked cue: 1 to 8 track indexes, played together, one console channel each |
| `audio` | a plain cue: the `name` of an audio file of the manifest, played as it is on one channel |
| `ticks` | a tracked cue's length; its longest track's by default |
| `loop` | `true`: a tracked cue repeats until it is stopped |
| `volume`, `pan` | 0 to 64 (64) and -256 to 256 (0), on every channel of the cue |
| `pitched` | one boolean for each entry of `tracks`: whether that track follows a transposition. All do by default; `false` keeps a drum at its pitch while a tone beside it moves |
| `vary` | `{"transpose": [lo, hi], "gain": [lo, hi]}`: the ranges the console varies the cue in. Pitch units, -12288 to 12288, and 0 to 256, where 256 is unity |

A cue has `tracks` or `audio`, not both. A tracked cue plays its tracks
from their row 0 as one order row of a song does, and ends when its ticks
are over and its last voice is silent.

**A plain cue's file** is a RIFF WAVE of integer PCM, 8 or 16 bits, or a
FLAC stream of 16-bit samples whose STREAMINFO block states its length (a
total of samples that is not 0). Either is mono, at any rate from 1000 to
384000 Hz, and plays at its own rate until it runs out. It counts against
the sample data limit below at 2 bytes a frame, by the length its header
states.

**Triggers.** `cue(bank, name, transpose, gain)` plays the cue *transpose*
semitones up or down and at *gain*, 0 to 1. Called with neither, the cue
is varied: the console draws a number *u* from a fixed sequence, the same
on every run, and the cue plays

    transpose = tlo + (((u & 65535) x (thi - tlo + 1)) >> 16)   pitch units
    gain      = glo + (((u >> 16) x (ghi - glo + 1)) >> 16)

from the two ranges of its `vary`. A cue without `vary` plays as written.

### What Kuula requires

| limit | |
|---|---|
| rate | `rate` is 44100 |
| channels | at most 8; a cue has at most 8 tracks |
| engines | `wave` and `sampler` only |
| samples | mono, `pcm16` or `flac`; no `opus`. A plain cue's file is mono too |
| exact playback | no `filter` in a sampler, no `cutoff` or `reso` effect, and `resampling` absent or `"nearest"` |
| rows | at most 1024 a track |
| sample data | 2 bytes per frame of every sample record and of every audio file a plain cue names: 2 MiB for everything the cart loads, `samples/*.wav` included |
| song data | the JSON text of a song or a bank, inflated: 1 MiB for all the songs and banks a cart loads. The `SONG` and `SONZ` chunks of one file must fit together in what is left of it |
| files | a manifest of at most 1 MiB, a file of at most 16 MiB |

A file that breaks a rule is refused when `sfx`, `music` or `cue` first
asks for it, with a Lua error that names the file and the first reason:

```
song_error: sfx/hit.omc: unknown container version 1.0
song_error: sfx/hit.omc: unknown-version at omt
song_error: sfx/hit.omc: bad-cell at tracks[0].cells[2]
song_error: sfx/hit.omc: no-instrument at tracks[0].cells[0]
song_error: sfx/hit.omc: sample-mismatch at samples[0]
song_error: music/a.omc: breaks the kuula profile: channels
song_error: music/a.omc: breaks the kuula profile: tier
sample_error: music/a.omc: sample budget exceeded: ...
song_error: cues/fx.omc: bad-reference at cues[0].tracks[0]
song_error: cues/fx.omc: breaks the kuula profile: channels
song_error: cues/fx.omc: breaks the kuula profile: stereo-audio
sample_error: cues/fx.omc: cue audio "click" is 4410000 bytes of samples; ...
```

`tier` is the rule on exact playback: for a bank it also means that
every plain cue's file is one of the two kinds above. A refused file stays
refused for the cart's run, and one cue that breaks a rule refuses its
whole bank. A name the bank has no cue for is another error,
`cue_not_found`, and refuses nothing.

### Writing a file

Python's standard library is enough:

```python
import json, struct, zlib

def chunk(kind, data):
    pad = b"\0" * (-len(data) % 4)
    crc = zlib.crc32(kind + data)
    return struct.pack("<I", len(data)) + kind + data + pad + struct.pack("<I", crc)

def omc(song, samples=(), audio=()):
    resources = [{"chunk": 2 + i} for i in range(len(samples))]
    entry = {"chunk": 1, "resources": resources}
    manifest = {"songs": [entry]}
    if "omq" in song:
        entry["format"] = "omq"
        manifest["audio"] = [
            {"chunk": 2 + len(samples) + i, "role": "cue", "name": name}
            for i, (name, _) in enumerate(audio)
        ]
    body = chunk(b"JSON", json.dumps(manifest).encode())
    body += chunk(b"SONG", json.dumps(song).encode())
    for pcm in samples:
        body += chunk(b"SMPL", pcm)
    for _, data in audio:
        body += chunk(b"AUDI", data)
    return b"\x89OMC" + struct.pack("<HHI", 0, 8, 12 + len(body)) + body
```

A sound effect on one channel, three notes of a pulse wave and a
release. Saved as `sfx/blip.omc`, `sfx("blip")` plays it:

```json
{
  "omt": "0.3",
  "rate": 44100,
  "tick": [1, 60],
  "channels": [{}],
  "instruments": [
    {"number": 1, "volume": 48,
     "envelope": {"attack": 0, "decay": 40, "sustain": 24, "release": 30},
     "engine": {"kind": "wave", "waveform": "pulse", "duty": 64}}
  ],
  "tracks": [
    {"rows": 4, "speed": 3,
     "cells": [[0, "C-5 01 64"], [1, "G-5"], [2, "C-6"], [3, "==="]]}
  ],
  "arrangements": [{"orders": [{"tracks": [0], "ticks": 12}]}]
}
```

Music that loops, on two panned channels with a third kept for effects
and one sampled voice. `omc(song, [pcm])` writes it, where `pcm` is
2205 frames of 16-bit mono at 22 050 Hz:

```json
{
  "omt": "0.3",
  "rate": 44100,
  "tick": [1, 50],
  "channels": [{"pan": -96}, {"pan": 96}, {"role": "reserved"}],
  "instruments": [
    {"number": 1, "engine": {"kind": "wave", "waveform": "saw"},
     "envelope": {"decay": 80, "sustain": 40, "release": 60}},
    {"number": 2, "engine": {"kind": "sampler", "sample": 0}}
  ],
  "samples": [
    {"resource": 0, "encoding": "pcm16", "rate": 22050, "channels": 1, "frames": 2205}
  ],
  "tracks": [
    {"rows": 8, "speed": 3, "cells": [[0, "C-2 01 40"], [4, "G-1"], [7, "==="]]},
    {"rows": 8, "speed": 3, "cells": [[0, "C-4 02 48"], [4, "E-4 .. .. vib 32 12"]]}
  ],
  "arrangements": [{"orders": [{"tracks": [0, 1, null], "ticks": 24, "next": 0}]}]
}
```

A bank of three cues. `zap` is a falling pulse the console varies by half
a semitone and a little in level, `crash` is the zap with a burst of noise
that keeps its pitch, and `click` plays a WAVE file.
`omc(bank, audio=[("click", wav)])` writes it, where `wav` is the bytes of
a mono WAVE file. Saved as `cues/fx.omc`, `cue("fx", "zap")` plays the
first:

```json
{
  "omq": "0.2",
  "profile": "kuula",
  "rate": 44100,
  "tick": [1, 120],
  "instruments": [
    {"number": 1, "volume": 40,
     "envelope": {"attack": 0, "decay": 90, "sustain": 0, "release": 20},
     "engine": {"kind": "wave", "waveform": "pulse", "duty": 80,
                "sequences": {"pitch": {"steps": [0, -400, -900, -1500, -2200, -3000]}}}},
    {"number": 2, "volume": 44,
     "envelope": {"attack": 0, "decay": 120, "sustain": 0, "release": 20},
     "engine": {"kind": "wave", "waveform": "noise"}}
  ],
  "tracks": [
    {"rows": 2, "speed": 5, "cells": [[0, "C-6 01"], [1, "G-5"]]},
    {"rows": 1, "speed": 12, "cells": [[0, "C-3 02"]]}
  ],
  "cues": [
    {"name": "zap", "tracks": [0],
     "vary": {"transpose": [-128, 128], "gain": [192, 256]}},
    {"name": "crash", "tracks": [0, 1], "pitched": [true, false]},
    {"name": "click", "audio": "click", "pan": 128}
  ]
}
```
