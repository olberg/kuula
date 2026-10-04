-- Hostile: music whose SONZ chunk is two MiB of zeros once inflated,
-- past the 1 MiB song budget. The loader refuses it with song_error
-- without ever holding the inflated bytes.
function _init() music("big") end
function _draw() cls(2) end
