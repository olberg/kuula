-- The level as it is played: the player, the creatures, the pickups and the
-- doors, one tick at a time.

local art = require("art")
local level = require("level")
local world = require("world")
local sound = require("sound")

local G = {}

local sqrt, cos, sin, atan = math.sqrt, math.cos, math.sin, math.atan
local random = math.random

local RAMP = {
  brick = 0, plate = 1, stone = 2, timber = 3, moss = 4, brass = 5, rust = 6,
  slate = 7, flag = 8, ember = 9, copper = 10, steel = 11, skin = 12, violet = 13,
}
-- What of a creature darkens with distance. Its glow, the ember of an eye
-- or of a fire, is left out, so that it shows however far off or however
-- dark the room; and `cap` below stops the rest one band down, where a
-- wall goes on to black, so a creature stands out from what is behind it.
local METAL = { RAMP.copper, RAMP.brass, RAMP.plate }
local WOOD = { RAMP.timber, RAMP.brass, RAMP.plate, RAMP.steel }

-- What a kind of thing is. `half` is half its width in world units.
local KINDS = {
  cog = { enemy = true, hp = 20, r = 11, speed = 1.7, sc = 1.25, cap = 1, ramps = METAL, walk = { "cog_walk1", "cog_walk2" } },
  kettle = { enemy = true, hp = 46, r = 15, speed = 0.85, sc = 1.3, cap = 1, ramps = METAL, walk = { "ket_walk1", "ket_walk2" } },
  ammo = { item = true, r = 22, sc = 1.0, ramps = WOOD, sprite = "ammo" },
  tonic = { item = true, r = 22, sc = 1.0, ramps = WOOD, sprite = "tonic" },
  key = { item = true, r = 22, sc = 1.2, ramps = WOOD, sprite = "key" },
  gun2 = { item = true, r = 24, sc = 1.0, ramps = WOOD, sprite = "gun2" },
  lamp = { r = 0, sc = 1.0, sprite = "lamp", zoff = 76 },
  barrel = { r = 0, sc = 1.2, ramps = { RAMP.rust, RAMP.plate }, sprite = "barrel" },
}

