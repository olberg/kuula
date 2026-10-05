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

use kuula_core::input::{BTN_MENU, BUTTON_NAMES};
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

/// The bit for a button name, case-insensitive: a cart's buttons by the
/// names in [`BUTTON_NAMES`], and `menu`.
pub fn button_bit(name: &str) -> Option<u16> {
    let name = name.to_ascii_lowercase();
    if name == "menu" {
        return Some(BTN_MENU);
    }
    BUTTON_NAMES.iter().position(|n| *n == name).map(|n| 1 << n)
}

impl InputScript {
    pub fn parse(text: &str) -> Result<InputScript, ScriptError> {
        let runs: Vec<Run> = serde_json::from_str(text).map_err(|e| ScriptError {
            message: format!("input script: {e}"),
        })?;
        InputScript::expand(runs)
    }

    /// [`InputScript::parse`] for a script already parsed as JSON.
    pub fn from_value(value: serde_json::Value) -> Result<InputScript, ScriptError> {
        let runs: Vec<Run> = serde_json::from_value(value).map_err(|e| ScriptError {
            message: format!("input script: {e}"),
        })?;
        InputScript::expand(runs)
    }

    fn expand(runs: Vec<Run>) -> Result<InputScript, ScriptError> {
        let mut frames = Vec::new();
        let mut total: u64 = 0;
        for (i, run) in runs.iter().enumerate() {
            let mut bits = 0u16;
            for b in &run.buttons {
                bits |= button_bit(b).ok_or_else(|| ScriptError {
                    message: format!(
                        "input script run {i}: unknown button {b:?} ({})",
                        BUTTON_NAMES.join(", ")
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
    use kuula_core::input::{
        BTN_A, BTN_B, BTN_L1, BTN_R2, BTN_RIGHT, BTN_SELECT, BTN_START, BTN_X, BTN_Y, CART_BUTTONS,
    };

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
    fn every_button_has_a_name() {
        let s = InputScript::parse(
            r#"[{"buttons": ["x", "Y"]}, {"buttons": ["l1", "r2"]}, {"buttons": ["start", "select"]}]"#,
        )
        .unwrap();
        assert_eq!(
            s.frames,
            [
                FrameInput::new(BTN_X | BTN_Y),
                FrameInput::new(BTN_L1 | BTN_R2),
                FrameInput::new(BTN_START | BTN_SELECT),
            ]
        );
        let all = BUTTON_NAMES
            .iter()
            .fold(0, |bits, name| bits | button_bit(name).unwrap());
        assert_eq!(all, CART_BUTTONS);
        assert_eq!(button_bit("Menu"), Some(BTN_MENU));
    }

    #[test]
    fn bad_scripts_are_errors() {
        let e = InputScript::parse(r#"[{"buttons": ["turbo"]}]"#).unwrap_err();
        assert!(e.message.contains("turbo"), "{e}");
        assert!(e.message.contains("l1, r1, l2, r2, start, select"), "{e}");
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
