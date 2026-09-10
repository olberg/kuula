use kuula_core::audio::AudioError;
use kuula_core::{Fault, GfxError};
use mlua::Error as LuaError;

use crate::meter::MeterError;
use crate::require::RequireError;

/// Turn an mlua error into a [`Fault`] with a stable code and the file and
/// line pulled out of Lua's `file.lua:line:` prefix. `chunk_name` is the
/// file to blame when no location can be found; `code` is the code to use
/// when the error is neither a syntax error nor a Kuula error carrying its
/// own code.
pub fn fault_from_lua(err: &LuaError, chunk_name: &str, code: &str) -> Fault {
    // The meter's fault is already complete; nothing to parse.
    if let Some(m) = err.downcast_ref::<MeterError>() {
        return m.0.clone();
    }
    let (message, traceback) = flatten(err);
    let (file, line, rest) = split_location(&message)
        .or_else(|| {
            traceback
                .as_deref()
                .and_then(find_location)
                .map(|(file, line)| (file, line, message.clone()))
        })
        .unwrap_or((chunk_name.to_string(), None, message.clone()));
    let code = if is_syntax(err) {
        Fault::COMPILE_ERROR
    } else if is_memory(err) {
        Fault::OUT_OF_MEMORY
    } else if let Some(g) = err.downcast_ref::<GfxError>() {
        g.code()
    } else if let Some(r) = err.downcast_ref::<RequireError>() {
        r.code()
    } else if let Some(a) = err.downcast_ref::<AudioError>() {
        a.code()
    } else if let Some(c) = err.downcast_ref::<kuula_core::codec::CodecError>() {
        c.code
    } else if let Some(s) = err.downcast_ref::<kuula_core::save::SaveError>() {
        s.code
    } else {
        code
    };
    Fault::new(code, &file, line, rest)
}

/// A syntax error anywhere in the chain, e.g. a module that failed to
/// compile inside `require`.
fn is_syntax(err: &LuaError) -> bool {
    match err {
        LuaError::SyntaxError { .. } => true,
        LuaError::CallbackError { cause, .. }
        | LuaError::BadArgument { cause, .. }
        | LuaError::WithContext { cause, .. } => is_syntax(cause),
        _ => false,
    }
}

/// The hard heap limit anywhere in the chain.
fn is_memory(err: &LuaError) -> bool {
    match err {
        LuaError::MemoryError(_) => true,
        LuaError::CallbackError { cause, .. }
        | LuaError::BadArgument { cause, .. }
        | LuaError::WithContext { cause, .. } => is_memory(cause),
        _ => false,
    }
}

/// The innermost message and, when available, the Lua traceback.
fn flatten(err: &LuaError) -> (String, Option<String>) {
    match err {
        LuaError::SyntaxError { message, .. } => (message.clone(), None),
        // mlua appends "\nstack traceback:..." to runtime errors.
        LuaError::RuntimeError(m) | LuaError::MemoryError(m) => {
            match m.split_once("\nstack traceback:") {
                Some((msg, tb)) => (msg.to_string(), Some(format!("stack traceback:{tb}"))),
                None => (m.clone(), None),
            }
        }
        LuaError::CallbackError { traceback, cause } => {
            let (m, inner_tb) = flatten(cause);
            (m, inner_tb.or_else(|| Some(traceback.clone())))
        }
        LuaError::BadArgument { to, pos, cause, .. } => {
            let (m, tb) = flatten(cause);
            let to = to.as_deref().unwrap_or("?");
            (format!("bad argument #{pos} to '{to}' ({m})"), tb)
        }
        LuaError::WithContext { context, cause } => {
            let (m, tb) = flatten(cause);
            (format!("{context}: {m}"), tb)
        }
        other => (other.to_string(), None),
    }
}

