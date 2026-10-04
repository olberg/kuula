-- Kuula example: a whole song.
--
-- music/atomic.omc is "Not an Atomic Playboy": three and a half minutes at
-- 138 BPM on all eight channels, seven of them the wave engine and one a
-- sampled voice (Vice Admiral Blandy, 1946, as Second Reality sampled him).
--
-- The screen follows the song by counting frames. A song's ticks are not
-- frames, but its tempo is exact, so beats are frames * 138 / 3600 and the
-- two stay in step. A (Z on the desktop host) plays it again.

local W, H = 320, 240
local BPM = 138
local SECTION_BEATS = 32          -- eight bars
local LENGTH = 12576              -- frames, with the tail
local SECTIONS = {
  "the voice, over the music",
  "the groove",
  "the squelch",
  "the lead",
  "up a minor third",
  "the pedal figure",
  "breakdown",
  "the build",
  "the drop",
  "the lead, an octave up",
  "up a whole tone",
  "the peak",
  "back on E",
  "leaving",
  "the fifth on C, and the voice",
}

local frame = 0
local was_a = false

local function start()
  music("atomic")
  frame = 0
end

function _init()
  start()
end

function _update(dt)
  frame = frame + 1
  local a = btn(4)
  if a and not was_a then start() end
  was_a = a
end

function _draw()
  cls(1)
  print("NOT AN ATOMIC PLAYBOY", 16, 16, 7)
  print("an Open Module Track song", 16, 28, 6)

  local playing = frame <= LENGTH
  local beats = frame * BPM / 3600
  local section = math.min(beats // SECTION_BEATS + 1, #SECTIONS)
  local bar = beats // 4 + 1
  local sixteenth = math.floor(beats * 4) % 16

  if playing then
    print(string.format("%d/%d  %s", section, #SECTIONS, SECTIONS[section]), 16, 60, 12)
    print(string.format("bar %d", bar), 16, 72, 6)
  else
    print("the end", 16, 60, 12)
  end

  -- The bar as sixteen steps, the sounding one lit, the beats marked.
  for i = 0, 15 do
    local x = 16 + i * 18
    local lit = playing and i == sixteenth
    local colour = lit and 10 or (i % 4 == 0 and 13 or 5)
    rectfill(x, 100, x + 13, 100 + (lit and 20 or 13), colour)
  end

  -- A pulse on the beat.
  if playing then
    local phase = beats % 1
    circfill(W // 2, 168, math.floor(22 - 14 * phase), 8)
  end

  -- How far in.
  local done = math.min(frame, LENGTH) * (W - 32) // LENGTH
  rectfill(16, 212, W - 17, 215, 5)
  rectfill(16, 212, 16 + done, 215, 12)
  print("A: again", 16, 222, 6)
end
