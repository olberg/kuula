-- Kuula example: a square you move with the D-pad.
--
-- Arrow keys move the square, Z is A and X is B on the desktop host.
-- The frame counter and the button state are printed in the corner.

local SCREEN_W, SCREEN_H = 320, 240
local SIZE = 20
local SPEED = 2

local x, y = (SCREEN_W - SIZE) // 2, (SCREEN_H - SIZE) // 2
local frame = 0

function _init()
  cls(1)
end

function _update(dt)
  frame = frame + 1
  if btn(0) then y = y - SPEED end
  if btn(1) then y = y + SPEED end
  if btn(2) then x = x - SPEED end
  if btn(3) then x = x + SPEED end
  x = math.max(0, math.min(SCREEN_W - SIZE, x))
  y = math.max(0, math.min(SCREEN_H - SIZE, y))
end

function _draw()
  cls(1)
  local colour = 8
  if btn(4) then colour = 11 end
  if btn(5) then colour = 12 end
  rectfill(x, y, x + SIZE - 1, y + SIZE - 1, colour)
  print("frame " .. frame, 2, 2, 7)
  local bits = ""
  for i = 0, 5 do
    bits = bits .. (btn(i) and "1" or "0")
  end
  print("btn " .. bits, 2, 10, 6)
end
