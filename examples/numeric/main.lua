-- Kuula example: the numeric profile probe.
--
-- Prints, through the log, the same lines the Wasm gate's numeric probe
-- prints (tools/wasm-gate/host/src/main.rs, NUMERIC and NAN): for ten
-- thousand arguments the routed math functions, the float operators,
-- formatting and integer arithmetic, then the special values and the NaN
-- spelling. The output must be identical on every supported host; its
-- digest and tail are pinned in expected.txt and checked by
-- crates/kuula-cli/tests/determinism.rs.
--
-- Twenty arguments per frame keep each frame inside its cycle budget and
-- under the 256-line log cap; frame 1 runs only _init, frames 2 to 501
-- print the arguments, frame 502 prints the tail. Run it with
--   kuula run examples/numeric --headless --frames 502
-- and compare stdout with the gate's numeric_wasm.txt.

local ITERATIONS = 10000
local PER_FRAME = 20

local i = 0
local done = false

local function bits(v)
  return (string.pack('<d', v):gsub('.', function(c) return string.format('%02x', c:byte()) end))
end

local function argument(n)
  local x = n / 7
  print(string.format('%.14g', x))
  print(string.format('%.14g', math.sin(x)))
  print(string.format('%.14g', math.exp(x / 100)))
  print(string.format('%.14g', math.log(x)))
  print(string.format('%.14g', x ^ 1.5))
  print(string.format('%.14g', math.fmod(x, 0.3)))
  print(string.format('%.14g', math.sqrt(x)))
  print(string.format('%.14g', math.atan(x, 1.5)))
  print(bits(math.sin(x)) .. bits(math.exp(x / 100)) .. bits(math.log(x)) .. bits(x ^ 1.5) .. bits(math.cos(x)) .. bits(math.tan(x)))
  print(tostring(x) .. tostring(x * 1e15) .. tostring(x * 1e16) .. tostring(math.floor(x)))
  print(tostring(math.tointeger(x * 7)) .. tostring(n // 3) .. tostring(-n // 3) .. tostring(-n % 3) .. tostring(n % -3))
  print(tostring(string.unpack('<i8', string.pack('<i8', n * 1234567))))
end

local function tail()
  local specials = {0.0, -0.0, 1/0, -1/0, 2^-1070, 2^-1022, 2^1023, 1e300 * 1e10, math.pi, math.huge,
    math.maxinteger + 0.0, math.mininteger + 0.0, 2^53, 2^53 + 1, 0.1 + 0.2, 1e15, 1e16, 123456789012345678}
  for _, v in ipairs(specials) do
    print(tostring(v) .. ' ' .. string.format('%.14g', v) .. ' ' .. bits(v) .. ' ' .. tostring(math.tointeger(v)))
  end
  print(tostring(math.maxinteger) .. tostring(math.mininteger) .. tostring(math.maxinteger + 1) .. tostring(math.mininteger - 1))
  print(tostring(3 // 0.0) .. tostring(-3 // 0.0) .. tostring(3 % math.huge) .. tostring(-3 % math.huge))
  print(tostring(pcall(function() return 1 // 0 end)))
  print(string.format('%5.2f|%e|%g|%a|%x|%d', 3.14159, 12345.678, 0.0001234, 1.5, 255, -7))
  -- The NAN probe: every NaN prints as "nan" whatever its sign.
  print(tostring(0/0) .. ' ' .. tostring(-(0/0)) .. ' ' .. string.format('%.14g', 0/0) .. ' ' .. tostring(math.sqrt(-1)))
end

function _init()
  cls(0)
end

function _update(dt)
  if i >= ITERATIONS then
    if not done then
      done = true
      tail()
    end
    return
  end
  for _ = 1, PER_FRAME do
    i = i + 1
    argument(i)
  end
end

function _draw()
  cls(0)
  print('numeric probe ' .. i .. '/' .. ITERATIONS, 2, 2, 7)
  if done then print('done', 2, 10, 11) end
end
