use std::fmt;

/// A cart-ending error. Errors are data: a stable code, a message and the
/// file and line they came from, so that a host, a log or an agent can act
/// on them without parsing prose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fault {
    /// Stable machine-readable code, e.g. `compile_error`, `runtime_error`.
    pub code: String,
    /// Human-readable message with any `file:line:` prefix stripped.
    pub message: String,
    /// Source file the error was attributed to, e.g. `main.lua`.
    pub file: String,
    /// One-based line, when known.
    pub line: Option<u32>,
}

impl Fault {
    pub const COMPILE_ERROR: &'static str = "compile_error";
    pub const RUNTIME_ERROR: &'static str = "runtime_error";
    pub const CART_READ_ERROR: &'static str = "cart_read_error";
    /// The cycle budget of a callback was spent.
    pub const BUDGET_EXCEEDED: &'static str = "budget_exceeded";
    /// The guest heap passed its cap, soft or hard.
    pub const OUT_OF_MEMORY: &'static str = "out_of_memory";
    /// A wall-clock backstop fired outside the deterministic meter;
    /// reported by a broker, never by the core.
    pub const WATCHDOG_TIMEOUT: &'static str = "watchdog_timeout";
    /// The worker's restricted token could not be set up, so the run
    /// was refused rather than started unsandboxed (fail closed);
    /// reported by a broker, never by the core.
    pub const SANDBOX_UNAVAILABLE: &'static str = "sandbox_unavailable";

    pub fn new(code: &str, file: &str, line: Option<u32>, message: impl Into<String>) -> Fault {
        Fault {
            code: code.to_string(),
            message: message.into(),
            file: file.to_string(),
            line,
        }
    }

    /// `file:line` or just `file` when the line is unknown.
    pub fn location(&self) -> String {
        match self.line {
            Some(line) => format!("{}:{}", self.file, line),
            None => self.file.clone(),
        }
    }
}

/// The one line hosts print for a fault: `code file:line: message`.
impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}: {}", self.code, self.location(), self.message)
    }
}

impl std::error::Error for Fault {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_is_code_location_message() {
        let f = Fault::new(Fault::RUNTIME_ERROR, "main.lua", Some(12), "boom");
        assert_eq!(f.to_string(), "runtime_error main.lua:12: boom");
        let f = Fault::new(Fault::CART_READ_ERROR, "main.lua", None, "missing");
        assert_eq!(f.to_string(), "cart_read_error main.lua: missing");
    }
}
