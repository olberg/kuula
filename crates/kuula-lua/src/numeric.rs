//! One libm on every platform.
//!
//! The C library's `sin`, `pow` and friends differ in the last bit
//! between the UCRT, glibc and musl, so a cart that used them would draw
//! differently on the desktop and on the device. Every transcendental
//! function a cart can reach goes through the `libm` crate instead, a
//! port of musl's, which is the same code on every target:
//!
//! - the `math` table's `sin`, `cos`, `tan`, `asin`, `acos`, `atan`,
//!   `exp` and `log` are replaced here with Rust functions of the same
//!   contract (`atan(y, x)` and `log(x, base)` included);
//! - the float `^` operator reaches [`kuu_numpow`] through the
//!   `luai_numpow` hook in `crates/kuula-lua/c/kuula_user.h`, so
//!   constants the compiler folds and powers computed at run time use
//!   the same code;
//! - `sqrt`, `fmod`, `floor`, `ceil` and the integer paths stay Lua's
//!   own: IEEE 754 requires those to be exact, so every libm agrees.
//!
//! NaN is the other platform difference. The UCRT prints `-nan(ind)`,
//! glibc and musl `-nan` or `nan` depending on the sign bit, which the
//! hardware chooses. [`kuu_fixnan`] sits behind Lua's `sprintf` and
//! `snprintf` (through the same header) and rewrites any NaN the C library printed
//! to `nan`, sign dropped, so `tostring(0/0)` is `nan` everywhere. The
//! sign and payload of a NaN remain observable through `string.pack`
//! and are not part of the numeric profile (`docs/api.md`).

use std::ffi::{c_char, c_int, CStr};

use crate::api::reg::Reg;
use crate::api::{Group, Price, Scope, Sig};
use mlua::Result;

/// Lua's own `^`: `b == 2` is a multiplication, everything else a `pow`.
#[no_mangle]
pub extern "C" fn kuu_numpow(a: f64, b: f64) -> f64 {
    if b == 2.0 {
        a * a
    } else {
        libm::pow(a, b)
    }
}

/// Normalise the NaN spelling in a buffer `snprintf` just filled from a
/// one-conversion format. `len` is what `snprintf` returned; the return
/// value replaces it. Anything that is not a float conversion, or was
/// truncated, or holds no NaN, is left alone.
///
/// # Safety
/// `buf` must point to at least `size` writable bytes and `fmt` to a
/// NUL-terminated string, as for `snprintf` itself.
#[no_mangle]
pub unsafe extern "C" fn kuu_fixnan(
    buf: *mut c_char,
    size: usize,
    fmt: *const c_char,
    len: c_int,
) -> c_int {
    if buf.is_null() || fmt.is_null() || len < 0 || len as usize >= size {
        return len;
    }
    let fmt = CStr::from_ptr(fmt).to_bytes();
    let Some(&conv) = fmt.last() else {
        return len;
    };
    if !b"eEfFgGaA".contains(&conv) {
        return len;
    }
    let out = std::slice::from_raw_parts_mut(buf as *mut u8, size);
    if !has_nan(&out[..len as usize]) {
        return len;
    }
    let (left, width) = parse_width(fmt);
    let text: &[u8] = if conv.is_ascii_uppercase() {
        b"NAN"
    } else {
        b"nan"
    };
    let total = width.max(text.len());
    if total >= size {
        return len;
    }
    let pad = total - text.len();
    let (front, back) = if left { (0, pad) } else { (pad, 0) };
    out[..front].fill(b' ');
    out[front..front + text.len()].copy_from_slice(text);
    out[front + text.len()..total].fill(b' ');
    out[total] = 0;
    debug_assert_eq!(front + text.len() + back, total);
    total as c_int
}

/// The `sprintf` form of [`kuu_fixnan`], for Lua's C89 configuration
/// (Windows). The buffer holds `len + 1` bytes of text, and a NaN spelt
/// by any C library is at least as long as the `nan` that replaces it
/// at the same width, so that is a sufficient bound.
///
/// # Safety
/// As for [`kuu_fixnan`], with `buf` holding `len + 1` written bytes.
#[no_mangle]
pub unsafe extern "C" fn kuu_fixnan_s(buf: *mut c_char, fmt: *const c_char, len: c_int) -> c_int {
    if len < 0 {
        return len;
    }
    kuu_fixnan(buf, len as usize + 1, fmt, len)
}

/// A printed float holds the letters `n` and `a` only in `nan`; `inf`
/// and hexadecimal floats never do.
fn has_nan(text: &[u8]) -> bool {
    text.windows(3).any(|w| w.eq_ignore_ascii_case(b"nan"))
}

