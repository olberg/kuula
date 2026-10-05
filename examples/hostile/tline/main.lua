-- Hostile: lines as long as the number range allows, with positions and
-- steps as large as it allows and a texture rectangle far off the sheet,
-- thousands of them a frame. A line is clipped to the screen before it is
-- walked and paid for by the pixels it touches, so none of them reads out of
-- the sheet or runs long, and the frame ends with budget_exceeded in _draw.
function _init()
  sheet(buf("u8", 64, 64))
end

function _draw()
  for i = 1, 5000 do
    tline(-2147483648, i % 240, 2147483647, i % 240, 1e300, -1e300, 1e300, 1e-300, 32, 32, 1e9, 1e9)
    tline(i % 320, -2147483648, i % 320, 2147483647, -1e300, 1e300, 0 / 0, 1e300, -5, -5, 1e9, 1e9)
  end
end
