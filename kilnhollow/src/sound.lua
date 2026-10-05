-- The effects, a bank of cues (cues/fx.omc): the cart asks for one by name
-- and the console finds it a channel. There is no music.

local S = {}

function S.play(name)
  cue("fx", name)
end

return S
