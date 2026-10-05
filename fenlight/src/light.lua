-- The cart's colours (palette indices 16 and up), scaled towards dark as
-- the oil falls below half.
local palette = require("palette")

local M = {}

local SCALE = { 1, 0.9, 0.8, 0.71, 0.63, 0.56 }
local step = 0

local function apply(s)
  step = s
  local k = SCALE[s + 1]
  for i, rgb in ipairs(palette) do
    local r, g, b = rgb >> 16 & 255, rgb >> 8 & 255, rgb & 255
    pal(15 + i, math.floor(r * k), math.floor(g * k), math.floor(b * k))
  end
end

function M.reset()
  apply(0)
end

-- Steps of ten oil below 50; the palette is only touched when the step moves.
function M.set(oil)
  local s = 0
  if oil < 50 then s = math.min(5, 1 + (49 - math.floor(oil)) // 10) end
  if s ~= step then apply(s) end
end

return M
