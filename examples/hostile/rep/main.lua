-- Hostile: string.rep of a gigabyte. Priced by output bytes before the
-- allocation, so this ends with budget_exceeded.
function _update(dt)
  local s = string.rep("x", 1 << 30)
end