/// Parse a leading `file.lua:line: rest`.
fn split_location(message: &str) -> Option<(String, Option<u32>, String)> {
    let (file, line, end) = location_at(message, 0)?;
    let rest = message[end..].strip_prefix(':')?.trim_start();
    Some((file, Some(line), rest.to_string()))
}

/// Find the first `file.lua:line:` anywhere in a traceback.
fn find_location(text: &str) -> Option<(String, Option<u32>)> {
    let mut start = 0;
    while let Some(pos) = text[start..].find(".lua:") {
        let token_start = text[..start + pos]
            .rfind(|c: char| c.is_whitespace() || c == '\'' || c == '"')
            .map(|i| i + 1)
            .unwrap_or(0);
        if let Some((file, line, end)) = location_at(text, token_start) {
            if text[end..].starts_with(':') {
                return Some((file, Some(line)));
            }
        }
        start += pos + 5;
    }
    None
}

/// `<file>.lua:<digits>` starting exactly at `at`; returns the file, the
/// line and the index just past the digits.
fn location_at(text: &str, at: usize) -> Option<(String, u32, usize)> {
    let rest = &text[at..];
    let ext = rest.find(".lua:")?;
    let file = &rest[..ext + 4];
    if file.is_empty() || file.chars().any(|c| c.is_whitespace() || c == ':') {
        return None;
    }
    let after = &rest[ext + 5..];
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let line = digits.parse().ok()?;
    Some((file.to_string(), line, at + ext + 5 + digits.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_file_line_and_message() {
        assert_eq!(
            split_location("main.lua:12: attempt to call a nil value"),
            Some((
                "main.lua".into(),
                Some(12),
                "attempt to call a nil value".into()
            ))
        );
        assert_eq!(
            split_location("src/game/util.lua:3: boom"),
            Some(("src/game/util.lua".into(), Some(3), "boom".into()))
        );
        assert_eq!(split_location("no location"), None);
        assert_eq!(split_location("main.lua: no digits"), None);
        assert_eq!(split_location("main.lua:5 no colon"), None);
    }

    #[test]
    fn finds_location_in_a_traceback() {
        let tb = "stack traceback:\n\t[C]: in function 'pset'\n\tmain.lua:7: in function '_draw'\n\t[C]: in ?";
        assert_eq!(find_location(tb), Some(("main.lua".into(), Some(7))));
        let tb = "stack traceback:\n\t[C]: in function 'error'\n\tsrc/a.lua:2: in function <src/a.lua:1>\n\tmain.lua:3: in main chunk";
        assert_eq!(find_location(tb), Some(("src/a.lua".into(), Some(2))));
        assert_eq!(find_location("main.lua: no digits"), None);
    }

    #[test]
    fn syntax_errors_always_get_the_compile_code() {
        let e = LuaError::SyntaxError {
            message: "main.lua:2: unexpected symbol near '='".into(),
            incomplete_input: false,
        };
        let f = fault_from_lua(&e, "main.lua", Fault::RUNTIME_ERROR);
        assert_eq!(f.code, "compile_error");
        assert_eq!(f.line, Some(2));
        assert_eq!(f.message, "unexpected symbol near '='");
    }

    #[test]
    #[allow(clippy::arc_with_non_send_sync)]
    fn kuula_errors_keep_their_codes() {
        let e = LuaError::external(GfxError::NoSheet);
        let f = fault_from_lua(&e, "main.lua", Fault::RUNTIME_ERROR);
        assert_eq!(f.code, "no_sheet");
        assert_eq!(f.file, "main.lua");
        let e = LuaError::CallbackError {
            traceback: "stack traceback:\n\tmain.lua:4: in function '_draw'".into(),
            cause: std::sync::Arc::new(LuaError::external(RequireError::Cycle {
                name: "a".into(),
            })),
        };
        let f = fault_from_lua(&e, "main.lua", Fault::RUNTIME_ERROR);
        assert_eq!(f.code, "require_cycle");
        assert_eq!(f.line, Some(4));
    }
}
