-- Hostile: a backtracking pattern on a long subject. Priced by its worst
-- case before it runs, so this ends with budget_exceeded, not a hang.
function _init()
  local s = string.rep("a", 100000)
  print(s:find(".-.-.-.-b"))
end
