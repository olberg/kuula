-- The Kuula shell. Runs as its own guest beside the cart with the `sys`
-- table the cart never sees. It draws into an overlay buffer that the
-- console keys on colour 0: whatever the shell leaves black shows the
-- cart's screen through it.
--
-- Screens: boot, the cart list, the pause overlay, settings and the
-- error screen. D-pad, A (Z) and B (X) only; Menu (Escape) toggles the
-- pause overlay while a cart runs.

local W, H = 320, 240

local PANEL, BORDER, TEXT, DIM, HILITE, CODE = 1, 6, 7, 6, 12, 15
local SCALES = { 1, 2, 3, 4 }

local screen = "boot"
local list_index = 1
local menu_index = 1
local settings_index = 1
local was = {}
local boot_frames = 0
local last_fault = nil

local MENU_ITEMS = { "resume", "restart", "settings", "quit to shell" }
local SETTINGS_ITEMS = { "window scale", "master volume", "networking", "back" }

function _init()
  local w, h = stat("width"), stat("height")
  if w and h then W, H = w, h end
end

-- Edge-triggered buttons, kept in the shell so the cart's own edge
-- state is not involved.
local function pressed(n)
  local now = btn(n)
  local edge = now and not was[n]
  was[n] = now
  return edge
end

-- On a screen change, buttons already held do not count as pressed on
-- the new screen: sample them now so only a fresh press is an edge.
local function reset_edges()
  for n = 0, 5 do was[n] = btn(n) end
end

