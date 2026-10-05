-- The enemies: parallel arrays, a spawn schedule, a 32-pixel grid rebuilt
-- every frame, and the hit tests the weapons go through.
local hero = require("hero")
local loot = require("loot")
local sound = require("sound")

local S = {}

local sqrt, floor, random, cos, sin = math.sqrt, math.floor, math.random, math.cos, math.sin

-- A run in frames, five minutes: the schedule below is laid over it.
S.RUN = 18000
local RUN = S.RUN
local WISP, MIDGE, TOAD, HULK = 1, 2, 3, 4
local HP = { 2, 1, 14, 90 }
local SPEED = { 0.85, 1.6, 0.6, 0.4 }
local SPEED_GAIN = { 1.0, 0.15, 1.0, 0.8 } -- how much quicker by the end of the run
local RAD = { 7, 6, 8, 15 }          -- half-size, for hits
local CONTACT = { 9, 8, 10, 18 }     -- how close touches the hero
local DRAIN = { 0.045, 0.03, 0.07, 0.16 } -- oil per frame while touching
local CELL = { 32, 36, 40, 64 }      -- first sprite cell of each kind
local MAX_DRAIN = 0.5

local GC, GR = 15, 12                -- grid cells; 480 x 384 pixels around the hero
local PEAK = 300                     -- enemies alive at the end of the run

local ex, ey, ek, ehp, ev, eox, eoy, elast, eflare = {}, {}, {}, {}, {}, {}, {}, {}, {}
local n = 0
local hulks = 0
local head, nxt = {}, {}
local gx0, gy0 = 0, 0
local now = 0
local spawn_acc = 0
local fl_id, fl_lo2, fl_hi2, fl_dmg = 0, 0, 0, 0

S.near_x, S.near_y, S.near_d2 = 0, 0, math.huge

function S.reset()
  n, hulks = 0, 0
  spawn_acc = 0
  fl_id = 0
  now = 0
  gx0, gy0 = -240, -192
  for c = 1, GC * GR do head[c] = 0 end
  S.near_d2 = math.huge
end

function S.count()
  return n
end

-- Later arrivals are tougher and quicker.
local function add(x, y, k, t)
  n = n + 1
  ex[n], ey[n] = x, y
  ek[n] = k
  ehp[n] = HP[k] * (1 + t / RUN * 2)
  ev[n] = SPEED[k] * (1 + t / RUN * SPEED_GAIN[k]) * (0.85 + random() * 0.3)
  eox[n], eoy[n] = random(-6, 6), random(-6, 6)
  elast[n] = -100
  if k == HULK then hulks = hulks + 1 end
  eflare[n] = 0
end

-- A point on a ring just outside the screen.
local function ring(hx, hy)
  local a = random() * 6.2832
  local r = 185 + random() * 20
  return hx + cos(a) * r, hy + sin(a) * r * 0.85
end

local function remove(i)
  ex[i], ey[i], ek[i], ehp[i] = ex[n], ey[n], ek[n], ehp[n]
  ev[i], eox[i], eoy[i] = ev[n], eox[n], eoy[n]
  elast[i], eflare[i] = elast[n], eflare[n]
  n = n - 1
end

-- Midges arrive together from one side.
local function burst(t, hx, hy)
  local a = random() * 6.2832
  local px, py = -sin(a), cos(a)
  local bx, by = hx + cos(a) * 190, hy + sin(a) * 170
  for _ = 1, 10 + t // 3600 * 4 do
    local s = random(-70, 70)
    add(bx + px * s + random(-12, 12), by + py * s * 0.85 + random(-12, 12), MIDGE, t)
  end
end

local function spawn(t, hx, hy)
  if t % 3600 == 0 and t > 0 then
    local x, y = ring(hx, hy)
    add(x, y, HULK, t)
    sound.hulk()
  end
  if t >= 900 and t % 450 == 0 then burst(t, hx, hy) end
  local target = math.min(PEAK, 14 + t * 0.026)
  spawn_acc = spawn_acc + 15 + t * 0.004
  local toad = t >= 7200 and 0.05 + 0.25 * (t - 7200) / 10800 or 0
  while spawn_acc >= 60 and n < target do
    spawn_acc = spawn_acc - 60
    local x, y = ring(hx, hy)
    add(x, y, random() < toad and TOAD or WISP, t)
  end
  if spawn_acc > 180 then spawn_acc = 180 end
end

local function kill(i)
  local x, y, k = ex[i], ey[i], ek[i]
  hero.kills = hero.kills + 1
  if k == HULK then hulks = hulks - 1 end
  loot.drop(x, y, k)
  sound.kill()
