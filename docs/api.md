---
title: Kuula cart API reference
status: current; the bindings in `crates/kuula-lua/src` are the source of truth
version: 0.0.6
date: 2026-09-10
related:
  - skill.md (constraints and idioms; read it first)
---

# Kuula cart API

A cart is a directory (or a zip built from one) holding `main.lua`, an
optional `cart.toml`, and assets under `gfx/`, `map/`, `src/`, `sfx/`,
`music/` and `samples/`. The cart runs Lua 5.5 in a sandbox at a fixed
60 Hz. Every call below is a global; there are no modules to require for
the API itself.

Conventions used here:

- Numbers passed as coordinates are floored towards negative infinity
  (`-0.5` is `-1`) and saturate to the 32-bit range.
- A *colour* argument is an integer: the low 7 bits (0 to 127) are the
  primary colour; bits 8 to 14 optionally carry a secondary colour for
  `fillp`. `0x0807` means primary 7, secondary 8.
- Prices are in *cycles*. A frame has a budget of 279,620 cycles;
  the main chunk plus `_init` together have 60 times that. Every call
  costs at least 1 cycle; Lua VM instructions cost 2 each, counted in
  blocks of 1000. Going over the budget ends the cart with
  `budget_exceeded`.
- Errors come in two kinds. A **fault** ends the cart and has a stable
  code, a file and a line. A **graphics error** is an ordinary Lua error
  raised from a call (catchable with `pcall`); if uncaught it becomes a
  `runtime_error` fault whose message starts with the graphics code.

## Callbacks

The cart defines any of these globals; each is optional.

| callback | when | notes |
|---|---|---|
| main chunk | once, frame 1 | runs before `_init`; shares the init budget |
| `_init()` | once, frame 1, after the chunk | load assets, set state |
| `_update(dt)` | every frame from frame 2 | `dt` is always `1/60` (`0.016666...`); never variable |
| `_draw()` | every frame from frame 2, after `_update` | draw the screen |

Frame 1 runs the chunk and `_init` only. There is no way to skip a
frame, run faster than 60 Hz, or read a clock: the only time a cart has
is the frame counter (`stat("frame")`) and the number of `_update`
calls it has counted itself.

## Screen and palette

The screen is a `u8` buffer of colour indices, 640x480 by default or
320x240 when the manifest says so. The palette has 128 RGB entries.
Entries 0 to 15 are the system colours (Sweetie-16 order: 0 black, 1
dark blue, 2 dark purple, 3 dark green, 4 brown, 5 dark grey, 6 light
grey, 7 white, 8 red, 9 orange, 10 yellow, 11 green, 12 blue, 13
lavender, 14 pink, 15 peach) and are locked. Entries 16 to 127 default
to a 4x4x7 RGB ramp and may be changed with `pal`. A pixel value above
127 is displayed masked to 7 bits.

## Drawing

Drawing goes to the current *draw target* (the screen unless
`draw_target` says otherwise), through the camera offset, the clip
rectangle, the fill pattern and the colour table (`palt`, `pal_map`).
Pixels touched are counted after clipping; that count is what you pay.

| call | returns | cycles |
|---|---|---|
| `cls([c])` | nothing | 1 + clip pixels / 64 |
| `pset(x, y, [c])` | nothing | 1 + touched / 3 |
| `pget(x, y)` | colour index at `(x, y)` of the target in raw target coordinates (no camera offset), 0 outside | 1 |
| `line(x0, y0, x1, y1, [c])` | nothing | 1 + touched / 3 |
| `rect(x0, y0, x1, y1, [c])` | nothing (outline, inclusive corners) | 1 + touched / 3 |
| `rectfill(x0, y0, x1, y1, [c])` | nothing | 1 + touched / 3 |
| `circ(x, y, r, [c])` | nothing (outline) | 1 + max(touched, perimeter) / 3 |
| `circfill(x, y, r, [c])` | nothing | 1 + max(touched, perimeter) / 3 |
| `print(text, x, y, [c])` | x after the last glyph | characters + touched / 3 |
| `clip(x, y, w, h)` / `clip()` | nothing; `clip()` resets to the whole target | 1 |
| `camera([x, y])` | nothing; `camera()` resets to `(0, 0)` | 1 |
| `fillp([pattern, transparent])` | nothing | 1 |

- `c` defaults to 7 (white) for shapes and text, 0 for `cls`.
- `print` in its drawing form needs numeric `x` and `y`. The system
  font is 4x6 pixels per glyph; text ignores `fillp`. Any other call
  shape, such as `print("x =", x)`, is the *logging* form below.
