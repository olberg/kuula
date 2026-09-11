---
title: Kuula cart API reference
status: current; the reference tables are generated from the binding descriptors in `crates/kuula-lua/src` (`cargo run -p kuula-apidoc -- --check`)
version: 0.0.7
date: 2026-09-11
related:
  - skill.md (constraints and idioms; read it first)
---

# Kuula cart API

A cart is a directory (or a zip built from one) holding `main.lua`, an
optional `cart.toml`, and assets under `gfx/`, `map/`, `src/`, `sfx/`,
`music/` and `samples/`. The cart runs Lua 5.5 in a sandbox at a fixed
60 Hz. Every call below is a global; there are no modules to require for
the API itself.

The tables between `<!-- generated -->` markers are produced from the
descriptors declared beside each binding; edit those, not the tables,
and run `cargo run -p kuula-apidoc -- --write`. The prose around them
is written by hand.

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
  The `Errors:` list under a table names the Lua errors that call can
  raise.

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

<!-- generated: drawing -->
| call | returns | cycles |
|---|---|---|
| `cls([c])` | nothing | 1 + clip pixels / 64 |
| `pset(x, y, [c])` | nothing | 1 + touched / 3 |
| `pget(x, y)` | colour index at `(x, y)` of the target in raw target coordinates (no camera offset), 0 outside | 1 |
| `line(x0, y0, x1, y1, [c])` | nothing | 1 + touched / 3 |
| `rect(x0, y0, x1, y1, [c])` | nothing (outline, inclusive corners) | 1 + touched / 3 |
| `rectfill(x0, y0, x1, y1, [c])` | nothing | 1 + touched / 3 |
| `circ(x, y, r, [c])` | nothing (outline) | 1 + max(touched, 6 r) / 3 |
| `circfill(x, y, r, [c])` | nothing | 1 + max(touched, 6 r) / 3 |
| `print(text, x, y, [c])` | x after the last glyph | characters + touched / 3 |
| `clip(x, y, w, h)` | nothing | 1 |
| `clip()` | nothing; resets to the whole target | 1 |
| `camera([x, y])` | nothing; `camera()` resets to `(0, 0)` | 1 |
| `fillp([pattern, transparent])` | nothing | 1 |

- `cls`: Fills the clip rectangle of the draw target. Defaults: `c` = 0.
- `pset`: Defaults: `c` = 7.
- `line`: Defaults: `c` = 7.
- `rect`: Defaults: `c` = 7.
- `rectfill`: Defaults: `c` = 7.
- `circ`: Circles cost by their radius even when mostly clipped; do not
  draw huge circles off screen. Defaults: `c` = 7.
- `circfill`: Priced like `circ`: by the radius even when mostly
  clipped. Defaults: `c` = 7.
- `print`: The drawing form needs numeric `x` and `y`. The system font
  is 4x6 pixels per glyph; text ignores `fillp`. Any other call shape,
  such as `print("x =", x)`, is the logging form. Defaults: `c` = 7.
- `clip`: Any other number of arguments is a Lua error.
- `camera`: Defaults: `x` = 0, `y` = 0.
- `fillp`: `pattern` is a 16-bit 4x4 dither; bit `(y % 4) * 4 + (x % 4)`
  set selects the secondary colour, or skips the pixel when
  `transparent` is true. `fillp()` clears it. Circles and lines honour
  it; text does not. Defaults: `pattern` = 0, `transparent` = false.
<!-- /generated -->

### Palette calls

<!-- generated: palette -->
| call | returns | cycles |
|---|---|---|
| `pal(i, r, g, b)` | nothing; sets palette entry `i` (16 to 127) | 1 |
| `pal(i, 0xRRGGBB)` | nothing; the same from one number | 1 |
| `pal()` | nothing; restores the default palette | 1 |
| `palt(i, [transparent])` | nothing; source colour `i` is skipped when blitting | 1 |
| `palt()` | nothing; clears all transparency | 1 |
| `pal_map(from, to)` | nothing; draws colour `from` as `to` (draw-time remap) | 1 |
| `pal_map()` | nothing; clears the remap | 1 |
| `pal_reset()` | nothing; clears transparency and remap | 1 |

