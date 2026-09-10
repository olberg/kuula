-- Hostile: unbounded recursion. The meter trips long before Lua's own
-- stack limit, so this ends with budget_exceeded.
local function down(n) return 1 + down(n + 1) end
function _update(dt)
  down(0)
end
