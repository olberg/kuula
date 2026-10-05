-- Music and effects. The music has channels 0 to 3 and keeps them. Each
-- kind of effect has a channel of its own, so a burst of one kind never
-- cuts another: pickups on 4, the weapons on 5, kills and wounds on 6 and
-- 7. The few longer figures take 4 and 5 and hold them until they are over.
local M = {}

local PIPS, SHOTS, BLOWS = 4, 5, 6
-- How long each figure sounds, in frames.
local FIGURE = { levelup = 40, panic = 28, calm = 34, hulk = 58, over = 120, win = 114 }

local now = 0
local hit_t, kill_t, hurt_t = -99, -99, -99
local mote_frame, streak, streak_t = -1, 0, -99
local figure_until = 0
local confident = true

function M.tick()
  now = now + 1
end

local function figure(name)
  figure_until = now + FIGURE[name]
  cue("fx", name, nil, nil, PIPS)
end

-- Channels 4 and 5 are free of a figure.
local function clear()
  return now >= figure_until
end

function M.start()
  confident = true
  figure_until = 0
  hit_t, kill_t, hurt_t = -99, -99, -99
  streak, streak_t = 0, -99
  -- The run before may still be ringing: its last figure holds two
  -- channels by name, and would sound on into this one.
  for ch = PIPS, 7 do stop(ch, true) end
  music("fenlight")
end

function M.stop(fade)
  music(nil, fade)
end

-- Below 50 oil the music turns panicky; it comes back at 55 or more, so
-- hovering near the line does not flip it every few frames.
function M.mood(oil)
  if confident and oil < 50 then
    confident = false
    music("fenlight", 0, "panicky")
    figure("panic")
  elseif not confident and oil >= 55 then
    confident = true
    music("fenlight", 0, "confident")
    figure("calm")
  end
end

function M.bolt(transpose)
  if clear() then cue("fx", "bolt", transpose, nil, SHOTS) end
end

function M.hit()
  if now - hit_t >= 3 and clear() then
    hit_t = now
    cue("fx", "hit", nil, nil, SHOTS)
  end
end

function M.kill()
  if now - kill_t >= 4 then
    kill_t = now
    cue("fx", "kill", nil, nil, BLOWS)
  end
end

function M.hurt()
  if now - hurt_t >= 20 then
    hurt_t = now
    kill_t = now + 6        -- let it be heard before the next kill
    cue("fx", "hurt", nil, nil, BLOWS)
  end
end

-- Each pickup of a streak sounds a semitone higher; a pause starts over.
function M.mote()
  if now - streak_t > 30 then streak = 0 else streak = streak + 1 end
  streak_t = now
  if mote_frame ~= now and clear() then
    mote_frame = now
    cue("fx", "mote", math.min(streak, 12), 1, PIPS)
  end
end

function M.flask()
  if clear() then cue("fx", "flask", nil, nil, PIPS) end
end

-- The menu's tick, where no kill is sounding.
function M.pick() cue("fx", "pick", nil, nil, BLOWS) end

function M.levelup() figure("levelup") end
function M.hulk() figure("hulk") end
function M.over() figure("over") end
function M.win() figure("win") end

return M