- `fillp(p, transparent)`: `p` is a 16-bit 4x4 dither; bit
  `(y % 4) * 4 + (x % 4)` set selects the secondary colour, or skips the
  pixel when `transparent` is true. `fillp()` clears it. Circles and
  lines honour it; text does not.
- Circles cost by their radius even when mostly clipped; do not draw
  huge circles off screen.

### Palette calls

| call | effect | cycles |
|---|---|---|
| `pal(i, r, g, b)` / `pal(i, 0xRRGGBB)` | set palette entry `i` (16 to 127) | 1 |
| `pal()` | restore the default palette | 1 |
| `palt(i, [transparent])` | source colour `i` is skipped when blitting (`transparent` defaults to true) | 1 |
| `palt()` | clear all transparency | 1 |
| `pal_map(from, to)` | draw colour `from` as `to` (draw-time remap) | 1 |
| `pal_map()` | clear the remap | 1 |
| `pal_reset()` | clear transparency and remap | 1 |

`pal` raises `palette_index_out_of_range` for `i >= 128` and
`palette_index_locked` for `i < 16`. Transparency and remap are part of
the draw state, not the palette; `pal()` does not clear them.

### Sprites and maps

Sprites are read from the current *sheet*, a `u8` buffer selected with
`sheet(b)`. Cell `n` of a sheet is the 8x8 cell at column `n % (width /
8)`, row `n / (width / 8)`; a cell below the sheet draws nothing.

| call | effect | cycles |
|---|---|---|
| `sheet(b)` / `sheet()` | select `b` as the sheet; `sheet()` clears it | 1 |
| `spr(n, x, y, [w, h, flip_x, flip_y])` | draw `w` by `h` cells (default 1x1) from cell `n` | 1 + touched / 3 |
| `sspr(sx, sy, sw, sh, dx, dy, [dw, dh, flip_x, flip_y])` | scaled copy of a sheet rectangle; `dw, dh` default to `sw, sh` | 1 + touched / 3 |
| `map(m, cx, cy, sx, sy, cw, ch, [layer])` | draw `cw` by `ch` cells of map `m` from cell `(cx, cy)` with its top-left at `(sx, sy)`, through the sheet | 2 per cell requested + touched / 3 |

`spr`, `sspr` and `map` raise `no_sheet` without a sheet. Map cells are
sprite numbers into the current sheet; a negative cell draws nothing. A
layer outside the map's layers draws nothing. `map` raises `not_a_map`
when `m` was not made by `load_map`.

## Buffers

Every image the cart owns is a *buffer*: a rectangle of typed elements
in the console's graphics memory (8 MiB total, `stat("gfx_limit")`).
A Lua value of type `buf` is a handle; the bytes live in the console.
Releasing a handle to the collector or calling `b:release()` frees the
buffer. The `screen` global is the screen's handle and cannot be
released.

| call | returns | cycles |
|---|---|---|
| `buf(kind, w, h)` | a new zeroed buffer; `kind` is `"u8"`, `"i16"`, `"i32"` or `"f32"`; `w, h` in 1..=4096 | 1 + bytes / 128 |
| `load_sheet(name)` | the `u8` buffer of `gfx/<name>.png`, decoded once and cached by name | 1 + decoded bytes / 8; 1 when already live |
| `load_map(name)` | the `i16` map buffer of `map/<name>.json` | same |
| `draw_target(b)` / `draw_target()` | draw into `b` (a `u8` buffer) or back to the screen; resets the clip | 1 |
| `screen` | the screen buffer handle (a value, not a call) | |

Methods on a buffer `b`:

| method | returns | cycles |
|---|---|---|
| `b:get(x, y)` | the element, 0 outside; an integer for integer kinds, a float for `f32` | 1 |
| `b:set(x, y, v)` | nothing; ignored outside | 1 |
| `b:fill(v)` | nothing | 1 + bytes / 128 |
| `b:width()`, `b:height()` | size in elements; for a map, `height` is rows per layer | 1 |
| `b:kind()` | `"u8"`, `"i16"`, `"i32"` or `"f32"` | 1 |
| `b:layers()` | map layers (1 for a plain buffer) | 1 |
| `b:tile_size()` | 8 or 16 for a map, 0 otherwise | 1 |
| `b:copy(src, sx, sy, w, h, dx, dy)` | nothing; raw copy of same-kind elements, no clip, no colour table | 1 + bytes / 128 |
| `b:blit(src, sx, sy, w, h, dx, dy)` | like `copy` but honours the clip when `b` is the draw target | 1 + bytes / 128 |
| `b:release()` | nothing; frees the buffer now | 1 |
| `tostring(b)` | `buf(u8 64x64)` or `buf(released)` | |

