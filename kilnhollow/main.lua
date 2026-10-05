-- Kilnhollow: the caretaker of an abandoned ironworks, a rivet gun, and the
-- clockwork vermin that have taken the place over. One level, in sectors:
-- floors and ceilings at different heights, walls at any angle, doors,
-- a key. D-pad walks and turns, L1 and R1 strafe, A fires, or swings the gun
-- when the rivets are gone, B uses (doors, the lever), Y swaps weapons, X
-- shows the map, Start pauses.
--
-- Holding L2 and R2 as A starts a run is the caretaker's mercy: nothing
-- hurts. It is how a recorded walk through the whole level, which a creature
-- would end, is played.

local art = require("art")
local render = require("render")
local game = require("game")
local hud = require("hud")

-- A global, so that a test can ask the console for it: "title", "play",
-- "dead", "won" or "paused".
state = "title"

-- How much of a frame's cycles the game has used: the worst and the mean.
perf = { n = 0, sum = 0, peak = 0 }

local frame = 0
local pause = 0
local was = {}

local function pressed(n)
  local down = btn(n)
  local p = down and not was[n]
  was[n] = down
  return p
end

-- The A that starts a run is not a shot: A fires once it has been let go,
-- which is looked for on every frame, paused or not.
local a_from_start = false

local function start(mercy)
  game.new(mercy)
  perf = { n = 0, sum = 0, peak = 0 }
  state = "play"
  a_from_start = true
end

local function report()
  if perf.n > 0 then
    print(string.format("cycles worst %.1f%% mean %.1f%% over %d frames",
      perf.peak * 100, perf.sum / perf.n * 100, perf.n))
  end
end

function _init()
  for i, rgb in ipairs(art.palette) do pal(15 + i, rgb) end
  palt(0, true)
  local walls, sprites, title = load_sheet("walls"), load_sheet("sprites"), load_sheet("title")
  render.init(walls, sprites)
  hud.sprites, hud.title_sheet = sprites, title
  game.init()
  game.new()
end

local show_map = false
local inp = {}

function _update()
  frame = frame + 1
  local a, b, x, y, st = pressed(4), pressed(5), pressed(6), pressed(7), pressed(12)
  if not btn(4) then a_from_start = false end
  if state == "title" then
    if a then start(btn(10) and btn(11)) end
  elseif state == "play" then
    if st then
      state = "paused"
      return
    end
    if x then show_map = not show_map end
    inp.up, inp.down, inp.left, inp.right = btn(0), btn(1), btn(2), btn(3)
    inp.l1, inp.r1, inp.a = btn(8), btn(9), btn(4) and not a_from_start
    inp.use, inp.swap = b, y
    game.update(inp)
    if game.p.dead then
      state, pause = "dead", 0
      report()
    elseif game.p.won then
      state, pause = "won", 0
      report()
    end
  elseif state == "paused" then
    if st then state = "play" end
  else
    pause = pause + 1
    if a and pause > 45 then
      if state == "dead" then start() else state = "title" end
    end
  end
end

function _draw()
  if state == "title" then
    hud.title(frame)
    return
  end
  local p = game.p
  if state == "won" then
    hud.won(p, game.total, frame)
    return
  end
  if show_map then
    hud.map(game.lv, p)
  else
    render.draw(game.cam(), game.things, game.lv.sectors)
    hud.weapon(p, game.WEAPONS)
    hud.crosshair(p)
    hud.flash(p)
  end
  hud.bar(p, game.WEAPONS)
  hud.message(p)
  if state == "dead" then
    hud.dead(frame)
  elseif state == "paused" then
    hud.paused(frame)
  end
  if state == "play" then
    local c = stat("cpu")
    perf.n = perf.n + 1
    perf.sum = perf.sum + c
    if c > perf.peak then perf.peak = c end
  end
end
