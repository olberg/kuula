-- What a level offers. Ids 1 to 4 are the weapons, 5 to 8 the rest; the
-- id is also the icon's place in the sheet's row of upgrade icons.
local hero = require("hero")

local U = {}

local BOLT, FIREFLIES, FLARE, EMBERS, BOOTS, OIL, PULL, MENDING = 1, 2, 3, 4, 5, 6, 7, 8

local defs = {
  { name = "Bolt", max = 6, desc = {
    "A bolt at the nearest foe", "Bolts hit harder", "Two bolts at a time",
    "Bolts come more often", "Three bolts at a time", "Harder, quicker bolts" } },
  { name = "Fireflies", max = 6, desc = {
    "Two lights circle you", "A third light", "The lights burn hotter",
    "A fourth light", "A fifth light", "A wider, hotter circle" } },
  { name = "Flare", max = 6, desc = {
    "A ring of fire bursts out", "A wider ring", "Bursts come sooner",
    "A wider, hotter ring", "Bursts come sooner", "A great, hot ring" } },
  { name = "Embers", max = 6, desc = {
    "Burning patches in your wake", "More patches, lasting longer",
    "The patches burn hotter", "Wider patches", "More, hotter patches",
    "A trail of fire" } },
  { name = "Boots", max = 5, desc = { "Walk faster" } },
  { name = "Oil Reserve", max = 5, desc = { "More oil, and a refill" } },
  { name = "Pull", max = 5, desc = { "Draw motes from farther" } },
  { name = "Mending", max = 5, desc = { "The lamp slowly mends" } },
}

U.lv = {}
U.choice = {}
U.count = 0
U.defs = defs

function U.reset()
  for id = 1, #defs do U.lv[id] = 0 end
  U.lv[BOLT] = 1
  U.count = 0
end

-- Picks up to three different upgrades that can still be taken. The count
-- is 0 once every one is at its last level.
function U.roll()
  local pool = {}
  for id = 1, #defs do
    if U.lv[id] < defs[id].max then pool[#pool + 1] = id end
  end
  U.count = math.min(3, #pool)
  for k = 1, U.count do
    local j = math.random(#pool)
    U.choice[k] = pool[j]
    table.remove(pool, j)
  end
end

function U.text(id)
  local d = defs[id]
  return d.desc[U.lv[id] + 1] or d.desc[1]
end

function U.take(id)
  local lv = U.lv[id] + 1
  U.lv[id] = lv
  if id == BOOTS then
    hero.speed = hero.SPEED * (1 + 0.1 * lv)
  elseif id == OIL then
    hero.maxoil = hero.OIL + 25 * lv
    hero.refill()
  elseif id == PULL then
    hero.pull = hero.PULL * (1 + 0.35 * lv)
  elseif id == MENDING then
    hero.regen = 0.006 * lv
  end
end

return U
