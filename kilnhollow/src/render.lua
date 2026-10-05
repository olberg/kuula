-- The renderer: a portal engine. The view starts in the sector the player is
-- in. Each of its walls that faces the player is projected to a run of
-- screen columns; a solid wall is drawn there and closes the columns, a
-- portal draws the steps above and below its opening, narrows the columns'
-- window to the opening and goes on into the sector beyond. Sectors are
-- convex, so the walls of one never overlap on screen and the order of the
-- walls within it does not matter; each column is walked on its own, nearest
-- sector first, through its own window (top[c] to bot[c]).
--
-- A column here is two pixels wide: the heights are worked out once for the
-- pair, the texture position for each of its pixels. Half the columns is
-- what keeps a frame inside the budget, since what a column costs is
-- Lua instructions, not pixels.
--
-- A wall column is a `tline` of the wall texture sheet with the sheet
-- stepping down it. A floor or ceiling that is not the base one is a column
-- of a strip, a picture the size of the view that is dark towards the
-- horizon, copied with `sspr`. The base floor and ceiling, which most
-- sectors have, are one `tline` for every row of the view, stepping across
-- the texture in the way the row meets the plane, which is what makes them
-- perspective-correct. That is drawn first, and what else the sectors have
-- goes over it.

local art = require("art")
local level = require("level")

local R = {}

local W, VH = 320, 200
local P = W // 2           -- columns of two pixels
local CX, HZ = W // 2, VH // 2
local F = 160              -- the distance to the screen, in pixels: a 90 degree view
local INVF = 1 / F
local NEAR = 6
local KZ = 1 / 100         -- bands of distance
local SY = art.strip_y
-- The darkest tone of the base floor and of the base ceiling.
local FLOOR_BLACK, CEIL_BLACK = 16 + 8 * 8 + 7, 16 + 8 * 1 + 7

local T = art.textures
local floor, ceil = math.floor, math.ceil
local cos, sin = math.cos, math.sin

local top, bot = {}, {}
local XOFF = {}
for p = 1, P do XOFF[p] = (2 * p - 2) % 8 end
local frame = 0
local px, py, ez, cs, sn = 0, 0, 40, 1, 0
local walls_buf, sprites_buf

-- How far a sprite's window may differ from its run's before the run ends.
local TOLERANCE = 3
-- The most a sprite is scaled by.
local MAX_SCALE = 4

function R.init(walls, sprites)
  walls_buf, sprites_buf = walls, sprites
end

-- The base floor and ceiling: one line for every two rows, drawn two thick.
-- The rows so far off that they are in the darkest band are one colour and
-- are filled.
local function planes(base_dark)
  local fx, fy, rx, ry = cs, sn, sn, -cs
  local kl = (0.5 - CX) * INVF
  local ax, ay = fx + rx * kl, fy + ry * kl
  local floor_sx, ceil_sx = T.flag * 64, T.ceiling * 64
  local eyef = ez * F
  local dark_z = (7 - base_dark) / KZ
  -- The rows from the horizon that are in the darkest band.
  local far = floor(eyef / dark_z)
  if far > HZ then far = HZ end
  for r = HZ + far, VH - 1, 2 do
    local z = eyef / (r + 1 - HZ)
    local b = (z * KZ + base_dark) // 1
    if b > 7 then b = 7 end
    local st = z * INVF
    tline(0, r, W - 1, r, px + z * ax, py + z * ay, rx * st, ry * st, floor_sx, b * 64, 64, 64, 2)
  end
  local ceilf = (level.BASE_CEIL - ez) * F
  local cfar = floor(ceilf / dark_z)
  if cfar > HZ then cfar = HZ end
  -- Down to row -1: where the first of these rows is an odd one the last
  -- line is half above the screen, and its lower half is row 0.
  for r = HZ - cfar - 2, -1, -2 do
    local z = ceilf / (HZ - r - 1)
    local b = (z * KZ + base_dark) // 1
    if b > 7 then b = 7 end
    local st = z * INVF
    tline(0, r, W - 1, r, px + z * ax, py + z * ay, rx * st, ry * st, ceil_sx, b * 64, 64, 64, 2)
  end
  if cfar > 0 then
    clip(0, HZ - cfar, W, cfar)
    cls(CEIL_BLACK)
  end
  if far > 0 then
    clip(0, HZ, W, far)
    cls(FLOOR_BLACK)
  end
  clip(0, 0, W, VH)
end

