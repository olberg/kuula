-- The Kuula shell. Runs as its own guest beside the cart with the `sys`
-- table the cart never sees. It draws into an overlay buffer that the
-- console keys on colour 0: whatever the shell leaves black shows the
-- cart's screen through it.
--
-- Screens: boot, the cart list, the pause overlay, settings and the
-- error screen. D-pad, A (Z) and B (X) only; Menu (Escape) toggles the
-- pause overlay while a cart runs.
--
-- Started on one cart (`sys.single()`), the shell opens that cart at once
-- and ends the host where it would have shown its list.

local W, H = 320, 240
-- The system font cell. Glyphs are 8 wide in both faces; the height is
-- 8 at 320x240 and 16 at 640x480, so every screen is 30 rows of text and
-- layouts below are in cells. Lists and paragraphs step by LH, a cell
-- plus a quarter of leading.
local CW, CH, LH = 8, 8, 10

local function set_font(h)
  CH = h or CH
  LH = CH + CH // 4
end

local PANEL, BORDER, TEXT, DIM, HILITE, CODE = 1, 6, 7, 6, 12, 15
local SCALES = { 1, 2, 3, 4 }

local screen = "boot"
local list_index = 1
local menu_index = 1
local settings_index = 1
local was = {}
local boot_frames = 0
local last_fault = nil
local net_index, nearby_index, text_index = 1, 1, 1
local net_managed = false
local text_kind = "ticket"
local single = nil
-- Where the question about a developer came up, and whether it paused
-- the cart; how long it has been up, and how long A has been held on it.
local dev_return, dev_paused = "list", false
local dev_frames, dev_hold = 0, 0
-- The question takes no answer for a second and a half, and approving is
-- A pressed after that and held for a second: a button that was being
-- tapped or held in a game when the question came up answers nothing.
local DEV_WAIT, DEV_HOLD = 90, 60
local NET_ITEMS = { "host game", "join with ticket", "nearby sessions", "relay URL", "relay-only", "LAN discovery", "networking", "play offline", "back" }
local CHARACTERS = "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ:/.-_?=&%+@[]"
local TEXT_KEYS = {}
for i = 1, #CHARACTERS do TEXT_KEYS[#TEXT_KEYS + 1] = CHARACTERS:sub(i, i) end
TEXT_KEYS[#TEXT_KEYS + 1] = "del"
TEXT_KEYS[#TEXT_KEYS + 1] = "clear"
TEXT_KEYS[#TEXT_KEYS + 1] = "paste"
TEXT_KEYS[#TEXT_KEYS + 1] = "done"

local MENU_ITEMS = { "resume", "restart", "settings", "quit to shell", "connection" }
local SETTINGS_ITEMS = { "window scale", "master volume", "networking", "back" }

function _init()
  local w, h = stat("width"), stat("height")
  if w and h then W, H = w, h end
  set_font(stat("font"))
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
  local x = (W - #text * CW) // 2
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
    if i == index then print(">", x - 2 * CW, y + (i - 1) * LH, HILITE) end
    print(label, x, y + (i - 1) * LH, c)
  end
end

-- Boot ---------------------------------------------------------------

-- A on the list: a cart that uses the network gets the multiplayer
-- screen first, any other starts.
local function open_selected()
  local cart = sys.carts()[list_index]
  if not cart then return end
  if cart.network then
    sys.net_action("browse", cart.name)
    sys.net_action("overlay", "", true)
    net_index = 1
    screen = "network"
  else
    net_managed = false
    sys.run(cart.name)
    screen = "cart"
  end
  reset_edges()
end

local function update_boot()
  boot_frames = boot_frames + 1
  -- Sample both buttons every frame so their edge state is current.
  local a, b = pressed(4), pressed(5)
  if single then
    -- No title to wait on: the one cart opens, and a list without it
    -- is the end.
    screen = "list"
    open_selected()
  elseif a or b then
    screen = "list"
    reset_edges()
  end
end

local function draw_boot()
  cls(PANEL)
  centre("K U U L A", H // 2 - 2 * CH, TEXT)
  centre("fantasy console", H // 2 - CH // 2, DIM)
  if boot_frames % 60 < 40 then
    centre("press A", H // 2 + 2 * CH, HILITE)
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
  if pressed(5) and carts[list_index] then
    screen = "info"
    reset_edges()
  elseif pressed(4) and carts[list_index] then
    open_selected()
  end
end

local function draw_list_screen()
  cls(PANEL)
  local carts = sys.carts()
  print("carts", CW, CH, DIM)
  if #carts == 0 then
    print("no carts found in carts/", CW, 3 * CH, TEXT)
  else
    draw_list(carts, list_index, 3 * CW, 3 * CH, function(c) return c.title end)
  end
  if sys.dev_state() > 0 then
    print("development receiver on", CW, H - 4 * CH, HILITE)
    print(sys.dev().note:sub(1, (W - 2 * CW) // CW), CW, H - 4 * CH + LH, DIM)
  end
  print("A run    B info", CW, H - CH - CH // 2, DIM)
end

local function update_info()
  if pressed(5) then screen = "list"; reset_edges() end
end

local function draw_info()
  cls(PANEL)
  local cart = sys.carts()[list_index]
  if not cart then return end
  local function wrapped(text, y)
    local width = math.max(1, (W - 4 * CW) // CW)
    for i = 1, math.min(#text, width * 5), width do
      print(text:sub(i, i + width - 1), 2 * CW, y, TEXT)
      y = y + LH
    end
  end
  print("cart info", 2 * CW, CH, HILITE)
  wrapped(cart.title, CH * 5 // 2)
  wrapped("author: " .. (cart.author ~= "" and cart.author or "not specified"), CH * 5 // 2 + 6 * LH)
  wrapped("license: " .. (cart.license or "not specified"), CH * 5 // 2 + 12 * LH)
  print("B back", 2 * CW, H - 2 * CH, DIM)
end

-- Running cart, pause overlay, settings ------------------------------

local function update_cart()
  local n = sys.network()
  if net_managed and (n.status == "ended" or n.ended) then
    sys.net_action("overlay", "", true)
    screen = "connecting"
    reset_edges()
    return
  end
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
  menu_index = ((menu_index - 1) % (net_managed and #MENU_ITEMS or #MENU_ITEMS - 1)) + 1
  if sys.menu() or pressed(5) then
    leave_overlay()
  elseif pressed(4) then
    local item = MENU_ITEMS[menu_index]
    if item == "resume" then
      leave_overlay()
    elseif item == "restart" then
      sys.restart()
      if net_managed then
        sys.paused(false)
        sys.net_action("overlay", "", true)
        screen = "connecting"; reset_edges()
      else leave_overlay() end
    elseif item == "settings" then
      settings_index = 1
      screen = "settings"
      reset_edges()
    elseif item == "connection" then
      sys.paused(false)
      sys.net_action("overlay", "", true)
      screen = "connecting"
      reset_edges()
    elseif item == "quit to shell" then
      net_managed = false
      sys.net_action("overlay", "", false)
      sys.net_action("browse", "")
      sys.quit()
      sys.paused(false)
      screen = "list"
      reset_edges()
    end
  end
end

-- A titled panel of `columns` by `rows` cells, centred; returns its
-- top-left corner.
local function menu_panel(title, columns, rows)
  local pw, ph = columns * CW, rows * LH + CH
  local x0, y0 = (W - pw) // 2, (H - ph) // 2
  panel(x0, y0, x0 + pw, y0 + ph)
  print(title, x0 + CW, y0 + CH // 2, DIM)
  return x0, y0
end

local function draw_pause()
  cls(0)
  local items = {}
  for i = 1, (net_managed and #MENU_ITEMS or #MENU_ITEMS - 1) do items[i] = MENU_ITEMS[i] end
  local x0, y0 = menu_panel("paused", 17, #items + 2)
  draw_list(items, menu_index, x0 + 3 * CW, y0 + CH // 2 + 2 * LH, function(item)
    -- With one cart there is no list to go back to.
    if single and item == "quit to shell" then return "quit" end
    return item
  end)
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
  local x0, y0 = menu_panel("settings", 26, #SETTINGS_ITEMS + 2)
  draw_list(SETTINGS_ITEMS, settings_index, x0 + 3 * CW, y0 + CH // 2 + 2 * LH, function(item)
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
    if net_managed then
      sys.net_action("overlay", "", true)
      screen = "connecting"
    else
      screen = "cart"
    end
    reset_edges()
  elseif pressed(5) then
    sys.quit()
    sys.net_action("overlay", "", false)
    sys.net_action("browse", "")
    net_managed = false
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
  local columns = (W - 4 * margin) // CW
  panel(margin, margin, W - margin - 1, H - margin - 1)
  local y = margin + CH // 2
  print(f.code, margin + 6, y, CODE)
  y = y + LH
  local where = f.file
  if f.line then where = where .. ":" .. f.line end
  print(where, margin + 6, y, DIM)
  y = y + LH + CH // 2
  -- Only six lines fit, so wrap only as much text as could fill them.
  local lines = wrap(f.message:sub(1, columns * 8), columns)
  for i = 1, math.min(#lines, 6) do
    print(lines[i], margin + 6, y, TEXT)
    y = y + LH
  end
  print("A restart   B quit", margin + 6, H - margin - CH - CH // 2, HILITE)
end

-- A developer asking for approval ------------------------------------

-- The receiver refused a developer nobody has approved; the person
-- holding the device says whether the next attempt is let in.
local function update_dev()
  dev_frames = dev_frames + 1
  local a, b = pressed(4), pressed(5)
  local answer
  if dev_frames <= DEV_WAIT then
    dev_hold = 0
  elseif b then
    answer = "refuse"
  elseif not btn(4) then
    dev_hold = 0
  elseif a then
    dev_hold = 1
  elseif dev_hold > 0 then
    dev_hold = dev_hold + 1
    if dev_hold >= DEV_HOLD then answer = "approve" end
  end
  if not answer then return end
  sys.dev_action(answer)
  if dev_paused then sys.paused(false) end
  screen = dev_return
  reset_edges()
end

local function draw_dev()
  if sys.running() then cls(0) else cls(PANEL) end
  local d = sys.dev()
  -- What the sender says it brings; it is its claim until it is approved.
  local what = d.cart ~= "" and (d.cart .. ", " .. d.bytes .. " bytes") or "it did not say"
  local lines = {
    { "A computer asks to send carts", TEXT },
    { "to this device.", TEXT },
    { "", TEXT },
    { "cart  " .. what:sub(1, 26), HILITE },
    { "from  " .. (d.from ~= "" and d.from or "unknown"):sub(1, 26), HILITE },
    { "id    " .. d.pending:sub(1, 16), CODE },
    { "      " .. d.pending:sub(17, 32), CODE },
    { "      " .. d.pending:sub(33, 48), CODE },
    { "      " .. d.pending:sub(49, 64), CODE },
    { "", TEXT },
    { "Approve your own computer only:", TEXT },
    { "`kuula deploy id` prints its id.", TEXT },
    { "", TEXT },
    { dev_frames <= DEV_WAIT and "..." or "hold A: approve    B: refuse", HILITE },
  }
  local x0, y0 = menu_panel("development", 36, #lines + 3)
  local x, y = x0 + 2 * CW, y0 + CH // 2 + 2 * LH
  for i, line in ipairs(lines) do
    print(line[1], x, y + (i - 1) * LH, line[2])
  end
  -- How far the hold has got.
  if dev_hold > 0 then
    local w = 32 * CW * dev_hold // DEV_HOLD
    rectfill(x, y + #lines * LH + 2, x + w, y + #lines * LH + 4, HILITE)
  end
end

-- Multiplayer --------------------------------------------------------
local function network_back()
  sys.quit()
  sys.paused(false)
  sys.net_action("overlay", "", true)
  net_managed = false
  screen = "network"
  reset_edges()
end

local function start_network(ticket)
  local cart = sys.carts()[list_index]
  if not cart then return end
  sys.run_network(cart.name, ticket)
  sys.net_action("overlay", "", true)
  net_managed = true
  screen = "connecting"
  reset_edges()
end

local function network_text(text, y, colour)
  local columns = (W - 32) // CW
  for i = 1, math.min(#text, columns * 4), columns do
    print(text:sub(i, i + columns - 1), 16, y, colour or TEXT)
    y = y + LH
  end
end

local function edit_network(kind)
  text_kind = kind
  text_index = #TEXT_KEYS
  sys.net_action("edit", kind)
  screen = "net_text"
  reset_edges()
end

local function update_network()
  if pressed(0) then net_index = net_index - 1 end
  if pressed(1) then net_index = net_index + 1 end
  net_index = ((net_index - 1) % #NET_ITEMS) + 1
  local a, b = pressed(4), pressed(5)
  local n, s = sys.network(), sys.settings()
  -- A host without networking has one thing to say; any button goes back.
  if n.unavailable ~= "" then a, b = false, a or b end
  if b or (a and net_index == 9) then
    sys.net_action("overlay", "", false)
    sys.net_action("browse", "")
    screen = "list"; reset_edges()
  elseif a then
    if net_index <= 3 and not s.net then
      screen = "net_permission"; reset_edges()
    elseif net_index == 1 then start_network(nil)
    elseif net_index == 2 then edit_network("ticket")
    elseif net_index == 3 then
      nearby_index = 1; screen = "nearby"; reset_edges()
    elseif net_index == 4 then edit_network("relay")
    elseif net_index == 5 then sys.net_action("relay", n.relay, not n.relay_only)
    elseif net_index == 6 then sys.net_action("discovery", "", not n.discovery)
    elseif net_index == 7 then sys.set_net(not s.net)
    elseif net_index == 8 then
      sys.set_net(false)
      sys.net_action("overlay", "", false)
      sys.run(sys.carts()[list_index].name)
      net_managed = false; screen = "cart"; reset_edges()
    end
  end
end

local function draw_network()
  cls(PANEL)
  local c, n, s = sys.carts()[list_index], sys.network(), sys.settings()
  print("multiplayer", 16, 12, HILITE)
  print(c and c.title:sub(1, 36) or "", 16, 24, TEXT)
  if n.unavailable ~= "" then
    network_text(n.unavailable, 56)
    print("A back", 16, H - 16, DIM)
    return
  end
  draw_list(NET_ITEMS, net_index, 24, 44, function(item)
    if item == "relay-only" then return item .. ": " .. (n.relay_only and "on" or "off") end
    if item == "LAN discovery" then return item .. ": " .. (n.discovery and "on" or "off") end
    if item == "networking" then return item .. ": " .. (s.net and "on" or "off") end
    return item
  end)
  network_text("relay: " .. (n.relay ~= "" and n.relay or "disabled (direct only)"), 144, DIM)
  network_text(n.detail, 176)
  print("A choose    B back", 16, H - 16, DIM)
end

local function update_permission()
  if pressed(4) then sys.set_net(true); screen = "network"; reset_edges()
  elseif pressed(5) then screen = "network"; reset_edges() end
end
local function draw_permission()
  cls(PANEL)
  print("allow networking?", 16, 24, HILITE)
  network_text("This lets network-enabled carts connect to another player. LAN discovery and relay use are separate settings.", 56)
  network_text("Turning networking off closes all sessions and discovery. Offline play stays available.", 108)
  print("A allow    B cancel", 16, H - 24, DIM)
end

local function update_net_text()
  local n = sys.network()
  if pressed(0) then text_index = math.max(1, text_index - 10) end
  if pressed(1) then text_index = math.min(#TEXT_KEYS, text_index + 10) end
  if pressed(2) then text_index = math.max(1, text_index - 1) end
  if pressed(3) then text_index = math.min(#TEXT_KEYS, text_index + 1) end
  if pressed(5) then
    sys.net_action("edit", ""); screen = "network"; reset_edges()
  elseif pressed(4) then
    local k = TEXT_KEYS[text_index]
    if k == "done" then
      if text_kind == "ticket" and #n.text > 0 then
        sys.net_action("edit", ""); start_network(n.text)
      elseif text_kind == "relay" then
        sys.net_action("relay", n.text, n.relay_only and #n.text > 0)
        sys.net_action("edit", ""); screen = "network"; reset_edges()
      end
    elseif k == "paste" then sys.net_action("paste")
    elseif k == "clear" then sys.net_action("text", "")
    elseif k == "del" then sys.net_action("text", n.text:sub(1, -2))
    elseif #n.text < 1024 then sys.net_action("text", n.text .. k)
    end
  end
end
local function draw_net_text()
  cls(PANEL)
  local n = sys.network()
  print(text_kind == "ticket" and "enter full join ticket" or "relay URL (empty disables)", 16, 12, HILITE)
  network_text(n.text:sub(-144), 24)
  for i, k in ipairs(TEXT_KEYS) do
    -- Ten characters to a row, then the four actions on their own row.
    local x, y
    if i <= #CHARACTERS then
      x, y = 10 + ((i - 1) % 10) * 30, 60 + ((i - 1) // 10) * 12
    else
      x, y = 10 + (i - #CHARACTERS - 1) * 72, 164
    end
    if i == text_index then rectfill(x - 2, y - 2, x + #k * CW + 1, y + CH, 2) end
    print(k, x, y, i == text_index and HILITE or TEXT)
  end
  print("type / Ctrl+V / Backspace", 16, H - 48, DIM)
  print("arrows select  Enter A  Escape B", 16, H - 36, DIM)
  network_text(n.detail, H - 24, DIM)
end

local function update_nearby()
  local n = sys.network()
  if pressed(5) then screen = "network"; reset_edges(); return end
  if pressed(0) then nearby_index = nearby_index - 1 end
  if pressed(1) then nearby_index = nearby_index + 1 end
  nearby_index = math.max(1, math.min(#n.candidates, nearby_index))
  if pressed(4) then
    if not n.discovery then sys.net_action("discovery", "", true)
    elseif n.candidates[nearby_index] then start_network(n.candidates[nearby_index].ticket) end
  end
end
local function draw_nearby()
  cls(PANEL)
  local n = sys.network()
  print("compatible LAN sessions", 16, 12, HILITE)
  if not n.discovery then network_text("Discovery is off. A enables announcements and searching on this LAN.", 40)
  elseif #n.candidates == 0 then network_text("No compatible sessions found. The host must enable LAN discovery too. Full tickets also work.", 40)
  else
    local first = math.max(1, nearby_index - 11)
    for i = first, math.min(#n.candidates, first + 11) do
      print((i == nearby_index and "> " or "  ") .. n.candidates[i].title:sub(1, 34), 16, 36 + (i - first) * 10, i == nearby_index and HILITE or TEXT)
    end
  end
  network_text(n.discovery_detail, H - 60, DIM)
  print("A join selected    B back", 16, H - 16, DIM)
end

local function update_connecting()
  local n = sys.network()
  local a, b = pressed(4), pressed(5)
  if b then network_back()
  elseif a then
    if n.status == "connected" then
      sys.net_action("overlay", "", false)
      screen = "cart"; reset_edges()
    elseif n.ticket ~= "" then sys.net_action("copy") end
  end
end
local function draw_connecting()
  cls(PANEL)
  local n = sys.network()
  print("connection: " .. n.status, 16, 12, HILITE)
  network_text(n.detail, 32)
  network_text("path: " .. (n.path ~= "" and n.path or "waiting for connection"), 70, DIM)
  network_text("network: " .. n.network_profile, 106, DIM)
  if n.ticket ~= "" then network_text("ticket: " .. n.ticket, 142) end
  if n.status == "connected" then print("A play    B end session", 16, H - 16, HILITE)
  else print("A copy host ticket    B cancel / back", 16, H - 16, DIM) end
end

local screens = {
  boot = { update_boot, draw_boot },
  list = { update_list, draw_list_screen },
  info = { update_info, draw_info },
  cart = { update_cart, draw_cart },
  pause = { update_pause, draw_pause },
  settings = { update_settings, draw_settings },
  error = { update_error, draw_error },
  network = { update_network, draw_network },
  net_permission = { update_permission, draw_permission },
  net_text = { update_net_text, draw_net_text },
  nearby = { update_nearby, draw_nearby },
  connecting = { update_connecting, draw_connecting },
  dev = { update_dev, draw_dev },
}

function _update(dt)
  -- The overlay follows the cart's screen mode while this state lives
  -- on, so the size is read every frame, not only at boot.
  local w, h = stat("width"), stat("height")
  if w and h then W, H = w, h end
  set_font(stat("font"))
  if single == nil then single = sys.single() end
  -- The host can start a cart itself (a development deploy): whatever
  -- the shell was showing, it shows that cart. A network screen held
  -- the overlay, which keeps the buttons from the cart.
  if sys.host_started() then
    net_managed = false
    last_fault = nil
    sys.net_action("overlay", "", false)
    sys.net_action("browse", "")
    screen = "cart"
    reset_edges()
  end
  -- A cart can die while the shell is on any screen.
  if screen ~= "error" and sys.fault() then
    last_fault = sys.fault()
    screen = "error"
    reset_edges()
  end
  -- A developer waiting for an answer comes before whatever was shown,
  -- and a running cart waits meanwhile.
  local asking = sys.dev_state() == 2
  if screen ~= "dev" and asking then
    dev_return = screen
    dev_paused = screen == "cart"
    if dev_paused then sys.paused(true) end
    dev_frames, dev_hold = 0, 0
    screen = "dev"
    reset_edges()
  elseif screen == "dev" and not asking then
    -- Answered elsewhere, or the receiver is gone: nothing to ask.
    if dev_paused then sys.paused(false) end
    screen = dev_return
    reset_edges()
  end
  screens[screen][1]()
  -- With one cart, the list is the way out.
  if single and screen == "list" then sys.exit() end
end

function _draw()
  local network_screen = screen == "network" or screen == "net_permission"
    or screen == "net_text" or screen == "nearby" or screen == "connecting"
  if network_screen and W == 640 and H == 480 then
    -- Keep the same readable 320x240 layout before and during a cart,
    -- in the 8x8 face scaled up. Release each frame because changing
    -- cart modes rebuilds the slab.
    local canvas = buf("u8", 320, 240)
    W, H = 320, 240
    font(8)
    set_font(8)
    draw_target(canvas)
    screens[screen][2]()
    draw_target()
    font()
    sheet(canvas)
    sspr(0, 0, 320, 240, 0, 0, 640, 480)
    canvas:release()
    W, H = 640, 480
    set_font(stat("font"))
  else screens[screen][2]() end
end