- `pal`: `i >= 128` is out of range and `i < 16` is locked. Channels
  clamp to 0 to 255; any other argument shape is a Lua error.
  Transparency and remap are part of the draw state, not the palette;
  `pal()` does not clear them. Errors: `palette_index_out_of_range`,
  `palette_index_locked`.
- `palt`: Defaults: `transparent` = true.
<!-- /generated -->

### Sprites and maps

Sprites are read from the current *sheet*, a `u8` buffer selected with
`sheet(b)`. Cell `n` of a sheet is the 8x8 cell at column `n % (width /
8)`, row `n / (width / 8)`.

<!-- generated: sprites -->
| call | returns | cycles |
|---|---|---|
| `sheet(b)` | nothing; selects the `u8` buffer `b` as the sheet | 1 |
| `sheet()` | nothing; clears the sheet | 1 |
| `spr(n, x, y, [w, h, flip_x, flip_y])` | nothing; draws `w` by `h` cells from cell `n` | 1 + touched / 3 |
| `sspr(sx, sy, sw, sh, dx, dy, [dw, dh, flip_x, flip_y])` | nothing; scaled copy of a sheet rectangle | 1 + touched / 3 |
| `map(m, cx, cy, sx, sy, cw, ch, [layer])` | nothing; draws `cw` by `ch` cells of map `m` from cell `(cx, cy)` with its top-left at `(sx, sy)`, through the sheet | 2 per cell requested + 1 + touched / 3 |

- `sheet`: Errors: `buf_kind_mismatch`, `buf_released`.
- `spr`: A cell below the sheet draws nothing. Defaults: `w` = 1, `h` =
  1, `flip_x` = false, `flip_y` = false. Errors: `no_sheet`.
- `sspr`: Defaults: `dw` = sw, `dh` = sh, `flip_x` = false, `flip_y` =
  false. Errors: `no_sheet`.
- `map`: Map cells are sprite numbers into the current sheet; a negative
  cell draws nothing. A layer outside the map's layers draws nothing.
  `not_a_map` when `m` was not made by `load_map`. Defaults: `layer` =
  0. Errors: `no_sheet`, `not_a_map`, `buf_released`.
<!-- /generated -->

## Buffers

Every image the cart owns is a *buffer*: a rectangle of typed elements
in the console's graphics memory (8 MiB total, `stat("gfx_limit")`).
A Lua value of type `buf` is a handle; the bytes live in the console.
Releasing a handle to the collector or calling `b:release()` frees the
buffer. The `screen` global is the screen's handle and cannot be
released.

<!-- generated: buffers -->
| call | returns | cycles |
|---|---|---|
| `screen` | the screen buffer handle (a value, not a call) |  |
| `buf(kind, w, h)` | a new zeroed buffer; `kind` is `"u8"`, `"i16"`, `"i32"` or `"f32"`; `w, h` in 1..=4096 | 2 + bytes / 128 |
| `load_sheet(name)` | the `u8` buffer of `gfx/<name>.png`, decoded once and cached by name | 2 + decoded bytes / 8; 1 when already live |
| `load_map(name)` | the `i16` map buffer of `map/<name>.json`, decoded once and cached by name | 2 + decoded bytes / 8; 1 when already live |
| `draw_target(b)` | nothing; draws into `b` (a `u8` buffer) and resets the clip | 1 |
| `draw_target()` | nothing; draws to the screen again | 1 |

- `screen`: Cannot be released.
- `buf`: An unknown kind is a Lua error. Priced after the ledger
  accepted the request, so a refused buffer costs nothing. Errors:
  `buf_bad_dimensions`, `graphics_budget_exceeded`.
- `load_sheet`: Errors: `asset_not_found`, `asset_invalid`,
  `asset_too_large`, `graphics_budget_exceeded`.
