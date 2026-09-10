-- Hostile: pcall around an overrun, then carry on as if nothing
-- happened. The fault is raised again outside the pcall; this ends
-- with budget_exceeded and nothing is drawn afterwards.
function _update(dt)
  pcall(function() while true do end end)
  cls(7)
  while true do pcall(function() end) end
end
