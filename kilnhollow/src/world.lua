-- Where things are and where they can go: which sector a point is in, a
-- circle pushed out of the walls, a ray through the portals.

local M = {}

local sqrt = math.sqrt

M.HEIGHT = 52 -- the headroom a creature needs
M.STEP = 20   -- the tallest step it climbs

local HEIGHT, STEP = M.HEIGHT, M.STEP

-- Whether a creature can pass from sector a into sector b: the step up is
-- low enough and the opening between the two ceilings and floors is tall
-- enough.
local function passable(a, b)
  local fa, fb = a.floor, b.floor
  if fb - fa > STEP then return false end
  local lo = fa > fb and fa or fb
  local ca, cb = a.ceil, b.ceil
  local hi = ca < cb and ca or cb
  return hi - lo >= HEIGHT
end
M.passable = passable

local function contains(s, x, y)
  local edges = s.edges
  for i = 1, #edges do
    local e = edges[i]
    if e.ex * (y - e.ay) - e.ey * (x - e.ax) < -0.01 then return false end
  end
  return true
end
M.contains = contains

-- The sector holding (x, y), trying `hint` and its neighbours first.
function M.find(level, x, y, hint)
  if hint then
    if contains(hint, x, y) then return hint end
    local edges = hint.edges
    for i = 1, #edges do
      local n = edges[i].next
      if n and contains(n, x, y) then return n end
    end
  end
  local all = level.sectors
  for i = 1, #all do
    if contains(all[i], x, y) then return all[i] end
  end
  return hint
end

local stamp = 0

-- Push the circle of `ent` (x, y, r, sec) out of every wall of sector `s`
-- within its reach. A portal that cannot be passed is a wall.
local near, nnear = {}, 0

local function push(ent, s, collect)
  local edges = s.edges
  local r = ent.r
  for i = 1, #edges do
    local e = edges[i]
    -- How far the circle's middle is from the line of the wall: the wall is
    -- out of reach unless that is less than the radius.
    local d = (ent.x - e.ax) * e.nx + (ent.y - e.ay) * e.ny
    if d < r and d > -r then
      local n = e.next
      if n and collect then
        nnear = nnear + 1
        near[nnear] = n
      end
      local blocked = true
      if n then
        -- Seen from the side the creature is on: a step down is not a wall
        -- on the lower side's copy of the edge.
        local a, b = s, n
        if ent.sec == n then a, b = n, s end
        local fa, fb = a.floor, b.floor
        if fb - fa <= STEP then
          local lo = fa > fb and fa or fb
          local ca, cb = a.ceil, b.ceil
          local hi = ca < cb and ca or cb
          blocked = hi - lo < HEIGHT
        end
      end
      if blocked then
        local ex, ey = e.ex, e.ey
        local t = ((ent.x - e.ax) * ex + (ent.y - e.ay) * ey) / e.len2
        if t < 0 then t = 0 elseif t > 1 then t = 1 end
        local dx, dy = ent.x - (e.ax + ex * t), ent.y - (e.ay + ey * t)
        local d2 = dx * dx + dy * dy
        if d2 < r * r then
          local dist = sqrt(d2)
          local nx, ny
          if dist < 1e-4 then
            nx, ny = -e.nx, -e.ny
          else
            nx, ny = dx / dist, dy / dist
          end
          ent.x = ent.x + nx * (r - dist)
          ent.y = ent.y + ny * (r - dist)
          ent.hit = true
        end
      end
    end
  end
end

-- Move `ent` by (dx, dy), sliding along what is in the way. Returns whether
-- a wall pushed back.
function M.move(level, ent, dx, dy)
  ent.x, ent.y = ent.x + dx, ent.y + dy
  ent.hit = false
  for pass = 1, 2 do
    if pass == 2 and not ent.hit then break end
    local s = ent.sec
    stamp = stamp + 1
    s.chk = stamp
    -- The sectors whose walls can be in reach are those beyond a portal the
    -- circle is near.
    nnear = 0
    push(ent, s, true)
    for i = 1, nnear do
      local n = near[i]
      if n.chk ~= stamp then
        n.chk = stamp
        push(ent, n)
      end
    end
  end
  -- Which sector it is in now: follow the portals it crossed.
  local s = ent.sec
  local crossed = false
  for _ = 1, 4 do
    local moved = false
    local edges = s.edges
    for i = 1, #edges do
      local e = edges[i]
      if e.ex * (ent.y - e.ay) - e.ey * (ent.x - e.ax) < -0.01 and e.next then
        s, moved, crossed = e.next, true, true
        break
      end
    end
    if not moved then break end
  end
  if crossed and not contains(s, ent.x, ent.y) then
    s = M.find(level, ent.x, ent.y, s)
  end
  ent.sec = s
  return ent.hit
end

local rayid = 0

-- A ray from (x, y) in sector `sec` along the unit vector (dx, dy), at
-- height z that changes by dz for every unit along it, up to `maxd`. It goes
-- through every portal whose opening holds it. Returns how far it got and
-- the edge that stopped it (nil when it did not stop). The sectors it went
-- through carry `rayid` and, in `rayend`, how far along it left them.
function M.ray(x, y, dx, dy, maxd, z, dz, sec)
  rayid = rayid + 1
  local s = sec
  for _ = 1, 24 do
    s.rayid = rayid
    local best, bt = nil, 1e9
    local edges = s.edges
    for i = 1, #edges do
      local e = edges[i]
      local dn = dx * e.nx + dy * e.ny
      if dn > 1e-7 then
        local t = ((e.ax - x) * e.nx + (e.ay - y) * e.ny) / dn
        if t < bt then best, bt = e, t end
      end
    end
    if not best or bt >= maxd then
      s.rayend = maxd
      return maxd, nil, rayid
    end
    if bt < 0 then bt = 0 end
    s.rayend = bt
    local n = best.next
    if not n then return bt, best, rayid end
    local zz = z + dz * bt
    local lo = s.floor > n.floor and s.floor or n.floor
    local hi = s.ceil < n.ceil and s.ceil or n.ceil
    if zz <= lo or zz >= hi then return bt, best, rayid end
    s = n
  end
  return maxd, nil, rayid
end

-- Whether a straight look from (ax, ay) to (bx, by) is open: eye heights
-- az and bz above the floors.
function M.sees(ax, ay, asec, az, bx, by, bz)
  local dx, dy = bx - ax, by - ay
  local d = sqrt(dx * dx + dy * dy)
  if d < 1 then return true end
  local dist, edge = M.ray(ax, ay, dx / d, dy / d, d, az, (bz - az) / d, asec)
  return edge == nil or dist >= d - 1
end

local heard = 0

-- Mark the sectors a sound made in `sec` carries into: through every portal
-- that is not shut, `depth` portals deep.
function M.hear(sec, depth)
  heard = heard + 1
  sec.heard = heard
  local frontier = { sec }
  for _ = 1, depth do
    local nextf = {}
    for _, s in ipairs(frontier) do
      local edges = s.edges
      for i = 1, #edges do
        local n = edges[i].next
        if n and n.heard ~= heard and n.ceil > n.floor + 8 and s.ceil > s.floor + 8 then
          n.heard = heard
          nextf[#nextf + 1] = n
        end
      end
    end
    frontier = nextf
  end
  return heard
end

return M
