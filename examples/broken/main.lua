-- Kuula example: a cart that breaks on purpose.
--
-- It counts frames, then indexes nil on frame 60 so the host has a
-- runtime error to report with a file and line. Keep the error on line 12.

local frame = 0

function _update(dt)
  frame = frame + 1
  if frame == 60 then
    local nothing = nil
    nothing.boom = frame
  end
end

function _draw()
  cls(2)
  print("breaks on frame 60", 2, 2, 7)
  print("frame " .. frame, 2, 10, 7)
end
