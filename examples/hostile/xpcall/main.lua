-- Hostile: an xpcall message handler that loops. The meter keeps its own
-- record of the fault, so this ends with budget_exceeded, not
-- "error in error handling".
function _update(dt)
  xpcall(function() while true do end end, function(e) while true do end end)
  cls(7)
end
