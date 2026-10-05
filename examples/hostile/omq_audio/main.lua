-- Hostile: a bank of cues whose one cue plays a FLAC file of 42 bytes
-- that declares 16 MiB of frames, 32 MiB of PCM, against the 2 MiB
-- sample budget. The loader refuses it with sample_error from the
-- length the file's header declares, before it decodes a frame; a
-- song_error here would mean the file got as far as the decoder.
function _init() cue("big", "boom") end
function _draw() cls(2) end