local function visit(sec, lo, hi, depth)
  sec.seen = frame
  local wt, wb, wz, ws
  if sec.things_n > 0 then
    wt, wb, wz, ws = sec.wt, sec.wb, sec.wz, sec.ws
    if not wt then
      wt, wb, wz, ws = {}, {}, {}, {}
      sec.wt, sec.wb, sec.wz, sec.ws = wt, wb, wz, ws
    end
  end
  local ceilh, floorh = sec.ceil, sec.floor
  local hc, hf = (ceilh - ez) * F, (floorh - ez) * F
  local dark = sec.dark
  local fsx = sec.floor_sx
  local plain = not (wt or fsx)
  local edges = sec.edges
  for i = 1, #edges do
    local e = edges[i]
    local ax, ay = e.ax - px, e.ay - py
    local bx, by = e.bx - px, e.by - py
    -- Only a wall seen from its inside is drawn: the viewer is to the left
    -- of it, so a to b runs counter-clockwise round the viewer.
    if ax * by - ay * bx > 0 then
      local zr, zl = ax * cs + ay * sn, bx * cs + by * sn
      if zr > NEAR or zl > NEAR then
        local xr, xl = ax * sn - ay * cs, bx * sn - by * cs
        local ul, ur = 0, e.len
        if zl < NEAR then
          local t = (NEAR - zl) / (zr - zl)
          xl = xl + t * (xr - xl)
          ul = t * ur
          zl = NEAR
        elseif zr < NEAR then
          local t = (NEAR - zr) / (zl - zr)
          xr = xr + t * (xl - xr)
          ur = ur - t * ur
          zr = NEAR
        end
        local sxl, sxr = CX + F * xl / zl, CX + F * xr / zr
        if sxr > sxl then
          -- The columns whose middle, between their two pixels, is on it.
          local c1, c2 = ceil((sxl + 1) * 0.5), ceil((sxr + 1) * 0.5) - 1
          if c1 < lo then c1 = lo end
          if c2 > hi then c2 = hi end
          if c1 <= c2 then
            local n = e.next
            local izl, izr = 1 / zl, 1 / zr
            local span = 1 / (sxr - sxl)
            local diz = (izr - izl) * span
            local duz = (ur * izr - ul * izl) * span
            local s0 = c1 + c1 - 1 - sxl
            local iz = izl + s0 * diz
            local uz = ul * izl + s0 * duz
            -- The step to the next column.
            diz, duz = diz * 2, duz * 2
            if not n then
              -- A solid wall: its columns are drawn and closed.
              local sxm = e.mid * 64
              for c = c1, c2 do
                local t, b = top[c], bot[c]
                if t <= b then
                  local yc = (HZ - hc * iz + 0.5) // 1
                  local yf = (HZ - hf * iz + 0.5) // 1
                  local z = 1 / iz
                  local x = c + c - 2
                  if wt then wt[c], wb[c], wz[c], ws[c] = t, b, z, frame end
                  if fsx then
                    local fs = yf
                    if fs < t then fs = t end
                    if fs <= b then sspr(fsx + XOFF[c], SY + fs, 2, b - fs + 1, x, fs) end
                  end
                  local w0 = yc > t and yc or t
                  local w1 = yf - 1 < b and yf - 1 or b
                  if w0 <= w1 then
                    local zf = z * INVF
                    local band = (z * KZ + dark) // 1
                    if band > 7 then band = 7 end
                    local v0, sy = (w0 + 0.5 - HZ) * zf - ez, band * 64
                    tline(x, w0, x, w1, uz * z, v0, 0, zf, sxm, sy, 64, 64, 2)
                  end
                  top[c] = VH
                end
                iz = iz + diz
                uz = uz + duz
              end
            else
              local nceil, nfloor = n.ceil, n.floor
              local hasup, haslow = nceil < ceilh, nfloor > floorh
              if plain and nceil == ceilh and nfloor == floorh then
                -- Nothing to draw or narrow: the sector beyond has the same
                -- floor and ceiling.
                if depth < 12 then visit(n, c1, c2, depth + 1) end
              else
                local nc, nf = (nceil - ez) * F, (nfloor - ez) * F
                local sxu, sxlow = e.upper * 64, e.lower * 64
                local first, last
                for c = c1, c2 do
                  local t, b = top[c], bot[c]
                  if t <= b then
                    local yc = (HZ - hc * iz + 0.5) // 1
                    local yf = (HZ - hf * iz + 0.5) // 1
                    local x = c + c - 2
                    if wt then wt[c], wb[c], wz[c], ws[c] = t, b, 1 / iz, frame end
                    if fsx then
                      local fs = yf
                      if fs < t then fs = t end
                      if fs <= b then sspr(fsx + XOFF[c], SY + fs, 2, b - fs + 1, x, fs) end
                    end
                    local ot, ob = yc, yf
                    if hasup then
                      local ycn = (HZ - nc * iz + 0.5) // 1
                      ot = ycn
                      local w0 = yc > t and yc or t
                      local w1 = ycn - 1 < b and ycn - 1 or b
                      if w0 <= w1 then
                        local z = 1 / iz
                        local zf = z * INVF
                        local band = (z * KZ + dark) // 1
                        if band > 7 then band = 7 end
                        local v0, sy = (w0 + 0.5 - HZ) * zf - ez, band * 64
                        tline(x, w0, x, w1, uz * z, v0, 0, zf, sxu, sy, 64, 64, 2)
                      end
                    end
                    if haslow then
                      local yfn = (HZ - nf * iz + 0.5) // 1
                      ob = yfn
                      local w0 = yfn > t and yfn or t
                      local w1 = yf - 1 < b and yf - 1 or b
                      if w0 <= w1 then
                        local z = 1 / iz
                        local zf = z * INVF
                        local band = (z * KZ + dark) // 1
                        if band > 7 then band = 7 end
                        local v0, sy = (w0 + 0.5 - HZ) * zf - ez, band * 64
                        tline(x, w0, x, w1, uz * z, v0, 0, zf, sxlow, sy, 64, 64, 2)
                      end
                    end
                    if ot > t then t = ot end
                    if ob - 1 < b then b = ob - 1 end
                    top[c], bot[c] = t, b
                    if t <= b then
                      if not first then first = c end
                      last = c
                    end
                  end
                  iz = iz + diz
                  uz = uz + duz
                end
                if first and depth < 12 then visit(n, first, last, depth + 1) end
              end
            end
          end
        end
      end
    end
  end
