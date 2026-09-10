local Swarm = require('swarm')
local palette = require('palette')
local swarm, gfx, ticks, automatic, last_a, last_b, last_up, last_down
local function draw_sprite(x, y, tile)
  spr(tile, x, y, 4, 4)
end
function _init()
  gfx = load_sheet('invaders')
  sheet(gfx)
  palt(0, true)
  for i, rgb in ipairs(palette) do pal(15 + i, rgb) end
  swarm = Swarm.new(32, 640, 480)
  ticks, automatic = 0, true
end
function _update()
  local a, b, up, down = btn(4), btn(5), btn(0), btn(1)
  if a and not last_a then automatic = not automatic end
  if b and not last_b then swarm = Swarm.new(32, 640, 480); ticks = 0 end
  if up and not last_up then Swarm.add(swarm, 32) end
  if down and not last_down then
    for _ = 1, math.min(32, #swarm.sprites - 32) do table.remove(swarm.sprites) end
  end
  last_a, last_b, last_up, last_down = a, b, up, down
  ticks = ticks + 1
  if automatic and ticks % 120 == 0 then Swarm.add(swarm, 32) end
end
function _draw()
  cls(0)
  Swarm.frame(swarm, draw_sprite)
  rectfill(0, 0, 639, 39, 0)
  print('ALIEN SPRITE TEST  SPRITES ' .. #swarm.sprites, 8, 6, 7)
  print('A AUTO/PAUSE  B RESET  UP/DOWN +/-32', 8, 16, 7)
  print('AUTO ' .. tostring(automatic) .. '  HOST BENCH FINDS 60/30 FPS', 8, 26, 7)
end
