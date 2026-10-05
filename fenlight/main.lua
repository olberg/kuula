-- Fenlight: a lamplighter crosses a fen at night and the fog comes for the
-- lamp. D-pad walks, everything else happens by itself; dawn is five
-- minutes away. A starts a run and takes the upgrade you choose.

local palette = require("palette")
local hero = require("hero")
local swarm = require("swarm")
local loot = require("loot")
local weapons = require("weapons")
local upgrades = require("upgrades")
local sound = require("sound")
local light = require("light")
local world = require("world")
local hud = require("hud")

local RUN = swarm.RUN

-- A global, so that a test can ask the console for it: "title", "playing",
-- "levelup", "won" or "lost".
state = "title"

local sheetbuf, titlebuf
local t = 0                 -- frames into the run
local frame = 0             -- frames since the cart started
local was_a, was_up, was_down = false, false, false
local pause = 0             -- frames in the current menu or end screen
local selected = 1

local function start()
  math.randomseed(stat("frame"))
  t = 0
  hero.reset()
  upgrades.reset()
  swarm.reset()
  loot.reset()
  weapons.reset()
  hud.reset()
  light.reset()
  sound.start()
  state = "playing"
end

local function finish(result)
  state = result
  pause = 0
  if result == "won" then
    light.reset()
    sound.stop(60)
    sound.win()
  else
    sound.stop(30)
    sound.over()
  end
end

local function level_up()
  hero.xp = hero.xp - hero.need()
  hero.level = hero.level + 1
  upgrades.roll()
  sound.levelup()
  if upgrades.count == 0 then
    -- Every upgrade is at its last level: the level fills the lamp a little.
    hero.refill()
    state = "playing"
    return
  end
  selected, pause = 1, 0
  state = "levelup"
end

local function play()
  t = t + 1
  hero.move()
  weapons.update(t)
  local drain = swarm.update(t)
  hero.xp = hero.xp + loot.update()

  local oil = hero.oil - drain + hero.regen
  if drain > 0 then
    hero.hurt = 8
    sound.hurt()
  end
  hero.oil = math.min(hero.maxoil, oil)
  sound.mood(hero.oil)
  light.set(hero.oil)

  if hero.oil <= 0 then
    hero.oil = 0
    finish("lost")
  elseif t >= RUN then
    finish("won")
  elseif hero.xp >= hero.need() then
    level_up()
  end
end

local function choose(up, down, a)
  pause = pause + 1
  if up and not was_up and selected > 1 then
    selected = selected - 1
    sound.pick()
  elseif down and not was_down and selected < upgrades.count then
    selected = selected + 1
    sound.pick()
  elseif a and pause > 12 then
    upgrades.take(upgrades.choice[selected])
    sound.pick()
    if hero.xp >= hero.need() then level_up() else state = "playing" end
  end
end

function _init()
  for i, rgb in ipairs(palette) do pal(15 + i, rgb) end
  sheetbuf = load_sheet("fenlight")
  titlebuf = load_sheet("title")
  sheet(sheetbuf)
  palt(0, true)
  world.init()
  hero.reset()
  upgrades.reset()
  light.reset()
end

function _update()
  frame = frame + 1
  sound.tick()
  local a, up, down = btn(4), btn(0), btn(1)
  local pressed = a and not was_a
  if state == "title" then
    if pressed then start() end
  elseif state == "playing" then
    play()
  elseif state == "levelup" then
    choose(up, down, pressed)
  else
    pause = pause + 1
    if pressed and pause > 45 then start() end
  end
  was_a, was_up, was_down = a, up, down
end

function _draw()
  if state == "title" then
    hud.title(frame, titlebuf)
    return
  end
  local camx, camy = math.floor(hero.x) - 160, math.floor(hero.y) - 120
  world.draw(camx, camy)
  weapons.draw_ground(camx, camy)
  loot.draw(camx, camy)
  swarm.draw(camx, camy, t)
  hero.draw(camx, camy, t)
  weapons.draw(camx, camy)
  loot.draw_puffs(camx, camy)
  hud.draw(RUN - t, frame)
  if state == "levelup" then
    hud.levelup(selected)
  elseif state == "won" then
    hud.won(t, frame)
  elseif state == "lost" then
    hud.lost(t, frame)
  end
end
