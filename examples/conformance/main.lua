-- Kuula conformance cart, iterations 2 and 4.
--
-- Every drawing primitive, the colour table, buffers, sheets and maps are
-- exercised over 60 frames with positions from the fixed-seed
-- math.random, and the mixer too: music, a sound
-- effect, a sample and a channel volume. hashes.txt beside this file
-- holds the canonical per-frame hash of indexed pixels, palette and
-- audio. If a change to the rasteriser or the mixer alters any of them,
-- the change must be intended and the file regenerated with:
--
--   kuula run examples/conformance --headless --frames 60 --out <dir>
--
-- and <dir>/hashes.txt copied here.

local W, H = 320, 240
local frame = 0
local tiles, world, scratch

local function r(n)
  return math.random(0, n)
end

function _init()
  tiles = load_sheet("tiles")
  world = load_map("small")
  scratch = buf("u8", 64, 64)
  sheet(tiles)
  music("loop")
end

function _update(dt)
  frame = frame + 1
  -- Audio: the effect every 20 frames, a pitched sample, a channel
  -- volume change, then the music fades out before the run ends.
  if frame % 20 == 0 then
    sfx("blip")
  end
  if frame == 30 then
    sample("tick", 5, 1.5)
  end
  if frame == 35 then
    volume(1, 0.5)
  end
  if frame == 48 then
    music(nil, 10)
  end
end

local function shapes(c)
  pset(r(W), r(H), c)
  line(r(W), r(H), r(W), r(H), c + 1)
  rect(r(W), r(H), r(W), r(H), c + 2)
  rectfill(r(W), r(H), r(W), r(H), c + 3)
  circ(r(W), r(H), r(40), c + 4)
  circfill(r(W), r(H), r(40), c + 5)
  print("frame " .. frame, r(W), r(H), c + 6)
end

function _draw()
  -- Cart colours drift so the palette part of the hash moves too.
  pal(16 + frame % 100, (frame * 7) % 256, (frame * 13) % 256, (frame * 29) % 256)

  cls(frame % 16)
  camera(frame % 7 - 3, frame % 5 - 2)
  shapes(8)

  clip(20, 20, 200, 150)
  fillp(0xa5a5 + frame, frame % 2 == 0)
  shapes(16 + frame % 50)
  fillp()
  clip()

  palt(0, true)
  pal_map(12, 8 + frame % 8)
  for i = 1, 8 do
    spr(i % 8, r(W), r(H), 1, 1, i % 2 == 0, i % 3 == 0)
  end
  sspr(0, 0, 16, 8, r(W), r(H), 8 + frame % 40, 4 + frame % 20, frame % 4 == 1, frame % 4 == 2)
  map(world, frame % 4, frame % 3, r(W), r(H), 8, 6, 0)
  map(world, 0, 0, r(W), r(H), 16, 12, 1)
  pal_reset()

  -- Draw into a buffer and copy it back both ways.
  draw_target(scratch)
  cls(frame % 128)
  circfill(32, 32, 20 + frame % 10, 7)
  draw_target()
  screen:copy(scratch, 0, 0, 64, 64, 250, 170)
  clip(0, 0, 40, 40)
  screen:blit(scratch, 0, 0, 64, 64, 10, 10)
  clip()
  scratch:set(frame % 64, 3, frame)
  pset(300, 230, scratch:get(frame % 64, 3))
  camera()

  print(pget(5, 5) .. " " .. pget(319, 239), 100, 230, 7)
end
