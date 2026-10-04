-- Marble Duel: the host owns state; the joiner submits numbered moves.
local palette = require('palette')
local host, started, waiting, dirty = false, false, false, false
local selected, was, gfx = 1, {}, nil
local rejoin_in = 0
game = { remaining = 15, turn = 0, active = 1, winner = 0,
         status = 'waiting', you = 1, detail = '' }

local function edge(button)
  local now = btn(button)
  local result = now and not was[button]
  was[button] = now
  return result
end

local function publish()
  if host and dirty and net.status() == 'connected' then
    dirty = not net.send('S' .. string.char(game.remaining, game.active, game.winner, game.turn))
  end
end

local function begin()
  game.remaining, game.turn, game.active, game.winner = 15, 0, 1, 0
  game.status, game.detail, selected, waiting = 'playing', '', 1, false
  dirty = host
end

local function move(player, turn, amount)
  if game.status ~= 'playing' or player ~= game.active or turn ~= game.turn then return end
  if amount < 1 or amount > 3 or amount > game.remaining then return end
  game.remaining = game.remaining - amount
  game.turn = game.turn + 1
  if game.remaining == 0 then
    game.winner, game.status = player, 'won'
    print('winner ' .. player .. ' turn ' .. game.turn)
  else game.active = 3 - player end
  dirty = true
  print('move ' .. player .. ' took ' .. amount .. ' remaining ' .. game.remaining)
end

local function drain()
  while true do
    local event = net.recv()
    if not event then break end
    if event.kind == 'hosting' then
      print('ticket: ' .. event.ticket)
    elseif event.kind == 'connected' then
      begin()
    elseif event.kind == 'message' then
      local data = event.data
      if host and #data == 3 and data:sub(1, 1) == 'M' then
        move(2, data:byte(2), data:byte(3))
      elseif not host and #data == 5 and data:sub(1, 1) == 'S' then
        local remaining, active, winner, turn = data:byte(2, 5)
        if remaining <= 15 and active >= 1 and active <= 2 and winner <= 2
            and turn <= 15 and turn >= game.turn and (winner == 0 or remaining == 0) then
          game.remaining, game.active, game.winner, game.turn = remaining, active, winner, turn
          game.status = winner == 0 and 'playing' or 'won'
          waiting = false
        end
      end
    elseif event.kind == 'disconnected' then
      game.status, game.detail, waiting = 'ended', 'peer disconnected - round ended', false
    elseif event.kind == 'permission' and not event.granted then
      game.status, game.detail = 'offline', 'network permission is off'
    elseif event.kind == 'failed' then
      if event.code == 'net_queue_full' then
        -- The move/state was accepted locally, but not delivered. Retrying
        -- the same turn is safe: the host rejects duplicate turn numbers.
        if host then dirty = true else waiting = false end
        game.detail = 'network busy - try again'
      else
        game.status = event.code == 'net_denied' and 'offline' or 'ended'
        game.detail = event.detail or event.code
      end
    end
  end
end

function _init()
  gfx = load_sheet('garden')
  sheet(gfx)
  for i, rgb in ipairs(palette) do pal(15 + i, rgb) end
  host = net.invite() == nil
  game.you = host and 1 or 2
end

function _update()
  drain()
  if rejoin_in > 0 then
    rejoin_in = rejoin_in - 1
    if rejoin_in == 0 then started = false end
  end
  if not started then
    started = true
    if host then net.host() else net.join(net.invite()) end
  end
  local left, right, a, b = edge(2), edge(3), edge(4), edge(5)
  if left then selected = math.max(1, selected - 1) end
  if right then selected = math.min(3, selected + 1) end
  selected = math.min(selected, math.max(1, game.remaining))
  if a and game.status == 'playing' and game.active == game.you and not waiting then
    if host then move(1, game.turn, selected)
    else waiting = net.send('M' .. string.char(game.turn, selected)) end
  end
  if b and (game.status == 'ended' or game.status == 'won') then
    if host and (net.status() == 'connected' or net.status() == 'hosting') then
      game.detail = 'joiner: B starts a fresh session'
    else
      net.leave()
      game.status, game.detail, waiting = 'waiting', '', false
      rejoin_in = 30
    end
  end
  publish()
end

local function centre(text, y, colour)
  print(text, (320 - #text * 8) // 2, y, colour)
end

function _draw()
  sspr(0, 0, 320, 240, 0, 0)
  rectfill(28, 10, 291, 39, 1)
  centre('M A R B L E   D U E L', 17, 7)
  centre('TAKE 1-3. THE LAST MARBLE WINS.', 29, 6)
  rectfill(28, 61, 291, 126, 1)
  if game.status == 'playing' then
    centre(game.active == game.you and 'YOUR TURN' or 'PEER TURN', 69, game.you == 1 and 10 or 12)
    centre(game.remaining .. ' MARBLES LEFT', 84, 7)
    centre(waiting and 'WAITING FOR HOST...' or ('<  TAKE ' .. selected .. '  >'), 103, 7)
    centre('A CONFIRM', 116, 6)
  elseif game.status == 'won' then
    centre(game.winner == game.you and 'YOU WIN!' or 'PEER WINS!', 77, game.winner == game.you and 10 or 12)
    centre(host and 'JOINER: B FRESH SESSION' or 'B FRESH SESSION', 99, 7)
  else
    centre(game.status:upper(), 72, 7)
    centre(game.status == 'waiting' and 'HOST OR JOIN FROM THE LAUNCHER' or 'ROUND ENDED', 90, 6)
    if game.status == 'ended' then centre('B FRESH JOIN / HOST', 106, 7) end
  end
  for i = 1, game.remaining do
    local row, column = (i - 1) // 5, (i - 1) % 5
    local x, y = 160 + (column - 2) * (33 + row * 2), 151 + row * 8
    rectfill(x - 4, y + 3, x + 4, y + 4, 4)
    circfill(x, y, 4, 1)
    circfill(x, y - 1, 3, game.active == 1 and 9 or 12)
    pset(x - 1, y - 3, 7)
    pset(x - 2, y - 2, 7)
  end
  rectfill(28, 206, 291, 233, 1)
  centre(game.you == 1 and 'AMBER / HOST' or 'TURQUOISE / JOINER', 211, game.you == 1 and 10 or 12)
  centre(game.detail:sub(1, 32), 222, 6)
end