Buffer errors (Lua errors, catchable): `buf_bad_dimensions`,
`buf_released` (a handle whose buffer is gone), `buf_protected`
(releasing the screen), `buf_aliased` (`copy` with `src == b`),
`buf_kind_mismatch` (kinds differ, or a non-`u8` sheet or target),
`graphics_budget_exceeded` (the 8 MiB is full even after a collection;
release something).

Sheets are PNGs of at most 1024x1024: indexed or greyscale PNGs are read
as indices directly; RGB(A) PNGs are matched against the default palette
and any other colour is `asset_invalid`. Maps are JSON:

```json
{"tile_size": 8, "width": 16, "height": 12,
 "layers": [[0, 0, 1, ...], [...]]}
```

`width` and `height` are at most 256 cells, 1 to 4 layers, each layer
`width * height` sprite numbers row-major, `-1` for empty. Asset errors:
`asset_not_found`, `asset_invalid`, `asset_too_large`.

## Input

Six logical buttons: 0 up, 1 down, 2 left, 3 right, 4 A, 5 B. On the
desktop host the arrows, Z (A) and X (B) drive them. The two-button tier
means a cart should be playable with the D-pad plus A and B; there are
no other buttons.

| call | returns | cycles |
|---|---|---|
| `btn(n)` | `true` while button `n` is held this frame; `false` for any other `n` | 1 |

There is no `btnp`; keep last frame's state yourself to detect presses.

## Logging and stats

| call | effect | cycles |
|---|---|---|
| `print(...)` (non-drawing form) | stringifies the arguments, joins them with a tab, appends the line to this frame's log | 1 + bytes / 8 |
| `stat(name)` | a number, see below | 1 |

The log holds 256 lines of at most 1024 bytes per frame; more lines are
dropped with a marker line. Hosts show the log on stderr, in
`run.json`, and through the MCP `logs` tool.

`stat` names: `"cpu"` (cycles used this frame divided by the budget, a
float that can pass 1 on frame 1), `"cpu_cycles"`, `"cpu_budget"`,
`"mem"` (Lua heap bytes), `"mem_limit"` (16 MiB), `"gfx_mem"`,
`"gfx_limit"` (8 MiB), `"frame"` (the frame being run, 1-based),
`"width"` and `"height"` (the screen size the manifest chose). Any
other name is a Lua error.

## Modules

`require(name)` loads `src/<name with . as />.lua` from the cart, once,
and returns what it returned (or `true`). Names are letters, digits and
`_` joined by `.`; at most 256 modules; a cycle is an error. Price: 1 +
source bytes / 4 on first load, 1 afterwards. Errors: `module_name_invalid`,
`module_not_found`, `require_cycle`, `require_limit`, `module_not_utf8`.

The Lua standard library available: `math` (with `math.random` seeded
to a fixed value every run; `math.randomseed()` with no argument reseeds
to the same fixed value), `string` (without `dump`), `table`, `utf8`,
`coroutine`, plus `pcall`, `error`, `assert`, `type`, `tostring`,
`tonumber`, `pairs`, `ipairs`, `next`, `select`, `rawget`, `rawset`,
`rawequal`, `rawlen`, `setmetatable`, `getmetatable`, `collectgarbage`
(`"count"`, `"collect"`, `"step"` only) and `xpcall`. Removed: `os`,
`io`, `package`, `debug`, `dofile`, `loadfile`, `loadstring` and
`string.dump`; the global `load` is the save-slot binding below, not
Lua's chunk loader. A metatable with `__gc` is refused (use `__close`).

Priced standard functions (each also costs the usual 1):

| call | cycles |
|---|---|
| `string.rep`, `string.format`, `table.concat` | result bytes / 8 |
| `string.find`, `match`, `gmatch`, `gsub` | pattern bytes x subject bytes ^ (backtracking quantifiers + 1 if unanchored) / 256; `gsub` also result bytes / 8 |
| `string.sub`, `upper`, `lower`, `reverse`, `byte`, `char`, `utf8.*` | bytes or values / 8 |
| `table.insert`, `remove`, `move`, `unpack` | elements shifted or copied / 8 |
| `table.sort` | n log2 n |
| `coroutine.create`, `coroutine.wrap` | 2000 |
| `collectgarbage("collect")` | heap bytes / 128 |

