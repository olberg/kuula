-- What is drawn over and under the view: the weapon in the hand, the status
-- bar, the messages, the map and the screens between runs.

local art = require("art")
local render = require("render")

local H = {}

local W, VH = render.W, render.VH
local sin, cos, floor = math.sin, math.cos, math.floor

local function ix(ramp, tone) return 16 + ramp * 8 + tone end
local PLATE, BRASS, EMBER, RUST = 1, 5, 9, 6

-- A solid rectangle, much cheaper than `rectfill`: a clear is paid for sixty-
-- four pixels at a time.
local function fill(x0, y0, x1, y1, c)
  clip(x0, y0, x1 - x0 + 1, y1 - y0 + 1)
  cls(c)
  clip()
end

local function shadow(text, x, y, c)
  print(text, x + 1, y + 1, 0)
  print(text, x, y, c)
end

local function centred(text, y, c)
  shadow(text, 160 - #text * 4, y, c)
end

-- The weapon, a little larger than it is drawn, with the walk's sway.
function H.weapon(p, weapons)
  local w = weapons[p.weapon]
  local name = w.frame
  if p.flash > 0 then name = name .. "_fire" end
  local s = art.sprites[name]
  local dw, dh = floor(s[3] * 1.5), floor(s[4] * 1.5)
  local sway = sin(p.bobt * 0.5) * 7
  local lift = math.abs(cos(p.bobt * 0.5)) * 4
  local kick = p.flash > 3 and 6 or 0
  -- A blow: the gun goes up and forward and comes back.
  local swing = p.strike
  local jab = swing > 0 and (swing > 8 and (12 - swing) or swing) * 4 or 0
  sheet(H.sprites)
  sspr(s[1], s[2], s[3], s[4], 160 - dw // 2 + sway - jab // 2, VH - dh + 6 + lift + kick - jab, dw, dh)
end

function H.crosshair(p)
  local c = p.marker > 0 and 8 or ix(BRASS, 1)
  line(156, 100, 158, 100, c)
  line(162, 100, 164, 100, c)
  line(160, 96, 160, 98, c)
  line(160, 102, 160, 104, c)
end

-- Red round the edge of the view after a hit.
function H.flash(p)
  if p.hurt <= 0 then return end
  fillp(p.hurt > 6 and 0xFFFF or 0xA5A5)
  local c = 8
  rectfill(0, 0, W - 1, 7, c)
  rectfill(0, VH - 8, W - 1, VH - 1, c)
  rectfill(0, 8, 7, VH - 9, c)
  rectfill(W - 8, 8, W - 1, VH - 9, c)
  fillp()
end

function H.bar(p, weapons)
  local y = VH
  fill(0, y, W - 1, 239, ix(PLATE, 5))
  fill(0, y, W - 1, y, ix(PLATE, 2))
  fill(0, y + 1, W - 1, y + 1, ix(PLATE, 7))
  for _, x in ipairs({ 6, 313 }) do
    fill(x, y + 4, x + 1, y + 5, ix(PLATE, 1))
  end
  -- Health.
  fill(10, y + 6, 104, y + 33, ix(PLATE, 6))
  rect(10, y + 6, 104, y + 33, ix(PLATE, 3))
  print("HEALTH", 16, y + 10, ix(PLATE, 1))
  local hp = p.hp
  shadow(string.format("%3d", hp), 16, y + 21, hp > 30 and 7 or 8)
  fill(52, y + 21, 99, y + 29, ix(PLATE, 7))
  local n = floor(hp * 47 / 100)
  if n > 0 then fill(52, y + 21, 52 + n, y + 29, hp > 30 and 11 or 8) end
  rect(52, y + 21, 99, y + 29, ix(PLATE, 4))
  -- The weapon and the key.
  fill(112, y + 6, 207, y + 33, ix(PLATE, 6))
  rect(112, y + 6, 207, y + 33, ix(PLATE, 3))
  local name = p.ammo > 0 and weapons[p.weapon].name or "GUN BUTT"
  print(name, 160 - #name * 4, y + 10, p.ammo > 0 and ix(BRASS, 1) or 8)
  local keys = p.key and "BRASS KEY" or "NO KEY"
  print(keys, 160 - #keys * 4, y + 22, p.key and ix(BRASS, 0) or ix(PLATE, 3))
  -- Rivets.
  fill(215, y + 6, 309, y + 33, ix(PLATE, 6))
  rect(215, y + 6, 309, y + 33, ix(PLATE, 3))
  print("RIVETS", 221, y + 10, ix(PLATE, 1))
  shadow(string.format("%3d", p.ammo), 221, y + 21, p.ammo > 8 and 7 or 8)
  local pips = math.min(10, p.ammo)
  for i = 0, pips - 1 do
    fill(256 + i * 5, y + 21, 258 + i * 5, y + 29, ix(BRASS, 1 + (i % 2)))
  end
end

function H.message(p)
  if p.msgt > 0 then shadow(p.msg, 6, 6, 7) end
end

-- The map: every wall, centred on the player, a pixel to four units.
function H.map(lv, p)
  cls(ix(PLATE, 7))
  local cx, cy, sc = 160, 100, 0.25
  local c, s = cos(p.a), sin(p.a)
  for _, sec in ipairs(lv.sectors) do
    for _, e in ipairs(sec.edges) do
      -- Rotated so that the way you face is up.
      local ax, ay = e.ax - p.x, e.ay - p.y
      local bx, by = e.bx - p.x, e.by - p.y
      local x0, y0 = cx + (ax * s - ay * c) * sc, cy - (ax * c + ay * s) * sc
      local x1, y1 = cx + (bx * s - by * c) * sc, cy - (bx * c + by * s) * sc
      local col
      if not e.next then
        col = ix(BRASS, 1)
      elseif sec.floor ~= e.next.floor or sec.door or e.next.door then
        col = ix(PLATE, 2)
      else
        col = ix(PLATE, 5)
      end
      line(x0, y0, x1, y1, col)
    end
  end
  line(cx, cy - 5, cx - 3, cy + 3, 8)
  line(cx, cy - 5, cx + 3, cy + 3, 8)
  line(cx - 3, cy + 3, cx + 3, cy + 3, 8)
  shadow("MAP", 6, 6, 7)
end

function H.title(frame)
  sheet(H.title_sheet)
  sspr(0, 0, 320, 240, 0, 0)
  if (frame // 30) % 2 == 0 then centred("PRESS A TO BEGIN", 190, 7) end
  centred("D-PAD MOVE AND TURN   L1 R1 STRAFE", 208, ix(PLATE, 1))
  centred("A FIRE   B USE   Y SWAP", 220, ix(PLATE, 1))
  centred("X MAP   START PAUSE", 230, ix(PLATE, 1))
end

local function box(y0, y1)
  fill(40, y0, 279, y1, ix(PLATE, 7))
  rect(40, y0, 279, y1, ix(PLATE, 2))
  rect(42, y0 + 2, 277, y1 - 2, ix(PLATE, 5))
end

function H.dead(frame)
  box(70, 130)
  centred("SCRAPPED", 82, 8)
  centred("THE VERMIN WIN THIS ROUND", 98, ix(PLATE, 1))
  if (frame // 30) % 2 == 0 then centred("PRESS A TO TRY AGAIN", 112, 7) end
end

function H.paused(frame)
  box(80, 120)
  centred("PAUSED", 92, 7)
  centred("START TO GO ON", 104, ix(PLATE, 1))
end

function H.won(p, total, frame)
  cls(ix(PLATE, 7))
  box(40, 190)
  centred("THE SLUICE IS OPEN", 54, ix(EMBER, 1))
  centred("KILNHOLLOW IS QUIET", 70, ix(PLATE, 1))
  local secs = p.time // 60
  shadow(string.format("TIME    %d:%02d", secs // 60, secs % 60), 90, 104, 7)
  shadow(string.format("KILLS   %d / %d", p.kills, total), 90, 120, 7)
  shadow(string.format("FOUND   %d ITEMS", p.items), 90, 136, 7)
  if (frame // 30) % 2 == 0 then centred("PRESS A", 166, 7) end
end

return H