- `load_map`: Errors: `asset_not_found`, `asset_invalid`,
  `asset_too_large`, `graphics_budget_exceeded`.
- `draw_target`: Errors: `buf_kind_mismatch`, `buf_released`.
<!-- /generated -->

Methods on a buffer `b`:

<!-- generated: buf-methods -->
| method | returns | cycles |
|---|---|---|
| `b:get(x, y)` | the element, 0 outside; an integer for integer kinds, a float for `f32` | 1 |
| `b:set(x, y, v)` | nothing; ignored outside | 1 |
| `b:fill(v)` | nothing | 1 + bytes / 128 |
| `b:width()` | width in elements | 1 |
| `b:height()` | height in elements; for a map, rows per layer | 1 |
| `b:kind()` | `"u8"`, `"i16"`, `"i32"` or `"f32"` | 1 |
| `b:layers()` | map layers (1 for a plain buffer) | 1 |
| `b:tile_size()` | 8 or 16 for a map, 0 otherwise | 1 |
| `b:copy(src, sx, sy, w, h, dx, dy)` | nothing; raw copy of same-kind elements, no clip, no colour table | 1 + bytes / 128 |
| `b:blit(src, sx, sy, w, h, dx, dy)` | nothing; like `copy` but honours the clip when `b` is the draw target | 1 + bytes / 128 |
| `b:release()` | nothing; frees the buffer now | 1 |
| `tostring(b)` | `buf(u8 64x64)` or `buf(released)` |  |

- `b:get`: Errors: `buf_released`.
- `b:set`: Errors: `buf_released`.
- `b:fill`: Errors: `buf_released`.
- `b:width`: Errors: `buf_released`.
- `b:height`: Errors: `buf_released`.
- `b:kind`: Errors: `buf_released`.
- `b:layers`: Errors: `buf_released`.
- `b:tile_size`: Errors: `buf_released`.
- `b:copy`: Errors: `buf_aliased`, `buf_kind_mismatch`, `buf_released`.
- `b:blit`: Errors: `buf_aliased`, `buf_kind_mismatch`, `buf_released`.
- `b:release`: Errors: `buf_protected`, `buf_released`.
<!-- /generated -->

Buffer errors are Lua errors, catchable: `buf_bad_dimensions`,
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

<!-- generated: input -->
| call | returns | cycles |
|---|---|---|
| `btn(n)` | `true` while button `n` is held this frame; `false` for any other `n` | 1 |

- `btn`: There is no `btnp`; keep last frame's state yourself to detect
  presses.
<!-- /generated -->

## Logging and stats

<!-- generated: logging -->
| call | returns | cycles |
|---|---|---|
| `print(...)` | nothing; stringifies the arguments, joins them with a tab and appends the line to this frame's log | 1 + bytes / 8 |
| `stat(name)` | a number, see below | 1 |

- `stat`: Names: `"cpu"` (cycles used this frame divided by the budget,
  a float that can pass 1 on frame 1), `"cpu_cycles"`, `"cpu_budget"`,
  `"mem"` (Lua heap bytes), `"mem_limit"` (16 MiB), `"gfx_mem"`,
  `"gfx_limit"` (8 MiB), `"frame"` (the frame being run, 1-based),
  `"width"` and `"height"` (the screen size the manifest chose). With
  the `net` service: `"net_sent"`, `"net_received"`, `"net_dropped"` and
  `"net_inbox"` (the counters and the unread events). Any other name is
  a Lua error.
<!-- /generated -->

The log holds 256 lines of at most 1024 bytes per frame; more lines are
dropped with a marker line. Hosts show the log on stderr, in
`run.json`, and through the MCP `logs` tool.

## Modules

<!-- generated: modules -->
| call | returns | cycles |
|---|---|---|
| `require(name)` | what `src/<name with . as />.lua` returned (or `true`), loaded once | 1 + source bytes / 4 on first load, 1 afterwards |

