-- The hero: position, facing, collision against the decoration layer and
-- drawing. Loaded with require("hero") from main.lua.

local M = {}

local x, y = 0, 0
local facing_left = false
local moving = false
local SPEED = 1

function M.reset(px, py)
  x, y = px, py
  facing_left = false
  moving = false
end

function M.position()
  return x, y
end

-- A tile blocks when the decoration layer has a tree (4) or rock (5) or
-- the ground layer is water (2).
local function blocked(world, tile, px, py)
  local cx, cy = px // tile, py // tile
  if cx < 0 or cy < 0 or cx >= world:width() or cy >= world:height() then
    return true
  end
  local ground = world:get(cx, cy)
  local deco = world:get(cx, cy + world:height())
  return ground == 2 or deco == 4 or deco == 5
end

function M.walk(dx, dy, world, tile)
  moving = dx ~= 0 or dy ~= 0
  if dx < 0 then facing_left = true end
  if dx > 0 then facing_left = false end
  local nx, ny = x + dx * SPEED, y + dy * SPEED
  -- Check the four corners of the 8x8 sprite, feet only (bottom half).
  local ok = true
  for _, c in ipairs({{nx, ny + 4}, {nx + 7, ny + 4}, {nx, ny + 7}, {nx + 7, ny + 7}}) do
    if blocked(world, tile, c[1], c[2]) then ok = false end
  end
  if ok then x, y = nx, ny end
end

function M.draw(frame)
  local cell = 6
  if moving and (frame // 8) % 2 == 1 then cell = 7 end
  spr(cell, x, y, 1, 1, facing_left, false)
end

return M
