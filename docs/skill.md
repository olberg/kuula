---
title: Writing a Kuula cart
status: current
version: 0.0.6
date: 2026-09-10
related:
  - api.md (every call, price and error code)
---

# Writing a Kuula cart

Kuula is a fantasy console for handhelds. A cart is a directory with
`main.lua` in it; the console runs it at exactly 60 frames per second in
a Lua 5.5 sandbox with a fixed cycle budget per frame. This document is
the set of constraints and habits that make a cart work first time.
`api.md` has every call; this file tells you which ones to reach for
and what not to try.

## The shape of a cart

```
mycart/
  main.lua        entry point, required
  cart.toml       manifest, optional
  src/*.lua       modules for require("name")
  gfx/*.png       sprite sheets, load_sheet("name")
  map/*.json      tile maps, load_map("name")
  sfx/*.trk       sound effects, sfx("name")
  music/*.trk     music, music("name")
  samples/*.wav   PCM samples, sample("name")
```

Nothing outside those paths is read. Files are snapshotted when the
cart starts; editing them does not affect a running console. Restart
(`run` again) to pick up changes.

`main.lua` defines up to three callbacks:

```lua
function _init() end        -- frame 1, once, after the main chunk
function _update(dt) end    -- every frame from 2 on; dt is always 1/60
function _draw() end        -- every frame from 2 on, after _update
```

Keep state in locals at file scope or in a table; there is no persistent
global state between runs except what `save`/`load` hold.

## Constraints that shape every cart

**Screen modes.** `640x480` (default) or `320x240`, chosen once in
`cart.toml` with `screen_mode`. Choose `320x240` unless you need the
detail: it draws four times fewer pixels for the same budget, the
system font (4x6) is readable at either, and the window is the same
size either way. `stat("width")` and `stat("height")` return the choice.
Coordinates are integers; the origin is the top-left; y grows downwards.

**Two-button tier.** Six inputs: D-pad (0 up, 1 down, 2 left, 3 right)
plus A (4) and B (5). Design for those alone; a handheld has nothing
else. `btn(n)` is *held*, not *pressed*: keep last frame's value to
detect an edge.

```lua
local was_a = false
function _update(dt)
  local a = btn(4)
  if a and not was_a then jump() end
  was_a = a
end
```

**Fixed timestep.** `dt` is always `1/60`. Do not integrate with it as
if it varied; treat it as a constant or ignore it and count frames.
There is no wall clock, no `os.time`, no `os.clock`, and no way to know
how long a frame took in real time. `stat("frame")` is the only clock.

**Determinism.** The same cart with the same inputs produces the same
frames, pixel for pixel, on every machine. `math.random` starts from a
fixed seed every run; `math.randomseed()` with no argument goes back to
it. Iteration order of `pairs` over string keys is stable too because the
string hash seed is pinned. `math.sin`, `cos`, `exp`, `log`, `^` and the
rest of the `math` table give the same bits everywhere, and any NaN
prints as `nan`. If you want variety between plays, derive it from input
timing (the frame the player first pressed A). `kuula run mycart
--record run.kr` keeps the inputs of a play; `kuula run mycart
--headless --replay run.kr --out dir/` reproduces it frame for frame.

**Budgets and prices.** Each frame may spend 279,620 cycles; the main
chunk plus `_init` together get 60 frames' worth. Going over ends the
cart with `budget_exceeded` naming the callback. Costs that matter:

- Lua instructions: 2 each. A loop of 10,000 iterations doing three
  things is already ~60,000 cycles.
- Drawing: 1 + pixels touched / 3. A full `cls` on 320x240 is 1,201; a
  full-screen `rectfill` is 25,601; a full-screen `map` of 40x30 cells
  is 2,400 + 25,600.
- `print` to the log: 1 + bytes / 8. Cheap, but not free in a loop.
- `string` pattern functions are priced by their worst case; anchor
  patterns (`^`) and avoid stacking `.-` on long strings.
- `load_sheet`/`load_map` on first use: decoded bytes / 8. Do it in
  `_init` or preload through the manifest, never per frame.
- `buf(...)`, `fill`, `copy`, `blit`: bytes / 128.

Read `stat("cpu")` (fraction of budget used so far this frame) or the
MCP `profile` tool to see where cycles go. The frame budget is far more
than a simple game needs; the point is that a runaway loop ends the
cart instead of freezing the device.

**Memory.** The Lua heap is capped at 16 MiB (`out_of_memory` after a
full collection fails to get under); graphics memory at 8 MiB for all
buffers together (`graphics_budget_exceeded`, a catchable Lua error).
Release big buffers with `b:release()` when done.

**The buffer model.** Every image is a `buf` value: the screen
(`screen`), sheets (`load_sheet`), maps (`load_map`) and your own
(`buf("u8", w, h)`). Drawing calls draw into the current target;
`draw_target(b)` retargets to any `u8` buffer and `draw_target()` comes
back to the screen. `sheet(b)` picks the sheet `spr`, `sspr` and `map`
read from; nothing draws sprites until you have called it. Buffers are
handles: dropping the last reference frees the memory at the next frame.

**The sandbox.** No `os`, `io`, `package`, `debug`, `dofile`,
`string.dump` or `__gc` metamethods; the global `load` reads a save
slot, it does not load code. `require("name")` loads
`src/name.lua` from the cart and nothing else. `print(a, b)` (no numeric
x and y) writes to the frame log, which is how you debug.

**Text is untrusted output.** Log lines, the manifest title and error
messages are shown to people and tools as plain text with control
characters removed. Do not try to format with escape sequences.

