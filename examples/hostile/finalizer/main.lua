-- Hostile: a __gc finaliser that loops. Lua runs finalisers with hooks
-- switched off, so the meter could never see this; setmetatable refuses
-- __gc instead and this ends with runtime_error on the line below.
function _update(dt)
  setmetatable({}, { __gc = function() while true do end end })
  cls(7)
end
