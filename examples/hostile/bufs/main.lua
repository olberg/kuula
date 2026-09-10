-- Hostile: allocate buffers forever and keep them. The graphics ledger
-- refuses the ninth megabyte; this ends with graphics_budget_exceeded.
keep = {}
function _update(dt)
  while true do keep[#keep + 1] = buf("u8", 1024, 1024) end
end