Patterns with several `.-`, `.*` or `.+` on long subjects are priced by
their worst case and can exceed the budget by themselves; anchor them or
split the work.

## Audio

Eight channels, 44.1 kHz mono, rendered per frame. Sounds come from
`sfx/*.trk` and `music/*.trk` (tracker text) and `samples/*.wav` (8 or
16-bit mono PCM, 2 MiB in total). A name is the file's stem, as for
`load_sheet`. Audio rendering is not charged; each call costs 1 cycle.

| call | returns |
|---|---|
| `sfx(name, [channel])` | the channel it plays on; `channel` defaults to a free one |
| `music(name, [fade])` | nothing; starts the track, fading over `fade` frames; `music()` or `music(nil, fade)` stops it |
| `sample(name, [channel, pitch])` | the channel; `pitch` 1.0 is native, clamped to 1/256 to 16 |
| `volume(channel, v)` | nothing; channel gain 0.0 to 1.0, clamped |

Audio errors are Lua errors: `asset_not_found` and `asset_invalid` for
the file, `track_error` and `sample_error` for its contents,
`audio_bad_channel` for a channel outside 0 to 7, and `audio_no_room`
when a track has more columns than fit from the channel asked for.

## Numeric profile

Arithmetic is the same on every host the console runs on, not only the
same from run to run. `math.sin`, `cos`, `tan`, `asin`, `acos`, `atan`,
`exp`, `log` (with its optional base) and the float `^` operator go
through one library (the `libm` crate, pinned in `kuula-lua`) on every
platform, so a cart that uses them draws the same pixels on the desktop
and on the device, and the conformance hashes depend on them. `sqrt`,
`fmod`, `floor`, `ceil`, `modf`, `abs`, the integer functions and
`math.random` are Lua's own: those are exact operations that every
library agrees on, or the console's own generator. `%g`, `%f` and `%e`
formatting and `tostring` are correctly rounded everywhere.

Every NaN prints as `nan` (`tostring(0/0)`, `..`, `string.format`), with
`NAN` for an upper-case conversion, padded to the format's width. The
sign and payload of a NaN are not part of the guarantee: `string.pack`
of a NaN may differ between hosts, and a cart that needs a stable value
should test for NaN (`x ~= x`) rather than pack it.

`examples/numeric` is the regression corpus and its `expected.txt` the
pinned output.

## Saves

Eight slots of 256 KiB each, stored through the canonical codec: nil,
booleans, integers, floats, strings, tables and buffers (`buf` bytes)
only. Functions, coroutines and cycles are errors, as are tables deeper
than 32 levels or with more than 65,536 entries. Headless and MCP runs
use an in-memory store that is discarded at the end of the run. On the
desktop (windowed runs and the shell) the cart sees memory semantics
too: its slots are read from disk when the cart starts and every `save`
is written through afterwards, so the only errors `save` can raise are
the slot and size checks. A disk failure is reported on the host's
stderr and does not reach the cart, which keeps save results the same
in a recording and its replay; a slot file that could not be read when
the cart started is left untouched on disk.

| call | returns | cycles |
|---|---|---|
| `save(slot, table)` | nothing; slot 0 to 7 | 64 + value bytes / 8 + encoded bytes / 8 |
| `load(slot)` | the table, or `nil` when the slot is empty | 64 + stored bytes / 8 |

Save errors are Lua errors: `save_slot` (not 0 to 7), `save_size` (over
256 KiB encoded), and the codec's `codec_unsupported`, `codec_cycle`,
`codec_depth`, `codec_size`, `codec_key` and `codec_number`.

## `cart.toml`

Every key is optional; unknown keys are a `manifest_error` with a line.

```toml
[cart]
title = "My cart"          # shown by the host; untrusted text elsewhere
screen_mode = "640x480"    # or "320x240"; default 640x480

[preload]
sheets = ["tiles", "hero"] # gfx/tiles.png, gfx/hero.png decoded at boot
maps = ["overworld"]       # map/overworld.json decoded at boot
```

Preloaded assets are live before `_init`, so `load_sheet("tiles")`
costs 1 cycle. Names are one path component of letters, digits, `_` and
`-`, no extension. A missing or undecodable preload is a fault at boot
(`asset_not_found`, `asset_invalid`, `asset_too_large`).

Cart limits: at most 4096 files, 16 MiB per file, 64 MiB in total; only
`main.lua`, `cart.toml` and files under `gfx/`, `map/`, `src/`, `sfx/`,
`music/` and `samples/` are read.

## Input scripts

