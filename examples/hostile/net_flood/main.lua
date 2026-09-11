-- Hostile: sends in a loop every frame. The outbox takes sixteen sends
-- per frame and answers `false` to the rest; the loop itself spends
-- the frame's cycles, so the cart ends with budget_exceeded, and the
-- outbox never exceeds sixteen.
function _init()
  net.host()
end

function _update(dt)
  while true do
    net.send("x")
  end
end