- `require`: Names are letters, digits and `_` joined by `.`; at most
  256 modules; a cycle is an error. Errors: `module_name_invalid`,
  `module_not_found`, `require_cycle`, `require_limit`,
  `module_not_utf8`.
<!-- /generated -->

## The standard library

Available: `math` (with `math.random` seeded to a fixed value every
run), `string` (without `dump`), `table`, `utf8`, `coroutine`, plus
`pcall`, `error`, `assert`, `type`, `tostring`, `tonumber`, `pairs`,
`ipairs`, `next`, `select`, `rawget`, `rawset`, `rawequal`, `rawlen`,
`getmetatable`, `warn`, and the wrapped `setmetatable`,
`collectgarbage` and `xpcall` below. Removed: `os`, `io`, `package`,
`debug`, `dofile`, `loadfile`, `loadstring` and `string.dump`; the
global `load` is the save-slot binding below, not Lua's chunk loader.

Standard functions whose cost is not in VM instructions are wrapped
with a price (each row also costs the usual 1); every entry not listed
here is Lua's own and costs only its instructions.

<!-- generated: stdlib -->
| call | cycles |
|---|---|
| `math.randomseed([x, y])` | instructions only |
| `string.find(s, pattern, [init, plain])` | pattern bytes x subject bytes ^ (quantifiers + 1 if unanchored) / 256 |
| `string.match(s, pattern, [init])` | pattern bytes x subject bytes ^ (quantifiers + 1 if unanchored) / 256 |
| `string.gmatch(s, pattern, [init])` | pattern bytes x subject bytes ^ (quantifiers + 1 if unanchored) / 256 |
| `string.gsub(s, pattern, repl, [n])` | pattern bytes x subject bytes ^ (quantifiers + 1 if unanchored) / 256, then result bytes / 8 |
| `string.rep(s, n, [sep])` | 1 + result bytes / 8 |
| `string.upper(s)` | 1 + result bytes / 8 |
| `string.lower(s)` | 1 + result bytes / 8 |
| `string.reverse(s)` | 1 + result bytes / 8 |
| `string.sub(s, i, [j])` | 1 + result bytes / 8 |
| `string.byte(s, [i, j])` | 1 + values returned / 8 |
| `string.char(...)` | 1 + arguments / 8 |
| `utf8.len(s, [i, j, lax])` | 1 + subject bytes / 8 |
| `utf8.codepoint(s, [i, j, lax])` | 1 + subject bytes / 8 |
| `utf8.offset(s, n, [i])` | 1 + subject bytes / 8 |
| `utf8.codes(s, [lax])` | 1 + subject bytes / 8 |
| `utf8.char(...)` | 1 + arguments / 8 |
| `table.insert(t, [pos,] v)` | 1 + elements shifted / 8 |
| `table.remove(t, [pos])` | 1 + elements shifted / 8 |
| `table.move(a, f, e, t, [b])` | 1 + elements copied / 8 |
| `table.unpack(t, [i, j])` | 1 + elements copied / 8 |
| `string.format(fmt, ...)` | 1 + result bytes / 8 |
| `table.concat(t, [sep, i, j])` | 1 + result bytes / 8 |
| `table.sort(t, [comp])` | n log2 n |
| `coroutine.create(f)` | 2000 |
| `coroutine.wrap(f)` | 2000 |
| `collectgarbage("count")` | 1 |
| `collectgarbage(["collect"])` | 1 + heap bytes / 128 |
| `collectgarbage("step", [n])` | 1 + heap bytes / 128 |
| `xpcall(f, handler, ...)` | 1 |
| `setmetatable(t, mt)` | 1 |

- `math.randomseed`: `math.random` is seeded to a fixed value every run,
  and `math.randomseed()` with no argument reseeds to that same fixed
  value instead of the clock. With arguments it seeds as Lua does.
- `string.find`: With `plain` true the price is that of an unanchored
  pattern with no quantifiers.
- `collectgarbage`: Any other option is a Lua error, so a cart cannot
  stop or tune the collector.