## Idioms

- Clear once per frame in `_draw` (`cls(c)`), then draw back to front.
- Use `camera(x, y)` for scrolling and `clip(...)` to bound UI panels;
  both reset cheaply (`camera()`, `clip()`).
- Colour 0 is transparent in sprites once you call `palt(0)`; the default
  is no transparency. `pal_map(from, to)` recolours a sprite at draw
  time without touching the sheet.
- Sprite sheets are 8x8 cells numbered left to right, top to bottom.
  `spr(n, x, y, 2, 2)` draws a 16x16 block of four cells.
- Maps are JSON (`api.md`, "Sprites and maps"); a cell of `-1` is empty.
  `map(m, 0, 0, 0, 0, 40, 30)` draws the visible part of a 320x240
  screen; move `cx, cy` for scrolling rather than drawing the whole map.
- Prefer integer arithmetic (`//`, `%`) for positions; floats are fine
  but floor before drawing to avoid sub-pixel jitter.
- Structure larger carts as modules: `local player = require("player")`
  at the top of `main.lua`, with `src/player.lua` returning a table.
- Detect a win or lose state in `_update` and expose it as a global
  (`state = "won"`) so the `state` tool and tests can read it.

## Iterating with the MCP tools

`kuula mcp --root <dir>` serves these tools over stdio; cart paths are
relative to the root. The loop:

1. **`validate {cart}`** loads the cart and runs frame 1. Errors come
   back as `{code, file, line, message, severity}`; an empty list means
   the manifest parsed, assets decoded, `main.lua` compiled and `_init`
   ran within budget. Fix everything it reports before running.
2. **`run {cart, frames?}`** starts a headless console and returns a
   handle (`"c1"`), the frame reached, the screen size and the title.
   At most 8 consoles live at once; **`stop {console}`** when done.
3. **`step {console, frames, expect_frame, input?}`** advances. Pass
   `expect_frame` (the frame the last reply reported) so a retried call
   is refused with `stale_frame` rather than stepping twice. `input` is
   an input script: `[{"frames": 30, "buttons": ["right"]}, {"buttons":
   ["a"]}]`. Alternatively **`input {console, frames: [masks]}`** queues
   masks (1 up, 2 down, 4 left, 8 right, 16 A, 32 B) for steps that
   carry no script. The reply holds the frames run, new log lines and a
   `fault` if the cart ended; `running: false` means it is over.
4. **`screenshot {console}`** returns the screen as a PNG image plus the
   palette. Look at it; check that what you drew is where you meant.
5. **`state {console, names}`** dumps named globals through the
   canonical codec, so a test can assert `state == "won"` instead of
   reading pixels.
6. **`logs {console, since_frame?}`** and **`profile {console, last?}`**
   give the frame log and cycles per category per frame. When a fault
   says `budget_exceeded`, `profile` shows which category ate it.
7. **`run {cart, record: true}`** records every input the cart sees;
   `stop` then returns the transcript, and **`replay {cart,
   transcript, frames?}`** plays it back from the same saves, so a bug
   found by hand can be reproduced exactly after the fix.

A fault freezes the console at the faulting frame; `step` on it runs
nothing. Fix the cart and `run` again: the console does not reload
files. Handles and everything behind them are dropped when the server
exits. Headless consoles keep saves in memory only.

The same loop from the shell: `kuula run mycart --headless --frames 120
--input script.json --out out/` writes PNG frames, `hashes.txt`,
`run.json` and `profile.json`; `kuula screenshot mycart --frame 60 --out
f.png` writes one frame.

## A minimal complete cart

`cart.toml`:

```toml
[cart]
title = "Catch"
screen_mode = "320x240"
```

`main.lua`:

```lua
-- Catch: move the paddle with left/right, catch 5 drops to win.
local W, H = 320, 240
local paddle = { x = 150, w = 24 }
local drop = { x = 40, y = 0 }
local caught = 0
state = "playing"            -- global on purpose, for the state tool

local function reset_drop()
  drop.x = math.random(0, W - 8)
  drop.y = 0
end

function _init()
  reset_drop()
end

function _update(dt)
  if state ~= "playing" then return end
  if btn(2) then paddle.x = paddle.x - 3 end
  if btn(3) then paddle.x = paddle.x + 3 end
  paddle.x = math.max(0, math.min(W - paddle.w, paddle.x))
  drop.y = drop.y + 2
  if drop.y >= H - 16 then
    if drop.x + 8 > paddle.x and drop.x < paddle.x + paddle.w then
      caught = caught + 1
      if caught >= 5 then state = "won" end
    else
      state = "lost"
    end
    reset_drop()
  end
end

function _draw()
  cls(1)
  rectfill(paddle.x, H - 12, paddle.x + paddle.w - 1, H - 9, 7)
  rectfill(drop.x, drop.y, drop.x + 7, drop.y + 7, 10)
  print("caught " .. caught, 2, 2, 6)
  if state ~= "playing" then
    print(state, W // 2 - 8, H // 2, 8)
  end
end
```

Checking it with the tools: `validate {cart: "catch"}` returns no
errors; `run {cart: "catch"}` gives `c1` at frame 1; `step {console:
"c1", frames: 600, expect_frame: 1, input: [{"frames": 600, "buttons":
["right"]}]}` plays it; `state {console: "c1", names: ["state",
"caught"]}` shows whether holding right was enough; `screenshot`
confirms the paddle is on the right edge. Then `stop`.
