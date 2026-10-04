-- Hostile: a song whose one sample record declares 64 MiB of PCM,
-- its size spelled as a float, against a FLAC stream of 42 bytes: past
-- the 2 MiB sample budget. The loader refuses it with sample_error
-- before decoding anything; a song_error here would mean the size got
-- as far as the decoder.
function _init() music("big") end
function _draw() cls(2) end
