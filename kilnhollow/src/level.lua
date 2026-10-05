-- The ironworks: a list of convex sectors, each a polygon with a floor and a
-- ceiling height, and the things standing in them. Two sectors that share
-- an edge are joined by it: the edge is a portal, and the walls of the two
-- sides above and below the opening are drawn from the heights. A sector
-- may be given vertices that lie on another sector's edge (a T-junction):
-- the edge is split there when the level is built.
--
-- Units: 1 is about a centimetre and a half. The player is 52 tall, steps up
-- 20, and the base floor and ceiling (the ones drawn with a texture) are 0
-- and 128.

local art = require("art")
local T = art.textures

local M = {}

M.BASE_CEIL = 128

local function rect(x0, y0, x1, y1)
  return { x0, y0, x1, y0, x1, y1, x0, y1 }
end

-- Sectors in the order they are listed. A name is for the things below and
-- for the doors.
local function define()
  local defs = {}
  local function S(name, verts, p)
    p.name, p.verts = name, verts
    p.light = p.light or 5
    p.wall = p.wall or T.stone
    defs[#defs + 1] = p
    return p
  end

  -- The room you start in, a door in its north wall.
  S("start", rect(0, 0, 320, 224), { floor = 0, ceil = 128, light = 6, wall = T.brick, upper = T.plate })
  S("door1", rect(128, 224, 192, 256), {
    floor = 0, ceil = 0, light = 5, wall = T.plate, facing = T.door, fam = "floor_steel",
    door = { open = 84 },
  })

  -- The casting hall, an octagon, with a stair up to a dais at its north end
  -- and a locked gate on the dais.
  S("hall", { 96, 256, 416, 256, 480, 320, 480, 576, 416, 640, 96, 640, 32, 576, 32, 320 },
    { floor = 0, ceil = 128, light = 4, wall = T.stone, upper = T.plate })
  S("step1", rect(160, 640, 352, 672), {
    floor = 16, ceil = 128, light = 5, wall = T.timber, facing = T.hazard, fam = "floor_brass",
  })
  S("step2", rect(160, 672, 352, 704), {
    floor = 32, ceil = 128, light = 5, wall = T.timber, facing = T.hazard, fam = "floor_brass",
  })
  S("dais", rect(128, 704, 384, 800), {
    floor = 48, ceil = 128, light = 6, wall = T.brick, fam = "floor_brass", lower = T.hazard,
  })
  S("gate", rect(224, 800, 288, 832), {
    floor = 48, ceil = 48, light = 5, wall = T.plate, facing = T.locked, fam = "floor_steel",
    door = { open = 80, locked = true },
  })
  S("gallery", { 96, 832, 416, 832, 416, 980, 320, 1056, 192, 1056, 96, 980 }, {
    floor = 0, ceil = 128, light = 5, wall = T.pipe, upper = T.plate,
  })
  S("sluice", rect(192, 1056, 320, 1152), {
    floor = 0, ceil = 128, light = 7, wall = T.plate,
    over = { [3] = { mid = T.lever } }, lever = 3,
  })

  -- The pump room off the hall's west side.
  S("pumps", rect(-160, 352, 32, 544), {
    floor = 0, ceil = 96, light = 4, wall = T.moss, upper = T.plate,
  })

  -- Out of the hall's east side, a crawlway and then the cold store, with a
  -- loading dock at its east end.
  S("crawl", rect(480, 384, 576, 448), {
    floor = 0, ceil = 60, light = 3, wall = T.plate,
  })
  S("store", rect(576, 256, 800, 576), {
    floor = 0, ceil = 96, light = 3, wall = T.crate, upper = T.plate,
  })
  S("dock", rect(800, 320, 896, 512), {
    floor = 16, ceil = 96, light = 3, wall = T.timber, facing = T.hazard, fam = "floor_wood",
  })
  S("dock_n", rect(800, 512, 896, 576), {
    floor = 0, ceil = 96, light = 3, wall = T.crate,
  })
  S("dock_s", rect(800, 256, 896, 320), {
    floor = 0, ceil = 96, light = 3, wall = T.crate,
  })

  -- A passage from the store to the kiln.
  S("passage", rect(640, 576, 736, 672), { floor = 0, ceil = 112, light = 3, wall = T.stone })

  -- The kiln: an octagon with a glowing ceiling light, four furnace doors
  -- in its diagonal walls, and an alcove with the key.
  S("kiln", { 592, 672, 784, 672, 920, 808, 920, 1000, 784, 1136, 592, 1136, 456, 1000, 456, 808 }, {
    floor = 0, ceil = 128, light = 7, wall = T.brick, upper = T.plate,
    flicker = true,
  })
  S("alcove", rect(640, 1136, 736, 1184), {
    floor = 16, ceil = 128, light = 6, wall = T.plate, facing = T.hazard, fam = "floor_brass",
    over = { [3] = { mid = T.grate } },
  })
  -- Furnace doors, shut until the key is taken.
  local function furnace(name, verts)
    S(name, verts, {
      floor = 0, ceil = 0, light = 6, wall = T.grate, facing = T.grate,
      door = { open = 76, trigger = true },
    })
  end
  furnace("oven_nw", { 480, 1024, 552, 1096, 504, 1144, 432, 1072 })
  furnace("oven_ne", { 808, 1112, 880, 1040, 928, 1088, 856, 1160 })
  furnace("oven_sw", { 568, 696, 496, 768, 448, 720, 520, 648 })
  furnace("oven_se", { 896, 784, 824, 712, 872, 664, 944, 736 })
  return defs
end

-- What stands where: kind, x, y and, for a creature, the way it faces.
-- Creatures sleep until they see the player or hear a shot.
local THINGS = {
  { "lamp", 160, 150 }, { "barrel", 24, 24 }, { "barrel", 296, 28 }, { "lamp", 60, 40 },
  { "ammo", 270, 40 },
  -- The hall.
  { "cog", 250, 420 }, { "cog", 300, 560 }, { "cog", 150, 470 }, { "kettle", 400, 500 },
  { "ammo", 100, 300 }, { "lamp", 256, 450 }, { "barrel", 100, 600 }, { "barrel", 440, 300 },
  { "tonic", 400, 600 },
  { "gun2", 256, 750 }, { "ammo", 180, 760 }, { "ammo", 330, 760 },
  -- The pump room.
  { "cog", -110, 400 }, { "cog", -60, 500 }, { "tonic", -140, 520 }, { "ammo", -140, 370 },
  { "barrel", -30, 372 },
  -- The cold store and the dock.
  { "cog", 640, 300 }, { "cog", 700, 500 }, { "cog", 760, 380 }, { "kettle", 850, 420 },
  { "ammo", 850, 360 }, { "tonic", 620, 530 }, { "barrel", 830, 540 }, { "barrel", 600, 280 },
  { "lamp", 690, 420 },
  -- The kiln, and the key in its alcove.
  { "cog", 560, 900 }, { "cog", 820, 960 }, { "kettle", 690, 1050 },
  { "key", 688, 1160 }, { "ammo", 520, 840 }, { "tonic", 860, 880 }, { "lamp", 688, 905 },
  { "ammo", 640, 790 }, { "ammo", 740, 790 }, { "ammo", 688, 1085 }, { "tonic", 560, 1000 },
  -- Behind the furnace doors.
  { "cog", 500, 1090, "oven_nw" }, { "cog", 470, 1060, "oven_nw" },
  { "kettle", 868, 1100, "oven_ne" }, { "cog", 890, 1080, "oven_ne" },
  { "cog", 520, 705, "oven_sw" }, { "cog", 490, 730, "oven_sw" },
  { "kettle", 900, 720, "oven_se" }, { "cog", 880, 700, "oven_se" },
  -- The gallery.
  { "cog", 150, 900 }, { "cog", 360, 920 }, { "kettle", 150, 1010 }, { "kettle", 360, 1010 },
  { "cog", 256, 960 }, { "ammo", 120, 860 }, { "tonic", 400, 850 }, { "lamp", 256, 1000 },
  { "ammo", 256, 880 },
}

M.START = { x = 160, y = 60, a = math.pi / 2 }

local function inside(sector, x, y)
  local v = sector.verts
  local n = #v // 2
  for i = 1, n do
    local j = i % n + 1
    local ax, ay = v[2 * i - 1], v[2 * i]
    local bx, by = v[2 * j - 1], v[2 * j]
    if (bx - ax) * (y - ay) - (by - ay) * (x - ax) < 0 then return false end
  end
  return true
end
M.inside = inside

-- Make the sectors: orient them, split edges at the corners other sectors
-- put on them, and join the edges two sectors share.
function M.build()
  local defs = define()
  -- Every polygon counter-clockwise.
  for _, s in ipairs(defs) do
    local v, area = s.verts, 0
    local n = #v // 2
    for i = 1, n do
      local j = i % n + 1
      area = area + v[2 * i - 1] * v[2 * j] - v[2 * j - 1] * v[2 * i]
    end
    if area < 0 then
      local r = {}
      for i = n, 1, -1 do r[#r + 1], r[#r + 2] = v[2 * i - 1], v[2 * i] end
      s.verts = r
    end
  end
  -- Every corner there is.
  local corners = {}
  for _, s in ipairs(defs) do
    for i = 1, #s.verts, 2 do corners[#corners + 1] = { s.verts[i], s.verts[i + 1] } end
  end
  local byname = {}
  local edgemap = {}
  for si, s in ipairs(defs) do
    s.id, s.edges, s.seen, s.things_n = si, {}, 0, 0
    s.dark = (7 - s.light) * 0.6
    s.base_dark = s.dark
    s.fam_i = s.fam and art.strips[s.fam] or nil
    s.base_floor = s.fam == nil
    byname[s.name] = s
    local v = s.verts
    local n = #v // 2
    local sx, sy = 0, 0
    for i = 1, n do
      local j = i % n + 1
      local ax, ay, bx, by = v[2 * i - 1], v[2 * i], v[2 * j - 1], v[2 * j]
      local ex, ey = bx - ax, by - ay
      local len2 = ex * ex + ey * ey
      sx, sy = sx + ax, sy + ay
      -- Corners strictly inside this edge, nearest first.
      local cuts = {}
      for _, c in ipairs(corners) do
        local px, py = c[1] - ax, c[2] - ay
        if ex * py - ey * px == 0 then
          local d = px * ex + py * ey
          if d > 0 and d < len2 then cuts[#cuts + 1] = { c[1], c[2], d } end
        end
      end
      table.sort(cuts, function(p, q) return p[3] < q[3] end)
      local pts = { { ax, ay } }
      local last = ax * 1000003 + ay
      for _, c in ipairs(cuts) do
        if c[1] * 1000003 + c[2] ~= last then
          pts[#pts + 1] = { c[1], c[2] }
          last = c[1] * 1000003 + c[2]
        end
      end
      pts[#pts + 1] = { bx, by }
      for k = 1, #pts - 1 do
        local p, q = pts[k], pts[k + 1]
        local dx, dy = q[1] - p[1], q[2] - p[2]
        local len = math.sqrt(dx * dx + dy * dy)
        local e = {
          ax = p[1], ay = p[2], bx = q[1], by = q[2], ex = dx, ey = dy,
          len = len, len2 = len * len,
          nx = dy / len, ny = -dx / len, -- outward
          sector = s, orig = i,
        }
        s.edges[#s.edges + 1] = e
        edgemap[p[1] .. "," .. p[2] .. "," .. q[1] .. "," .. q[2]] = e
      end
    end
    s.cx, s.cy = sx / n, sy / n
  end
  -- Join the edges that two sectors share, and choose what each side shows.
  for _, s in ipairs(defs) do
    for _, e in ipairs(s.edges) do
      local o = edgemap[e.bx .. "," .. e.by .. "," .. e.ax .. "," .. e.ay]
      if o then e.next, e.back = o.sector, o end
    end
  end
  for _, s in ipairs(defs) do
    local over = s.over
    for _, e in ipairs(s.edges) do
      local o = over and over[e.orig]
      e.mid = o and o.mid or s.wall
      local n = e.next
      e.upper = (n and n.facing) or (o and o.upper) or s.upper or s.wall
      e.lower = (n and n.facing) or (o and o.lower) or s.lower or s.wall
      if s.lever == e.orig then e.lever = true end
    end
    if s.door then
      s.door.shut = s.floor
      s.door.state = "shut"
      s.door.timer = 0
    end
  end
  local things = {}
  for _, t in ipairs(THINGS) do
    things[#things + 1] = { kind = t[1], x = t[2], y = t[3], wake = t[4] }
  end
  return { sectors = defs, byname = byname, things = things }
end

return M
