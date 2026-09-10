//! Input scripts: a JSON list of runs, each holding some buttons for some
//! frames, expanded into one `FrameInput` per frame.
//!
//! ```json
//! [
//!   {"frames": 30, "buttons": ["right"]},
//!   {"frames": 1,  "buttons": ["a", "right"]},
//!   {"frames": 10}
//! ]
//! ```

use std::fmt;

use kuula_core::input::{BTN_A, BTN_B, BTN_DOWN, BTN_LEFT, BTN_MENU, BTN_RIGHT, BTN_UP};
use kuula_core::FrameInput;
use serde::Deserialize;

/// Most frames one script may expand to.
pub const MAX_SCRIPT_FRAMES: u64 = 1_000_000;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Run {
    #[serde(default = "one")]
    frames: u64,
    #[serde(default)]
    buttons: Vec<String>,
}

fn one() -> u64 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptError {
    pub message: String,
}

impl fmt::Display for ScriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ScriptError {}

/// The expanded script.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InputScript {
    pub frames: Vec<FrameInput>,
}

/// The bit for a button name, case-insensitive.
pub fn button_bit(name: &str) -> Option<u8> {
    match name.to_ascii_lowercase().as_str() {
        "up" => Some(BTN_UP),
        "down" => Some(BTN_DOWN),
        "left" => Some(BTN_LEFT),
        "right" => Some(BTN_RIGHT),
        "a" => Some(BTN_A),
        "menu" => Some(BTN_MENU),
        "b" => Some(BTN_B),
        _ => None,
    }
}

impl InputScript {
    pub fn parse(text: &str) -> Result<InputScript, ScriptError> {
        let runs: Vec<Run> = serde_json::from_str(text).map_err(|e| ScriptError {
            message: format!("input script: {e}"),
        })?;
        let mut frames = Vec::new();
        let mut total: u64 = 0;
        for (i, run) in runs.iter().enumerate() {
            let mut bits = 0u8;
            for b in &run.buttons {
                bits |= button_bit(b).ok_or_else(|| ScriptError {
                    message: format!(
                        "input script run {i}: unknown button {b:?} (up, down, left, right, a, b)"
                    ),
                })?;
            }
            total = total.saturating_add(run.frames);
            if total > MAX_SCRIPT_FRAMES {
                return Err(ScriptError {
                    message: format!(
                        "input script expands to more than {MAX_SCRIPT_FRAMES} frames"
                    ),
                });
            }
            frames.extend(std::iter::repeat_n(
                FrameInput::new(bits),
                run.frames as usize,
            ));
        }
        Ok(InputScript { frames })
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_expand_in_order() {
        let s = InputScript::parse(
            r#"[{"frames": 2, "buttons": ["right"]}, {"buttons": ["A", "b"]}, {"frames": 1}]"#,
        )
        .unwrap();
        assert_eq!(
            s.frames,
            [
                FrameInput::new(BTN_RIGHT),
                FrameInput::new(BTN_RIGHT),
                FrameInput::new(BTN_A | BTN_B),
                FrameInput::NONE
            ]
        );
        assert!(InputScript::parse("[]").unwrap().is_empty());
    }

    #[test]
    fn bad_scripts_are_errors() {
        let e = InputScript::parse(r#"[{"buttons": ["start"]}]"#).unwrap_err();
        assert!(e.message.contains("start"), "{e}");
        assert!(InputScript::parse("not json").is_err());
        assert!(
            InputScript::parse(r#"[{"frame": 1}]"#).is_err(),
            "unknown key"
        );
        let e = InputScript::parse(r#"[{"frames": 2000000}]"#).unwrap_err();
        assert!(e.message.contains("more than"), "{e}");
        let e = InputScript::parse(r#"[{"frames": 600000}, {"frames": 600000}]"#).unwrap_err();
        assert!(e.message.contains("more than"), "{e}");
    }
}
