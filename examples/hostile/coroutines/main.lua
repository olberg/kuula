-- Hostile: a coroutine storm. Each new thread starts with a fresh hook
-- count, so creation is priced; this ends with budget_exceeded.
function _update(dt)
  while true do
    local co = coroutine.wrap(function()
      local x = 0
      for i = 1, 500 do x = x + i end
    end)
    co()
  end
end
