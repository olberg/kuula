-- The lamplighter: position, oil, experience.
local H = {}

local DIAG = 0.7071
local TILE = { idle = 0, step_a = 2, step_b = 4, hurt = 6 }

-- What a run starts with, and the oil a flask holds.
H.SPEED, H.PULL, H.OIL, H.FLASK = 1.4, 40, 100, 25

function H.reset()
  H.x, H.y = 0, 0
  H.oil, H.maxoil = H.OIL, H.OIL
  H.speed, H.pull, H.regen = H.SPEED, H.PULL, 0
  H.face, H.moving, H.hurt = 1, false, 0
  H.level, H.xp, H.kills = 1, 0, 0
end

-- A flask's worth of oil.
function H.refill()
  H.oil = math.min(H.maxoil, H.oil + H.FLASK)
end

-- Experience needed to leave the current level.
function H.need()
  return 6 + 4 * H.level + H.level * H.level * 3 // 4
end

function H.move()
  if H.hurt > 0 then H.hurt = H.hurt - 1 end
  local dx = (btn(3) and 1 or 0) - (btn(2) and 1 or 0)
  local dy = (btn(1) and 1 or 0) - (btn(0) and 1 or 0)
  H.moving = dx ~= 0 or dy ~= 0
  if not H.moving then return end
  local s = H.speed
  if dx ~= 0 and dy ~= 0 then s = s * DIAG end
  H.x = H.x + dx * s
  H.y = H.y + dy * s
  if dx ~= 0 then H.face = dx end
end

function H.draw(camx, camy, t)
  local cell = TILE.idle
  if H.hurt > 0 then
    cell = TILE.hurt
  elseif H.moving then
    cell = (t // 8) % 2 == 0 and TILE.step_a or TILE.step_b
  end
  local x, y = H.x // 1 - camx, H.y // 1 - camy
  -- The lamp's halo, in a fixed colour: it stays bright when the rest dims,
  -- and it is how the eye finds the hero in a crowd.
  circ(x, y, 12 + (t // 6) % 2, 9)
  spr(cell, x - 8, y - 8, 2, 2, H.face < 0)
end

return H