- `xpcall`: The handler is wrapped: once the cart is past its budget the
  wrapper hands the fault back without running the handler.
- `setmetatable`: A metatable with `__gc` is refused (use `__close`).
<!-- /generated -->

Patterns with several `.-`, `.*` or `.+` on long subjects are priced by
their worst case and can exceed the budget by themselves; anchor them or
split the work.

## Audio

Eight channels, 44.1 kHz mono, rendered per frame. Sounds come from
`sfx/*.trk` and `music/*.trk` (tracker text) and `samples/*.wav` (8 or
16-bit mono PCM, 2 MiB in total). A name is the file's stem, as for
`load_sheet`. Audio rendering is not charged; each call costs 1 cycle.

<!-- generated: audio -->
| call | returns | cycles |
|---|---|---|
| `sfx(name, [channel])` | the channel it plays on | 1 |
| `music(name, [fade])` | nothing; starts the track, fading over `fade` frames | 1 |
| `music()` | nothing; stops the track | 1 |
| `music(nil, fade)` | nothing; stops the track over `fade` frames | 1 |
| `sample(name, [channel, pitch])` | the channel it plays on | 1 |
| `volume(channel, v)` | nothing; channel gain 0.0 to 1.0, clamped | 1 |

- `sfx`: `name` is the stem of `sfx/<name>.trk`. Defaults: `channel` = a
  free one. Errors: `asset_not_found`, `asset_invalid`, `track_error`,
  `audio_bad_channel`, `audio_no_room`.
- `music`: `name` is the stem of `music/<name>.trk`. Defaults: `fade` =
  0. Errors: `asset_not_found`, `asset_invalid`, `track_error`,
  `audio_bad_channel`, `audio_no_room`.
- `sample`: `name` is the stem of `samples/<name>.wav`. `pitch` 1.0 is
  native, clamped to 1/256 to 16. Defaults: `channel` = a free one,
  `pitch` = 1.0. Errors: `asset_not_found`, `asset_invalid`,
  `sample_error`, `audio_bad_channel`.
- `volume`: Errors: `audio_bad_channel`.
<!-- /generated -->

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

<!-- generated: numeric -->
| call | cycles |
|---|---|
| `math.sin(x)` | instructions only |
| `math.cos(x)` | instructions only |
| `math.tan(x)` | instructions only |
| `math.asin(x)` | instructions only |
| `math.acos(x)` | instructions only |
| `math.exp(x)` | instructions only |
| `math.atan(y, [x])` | instructions only |
| `math.log(x, [base])` | instructions only |
<!-- /generated -->

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

<!-- generated: saves -->
| call | returns | cycles |
|---|---|---|
| `save(slot, table)` | nothing; slot 0 to 7 | 66 + value bytes / 8 + encoded bytes / 8 |
| `load(slot)` | the table, or `nil` when the slot is empty | 65 + stored bytes / 8; 64 when the slot is empty |

- `save`: A value that is not a table is a Lua error. The walk is paid
  for whether or not the value fits, so a save that fails on size is not
  cheaper than one that succeeds. Errors: `save_slot`, `save_size`,
  `codec_unsupported`, `codec_cycle`, `codec_depth`, `codec_size`,
  `codec_key`, `codec_number`.
- `load`: Every `buf` in the table comes back as a fresh buffer, priced
  like `buf`. A slot whose text does not decode is a codec error.
  Errors: `save_slot`, `codec_syntax`, `codec_depth`, `codec_size`,
  `codec_key`, `codec_number`, `graphics_budget_exceeded`.
<!-- /generated -->

Save errors are Lua errors: `save_slot` (not 0 to 7), `save_size` (over
256 KiB encoded), and the codec's `codec_unsupported`, `codec_cycle`,
`codec_depth`, `codec_size`, `codec_key` and `codec_number`.

## Networking