-- For each kind, the palette changes that make it `b` bands darker.
for _, k in pairs(KINDS) do
  if k.ramps then
    k.maps = {}
    for b = 1, 7 do
      local m = {}
      for _, r in ipairs(k.ramps) do
        for tone = 0, 6 do
          m[#m + 1] = 16 + r * 8 + tone
          m[#m + 1] = 16 + r * 8 + math.min(7, tone + b)
        end
      end
      k.maps[b] = m
    end
  end
end

local WEAPONS = {
  { name = "RIVET GUN", cost = 1, cool = 16, pellets = 1, spread = 0.012, lo = 13, hi = 21, frame = "hand1" },
  { name = "SCATTER-GUN", cost = 2, cool = 44, pellets = 6, spread = 0.17, lo = 5, hi = 10, frame = "hand2" },
}
G.WEAPONS = WEAPONS

-- With no rivets left the gun is swung: a blow at arm's length, slower and
-- weaker than a shot, and quiet, so nothing in the next room hears it.
local STRIKE = { reach = 60, wide = 10, cool = 24, lo = 9, hi = 15, swing = 12 }
G.STRIKE = STRIKE

local EYE = 42
local RUN, STRAFE, TURN = 2.3, 1.9, 0.052

-- The state of a run, kept in these globals-in-a-table so that the HUD, the
-- renderer and a test can read them.
G.lv = nil
G.p = nil
G.things = nil
G.enemies = nil
G.doors = nil
G.frame = 0

local lv, p, things, enemies, doors
local heardid = 0
local active = {}

local function say(text)
  p.msg, p.msgt = text, 140
end

-- Build the level, once: it costs more than a frame has.
function G.init()
  lv = level.build()
  G.lv = lv
  for _, s in ipairs(lv.sectors) do
    s.floor0, s.ceil0, s.light0 = s.floor, s.ceil, s.light
  end
  doors = {}
  for _, s in ipairs(lv.sectors) do
    if s.door then doors[#doors + 1] = s end
  end
  G.doors = doors
  for _, d in ipairs(lv.things) do
    d.sec = world.find(lv, d.x, d.y, nil)
  end
end

-- Start a run: the sectors as they were built, everything standing where
-- the level puts it.
function G.new(mercy)
  for _, s in ipairs(lv.sectors) do
    s.floor, s.ceil, s.light = s.floor0, s.ceil0, s.light0
    s.dark = (7 - s.light) * 0.6
    s.floor_sx = s.fam_i and (s.fam_i * 8 + s.light) * 8 or nil
    s.seen, s.things_n = 0, 0
    if s.door then s.door.state, s.door.timer = "shut", 0 end
  end
  local st = level.START
  local sec = world.find(lv, st.x, st.y, nil)
  p = {
    x = st.x, y = st.y, a = st.a, sec = sec, r = 14,
    hp = 100, ammo = 30, weapon = 1, has2 = false, key = false,
    kills = 0, cool = 0, bobt = 0, ez = sec.floor + EYE, flash = 0, hurt = 0, strike = 0,
    msg = "", msgt = 0, time = 0, dead = false, won = false, items = 0,
    mercy = mercy, marker = 0,
  }
  G.p = p
  things, enemies = {}, {}
  for i, d in ipairs(lv.things) do
    local k = KINDS[d.kind]
    local t = {
      kind = d.kind, x = d.x, y = d.y, k = k, wake = d.wake, sec = d.sec,
      r = k.r, sc = k.sc, zoff = k.zoff or 0, ramps = k.ramps, maps = k.maps, cap = k.cap,
      sprite = k.sprite or k.walk[1], idx = i,
    }
    t.half = (art.sprites[t.sprite][3] * t.sc) * 0.5
    if k.enemy then
      t.hp, t.state, t.t, t.cool, t.avoid, t.side, t.pain = k.hp, "idle", 0, 0, 0, 1, 0
      enemies[#enemies + 1] = t
    end
    things[#things + 1] = t
  end
  G.things, G.enemies = things, enemies
  G.total = #enemies
  G.frame = 0
end

local function hurt_player(dmg)
  if p.dead or p.mercy then return end
  p.hp = p.hp - dmg
  p.hurt = 14
  sound.play("hurt")
  if p.hp <= 0 then
    p.hp = 0
    p.dead = true
  end
end

local function kill(e)
  e.state = "dead"
  e.t = 0
  e.r = 0
  -- What is left of it lies as dark as the floor it is on.
  e.cap = nil
  p.kills = p.kills + 1
end

local function hurt_enemy(e, dmg)
  e.hp = e.hp - dmg
  if e.state == "idle" then e.state = "chase" end
  if e.hp <= 0 then
    kill(e)
    sound.play("die")
  elseif random() < (e.kind == "cog" and 0.7 or 0.4) then
    e.pain = 9
    sound.play("pain")
  else
    sound.play("hit")
  end
end

-- A shot is heard in the sectors around the player.
local function noise()
  heardid = world.hear(p.sec, 3)
  for i = 1, #enemies do
    local e = enemies[i]
    if e.state == "idle" and e.sec.heard == heardid and not e.wake then
      e.state, e.t = "chase", -10 - (i % 5) * 4
    end
  end
end

local function shoot()
  local w = WEAPONS[p.weapon]
  p.ammo = p.ammo - w.cost
  p.cool = w.cool
  p.flash = 6
  sound.play(p.weapon == 2 and "scatter" or "shot")
  noise()
  local zeye = p.sec.floor + EYE
  for _ = 1, w.pellets do
    local a = p.a + (random() - 0.5) * 2 * w.spread
    local dx, dy = cos(a), sin(a)
    local dist, _, id = world.ray(p.x, p.y, dx, dy, 1600, zeye, 0, p.sec)
    local best, bt = nil, dist
    for i = 1, #enemies do
      local e = enemies[i]
      if e.state ~= "dead" and e.sec.rayid == id then
        local ex, ey = e.x - p.x, e.y - p.y
        local tt = ex * dx + ey * dy
        if tt > 0 and tt < bt then
          local lat = ex * dy - ey * dx
          if lat < 0 then lat = -lat end
          if lat < e.r + 3 then best, bt = e, tt end
        end
      end
    end
    if best then
      hurt_enemy(best, w.lo + random(0, w.hi - w.lo))
      p.marker = 5
    end
  end
end

local function strike()
  p.cool = STRIKE.cool
  p.strike = STRIKE.swing
  local dx, dy = cos(p.a), sin(p.a)
  local dist, _, id = world.ray(p.x, p.y, dx, dy, 1600, p.sec.floor + EYE, 0, p.sec)
  local best, bt = nil, dist
  for i = 1, #enemies do
    local e = enemies[i]
    if e.state ~= "dead" and e.sec.rayid == id then
      local ex, ey = e.x - p.x, e.y - p.y
      local tt = ex * dx + ey * dy
      if tt > 0 and tt < bt and tt - e.r < STRIKE.reach then
        local lat = ex * dy - ey * dx
        if lat < 0 then lat = -lat end
        if lat < e.r + STRIKE.wide then best, bt = e, tt end
      end
    end
  end
  if best then
    hurt_enemy(best, STRIKE.lo + random(0, STRIKE.hi - STRIKE.lo))
    p.marker = 5
  end
end

-- A: a shot if there are rivets for it, with the rivet gun if the
-- scatter-gun wants more than are left, and a blow with the gun if none are.
local function attack()
  if p.ammo < 1 then
    strike()
    return
  end
  if p.ammo < WEAPONS[p.weapon].cost then p.weapon = 1 end
  shoot()
  if p.ammo < 1 then say("OUT OF RIVETS: A SWINGS THE GUN") end
end

-- B: open the door in front of you, or throw the lever.
local function use()
  for i = 1, #doors do
    local s = doors[i]
    local d = s.door
    if not d.trigger then
      local dx, dy = s.cx - p.x, s.cy - p.y
      local dd = sqrt(dx * dx + dy * dy)
      if dd < 96 and dx * cos(p.a) + dy * sin(p.a) > dd * 0.5 then
        if d.locked and not p.key then
          say("LOCKED. IT NEEDS THE BRASS KEY.")
          sound.play("locked")
        elseif d.state == "shut" or d.state == "closing" then
          d.state = "opening"
          d.timer = 0
          sound.play("door")
        end
        return
      end
    end
  end
  local s = lv.byname.sluice
  local dx, dy = 256 - p.x, 1152 - p.y
  local dd = sqrt(dx * dx + dy * dy)
  if p.sec == s and dd < 110 and dx * cos(p.a) + dy * sin(p.a) > dd * 0.4 then
    p.won = true
    say("THE SLUICE IS OPEN.")
    sound.play("win")
  end
end

local function open_ovens()
  for i = 1, #doors do
    local s = doors[i]
    if s.door.trigger then
      s.door.state = "opening"
    end
  end
  for i = 1, #enemies do
    local e = enemies[i]
    if e.wake then e.state, e.t, e.wake = "chase", -30 - (i % 6) * 6, nil end
  end
  say("THE FURNACE DOORS GRIND OPEN.")
  sound.play("door")
end

local function pickups()
  for i = 1, #things do
    local t = things[i]
    if t.k.item and not t.hidden then
      local dx, dy = t.x - p.x, t.y - p.y
      if dx * dx + dy * dy < t.k.r * t.k.r and math.abs(t.sec.floor - p.sec.floor) < 30 then
        local took = false
        if t.kind == "ammo" and p.ammo < 99 then
          p.ammo = math.min(99, p.ammo + 12)
          say("A BOX OF RIVETS.")
          sound.play("pickup")
          took = true
        elseif t.kind == "tonic" and p.hp < 100 then
          p.hp = math.min(100, p.hp + 30)
          say("A DRAUGHT OF TONIC.")
          sound.play("pickup")
          took = true
        elseif t.kind == "key" then
          p.key = true
          say("THE BRASS KEY.")
          sound.play("key")
          took = true
          open_ovens()
        elseif t.kind == "gun2" then
          p.has2, p.weapon = true, 2
          p.ammo = math.min(99, p.ammo + 8)
          p.cool = 12
          say("THE SCATTER-GUN.")
          sound.play("key")
          took = true
        end
        if took then
          t.hidden = true
          p.items = p.items + 1
        end
      end
    end
  end
end

local function update_doors()
  for i = 1, #doors do
    local s = doors[i]
    local d = s.door
    local target = s.floor + d.open
    if d.state == "opening" then
      s.ceil = s.ceil + 2.5
      if s.ceil >= target then
        s.ceil, d.state, d.timer = target, "open", 300
      end
    elseif d.state == "open" then
      if not d.trigger then
        d.timer = d.timer - 1
        if d.timer <= 0 then d.state = "closing" end
      end
    elseif d.state == "closing" then
      local blocked = p.sec == s
      for k = 1, #enemies do
        if enemies[k].sec == s and enemies[k].state ~= "dead" then blocked = true end
      end
      if blocked then
        d.state = "opening"
      else
        s.ceil = s.ceil - 2.5
        if s.ceil <= s.floor then s.ceil, d.state = s.floor, "shut" end
      end
    end
  end
end

local function update_enemy(e, i)
  local k = e.k
  e.t = e.t + 1
  if e.state == "dead" then
    e.sprite = (e.kind == "cog" and (e.t < 8 and "cog_dead1" or "cog_dead2"))
      or (e.t < 10 and "ket_dead1" or "ket_dead2")
    e.zoff = 0
    return
  end
  if e.state == "idle" then
    if (e.t + i) % 8 == 0 then
      local dx, dy = p.x - e.x, p.y - e.y
      if dx * dx + dy * dy < 700 * 700 and not e.wake
          and world.sees(e.x, e.y, e.sec, e.sec.floor + 30, p.x, p.y, p.sec.floor + EYE) then
        e.state, e.t = "chase", -8
      end
    end
    return
  end
  if e.pain > 0 then
    e.pain = e.pain - 1
    return
  end
  if e.t < 0 then return end
  local dx, dy = p.x - e.x, p.y - e.y
  local d = sqrt(dx * dx + dy * dy)
  local state = e.state
  if state == "chase" then
    if e.cool > 0 then e.cool = e.cool - 1 end
    if e.kind == "cog" then
      if d < 42 then
        e.state, e.t = "bite", 0
        return
      end
    elseif e.cool <= 0 and d < 520 and d > 60 and (e.t + i) % 6 == 0
        and world.sees(e.x, e.y, e.sec, e.sec.floor + 36, p.x, p.y, p.sec.floor + EYE) then
      e.state, e.t = "wind", 0
      return
    end
    local ang = atan(dy, dx)
    if e.avoid > 0 then
      e.avoid = e.avoid - 1
      ang = ang + e.side * 1.0
    end
    local sp = k.speed
    if e.kind == "kettle" and d < 170 then sp = 0 end
    -- Far off it moves every other frame, twice as far.
    if d > 650 then
      if (G.frame + i) % 2 == 0 then return end
      sp = sp * 2
    end
    if sp > 0 then
      if world.move(lv, e, cos(ang) * sp, sin(ang) * sp) and e.avoid == 0 and d > 50 then
        e.avoid = 16 + random(0, 10)
        e.side = random() < 0.5 and 1 or -1
      end
    end
    e.sprite = k.walk[(e.t // 8) % 2 + 1]
    -- Keep off the others: every third frame, and only the ones awake.
    if (G.frame + i) % 3 == 0 then
      for j = 1, #active do
        local o = active[j]
        if o ~= e then
          local ox, oy = e.x - o.x, e.y - o.y
          local rr = e.r + o.r
          local d2 = ox * ox + oy * oy
          if d2 < rr * rr and d2 > 0.01 then
            local dd = sqrt(d2)
            world.move(lv, e, ox / dd * 2.4, oy / dd * 2.4)
          end
        end
      end
    end
  elseif state == "bite" then
    e.sprite = "cog_bite"
    if e.t == 9 then
      sound.play("bite")
      if d < 54 then hurt_player(random(4, 8)) end
    end
    if e.t >= 20 then e.state, e.t = "chase", 0 end
  elseif state == "wind" then
    e.sprite = "ket_wind"
    if e.t == 1 then sound.play("hiss") end
    if e.t >= 28 then e.state, e.t = "fire", 0 end
  elseif state == "fire" then
    e.sprite = "ket_fire"
    if e.t == 1 then
      sound.play("scald")
      local near = 1 - math.min(d, 520) / 520
      if world.sees(e.x, e.y, e.sec, e.sec.floor + 36, p.x, p.y, p.sec.floor + EYE)
          and random() < 0.3 + near * 0.5 then
        hurt_player(random(8, 14))
      end
    end
    if e.t >= 10 then e.state, e.t, e.cool = "chase", 0, 60 + random(0, 50) end
  end
end

-- One tick. `inp` holds what is held (up, down, left, right, l1, r1, a) and
-- what was pressed this frame (use, swap).
function G.update(inp)
  G.frame = G.frame + 1
  local fr = G.frame
  p.time = p.time + 1
  if p.cool > 0 then p.cool = p.cool - 1 end
  if p.flash > 0 then p.flash = p.flash - 1 end
  if p.strike > 0 then p.strike = p.strike - 1 end
  if p.hurt > 0 then p.hurt = p.hurt - 1 end
  if p.marker > 0 then p.marker = p.marker - 1 end
  if p.msgt > 0 then p.msgt = p.msgt - 1 end
  -- Turning and walking.
  if inp.left then p.a = p.a + TURN end
  if inp.right then p.a = p.a - TURN end
  local fwd = (inp.up and 1 or 0) - (inp.down and 1 or 0)
  local str = (inp.r1 and 1 or 0) - (inp.l1 and 1 or 0)
  local moving = fwd ~= 0 or str ~= 0
  if moving then
    local c, s = cos(p.a), sin(p.a)
    local f, sd = fwd * RUN, str * STRAFE
    if fwd ~= 0 and str ~= 0 then f, sd = f * 0.8, sd * 0.8 end
    -- The strafe is to the right of the way you face.
    world.move(lv, p, c * f + s * sd, s * f - c * sd)
    p.bobt = p.bobt + 0.2
  end
  local want = p.sec.floor + EYE
  p.ez = p.ez + (want - p.ez) * 0.25
  -- The weapon.
  if inp.swap and p.has2 then
    p.weapon = 3 - p.weapon
    p.cool = 10
  end
  if inp.a and p.cool == 0 then attack() end
  if inp.use then use() end
  pickups()
  update_doors()
  local n = 0
  for i = 1, #enemies do
    local e = enemies[i]
    if e.state ~= "idle" and e.state ~= "dead" then
      n = n + 1
      active[n] = e
    end
  end
  for i = #active, n + 1, -1 do active[i] = nil end
  for i = 1, #enemies do update_enemy(enemies[i], i) end
  -- The kiln flickers.
  for _, s in ipairs(lv.sectors) do
    if s.flicker then
      local l = s.light
      s.light = 6 + ((fr * 7 + (fr // 3) * 5) % 11 < 3 and 1 or 0)
      if s.light ~= l then
        s.dark = (7 - s.light) * 0.6
        s.floor_sx = s.fam_i and (s.fam_i * 8 + s.light) * 8 or nil
          end
    end
  end
end

-- Where the view is taken from.
local cam = { x = 0, y = 0, z = 0, a = 0, sec = nil }
function G.cam()
  cam.x, cam.y, cam.a, cam.sec = p.x, p.y, p.a, p.sec
  cam.z = p.ez + math.sin(p.bobt) * 1.6
  return cam
end

return G
