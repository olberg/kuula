-- Alien Invaders: a small Kuula cart. Left/right move the ship, A fires
-- (one shot on screen at a time). Clear the formation for the next wave;
-- lose when a bomb lands or the aliens reach the ship.
--
-- gfx/invaders.png holds five 32x32 tiles in a row; src/palette.lua is
-- its colour list. Both come from tools/make_invaders_assets.py.

local palette = require("palette")

local W, H = 640, 480
local SHIP, CRAB, SQUID, SHOT, BOMB = 0, 1, 2, 3, 4
local COLS, ROWS = 10, 4

local gfx, aliens, bombs, shot, ship, dir
local score, lives, wave, frame, over, was_a

local function tile(t, x, y)
  spr(t * 4, x, y, 4, 4)
end

local function hits(ax, ay, aw, ah, bx, by, bw, bh)
  return ax < bx + bw and bx < ax + aw and ay < by + bh and by < ay + ah
end

-- Bullets are drawn as 32x32 tiles but only the middle column is ink.
local function bullet_hits(b, x, y, w, h)
  return hits(b.x + 12, b.y + 4, 8, 24, x, y, w, h)
end

local function new_wave()
  aliens, bombs, shot, dir = {}, {}, nil, 1
  for r = 0, ROWS - 1 do
    for c = 0, COLS - 1 do
      aliens[#aliens + 1] = {
        x = 100 + c * 44,
        y = 56 + r * 40 + wave * 8,
        kind = r == 0 and SQUID or CRAB,
      }
    end
  end
end

local function reset()
  score, lives, wave, frame, over = 0, 3, 0, 0, false
  ship = { x = W // 2 - 16, y = H - 60 }
  new_wave()
end

function _init()
  gfx = load_sheet("invaders") -- kept alive: a collected handle frees the sheet
  sheet(gfx)
  palt(0, true)
  for i, rgb in ipairs(palette) do
    pal(15 + i, rgb)
  end
  reset()
end

local function update_aliens()
  -- The fewer aliens remain, the more often the formation steps.
  if frame % math.max(1, #aliens // 4) ~= 0 then return end
  local turn = false
  for _, a in ipairs(aliens) do
    a.x = a.x + dir * 4
    if a.x < 8 or a.x > W - 40 then turn = true end
    if a.y + 28 >= ship.y then over = true end
  end
  if turn then
    dir = -dir
    for _, a in ipairs(aliens) do
      a.y = a.y + 16
    end
  end
end

local function update_shot()
  if shot then
    shot.y = shot.y - 8
    if shot.y < -32 then shot = nil end
  elseif btn(4) then
    shot = { x = ship.x, y = ship.y - 24 }
  end
  if not shot then return end
  for i = #aliens, 1, -1 do
    local a = aliens[i]
    if bullet_hits(shot, a.x + 2, a.y + 4, 28, 24) then
      table.remove(aliens, i)
      score = score + (a.kind == SQUID and 20 or 10)
      shot = nil
      return
    end
  end
end

local function update_bombs()
  if frame % 45 == 0 and #aliens > 0 then
    local a = aliens[math.random(#aliens)]
    bombs[#bombs + 1] = { x = a.x, y = a.y + 24 }
  end
  for i = #bombs, 1, -1 do
    local b = bombs[i]
    b.y = b.y + 4
    if bullet_hits(b, ship.x + 4, ship.y + 4, 24, 26) then
      lives = lives - 1
      bombs = {}
      if lives == 0 then over = true end
      return
    elseif b.y > H then
      table.remove(bombs, i)
    end
  end
end

function _update(dt)
  local pressed = btn(4) and not was_a
  was_a = btn(4)
  if over then
    if pressed then reset() end
    return
  end
  frame = frame + 1
  if btn(2) then ship.x = math.max(0, ship.x - 4) end
  if btn(3) then ship.x = math.min(W - 32, ship.x + 4) end
  update_aliens()
  update_shot()
  update_bombs()
  if #aliens == 0 then
    wave = wave + 1
    new_wave()
  end
end

function _draw()
  cls(0)
  line(0, H - 20, W - 1, H - 20, 3)
  for _, a in ipairs(aliens) do
    tile(a.kind, a.x, a.y)
  end
  for _, b in ipairs(bombs) do
    tile(BOMB, b.x, b.y)
  end
  if shot then tile(SHOT, shot.x, shot.y) end
  tile(SHIP, ship.x, ship.y)
  print("SCORE " .. score, 8, 8, 7)
  print("WAVE " .. (wave + 1), W // 2 - 12, 8, 7)
  print("LIVES " .. lives, W - 40, 8, 7)
  if over then
    rectfill(W // 2 - 48, H // 2 - 8, W // 2 + 48, H // 2 + 8, 1)
    print("GAME OVER - PRESS A", W // 2 - 38, H // 2 - 2, 8)
  end
end
