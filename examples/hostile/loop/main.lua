-- Hostile: an infinite loop in _update. Must end with budget_exceeded.
function _draw() cls(1) print("loop", 2, 2, 7) end
function _update(dt)
  while true do end
end