A cart that declares `services = ["net"]` in `cart.toml` gets the
`net` table; any other cart has no `net` global. Nothing here opens a
socket from the cart: the cart queues **commands** during its frame
(`host`, `join`, `send`, `leave`) and reads **events** the host admits
at the top of a later frame (`hosting`, `connected`, `message`,
`disconnected`, `failed`, `permission`). There is one remote peer,
numbered 1, one reliable ordered channel, and messages are strings of
1 to 1024 bytes of any content.

Permission is the player's: a run started with `kuula run <cart> --net
host` or `--net join <ticket>` is permitted, and a cart the shell
starts is permitted when the shell's settings say `networking: on`.
Without permission `net.host()` and `net.join()` answer with a `failed`
event whose code is `net_denied`, and no endpoint is ever built. When
the setting is switched off while a cart runs, the cart sees
`permission` with `granted = false`, its session ends, and every event
still queued is discarded.

Events are admitted at most 64 per frame into an inbox of 256; what
the cart does not `recv()` waits, and when the inbox is full the host
stops offering events and the peer is stalled by flow control, so
nothing is lost or reordered. The outbox takes 16 `send` calls per
frame; the seventeenth returns `false` and counts in `stat("net_dropped")`.
Drain `net.recv()` at the top of `_update` into your own state and
draw from that state, never from the inbox.

A recorded run of a networked cart is a version 2 transcript (see
"Transcripts") holding every admitted batch and every command, so it
replays headless without a peer: `kuula run <cart> --headless --replay
run.kr` feeds the recorded events and checks that the cart issues the
recorded commands on the recorded frames. A cart that issues a
different command ends the replay with `transcript_divergence`; the
frames before it still hash and are reported.

Offline, a networked cart still boots and runs: `net.status()` is
`"off"`, `net.invite()` is `nil`, and every call is answered by an
event or a return value, never a Lua error. `examples/netbuttons` shows
the shape.

<!-- generated: net -->
| call | returns | cycles |
|---|---|---|
| `net.host()` | nothing; asks the host to listen for one peer | 1 |
| `net.join(ticket)` | nothing; asks the host to connect to a peer's ticket | 1 |
| `net.send(data)` | `true` if queued in this frame's outbox, `false` if there is no session or the outbox (16 per frame) is full | 1 + data bytes / 128 |
| `net.recv()` | the oldest unread event as a table, or `nil` when there is none | 1 + data bytes / 128 |
| `net.status()` | `"off"`, `"hosting"`, `"joining"`, `"connected"` or `"ended"` | 1 |
| `net.ticket()` | the ticket this cart is hosting under, or `nil` | 1 |
| `net.peers()` | a list of peer numbers: `{}` or `{ 1 }` | 1 |
| `net.invite()` | the ticket the run was launched with (`--net join`), or `nil` | 1 |
| `net.leave()` | nothing; ends the session or stops hosting | 1 |

- `net.host`: The ticket arrives as a `hosting` event and is readable
  from `net.ticket()` afterwards. Without permission the answer is a
  `failed` event with code `net_denied`; while already hosting, joining
  or connected it is `net_declined`.
- `net.join`: A ticket over 1024 bytes or not UTF-8 is a Lua error. A
  ticket that does not parse or a peer that does not answer is a
  `failed` event (`net_ticket`, `net_connect`), never an error. Errors:
  `net_ticket`.
- `net.send`: `data` is a string of 1 to 1024 bytes, any content; other
  sizes are a Lua error. Accepted locally does not mean delivered: the
  message leaves after the frame, and a session that ends first loses
  it. Errors: `net_data`.
- `net.recv`: The table has `kind` and the fields of that kind:
  `hosting` (`ticket`), `connected` (`peer`), `message` (`from`,
  `data`), `disconnected` (`reason`: `left`, `lost` or `closed`),
  `failed` (`code`, `detail`) and `permission` (`granted`). Events are
  admitted at the top of the frame, at most 64 per frame, and the inbox
  holds 256; drain it every frame.
- `net.leave`: A `disconnected` event with reason `closed` follows.
<!-- /generated -->

## `cart.toml`

Every key is optional; unknown keys are a `manifest_error` with a line.

```toml
[cart]
title = "My cart"          # shown by the host; untrusted text elsewhere
screen_mode = "640x480"    # or "320x240"; default 640x480
services = ["net"]         # host services the cart may use; only "net" exists

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
a record carrying the reserved `messages` or `connections` fields, a
run past the header's frame count or a button mask above 63 is refused
with `transcript_format`, `transcript_version` or `transcript_size`.

