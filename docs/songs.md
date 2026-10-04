---
title: Kuula song files
status: current; a short form of Open Module Container 0.7 and Open Module Track 0.3 as Kuula reads them, not their specification
date: 2026-10-04
related:
  - api.md (the calls that play these files, under Audio)
  - skill.md
---

## Appendix: song files

`sfx/<name>.omc` and `music/<name>.omc` each hold one song: an Open
Module Track (OMT) song, version 0.3, inside an Open Module Container
(OMC) file. The calls that play them are under [Audio](api.md#audio) in
the cart API reference. A tracker saves such files, and a script can
write them: this page is the part of the two formats Kuula reads, with
the limits Kuula adds. It leaves out what Kuula does not play (other song
formats, pictures, rendered audio, albums, Opus samples, the filter) and
the exact arithmetic of the sound. Where it is silent, the formats' own
specifications decide.

### The file

All integers are little-endian. The file starts with a 12-byte header:

| offset | size | field |
|---|---|---|
| 0 | 4 | magic `89 4F 4D 43` (`\x89OMC`) |
| 4 | 4 | version 0.7: `00 00 07 00` |
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
- `subsong` picks the arrangement that plays; 0 when absent.
- A manifest without `songs` (`{}` will do) means `[{"chunk": 1}]` when
  chunk 1 is a `SONG` or `SONZ`. Kuula ignores every other member.

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
separate number: Kuula reads 0.7, and refuses a file whose header gives
any other.

**Channels.** `{"name", "role", "volume", "pan"}`, all optional: `role`
is `"music"` (the default) or `"reserved"`, `volume` 0 to 64 (64), `pan`
-256 (left) to 256 (right). A reserved channel has no track anywhere.
When the song plays as music, that console channel is left to effects.

**Tracks.** The rows of one channel: `rows` 1 to 1024 and `speed`, ticks
a row, 1 to 255, both required; `loop`, the row to go on from after the
last one; and `cells`, `[[row, "cell"], ...]` with rows strictly
increasing. Rows with nothing in them are left out.

**Arrangements.** `{"name", "orders", "global"}`. Each order row is
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

### What Kuula requires

| limit | |
|---|---|
| rate | `rate` is 44100 |
| channels | at most 8 |
| engines | `wave` and `sampler` only |
| samples | mono, `pcm16` or `flac`; no `opus` |
| exact playback | no `filter` in a sampler, no `cutoff` or `reso` effect, and `resampling` absent or `"nearest"` |
| rows | at most 1024 a track |
| sample data | 2 bytes per frame of every sample record: 2 MiB for everything the cart loads, `samples/*.wav` included |
| song data | the song's JSON text, inflated: 1 MiB for all the songs a cart loads. The `SONG` and `SONZ` chunks of one file must fit together in what is left of it |
| files | a manifest of at most 1 MiB, a file of at most 16 MiB |

A file that breaks a rule is refused when `sfx` or `music` first asks for
it, with a Lua error that names the file and the first reason:

```
song_error: sfx/hit.omc: unknown container version 1.0
song_error: sfx/hit.omc: unknown-version at omt
song_error: sfx/hit.omc: bad-cell at tracks[0].cells[2]
song_error: sfx/hit.omc: no-instrument at tracks[0].cells[0]
song_error: sfx/hit.omc: sample-mismatch at samples[0]
song_error: music/a.omc: breaks the kuula profile: channels
song_error: music/a.omc: breaks the kuula profile: tier
sample_error: music/a.omc: sample budget exceeded: ...
```

`tier` is the rule on exact playback. A refused file stays refused for
the cart's run.

### Writing a file

Python's standard library is enough:

```python
import json, struct, zlib

def chunk(kind, data):
    pad = b"\0" * (-len(data) % 4)
    crc = zlib.crc32(kind + data)
    return struct.pack("<I", len(data)) + kind + data + pad + struct.pack("<I", crc)

def omc(song, samples=()):
    resources = [{"chunk": 2 + i} for i in range(len(samples))]
    manifest = {"songs": [{"chunk": 1, "resources": resources}]}
    body = chunk(b"JSON", json.dumps(manifest).encode())
    body += chunk(b"SONG", json.dumps(song).encode())
    for pcm in samples:
        body += chunk(b"SMPL", pcm)
    return b"\x89OMC" + struct.pack("<HHI", 0, 7, 12 + len(body)) + body
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
