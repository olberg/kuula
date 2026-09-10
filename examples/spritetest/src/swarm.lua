-- Shared by the visual cart and the native/Wasm raster benchmark.
local Swarm = {}
function Swarm.new(count, width, height)
  math.randomseed(42, 0)
  local s = {sprites = {}, width = width, height = height}
  Swarm.add(s, count)
  return s
end
function Swarm.add(s, count)
  for _ = 1, count do
    local vx, vy = math.random(1, 3), math.random(1, 3)
    if math.random(0, 1) == 0 then vx = -vx end
    if math.random(0, 1) == 0 then vy = -vy end
    s.sprites[#s.sprites + 1] = {
      x = math.random(0, s.width - 32), y = math.random(0, s.height - 32),
      vx = vx, vy = vy, tile = math.random(1, 2) * 4,
    }
  end
end
function Swarm.frame(s, draw_sprite)
  for i = 1, #s.sprites do
    local a = s.sprites[i]
    a.x, a.y = a.x + a.vx, a.y + a.vy
    if a.x < 0 then a.x = 0; a.vx = -a.vx end
    if a.x > s.width - 32 then a.x = s.width - 32; a.vx = -a.vx end
    if a.y < 0 then a.y = 0; a.vy = -a.vy end
    if a.y > s.height - 32 then a.y = s.height - 32; a.vy = -a.vy end
    draw_sprite(a.x, a.y, a.tile, 0)
  end
end
return Swarm
