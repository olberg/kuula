-- Kuula example: every button a cart has, lit while it is held.
--
-- The boxes are laid out as on a handheld: the shoulder buttons at the
-- top, the D-pad on the left, the face buttons on the right, Select and
-- Start below. Each shows the number `btn` knows it by. Menu is not here:
-- it is the console's, and a cart never sees it.
--
-- On the desktop host: the arrows; Z, X, C and V for A, B, X and Y; A and
-- S for L1 and R1; Q and W for L2 and R2; Enter for Start and Right Shift
-- for Select.

local BACK, IDLE, TEXT, LIT = 1, 6, 7, 11
local GLYPH = 8

-- n, the lines of the label, then x, y, width and height.
local BOXES = {
  { 0, { "0" }, 42, 86, 28, 28 },
  { 1, { "1" }, 42, 142, 28, 28 },
  { 2, { "2" }, 14, 114, 28, 28 },
  { 3, { "3" }, 70, 114, 28, 28 },
  { 4, { "A", "4" }, 278, 114, 28, 28 },
  { 5, { "B", "5" }, 250, 142, 28, 28 },
  { 6, { "X", "6" }, 250, 86, 28, 28 },
  { 7, { "Y", "7" }, 222, 114, 28, 28 },
  { 8, { "L1 8" }, 20, 48, 72, 20 },
  { 9, { "R1 9" }, 228, 48, 72, 20 },
  { 10, { "L2 10" }, 20, 24, 72, 20 },
  { 11, { "R2 11" }, 228, 24, 72, 20 },
  { 12, { "START 12" }, 164, 188, 84, 20 },
  { 13, { "SELECT 13" }, 72, 188, 84, 20 },
}

local function box(n, lines, x, y, w, h)
  local held = btn(n)
  if held then
    rectfill(x, y, x + w - 1, y + h - 1, LIT)
  else
    rect(x, y, x + w - 1, y + h - 1, IDLE)
  end
  local step = GLYPH + 2
  local top = y + (h - #lines * step + 2) // 2
  for i, line in ipairs(lines) do
    print(line, x + (w - #line * GLYPH) // 2, top + (i - 1) * step, held and BACK or TEXT)
  end
end

function _draw()
  cls(BACK)
  print("btn(n)", 136, 4, TEXT)
  local held = 0
  for _, b in ipairs(BOXES) do
    box(b[1], b[2], b[3], b[4], b[5], b[6])
    if btn(b[1]) then held = held + 1 end
  end
  print(held .. " held", 132, 224, IDLE)
end
