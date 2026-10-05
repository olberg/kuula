-- The ground: 16-pixel tiles picked from their place in the world, baked
-- once into a 512 x 512 picture that repeats without end.
local M = {}

local SIZE = 512
local ground

-- Plain tiles are common, reeds and water are not.
local function pick(tx, ty)
  local h = (tx * 7919 + ty * 104729 + (tx ~ ty) * 31) % 97
  if h < 62 then return h % 4 end
  if h < 82 then return 4 + h % 2 end
  return 6 + h % 2
end

function M.init()
  ground = buf("u8", SIZE, SIZE)
  draw_target(ground)
  for ty = 0, SIZE // 16 - 1 do
    for tx = 0, SIZE // 16 - 1 do
      spr(160 + pick(tx, ty) * 2, tx * 16, ty * 16, 2, 2)
    end
  end
  draw_target()
end

function M.draw(camx, camy)
  local ox, oy = camx % SIZE, camy % SIZE
  local w1, h1 = math.min(320, SIZE - ox), math.min(240, SIZE - oy)
  screen:blit(ground, ox, oy, w1, h1, 0, 0)
  if w1 < 320 then screen:blit(ground, 0, oy, 320 - w1, h1, w1, 0) end
  if h1 < 240 then
    screen:blit(ground, ox, 0, w1, 240 - h1, 0, h1)
    if w1 < 320 then screen:blit(ground, 0, 0, 320 - w1, 240 - h1, w1, h1) end
  end
end

return M