local function centre(text, y, c)
  local x = (W - #text * 4) // 2
  print(text, x, y, c)
end

local function panel(x0, y0, x1, y1)
  rectfill(x0, y0, x1, y1, PANEL)
  rect(x0, y0, x1, y1, BORDER)
end

local function draw_list(items, index, x, y, render)
  for i, item in ipairs(items) do
    local c = (i == index) and HILITE or TEXT
    local label = render and render(item) or item
    if i == index then print(">", x - 6, y + (i - 1) * 8, HILITE) end
    print(label, x, y + (i - 1) * 8, c)
  end
end

-- Boot ---------------------------------------------------------------

local function update_boot()
  boot_frames = boot_frames + 1
  -- Sample both buttons every frame so their edge state is current.
  local a, b = pressed(4), pressed(5)
  if boot_frames > 90 or a or b then
    screen = "list"
    reset_edges()
  end
end

local function draw_boot()
  cls(PANEL)
  centre("K U U L A", H // 2 - 12, TEXT)
  centre("fantasy console", H // 2 - 2, DIM)
  if boot_frames % 60 < 40 then
    centre("press A", H // 2 + 20, HILITE)
  end
end

-- Cart list ----------------------------------------------------------

local function update_list()
  local carts = sys.carts()
  if pressed(0) then list_index = list_index - 1 end
  if pressed(1) then list_index = list_index + 1 end
  if #carts == 0 then
    list_index = 1
  else
    list_index = ((list_index - 1) % #carts) + 1
  end
  if pressed(4) and carts[list_index] then
    sys.run(carts[list_index].name)
    screen = "cart"
    reset_edges()
  end
end

local function draw_list_screen()
  cls(PANEL)
  local carts = sys.carts()
  print("carts", 8, 8, DIM)
  if #carts == 0 then
    print("no carts found in carts/", 8, 24, TEXT)
  else
    draw_list(carts, list_index, 16, 24, function(c) return c.title end)
  end
  print("A run", 8, H - 12, DIM)
end

-- Running cart, pause overlay, settings ------------------------------

local function update_cart()
  local fault = sys.fault()
  if fault then
    last_fault = fault
    screen = "error"
    reset_edges()
    return
  end
  if sys.menu() then
    sys.paused(true)
    menu_index = 1
    screen = "pause"
    reset_edges()
  end
end

local function draw_cart()
  cls(0) -- fully transparent: the cart shows through
end

local function leave_overlay()
  sys.paused(false)
  screen = "cart"
  reset_edges()
end

local function update_pause()
  if pressed(0) then menu_index = menu_index - 1 end
  if pressed(1) then menu_index = menu_index + 1 end
  menu_index = ((menu_index - 1) % #MENU_ITEMS) + 1
  if sys.menu() or pressed(5) then
    leave_overlay()
  elseif pressed(4) then
    local item = MENU_ITEMS[menu_index]
    if item == "resume" then
      leave_overlay()
    elseif item == "restart" then
      sys.restart()
      leave_overlay()
    elseif item == "settings" then
      settings_index = 1
      screen = "settings"
      reset_edges()
    elseif item == "quit to shell" then
      sys.quit()
      sys.paused(false)
      screen = "list"
      reset_edges()
    end
  end
end

local function draw_pause()
  cls(0)
  local x0, y0 = W // 2 - 60, H // 2 - 30
  panel(x0, y0, x0 + 120, y0 + 60)
  print("paused", x0 + 8, y0 + 6, DIM)
  draw_list(MENU_ITEMS, menu_index, x0 + 16, y0 + 18)
end

local function update_settings()
  local s = sys.settings()
  if pressed(0) then settings_index = settings_index - 1 end
  if pressed(1) then settings_index = settings_index + 1 end
  settings_index = ((settings_index - 1) % #SETTINGS_ITEMS) + 1
  local item = SETTINGS_ITEMS[settings_index]
  local delta = 0
  if pressed(2) then delta = -1 end
  if pressed(3) then delta = 1 end
  if item == "window scale" and delta ~= 0 then
    local n = s.scale + delta
    if n >= 1 and n <= 4 then sys.set_scale(n) end
  elseif item == "master volume" and delta ~= 0 then
    local v = s.volume + delta * 10
    if v >= 0 and v <= 100 then sys.set_volume(v) end
  elseif item == "networking" and (delta ~= 0 or pressed(4)) then
    sys.set_net(not s.net)
  end
  if pressed(5) or (pressed(4) and item == "back") then
    if sys.running() then
      screen = "pause"
    else
      screen = "list"
    end
    reset_edges()
  end
end

local function draw_settings()
  if sys.running() then cls(0) else cls(PANEL) end
  local s = sys.settings()
  local x0, y0 = W // 2 - 70, H // 2 - 30
  panel(x0, y0, x0 + 140, y0 + 60)
  print("settings", x0 + 8, y0 + 6, DIM)
  draw_list(SETTINGS_ITEMS, settings_index, x0 + 16, y0 + 18, function(item)
    if item == "window scale" then return "window scale  < " .. s.scale .. "x >" end
    if item == "master volume" then return "master volume < " .. s.volume .. " >" end
    if item == "networking" then return "networking    < " .. (s.net and "on" or "off") .. " >" end
    return item
  end)
end

-- Error screen -------------------------------------------------------

local function update_error()
  if pressed(4) then
    sys.restart()
    last_fault = nil
    screen = "cart"
    reset_edges()
  elseif pressed(5) then
    sys.quit()
    last_fault = nil
    screen = "list"
    reset_edges()
  end
end

local function wrap(text, columns)
  local lines = {}
  for para in (text .. "\n"):gmatch("(.-)\n") do
    local line = ""
    for word in para:gmatch("%S+") do
      if #line + #word + 1 > columns and #line > 0 then
        lines[#lines + 1] = line
        line = word
      elseif #line == 0 then
        line = word
      else
        line = line .. " " .. word
      end
    end
    lines[#lines + 1] = line
  end
  return lines
end

local function draw_error()
  cls(0)
  local f = last_fault
  if not f then return end
  local margin = 8
  local columns = (W - 4 * margin) // 4
  panel(margin, margin, W - margin - 1, H - margin - 1)
  local y = margin + 6
  print(f.code, margin + 6, y, CODE)
  y = y + 8
  local where = f.file
  if f.line then where = where .. ":" .. f.line end
  print(where, margin + 6, y, DIM)
  y = y + 10
  -- Only six lines fit, so wrap only as much text as could fill them.
  local lines = wrap(f.message:sub(1, columns * 8), columns)
  for i = 1, math.min(#lines, 6) do
    print(lines[i], margin + 6, y, TEXT)
    y = y + 8
  end
  print("A restart   B quit", margin + 6, H - margin - 12, HILITE)
end

-- Dispatch -----------------------------------------------------------

local screens = {
  boot = { update_boot, draw_boot },
  list = { update_list, draw_list_screen },
  cart = { update_cart, draw_cart },
  pause = { update_pause, draw_pause },
  settings = { update_settings, draw_settings },
  error = { update_error, draw_error },
}

function _update(dt)
  -- The overlay follows the cart's screen mode while this state lives
  -- on, so the size is read every frame, not only at boot.
  local w, h = stat("width"), stat("height")
  if w and h then W, H = w, h end
  -- A cart can die while the shell is on any screen.
  if screen ~= "error" and sys.fault() then
    last_fault = sys.fault()
    screen = "error"
    reset_edges()
  end
  screens[screen][1]()
end

function _draw()
  screens[screen][2]()
end
