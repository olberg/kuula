//! What a host writes to its log about a cart it was started on, and how
//! the desktop reads it back.
//!
//! A host with no channel of its own to a developer (the Android app,
//! reached over `adb`) says how a deployed cart fared in its log, one line:
//!
//! ```text
//! deploy: started hello.cart
//! deploy: faulted hello.cart: runtime_error main.lua:3: ...
//! deploy: not_run hello.cart: cart_read_error ...
//! ```
//!
//! and the desktop reads the log for it. A cart's own `print` lines go to
//! the same log, so they are written with [`cart_line`], which puts
//! `cart: ` before them and keeps each on one line: nothing a cart prints
//! reads as the host's word about it.

/// What every line about a deploy starts with.
pub const PREFIX: &str = "deploy: ";

/// What every line a cart printed starts with.
pub const CART_PREFIX: &str = "cart: ";

/// Frames a cart must run without a fault to count as started: half a
/// second, so a fault in its first updates and draws is still its answer.
pub const STARTED_AFTER: u32 = 30;

/// How a cart the host was started on fared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// It ran [`STARTED_AFTER`] frames.
    Started,
    /// It faulted, at load or in those frames; the fault as text.
    Faulted(String),
    /// It was never loaded, and why.
    NotRun(String),
}

/// `text` as it can stand in one line of a log: every control character,
/// a line break among them, becomes a space. A log splits a message at its
/// line breaks, and the second line of a cart's text would then stand
/// there with nothing before it; anything a host logs that a cart had a
/// hand in (a fault's message, a save's error) goes through this.
pub fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// The line a host logs for `name`.
pub fn line(name: &str, outcome: &Outcome) -> String {
    let name = one_line(name);
    match outcome {
        Outcome::Started => format!("{PREFIX}started {name}"),
        Outcome::Faulted(why) => format!("{PREFIX}faulted {name}: {}", one_line(why)),
        Outcome::NotRun(why) => format!("{PREFIX}not_run {name}: {}", one_line(why)),
    }
}

/// The line a host logs for something a cart printed.
pub fn cart_line(text: &str) -> String {
    format!("{CART_PREFIX}{}", one_line(text))
}

/// Read a log message for the line [`line`] writes: the cart's name and
/// how it fared. The message may begin with what a logger puts first, one
/// word and `": "` (a module's path); a line a cart printed has
/// [`CART_PREFIX`] there and is nobody's word but the cart's.
pub fn parse(message: &str) -> Option<(&str, Outcome)> {
    let at = message.find(PREFIX)?;
    let head = &message[..at];
    let logger_only = head.is_empty()
        || head
            .strip_suffix(": ")
            .is_some_and(|word| !word.is_empty() && !word.contains([' ', '\t']));
    // With no logger's word before it, a cart's line has `cart` there.
    if !logger_only || head.contains(CART_PREFIX) {
        return None;
    }
    let rest = &message[at + PREFIX.len()..];
    let (state, rest) = rest.split_once(' ')?;
    let (name, why) = match rest.split_once(": ") {
        Some((name, why)) => (name, why),
        None => (rest, ""),
    };
    if name.is_empty() || name.contains(' ') {
        return None;
    }
    let outcome = match state {
        "started" => Outcome::Started,
        "faulted" => Outcome::Faulted(why.to_string()),
        "not_run" => Outcome::NotRun(why.to_string()),
        _ => return None,
    };
    Some((name, outcome))
}

/// Follows a cart the host was started on until it can say how it fared.
#[derive(Debug)]
pub struct Watch {
    name: String,
    frames: u32,
}

impl Watch {
    pub fn new(name: impl Into<String>) -> Watch {
        Watch {
            name: name.into(),
            frames: 0,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// After a step of the console, with the cart's fault when it has one.
    /// `Some` once there is an answer; the watch is done with then.
    pub fn after_step(&mut self, fault: Option<&str>) -> Option<Outcome> {
        if let Some(fault) = fault {
            return Some(Outcome::Faulted(fault.to_string()));
        }
        self.frames += 1;
        (self.frames >= STARTED_AFTER).then_some(Outcome::Started)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_read_back_as_it_was_written() {
        let cases = [
            Outcome::Started,
            Outcome::Faulted("runtime_error main.lua:3: attempt to call a nil value".into()),
            Outcome::NotRun("cart_read_error hello.cart: no such cart".into()),
            Outcome::Faulted(String::new()),
        ];
        for outcome in cases {
            let written = line("hello.cart", &outcome);
            assert_eq!(parse(&written), Some(("hello.cart", outcome.clone())));
            // As a logger writes it, a module's path first.
            let logged = format!("kuula_android::app: {written}");
            assert_eq!(parse(&logged), Some(("hello.cart", outcome)));
        }
    }

    #[test]
    fn a_line_stays_one_line_whatever_the_fault_says() {
        let written = line(
            "a.cart",
            &Outcome::Faulted("one\ntwo\r\n\u{1b}[31mthree".into()),
        );
        assert!(!written.contains(['\n', '\r', '\u{1b}']), "{written:?}");
        assert_eq!(
            parse(&written),
            Some(("a.cart", Outcome::Faulted("one two   [31mthree".into())))
        );
    }

    #[test]
    fn what_a_cart_prints_is_never_the_hosts_word() {
        for printed in [
            "deploy: started a.cart",
            "x\ndeploy: started a.cart",
            "kuula_android::app: deploy: faulted a.cart: no",
        ] {
            let written = cart_line(printed);
            assert!(written.starts_with(CART_PREFIX));
            assert!(!written.contains('\n'));
            assert_eq!(parse(&written), None, "{written:?}");
            assert_eq!(parse(&format!("kuula_android::app: {written}")), None);
        }
    }

    #[test]
    fn other_lines_and_broken_ones_are_not_read() {
        for message in [
            "",
            "kuula_android::app: in front",
            "deploy: ",
            "deploy: started",
            "deploy: started ",
            "deploy: finished a.cart",
            "deploy: started two words",
            "some words here: deploy: started a.cart",
        ] {
            assert_eq!(parse(message), None, "{message:?}");
        }
    }

    #[test]
    fn a_watch_answers_once_the_cart_has_run_or_faulted() {
        let mut watch = Watch::new("a.cart");
        for _ in 1..STARTED_AFTER {
            assert_eq!(watch.after_step(None), None);
        }
        assert_eq!(watch.after_step(None), Some(Outcome::Started));

        let mut watch = Watch::new("b.cart");
        assert_eq!(watch.after_step(None), None);
        assert_eq!(
            watch.after_step(Some("runtime_error main.lua:1: boom")),
            Some(Outcome::Faulted("runtime_error main.lua:1: boom".into()))
        );
        assert_eq!(watch.name(), "b.cart");
    }
}