Headless runs (`kuula run --input script.json`, `kuula screenshot
--input`) and the MCP `step` tool take a JSON list of runs; each run
holds some buttons for some frames, in order:

```json
[
  {"frames": 30, "buttons": ["right"]},
  {"frames": 1,  "buttons": ["a", "right"]},
  {"frames": 10}
]
```

`frames` defaults to 1, `buttons` to none; names are `up`, `down`,
`left`, `right`, `a`, `b` (case-insensitive). Frames past the script's
end get no input. A script may expand to at most 1,000,000 frames. The
MCP `input` tool takes the same thing as raw masks instead: 1 up, 2
down, 4 left, 8 right, 16 A, 32 B, or-ed together per frame.

## Transcripts

`kuula run <cart> --record out.kr`, windowed or headless, writes a
transcript of the run: the cart's content digest, the runtime version,
the seed, the save slots as the cart found them, and the input the cart
saw on every frame. `kuula run <cart> --headless --replay out.kr` steps
the cart with exactly those inputs from exactly those saves, in an
isolated in-memory store that never touches the player's slots, and
with `--out` writes the same hashes the recorded run produced. A
transcript from a different cart is refused (`transcript_cart_mismatch`)
unless `--replay-any-cart` is given, in which case the result verifies
nothing. A restart from the error screen while recording ends the
transcript at the restart.

The file is `kuula-transcript 1` on its first line, then one canonical
codec document per line: the header, one chunk record
per 96 KiB of each non-empty save slot, then one `{ buttons, frames }`
record per run of identical inputs. Bounds: 1,000,000 frames, 256 KiB
per line, 64 MiB per file. A truncated file, an unknown format version,
a record carrying the reserved `messages`, `io` or `connections` fields,
a run past the header's frame count or a button mask above 63 is
refused with `transcript_format`, `transcript_version` or
`transcript_size`.

## Windowed runs and the worker

`kuula run <cart>` and the shell run every cart in a worker process
under the OS sandbox; `--in-process` runs it in the host process
instead, and `--no-sandbox` launches the worker plainly for debugging.
Headless runs are in-process unless `--worker`. A worker that dies,
stalls or misbehaves ends the cart with `worker_error`,
`watchdog_timeout` or `sandbox_unavailable` on the error screen, and the
window stays responsive while it waits.

The window is 640x480 times the scale (`--scale 1` to `4`, default 2,
Ctrl+1 to Ctrl+4 at runtime, and the shell's settings screen) whatever
the cart's screen mode: a 320x240 cart is drawn at twice the scale, so
switching carts never resizes the window.

## Faults

A fault ends the cart; the screen keeps the last frame. Every fault is
`{code, file, line, message}`; `line` may be absent.

| code | meaning |
|---|---|
| `compile_error` | `main.lua` or a module did not parse |
| `runtime_error` | an uncaught Lua error, including uncaught graphics errors (the message starts with the graphics code) and `bad argument` errors from the API |
| `budget_exceeded` | a callback spent its cycles; message `<callback> used <n> of <budget> cycles` |
| `out_of_memory` | the Lua heap passed 16 MiB after a full collection, or 24 MiB inside a call |
| `cart_read_error` | `main.lua` missing, unreadable or not UTF-8 |
| `manifest_error` | `cart.toml` did not parse or has an unknown key |
| `asset_not_found`, `asset_invalid`, `asset_too_large` | a preload failed |
| `watchdog_timeout` | the worker process stopped answering (host side only) |
| `worker_error` | the worker process died or broke protocol (host side only) |
| `sandbox_unavailable` | the worker's sandbox could not be set up, so the run was refused (host side only) |

Graphics and module errors are Lua errors with these codes in their
message: `graphics_budget_exceeded`, `buf_bad_dimensions`,
`buf_released`, `buf_protected`, `buf_aliased`, `buf_kind_mismatch`,
`no_sheet`, `not_a_map`, `palette_index_out_of_range`,
`palette_index_locked`, `asset_not_found`, `asset_invalid`,
`asset_too_large`, `module_name_invalid`, `module_not_found`,
`require_cycle`, `require_limit`, `module_not_utf8`, the audio codes
above and the save codes above.

## Tool-side errors

The CLI and the MCP server add their own codes, which never come from
the cart: `stale_handle`, `stale_frame`, `too_many_consoles`,
`too_many_inputs`, `cart_not_found`, `path_outside_root`,
`invalid_arguments`, `invalid_input_script`, `invalid_transcript`,
`transcript_cart_mismatch`, `transcript_error`, `result_too_large`,
`unknown_tool` and `unsupported` (the guest cannot answer).