end

-- The sprites of `things` in the sectors that were seen, far to near, each
-- clipped to the window its sector was entered through.
local function sprites(things)
  local list = {}
  for i = 1, #things do
    local t = things[i]
    local sec = t.sec
    if sec.seen == frame and not t.hidden then
      local dx, dy = t.x - px, t.y - py
      local z = dx * cs + dy * sn
      if z > NEAR * 2 then
        local x = dx * sn - dy * cs
        local scale = F / z
        local half = t.half * scale
        local sx = CX + x * scale
        if sx + half >= 0 and sx - half < W then
          list[#list + 1] = { t = t, z = z, x = x, sec = sec }
        end
      end
    end
  end
  table.sort(list, function(a, b) return a.z > b.z end)
  sheet(sprites_buf)
  for k = 1, #list do
    local v = list[k]
    local t, z, sec = v.t, v.z, v.sec
    local s = art.sprites[t.sprite]
    local scale = F / z
    local sc = t.sc
    -- Close up it stops growing: what it costs is the pixels it covers.
    local size = scale > MAX_SCALE and MAX_SCALE or scale
    local wpx, hpx = s[3] * sc * size, s[4] * sc * size
    local dx = floor(CX + v.x * scale - wpx * 0.5 + 0.5)
    local dw, dh = floor(wpx + 0.5), floor(hpx + 0.5)
    local dy = floor(HZ - (sec.floor + t.zoff - ez) * scale - hpx + 0.5)
    if dw > 0 and dh > 0 then
      local maps = nil
      if t.ramps then
        local b = (z * KZ + sec.dark) // 1
        local cap = t.cap or 7
        if b > cap then b = cap end
        if b > 0 then
          maps = t.maps[b]
          for i = 1, #maps, 2 do pal_map(maps[i], maps[i + 1]) end
        end
      end
      -- The columns it covers, as pairs.
      local c1, c2 = dx // 2 + 1, (dx + dw - 1) // 2 + 1
      if c1 < 1 then c1 = 1 end
      if c2 > P then c2 = P end
      local wt, wb, wz, ws = sec.wt, sec.wb, sec.wz, sec.ws
      local sx, sy, sw, sh = s[1], s[2], s[3], s[4]
      local r0, rt, rb, st, sb, r1
      for c = c1, c2 + 1 do
        local ok = c <= c2 and ws[c] == frame and wz[c] > z
        local wtc, wbc
        if ok then wtc, wbc = wt[c], wb[c] end
        if r0 and not (ok and wtc - st <= TOLERANCE and st - wtc <= TOLERANCE
            and wbc - sb <= TOLERANCE and sb - wbc <= TOLERANCE) then
          if rt <= rb then
            clip(r0 + r0 - 2, rt, (r1 - r0 + 1) * 2, rb - rt + 1)
            sspr(sx, sy, sw, sh, dx, dy, dw, dh)
          end
          r0 = nil
        end
        if ok then
          if not r0 then
            r0, st, sb, rt, rb = c, wtc, wbc, wtc, wbc
          else
            if wtc > rt then rt = wtc end
            if wbc < rb then rb = wbc end
          end
          r1 = c
        end
      end
      if maps then pal_map() end
    end
  end
  clip()
end

-- Draw the view from `cam` (x, y, z, a, sec) over the top VH rows of the
-- screen, with `things` standing in the sectors.
function R.draw(cam, things, sectors)
  frame = frame + 1
  px, py, ez = cam.x, cam.y, cam.z
  cs, sn = cos(cam.a), sin(cam.a)
  sheet(walls_buf)
  for i = 1, #sectors do sectors[i].things_n = 0 end
  for i = 1, #things do
    local t = things[i]
    if not t.hidden then t.sec.things_n = t.sec.things_n + 1 end
  end
  clip(0, 0, W, VH)
  planes(cam.sec.dark)
  for c = 1, P do top[c], bot[c] = 0, VH - 1 end
  visit(cam.sec, 1, P, 0)
  sprites(things)
  clip()
end

R.W, R.VH, R.CX, R.HZ, R.F = W, VH, CX, HZ, F

return R
