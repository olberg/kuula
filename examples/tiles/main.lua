-- Kuula example: a scrolling tile map with a walking hero.
--
-- Arrows walk (the sprite flips when facing left), Z (A) swaps the
-- hero's tunic colour with pal_map, X (B) dims the screen with a fill
-- pattern. The lake colour is animated with pal on a cart palette entry.
-- The map and sheet are decoded at boot through cart.toml's preload
-- list; src/hero.lua shows require.

local hero = require("hero")

local SCREEN_W, SCREEN_H = 320, 240
local TILE = 8
local WATER = 16 -- cart colour used by the water tile

local tiles, world
local cam_x, cam_y = 0, 0
local frame = 0

function _init()
  tiles = load_sheet("tiles")
  world = load_map("overworld")
  sheet(tiles)
  palt(0, true)
  hero.reset(11 * TILE, 11 * TILE)
end

local function update_camera()
  local hx, hy = hero.position()
  cam_x = hx - SCREEN_W // 2
  cam_y = hy - SCREEN_H // 2
  local max_x = world:width() * TILE - SCREEN_W
  local max_y = world:height() * TILE - SCREEN_H
  cam_x = math.max(0, math.min(max_x, cam_x))
  cam_y = math.max(0, math.min(max_y, cam_y))
end

function _update(dt)
  frame = frame + 1
  local dx, dy = 0, 0
  if btn(0) then dy = dy - 1 end
  if btn(1) then dy = dy + 1 end
  if btn(2) then dx = dx - 1 end
  if btn(3) then dx = dx + 1 end
  hero.walk(dx, dy, world, TILE)
  update_camera()

  -- The lake breathes: a cart colour cycles between two blues.
  local t = (frame % 60) / 60
  local wave = math.floor(60 + 40 * math.sin(t * 2 * math.pi))
  pal(WATER, 30, 80 + wave // 2, 160 + wave // 2)
end

function _draw()
  cls(1)
  camera(cam_x, cam_y)
  local cw, ch = SCREEN_W // TILE + 2, SCREEN_H // TILE + 2
  local cx, cy = cam_x // TILE, cam_y // TILE
  map(world, cx, cy, cx * TILE, cy * TILE, cw, ch, 0)
  map(world, cx, cy, cx * TILE, cy * TILE, cw, ch, 1)

  if btn(4) then pal_map(12, 8) end
  hero.draw(frame)
  pal_map()

  camera()
  if btn(5) then
    fillp(0xa5a5, true)
    rectfill(0, 0, SCREEN_W - 1, SCREEN_H - 1, 1)
    fillp()
  end
  print("frame " .. frame, 2, 2, 7)
  local hx, hy = hero.position()
  print("hero " .. hx .. "," .. hy, 2, 10, 6)
  print("A: swap  B: dim", 2, SCREEN_H - 8, 15)
end
