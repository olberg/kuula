-- The overlay in the corners, the title and the menus. Colours here are the
-- console's fixed ones (0 to 15), so the HUD stays readable in low light.
local hero = require("hero")
local upgrades = require("upgrades")

local M = {}

local LAMP, SKULL, HOURGLASS = 224, 228, 230

local clock_sec, clock_text = -1, ""
local kills_n, kills_text = -1, ""

local function text(s, x, y, c)
  print(s, x + 1, y + 1, 0)
  print(s, x, y, c)
end

local function centered(s, y, c)
  text(s, (320 - #s * 8) // 2, y, c)
end

local function big(s, y, c)
  font(16)
  centered(s, y, c)
  font()
end

local function bar(x, y, w, h, frac, c, back)
  rectfill(x - 1, y - 1, x + w, y + h, 0)
  rectfill(x, y, x + w - 1, y + h - 1, back)
  local fill = math.floor(w * math.max(0, math.min(1, frac)))
  if fill > 0 then rectfill(x, y, x + fill - 1, y + h - 1, c) end
end

-- The time left, from the frames left.
local function clock(left)
  local sec = (left + 59) // 60
  if sec ~= clock_sec then
    clock_sec = sec
    clock_text = string.format("%d:%02d", sec // 60, sec % 60)
  end
  return clock_text
end

function M.reset()
  clock_sec, kills_n = -1, -1
end

function M.draw(left, frame)
  -- experience along the top edge
  rectfill(0, 0, 319, 2, 0)
  local frac = hero.xp / hero.need()
  rectfill(0, 0, math.floor(319 * frac), 1, 10)

  -- oil, top left
  local oil = hero.oil
  spr(LAMP, 2, 5, 2, 2)
  -- Red below half, and flashing white below a quarter.
  local c = 9
  if oil < 25 and frame // 8 % 2 == 0 then c = 7 elseif oil < 50 then c = 8 end
  bar(22, 9, 56, 6, oil / hero.maxoil, c, 5)
  text("LV " .. hero.level, 22, 19, 7)

  -- time and kills, top right
  local s = clock(left)
  text(s, 300 - #s * 8, 9, 7)
  spr(HOURGLASS, 302, 5, 2, 2)
  if hero.kills ~= kills_n then
    kills_n = hero.kills
    kills_text = tostring(kills_n)
  end
  text(kills_text, 300 - #kills_text * 8, 25, 6)
  spr(SKULL, 302, 21, 2, 2)
end

function M.title(frame, titlebuf)
  screen:blit(titlebuf, 0, 0, 320, 240, 0, 0)
  if frame // 30 % 2 == 0 then centered("PRESS A", 208, 7) end
  centered("D-pad walks, the lamp fights", 222, 6)
end

local function dim()
  fillp(0x5A5A, true)
  rectfill(0, 0, 319, 239, 0)
  fillp()
end

function M.levelup(selected)
  dim()
  rectfill(24, 36, 295, 204, 1)
  rect(24, 36, 295, 204, 6)
  big("LEVEL UP", 44, 10)
  for k = 1, upgrades.count do
    local id = upgrades.choice[k]
    local y = 74 + (k - 1) * 40
    local def = upgrades.defs[id]
    if k == selected then
      rectfill(32, y - 4, 287, y + 31, 5)
      rect(32, y - 4, 287, y + 31, 10)
    end
    spr(192 + (id - 1) * 2, 40, y + 2, 2, 2)
    local lv = upgrades.lv[id]
    local tag = ""
    if id <= 4 then tag = lv == 0 and "  NEW" or "  Lv " .. (lv + 1) end
    text(def.name .. tag, 68, y + 2, k == selected and 10 or 7)
    text(upgrades.text(id), 68, y + 16, 6)
  end
  centered("UP / DOWN choose     A take", 190, 6)
end

local function summary(t)
  centered("Level " .. hero.level .. "   Kills " .. hero.kills, 136, 7)
  local sec = t // 60
  centered(string.format("Survived %d:%02d", sec // 60, sec % 60), 150, 6)
end

function M.won(t, frame)
  dim()
  rectfill(40, 84, 279, 180, 1)
  rect(40, 84, 279, 180, 10)
  big("DAWN BREAKS", 94, 10)
  centered("The lamp burns on.", 120, 7)
  summary(t)
  if frame // 30 % 2 == 0 then centered("PRESS A", 164, 7) end
end

function M.lost(t, frame)
  dim()
  rectfill(40, 84, 279, 180, 1)
  rect(40, 84, 279, 180, 8)
  big("THE LIGHT GOES OUT", 94, 8)
  centered("The fen takes the lamplighter.", 120, 7)
  summary(t)
  if frame // 30 % 2 == 0 then centered("PRESS A", 164, 7) end
end

return M
