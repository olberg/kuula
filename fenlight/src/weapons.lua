-- The four weapons. All fire by themselves; the swarm's grid finds what
-- they hit.
local hero = require("hero")
local swarm = require("swarm")
local loot = require("loot")
local sound = require("sound")
local upgrades = require("upgrades")

local W = {}

local cos, sin, atan = math.cos, math.sin, math.atan
local lv = upgrades.lv

-- One row per level.
local BOLT = {
  { n = 1, dmg = 3, every = 28 }, { n = 1, dmg = 4, every = 28 },
  { n = 2, dmg = 3, every = 40 }, { n = 2, dmg = 3, every = 28 },
  { n = 3, dmg = 4, every = 28 }, { n = 3, dmg = 5, every = 22 },
}
local FIREFLIES = {
  { n = 2, dmg = 2, orbit = 34 }, { n = 3, dmg = 2, orbit = 36 },
  { n = 3, dmg = 3, orbit = 38 }, { n = 4, dmg = 3, orbit = 40 },
  { n = 5, dmg = 3, orbit = 44 }, { n = 5, dmg = 4, orbit = 48 },
}
local FLARE = {
  { every = 300, reach = 70, dmg = 6 }, { every = 300, reach = 90, dmg = 6 },
  { every = 230, reach = 90, dmg = 8 }, { every = 230, reach = 110, dmg = 10 },
  { every = 170, reach = 110, dmg = 12 }, { every = 140, reach = 130, dmg = 16 },
}
local EMBERS = {
  { every = 40, life = 150, r = 11, dmg = 1 }, { every = 36, life = 170, r = 12, dmg = 1 },
  { every = 32, life = 190, r = 13, dmg = 2 }, { every = 28, life = 210, r = 14, dmg = 2 },
  { every = 24, life = 240, r = 15, dmg = 3 }, { every = 20, life = 270, r = 16, dmg = 3 },
}

local BOLT_SPEED, BOLT_LIFE, BOLT_RANGE2 = 4.5, 55, 190 * 190
local FLARE_FRAMES = 24
local MAX_BOLTS, MAX_EMBERS = 24, 16

local bx, by, bvx, bvy, bage = {}, {}, {}, {}, {}
local bn = 0
local bolt_cd = 0

local ffx, ffy = {}, {}
local ffn = 0

local flare_cd, flare_id, flare_t = 0, 0, -1
local flare_r = 0

local px, py, page = {}, {}, {}
local pn = 0
local ember_cd = 0

function W.reset()
  bn, ffn, pn = 0, 0, 0
  bolt_cd, flare_cd, ember_cd = 20, 90, 0
  flare_id, flare_t = 0, -1
  swarm.flare(0, 0, 0, 0)
end

local function fire_bolts(p)
  local dx, dy = swarm.near_x - hero.x, swarm.near_y - hero.y
  local a = atan(dy, dx)
  for k = 1, p.n do
    if bn >= MAX_BOLTS then break end
    local b = a + (k - (p.n + 1) / 2) * 0.22
    bn = bn + 1
    bx[bn], by[bn] = hero.x, hero.y
    bvx[bn], bvy[bn] = cos(b) * BOLT_SPEED, sin(b) * BOLT_SPEED
    bage[bn] = 0
  end
  sound.bolt()
end

local function update_bolts()
  local p = BOLT[lv[1]]
  bolt_cd = bolt_cd - 1
  if bolt_cd <= 0 and swarm.near_d2 < BOLT_RANGE2 then
    bolt_cd = p.every
    fire_bolts(p)
  end
  local dmg = p.dmg
  local i = 1
  while i <= bn do
    local x, y = bx[i] + bvx[i], by[i] + bvy[i]
    local age = bage[i] + 1
    local gone = age > BOLT_LIFE
    if not gone and swarm.hit(x, y, 4, dmg, 0, true) > 0 then
      loot.puff(x, y)
      gone = true
    end
    if gone then
      bx[i], by[i], bvx[i], bvy[i], bage[i] = bx[bn], by[bn], bvx[bn], bvy[bn], bage[bn]
      bn = bn - 1
    else
      bx[i], by[i], bage[i] = x, y, age
      i = i + 1
    end
  end
end

local function update_fireflies(t)
  local p = FIREFLIES[lv[2]]
  ffn = p.n
  local spin = t * 0.07
  local step = 6.2832 / ffn
  for k = 1, ffn do
    local a = spin + (k - 1) * step
    local x, y = hero.x + cos(a) * p.orbit, hero.y + sin(a) * p.orbit
    ffx[k], ffy[k] = x, y
    if (t + k) % 2 == 0 then swarm.hit(x, y, 7, p.dmg, 12) end
  end
end

local function update_flare()
  local p = FLARE[lv[3]]
  if flare_t < 0 then
    flare_cd = flare_cd - 1
    if flare_cd <= 0 then
      flare_t, flare_id = 0, flare_id + 1
      sound.bolt(-7)
    end
    return
  end
  flare_t = flare_t + 1
  flare_r = p.reach * flare_t / FLARE_FRAMES
  swarm.flare(flare_id, math.max(0, flare_r - 10), flare_r + 10, p.dmg)
  if flare_t >= FLARE_FRAMES then
    flare_t = -1
    flare_cd = p.every
    swarm.flare(0, 0, 0, 0)
  end
end

local function update_embers(t)
  local p = EMBERS[lv[4]]
  ember_cd = ember_cd - 1
  if ember_cd <= 0 and hero.moving and pn < MAX_EMBERS then
    ember_cd = p.every
    pn = pn + 1
    px[pn], py[pn], page[pn] = hero.x, hero.y, 0
  end
  local i = 1
  while i <= pn do
    local age = page[i] + 1
    if age > p.life then
      px[i], py[i], page[i] = px[pn], py[pn], page[pn]
      pn = pn - 1
    else
      page[i] = age
      -- each patch is looked at every fourth frame; a hurt enemy waits 16 anyway
      if (age + i) % 4 == 0 then swarm.hit(px[i], py[i], p.r, p.dmg, 16) end
      i = i + 1
    end
  end
end

function W.update(t)
  update_bolts()
  if lv[2] > 0 then update_fireflies(t) else ffn = 0 end
  if lv[3] > 0 then update_flare() end
  if lv[4] > 0 then update_embers(t) end
end

-- Under the swarm: the ember patches.
function W.draw_ground(camx, camy)
  for i = 1, pn do
    local age = page[i]
    if age < EMBERS[lv[4]].life - 30 or age % 4 < 2 then
      spr(138 + (age // 8 % 2) * 2, px[i] - camx - 8, py[i] - camy - 8, 2, 2)
    end
  end
end

-- Over the swarm: bolts, fireflies, the flare's ring.
function W.draw(camx, camy)
  for i = 1, bn do
    spr(134, bx[i] - camx - 8, by[i] - camy - 8, 2, 2)
  end
  for k = 1, ffn do
    spr(136, ffx[k] - camx - 8, ffy[k] - camy - 8, 2, 2)
  end
  if flare_t >= 0 then
    local cx, cy = hero.x // 1 - camx, hero.y // 1 - camy
    circ(cx, cy, flare_r, 10)
    circ(cx, cy, flare_r - 3, flare_t < FLARE_FRAMES // 2 and 7 or 9)
  end
end

return W
