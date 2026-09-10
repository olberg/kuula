-- Kuula example: a counter that survives restarts through save slots.
--
-- Slot 0 holds { runs = n, presses = n, best = n }. Every boot bumps
-- `runs` and saves; A (Z on the desktop host) bumps `presses` and saves;
-- B (X) clears the slot back to zero. Close the window and run the cart
-- again to see the numbers come back.
--
-- Headless runs use an in-memory store, so `kuula run examples/saves
-- --headless` always starts from run 1 and leaves nothing on disk.

local data
local status = ""
local flash = 0

local function fresh()
  return { runs = 0, presses = 0, best = 0 }
end

local function persist(why)
  local ok, err = pcall(save, 0, data)
  status = ok and ("saved: " .. why) or ("save failed: " .. tostring(err))
  flash = 30
end

function _init()
  data = load(0) or fresh()
  data.runs = data.runs + 1
  persist("boot")
end

local was_a, was_b = false, false

function _update(dt)
  local a, b = btn(4), btn(5)
  if a and not was_a then
    data.presses = data.presses + 1
    if data.presses > data.best then data.best = data.presses end
    persist("press")
  end
  if b and not was_b then
    data = fresh()
    data.runs = 1
    persist("reset")
  end
  was_a, was_b = a, b
  if flash > 0 then flash = flash - 1 end
end

function _draw()
  cls(1)
  print("SAVE SLOT 0", 8, 8, 7)
  print("runs    " .. data.runs, 8, 24, 10)
  print("presses " .. data.presses, 8, 32, 11)
  print("best    " .. data.best, 8, 40, 12)
  print("A: press   B: reset", 8, 60, 6)
  print(status, 8, 76, flash > 0 and 7 or 5)
  local w = math.min(300, data.presses * 4)
  rectfill(8, 96, 8 + w, 104, 11)
  rect(8, 96, 308, 104, 6)
end
