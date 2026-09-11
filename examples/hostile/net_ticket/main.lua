-- Hostile: hands a 1 MiB ticket to join. The binding refuses it as a
-- Lua error before anything is queued (net_ticket in the message);
-- uncaught, that is runtime_error.
function _init()
  net.join(string.rep("t", 1024 * 1024))
end
