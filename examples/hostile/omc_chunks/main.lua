-- Hostile: music of 32 SONZ chunks, each just under a MiB of zeros
-- once inflated, asked for in a loop behind pcall. The loader refuses
-- the file once, with song_error, having inflated no more than the
-- song budget, and answers every later call from that refusal, so
-- the loop runs into the frame's budget instead of stalling the frame.
function _update(dt)
  while true do pcall(music, "big") end
end
function _draw() cls(2) end
