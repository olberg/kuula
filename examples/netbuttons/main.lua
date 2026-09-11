-- Kuula example: buttons over the network.
--
-- Hosts when launched without an invite, joins the invite otherwise.
-- Every button edge sends one byte ("d" plus the button number), and
-- the last sixteen presses the peer sent are drawn as a row of glyphs.
-- Offline (no --net, or permission withdrawn) it still runs and shows
-- its own presses, so the useful offline state is visible.
--
--   kuula run examples/netbuttons --net host
--   kuula run examples/netbuttons --net join <ticket>
--
-- Everything the cart knows about the network comes from net.recv()
-- at the top of _update, drained into these locals; nothing is drawn
-- from the inbox itself.

local W, H = 320, 240
local NAMES = { "up", "down", "left", "right", "a", "b" }
local GLYPHS = { "^", "v", "<", ">", "A", "B" }

local status = "off"
local ticket = nil
local last_event = "none"
local received = {}      -- last sixteen presses from the peer
local count = 0
local sent = 0
local mine = {}          -- last sixteen local presses
local was = {}
local started = false

local function push(list, v)
  list[#list + 1] = v
  if #list > 16 then table.remove(list, 1) end
end

local function drain()
  while true do
    local e = net.recv()
    if not e then break end
    last_event = e.kind
    if e.kind == "hosting" then
      ticket = e.ticket
    elseif e.kind == "message" then
      local n = e.data:byte(2) or 0
      push(received, GLYPHS[n] or "?")
      count = count + 1
    elseif e.kind == "failed" then
      last_event = "failed " .. e.code
    elseif e.kind == "disconnected" then
      last_event = "disconnected " .. e.reason
    elseif e.kind == "permission" then
      last_event = "permission " .. tostring(e.granted)
    end
  end
  status = net.status()
end

function _update(dt)
  drain()
  if not started then
    started = true
    local invite = net.invite()
    if invite then net.join(invite) else net.host() end
  end
  for i = 0, 5 do
    local down = btn(i)
    if down and not was[i] then
      push(mine, GLYPHS[i + 1])
      if net.send("d" .. string.char(i + 1)) then sent = sent + 1 end
    end
    was[i] = down
  end
end

local function wrapped(text, x, y, width, colour)
  local per = width // 4
  local line = 0
  for i = 1, #text, per do
    print(text:sub(i, i + per - 1), x, y + line * 8, colour)
    line = line + 1
  end
  return line
end

function _draw()
  cls(1)
  print("net buttons", 4, 4, 7)
  if status == "off" then
    print("offline", 4, 14, 8)
  else
    print(status, 4, 14, 11)
  end
  print("last: " .. last_event, 4, 24, 6)
  local y = 36
  if ticket and status ~= "connected" then
    print("ticket:", 4, y, 6)
    y = y + 8 + 8 * wrapped(ticket, 4, y + 8, W - 8, 13)
  end
  print("mine  " .. table.concat(mine, " "), 4, 120, 10)
  print("peer  " .. table.concat(received, " "), 4, 132, 12)
  print("sent " .. sent .. " got " .. count .. " inbox " .. stat("net_inbox"), 4, 148, 6)
  for i = 1, 6 do
    local colour = btn(i - 1) and 11 or 5
    rectfill(4 + (i - 1) * 20, H - 24, 18 + (i - 1) * 20, H - 10, colour)
    print(GLYPHS[i], 8 + (i - 1) * 20, H - 21, 0)
  end
end
