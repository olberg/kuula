-- Hostile: grow a table forever. Each frame stays under the cycle budget
-- but the heap climbs past the soft cap; this ends with out_of_memory.
t = {}
function _update(dt)
  for i = 1, 8000 do t[#t + 1] = {i} end
end
function _draw()
  cls(2)
  print("mem " .. math.floor(stat("mem") / 1024) .. " KiB", 2, 2, 7)
end
