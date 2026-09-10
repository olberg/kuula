//! The numeric profile: the routed `math` functions and
//! the `^` operator agree bit for bit with the `libm` crate, and NaN
//! prints as `nan` whatever the C library would have said.

use super::console;
use kuula_core::FrameInput;

/// Whatever the cart printed on its first frame.
fn logged(body: &str) -> Vec<String> {
    let mut c = console(body);
    let log = c.step(FrameInput::NONE).log.to_vec();
    assert_eq!(c.state().fault(), None, "{:?}", c.state());
    log
}

fn hex(x: f64) -> String {
    x.to_le_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn nan_prints_as_nan_everywhere() {
    let lines = logged(
        r#"
        local n = 0/0
        print(tostring(n))
        print(tostring(-n))
        print(n .. "")
        print(string.format("%.14g|%8.2f|%-6G|%a|%e", n, n, n, n, -n))
        print(string.format("%10s", "nan(ind)"))
        print(tostring(1/0) .. "	" .. tostring(-1/0) .. "	" .. tostring(math.sqrt(-1)))
        "#,
    );
    assert_eq!(
        lines,
        [
            "nan",
            "nan",
            "nan",
            "nan|     nan|NAN   |nan|nan",
            "  nan(ind)",
            "inf\t-inf\tnan",
        ]
    );
}

#[test]
fn pow_folded_and_at_run_time_matches_libm() {
    let lines = logged(
        r#"
        local function bits(v) return (string.pack('<d', v):gsub('.', function(c) return string.format('%02x', c:byte()) end)) end
        local b = 1.5
        local x = 7/3
        print(bits(x ^ b), bits(x ^ 1.5), bits(2.5 ^ 1.5), bits(x ^ 2), bits(2 ^ 0.5), bits((-8) ^ (1/3)))
        "#,
    );
    let x = 7.0 / 3.0;
    let want = [
        libm::pow(x, 1.5),
        libm::pow(x, 1.5),
        libm::pow(2.5, 1.5),
        x * x,
        libm::pow(2.0, 0.5),
        libm::pow(-8.0, 1.0 / 3.0),
    ]
    .iter()
    .map(|&v| hex(v))
    .collect::<Vec<_>>()
    .join("\t");
    assert_eq!(lines, [want]);
}

#[test]
fn math_functions_match_libm() {
    let lines = logged(
        r#"
        local function bits(v) return (string.pack('<d', v):gsub('.', function(c) return string.format('%02x', c:byte()) end)) end
        for i = 1, 50 do
          local x = i / 7
          print(bits(math.sin(x)) .. bits(math.cos(x)) .. bits(math.tan(x)) .. bits(math.exp(x))
            .. bits(math.log(x)) .. bits(math.atan(x)) .. bits(math.atan(x, -2)) .. bits(math.asin(x / 8))
            .. bits(math.acos(x / 8)) .. bits(math.log(x, 2)) .. bits(math.log(x, 10)) .. bits(math.log(x, 3)))
        end
        print(table.concat({math.log(8, 2), math.log(1000, 10), math.atan(1, 1) * 4, math.sin("0.5")}, "	"))
        "#,
    );
    let text = lines.join("\n");
    let mut want = Vec::new();
    for i in 1..=50 {
        let x = i as f64 / 7.0;
        let row = [
            libm::sin(x),
            libm::cos(x),
            libm::tan(x),
            libm::exp(x),
            libm::log(x),
            libm::atan2(x, 1.0),
            libm::atan2(x, -2.0),
            libm::asin(x / 8.0),
            libm::acos(x / 8.0),
            libm::log2(x),
            libm::log10(x),
            libm::log(x) / libm::log(3.0),
        ]
        .iter()
        .map(|&v| hex(v))
        .collect::<String>();
        want.push(row);
    }
    let last = format!(
        "3.0\t3.0\t{}\t{}",
        lua_number(libm::atan2(1.0, 1.0) * 4.0),
        lua_number(libm::sin(0.5))
    );
    want.push(last);
    assert_eq!(text, want.join("\n"));
}

/// Lua's `%.15g`, falling back to `%.17g` when that does not round
/// trip, for the few values the test prints as text.
fn lua_number(x: f64) -> String {
    let short = format_g(x, 15);
    if short.parse::<f64>() == Ok(x) {
        short
    } else {
        format_g(x, 17)
    }
}

fn format_g(x: f64, prec: usize) -> String {
    // The values used here are of magnitude 0.1 to 10, where %g is the
    // fixed form with trailing zeros removed.
    let s = format!("{x:.*}", prec - 1 - x.abs().log10().floor() as usize);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

#[test]
fn integer_and_exact_paths_are_lua_own() {
    let lines = logged(
        r#"
        print(table.concat({math.fmod(7, 3), math.fmod(-7, 3), math.fmod(7.5, 2), math.sqrt(16), math.floor(-0.5), 7 // 2, 7.0 // 2, -7 % 3}, "	"))
        print(table.concat({tostring(pcall(math.fmod, 1, 0))}, "	") .. "	" .. select(2, pcall(math.fmod, 1, 0)))
        print(math.type(math.fmod(7, 3)) .. "	" .. math.type(2 ^ 2) .. "	" .. math.type(math.sin(0)))
        "#,
    );
    assert_eq!(lines[0], "1\t-1\t1.5\t4.0\t-1\t3\t3.0\t2");
    assert!(lines[1].starts_with("false\t"), "{}", lines[1]);
    assert!(lines[1].contains("zero"), "{}", lines[1]);
    assert_eq!(lines[2], "integer\tfloat\tfloat");
}
