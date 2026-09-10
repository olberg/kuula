-- Hostile: a thousand full-screen clears per frame. Drawing is priced by
-- pixels touched after clipping; this ends with budget_exceeded in _draw.
function _draw()
  for i = 1, 1000 do cls(i % 16) end
end