A cart that declares the `net` service records **format version 2**:
the first line is `kuula-transcript 2`, the header gains `services`,
`invite` and `net` (the permission the run had), and after each input
run that ends on a frame with network traffic comes one `{ at, events,
io }` record: the events admitted at the top of frame `at`, in order,
as `{ kind, ... }` tables, and the commands the cart issued during it
as `{ op, ticket, data }` tables, at most 64 and 32 per record. Both
versions are read; every other cart keeps writing version 1. A version
2 transcript replays in process only, never through `--worker`, and a
run outgrows it after 40 MiB of network payload, in which case the
transcript is written and reported as incomplete.

## Windowed runs and the worker

`kuula run <cart>` and the shell run every cart in a worker process
under the OS sandbox; `--in-process` runs it in the host process
instead, and `--no-sandbox` launches the worker plainly for debugging.
The OS sandbox is Windows only; elsewhere the worker is a plain child
process and a warning line says so.
Headless runs are in-process unless `--worker`. `--net host` and `--net
join <ticket>` permit networking for the run, windowed or headless; a
headless host prints `ticket: ...` and is paced to real time so a peer
can join it, and `--timing` prints the median and maximum wall time of
a step at exit. The worker never holds a socket: the broker owns the
network link and passes events in and commands out with each step. A worker that dies,
stalls or misbehaves ends the cart with `worker_error`,
`watchdog_timeout` or `sandbox_unavailable` on the error screen, and the
window stays responsive while it waits.

The window is 640x480 times the scale (`--scale 1` to `4`, default 2,
Ctrl+1 to Ctrl+4 at runtime, and the shell's settings screen) whatever
the cart's screen mode: a 320x240 cart is drawn at twice the scale, so
switching carts never resizes the window.

## The shell's `sys` table

The shell (`rom/main.lua`, embedded in the CLI) is a guest with the
same sandbox as a cart plus one extra global, `sys`, through which it
lists carts and asks the host to run, pause, restart and quit them. A
cart never has `sys`; nothing here is cart API.

<!-- generated: shell -->
| call | returns | cycles |
|---|---|---|
| `sys.carts()` | a list of `{name, title}` tables, one per installed cart | 1 |
| `sys.run(name)` | nothing; asks the host to start the named cart | 1 |
| `sys.quit()` | nothing; asks the host to stop the running cart | 1 |
| `sys.restart()` | nothing; asks the host to restart the running cart | 1 |
| `sys.paused(on)` | nothing; pauses or resumes the running cart | 1 |
| `sys.is_paused()` | whether the cart is paused | 1 |
| `sys.running()` | whether a cart is running | 1 |
| `sys.menu()` | whether the menu button was pressed this frame | 1 |
| `sys.fault()` | the running cart's fault as `{code, file, line, message}`, or `nil` | 1 |
| `sys.settings()` | `{scale, volume, net}` | 1 |
| `sys.set_net(on)` | nothing; grants or withdraws networking for carts the shell runs | 1 |
| `sys.set_scale(n)` | nothing; asks the host for window scale `n`, 1 to 4 | 1 |
| `sys.set_volume(v)` | nothing; asks the host for volume `v`, 0 to 100 | 1 |

- `sys.set_net`: Withdrawing it while a cart has a session ends the
  session; the cart sees a `permission` event with `granted = false`.
- `sys.set_scale`: A scale outside 1 to 4 is a Lua error.
- `sys.set_volume`: A volume outside 0 to 100 is a Lua error.
<!-- /generated -->

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