end

-- Moves everything one frame, buries the dead, rebuilds the grid, adds the
-- newcomers. Returns the oil the hero loses to contact this frame.
function S.update(t)
  now = t
  local hx, hy = hero.x, hero.y
  gx0, gy0 = floor(hx) - 240, floor(hy) - 192
  for c = 1, GC * GR do head[c] = 0 end
  local drain = 0
  local best, bi = math.huge, 0
  local lid, lo2, hi2, ldmg = fl_id, fl_lo2, fl_hi2, fl_dmg
  local i = 1
  while i <= n do
    local hp = ehp[i]
    if hp <= 0 then
      kill(i)
      remove(i)
    else
      local x, y = ex[i], ey[i]
      local dx, dy = hx + eox[i] - x, hy + eoy[i] - y
      local d2 = dx * dx + dy * dy
      local d = sqrt(d2) + 0.01
      if d2 > 62500 then
        x, y = ring(hx, hy)
      else
        local s = ev[i] / d
        if s > 1 then s = 1 end
        x, y = x + dx * s, y + dy * s
        if dx < 17 and dx > -17 and dy < 17 and dy > -17 then
          local k = ek[i]
          local c = CONTACT[k]
          if dx < c and dx > -c and dy < c and dy > -c then drain = drain + DRAIN[k] end
        end
        if lid ~= 0 and d2 > lo2 and d2 < hi2 and eflare[i] ~= lid then
          eflare[i] = lid
          ehp[i] = hp - ldmg
          local push = 10 / d
          x, y = x - dx * push, y - dy * push
          if hp > ldmg then sound.hit() end
        end
      end
      ex[i], ey[i] = x, y
      if d2 < best then best, bi = d2, i end
      local cx, cy = (x - gx0) // 32 | 0, (y - gy0) // 32 | 0
      if cx >= 0 and cx < GC and cy >= 0 and cy < GR then
        local c = cy * GC + cx + 1
        nxt[i] = head[c]
        head[c] = i
      end
      i = i + 1
    end
  end
  if bi ~= 0 then
    S.near_x, S.near_y, S.near_d2 = ex[bi], ey[bi], best
  else
    S.near_d2 = math.huge
  end
  spawn(t, hx, hy)
  if drain > MAX_DRAIN then drain = MAX_DRAIN end
  return drain
end

-- The flare is a ring around the hero; the update loop tests every enemy
-- against it, since it reaches most of them anyway.
function S.flare(id, lo, hi, dmg)
  fl_id, fl_lo2, fl_hi2, fl_dmg = id, lo * lo, hi * hi, dmg
end

-- Hurts what lies within r of (x, y) and has not been hurt in the last
-- `cool` frames. With `one`, stops at the first. Returns how many it hurt.
function S.hit(x, y, r, dmg, cool, one)
  local reach = hulks > 0 and r + 16 or r + 8 -- the widest of the small ones is 8
  local cx0, cx1 = (x - reach - gx0) // 32 | 0, (x + reach - gx0) // 32 | 0
  local cy0, cy1 = (y - reach - gy0) // 32 | 0, (y + reach - gy0) // 32 | 0
  if cx0 < 0 then cx0 = 0 end
  if cy0 < 0 then cy0 = 0 end
  if cx1 >= GC then cx1 = GC - 1 end
  if cy1 >= GR then cy1 = GR - 1 end
  local hits = 0
  for cy = cy0, cy1 do
    local row = cy * GC + 1
    for cx = cx0, cx1 do
      local i = head[row + cx]
      while i ~= 0 do
        local hp = ehp[i]
        if hp > 0 and now - elast[i] >= cool then
          local rr = r + RAD[ek[i]]
          local dx, dy = ex[i] - x, ey[i] - y
          if dx < rr and dx > -rr and dy < rr and dy > -rr then
            ehp[i] = hp - dmg
            elast[i] = now
            hits = hits + 1
            if hp > dmg then sound.hit() end
            if one then return hits end
          end
        end
        i = nxt[i]
      end
    end
  end
  return hits
end

function S.draw(camx, camy, t)
  local ph = t // 12
  for i = 1, n do
    local k = ek[i]
    local x, y = ex[i] - camx, ey[i] - camy
    if k == HULK then
      spr(64 + (ph & 1) * 4, x - 16, y - 16, 4, 4, x > 160)
    else
      spr(CELL[k] + ((ph + i) & 1) * 2, x - 8, y - 8, 2, 2, x > 160)
    end
  end
end

return S
