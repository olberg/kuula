/*
** Kuula's build configuration for the vendored Lua 5.5.1.
**
** lua.h includes this file through LUA_USER_H, which .cargo/config.toml
** defines for every C compile in the workspace, and the same file makes
** this directory an include path. Nothing in the Lua sources changes;
** this is the "one numeric profile", wired
** through the hooks Lua leaves open for it:
**
**  - luai_numpow is the float `^` operator (llimits.h defines its own
**    only when the macro is unset). It is routed to kuu_numpow, a
**    Rust function over the musl-derived libm crate, so a constant
**    folded by the compiler and a power computed at run time agree with
**    each other and with every other supported host.
**  - sprintf (Windows, where luaconf.h selects LUA_USE_C89) or snprintf
**    (elsewhere) is what l_sprintf expands to, and every float Lua ever
**    prints (tostring, `..`, print, string.format) goes through it.
**    The wrappers keep the C library's conversion and then hand the
**    text to kuu_fixnan, which rewrites the platform's NaN spelling
**    (UCRT's "-nan(ind)", glibc's "-nan") to "nan".
**
** The symbols live in crates/kuula-lua/src/numeric.rs.
*/
#ifndef KUULA_USER_H
#define KUULA_USER_H

/* stdio.h must be seen before snprintf becomes a macro, or its inline
** definition of snprintf would be renamed as well. */
#include <stddef.h>
#include <stdio.h>

extern double kuu_numpow(double a, double b);
extern int kuu_fixnan(char *buf, size_t size, const char *fmt, int len);
extern int kuu_fixnan_s(char *buf, const char *fmt, int len);

#define luai_numpow(L, a, b) ((void)(L), kuu_numpow((a), (b)))

/* Every use in Lua has exactly one conversion argument (luaconf.h's
** l_sprintf), so the parenthesised call to the real function is
** well-formed and the format is still at hand for the fixer. The
** sprintf form has no buffer size; kuu_fixnan_s derives one from the
** length written, which is enough because the rewritten text is never
** longer than what the C library printed for the NaN. */
#define snprintf(s, sz, f, ...)   kuu_fixnan((s), (sz), (f), (snprintf)((s), (sz), (f), __VA_ARGS__))
#define sprintf(s, f, ...)   kuu_fixnan_s((s), (f), (sprintf)((s), (f), __VA_ARGS__))

#endif