/// The `-` flag and the width of a `%[flags][width][.prec]conv` format.
fn parse_width(fmt: &[u8]) -> (bool, usize) {
    let mut left = false;
    let mut width = 0usize;
    let mut i = 1;
    while i < fmt.len() && b"-+ #0".contains(&fmt[i]) {
        left |= fmt[i] == b'-';
        i += 1;
    }
    while i < fmt.len() && fmt[i].is_ascii_digit() {
        width = width * 10 + usize::from(fmt[i] - b'0');
        i += 1;
    }
    (left, width)
}

/// Replace the `math` table's transcendental functions. Called once per
/// state from the bindings, next to the `randomseed` replacement.
pub(crate) fn install(reg: &mut Reg<'_>) -> Result<()> {
    reg.function(&SIN, |_, x: f64| Ok(libm::sin(x)))?;
    reg.function(&COS, |_, x: f64| Ok(libm::cos(x)))?;
    reg.function(&TAN, |_, x: f64| Ok(libm::tan(x)))?;
    reg.function(&ASIN, |_, x: f64| Ok(libm::asin(x)))?;
    reg.function(&ACOS, |_, x: f64| Ok(libm::acos(x)))?;
    reg.function(&EXP, |_, x: f64| Ok(libm::exp(x)))?;
    reg.function(&ATAN, |_, (y, x): (f64, Option<f64>)| {
        Ok(libm::atan2(y, x.unwrap_or(1.0)))
    })?;
    reg.function(&LOG, |_, (x, base): (f64, Option<f64>)| {
        // Lua's own special cases for the two common bases.
        Ok(match base {
            None => libm::log(x),
            Some(b) => {
                if b == 2.0 {
                    libm::log2(x)
                } else if b == 10.0 {
                    libm::log10(x)
                } else {
                    libm::log(x) / libm::log(b)
                }
            }
        })
    })?;
    // A reference from Rust keeps the exported symbols in the link even
    // on linkers that only pull archive members for symbols already
    // undefined (GNU ld); the C objects of Lua come later on the line.
    debug_assert_eq!(kuu_numpow(2.0, 3.0), 8.0);
    std::hint::black_box(kuu_numpow as extern "C" fn(f64, f64) -> f64);
    std::hint::black_box(
        kuu_fixnan as unsafe extern "C" fn(*mut c_char, usize, *const c_char, c_int) -> c_int,
    );
    std::hint::black_box(
        kuu_fixnan_s as unsafe extern "C" fn(*mut c_char, *const c_char, c_int) -> c_int,
    );
    Ok(())
}

macro_rules! routed {
    ($id:ident, $name:literal, $call:literal) => {
        binding!($id {
            name: $name,
            scope: Scope::Std(Some("math")),
            group: Group::Numeric,
            sigs: &[Sig::new($call, "as Lua, computed by the pinned libm")],
            price: Price::Native,
        });
    };
}

routed!(SIN, "sin", "math.sin(x)");
routed!(COS, "cos", "math.cos(x)");
routed!(TAN, "tan", "math.tan(x)");
routed!(ASIN, "asin", "math.asin(x)");
routed!(ACOS, "acos", "math.acos(x)");
routed!(EXP, "exp", "math.exp(x)");
routed!(ATAN, "atan", "math.atan(y, [x])");
routed!(LOG, "log", "math.log(x, [base])");

#[cfg(test)]
mod tests {
    use super::*;

    fn fix(fmt: &str, text: &str) -> String {
        let mut buf = vec![0u8; 64];
        buf[..text.len()].copy_from_slice(text.as_bytes());
        let fmt = std::ffi::CString::new(fmt).unwrap();
        let len = unsafe {
            kuu_fixnan(
                buf.as_mut_ptr() as *mut c_char,
                buf.len(),
                fmt.as_ptr(),
                text.len() as c_int,
            )
        };
        String::from_utf8(buf[..len as usize].to_vec()).unwrap()
    }

    #[test]
    fn nan_spellings_collapse() {
        assert_eq!(fix("%.15g", "-nan(ind)"), "nan");
        assert_eq!(fix("%.15g", "-nan"), "nan");
        assert_eq!(fix("%.15g", "nan"), "nan");
        assert_eq!(fix("%G", "-NAN(IND)"), "NAN");
        assert_eq!(fix("%a", "-nan(ind)"), "nan");
    }

    #[test]
    fn width_and_left_flag_survive() {
        assert_eq!(fix("%8.2f", "-nan(ind)"), "     nan");
        assert_eq!(fix("%-8.2f", "-nan(ind)"), "nan     ");
    }

    #[test]
    fn non_nan_and_non_float_are_untouched() {
        assert_eq!(fix("%.15g", "inf"), "inf");
        assert_eq!(fix("%.15g", "1.5"), "1.5");
        assert_eq!(fix("%10s", "nan(ind)"), "nan(ind)");
        assert_eq!(fix("%d", "42"), "42");
    }

    #[test]
    fn pow_special_case() {
        assert_eq!(kuu_numpow(3.0, 2.0), 9.0);
        assert_eq!(kuu_numpow(2.0, 10.0), 1024.0);
    }
}
