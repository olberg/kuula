-- What the dead leave behind: glow motes, oil flasks and a puff of light.
local hero = require("hero")
local sound = require("sound")

local L = {}

local sqrt, random = math.sqrt, math.random

local MAX_MOTES, MAX_FLASKS, MAX_PUFFS = 110, 6, 12
local MOTE_LIFE = 1500
local MOTE_VALUE = { 1, 1, 3, 15 }
local FLASK_CHANCE = { 0.008, 0, 0.05, 1 }

local mx, my, mv, mage = {}, {}, {}, {}
local mn = 0
local fx, fy = {}, {}
local fn, fnext = 0, 0
local px, py, pt = {}, {}, {}
local pn = 0

function L.reset()
  mn, fn, fnext, pn = 0, 0, 0, 0
end

function L.puff(x, y)
  if pn < MAX_PUFFS then
    pn = pn + 1
    px[pn], py[pn], pt[pn] = x, y, 0
  end
end

local function flask(x, y)
  if fn < MAX_FLASKS then
    fn = fn + 1
    fx[fn], fy[fn] = x, y
  else
    fnext = fnext % MAX_FLASKS + 1
    fx[fnext], fy[fnext] = x, y
  end
end

-- Called when an enemy of `kind` dies at (x, y).
function L.drop(x, y, kind)
  local v = MOTE_VALUE[kind]
  if mn < MAX_MOTES then
    mn = mn + 1
    mx[mn], my[mn], mv[mn], mage[mn] = x, y, v, 0
  else
    local j = random(mn)
    mv[j] = mv[j] + v
  end
  if random() < FLASK_CHANCE[kind] then flask(x, y) end
  L.puff(x, y)
end

-- Draws motes in and picks things up. Returns the experience gained.
function L.update()
  local hx, hy = hero.x, hero.y
  local pull = hero.pull
  local pull2 = pull * pull
  local gain = 0
  local i = 1
  while i <= mn do
    local dx, dy = hx - mx[i], hy - my[i]
    local d2 = dx * dx + dy * dy
    local age = mage[i] + 1
    if d2 < 100 then
      gain = gain + mv[i]
      sound.mote()
      mx[i], my[i], mv[i], mage[i] = mx[mn], my[mn], mv[mn], mage[mn]
      mn = mn - 1
    elseif age > MOTE_LIFE or d2 > 160000 then
      mx[i], my[i], mv[i], mage[i] = mx[mn], my[mn], mv[mn], mage[mn]
      mn = mn - 1
    else
      if d2 < pull2 then
        local s = 3.6 / sqrt(d2)
        mx[i], my[i] = mx[i] + dx * s, my[i] + dy * s
      end
      mage[i] = age
      i = i + 1
    end
  end

  i = 1
  while i <= fn do
    local dx, dy = hx - fx[i], hy - fy[i]
    if dx < 12 and dx > -12 and dy < 12 and dy > -12 and hero.oil < hero.maxoil then
      hero.refill()
      sound.flask()
      fx[i], fy[i] = fx[fn], fy[fn]
      fn = fn - 1
      if fnext > fn then fnext = 0 end
    else
      i = i + 1
    end
  end

  i = 1
  while i <= pn do
    local age = pt[i] + 1
    if age > 8 then
      px[i], py[i], pt[i] = px[pn], py[pn], pt[pn]
      pn = pn - 1
    else
      pt[i] = age
      i = i + 1
    end
  end
  return gain
end

function L.draw(camx, camy)
  for i = 1, mn do
    spr(mv[i] >= 3 and 130 or 128, mx[i] - camx - 8, my[i] - camy - 8, 2, 2)
  end
  for i = 1, fn do
    spr(132, fx[i] - camx - 8, fy[i] - camy - 8, 2, 2)
  end
end

function L.draw_puffs(camx, camy)
  for i = 1, pn do
    spr(142, px[i] - camx - 8, py[i] - camy - 8, 2, 2)
  end
end

return L
