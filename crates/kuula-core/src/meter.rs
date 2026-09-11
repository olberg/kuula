//! The cycle meter: a fixed budget per frame,
//! a fixed price per unit of work, never host time. The core owns the
//! accounting and the price list; a guest charges through it from every
//! binding and from its instruction hook, and reads the profile out at
//! the end of a step.

use crate::fault::Fault;

/// Cycles one frame may spend: 2^24 per second at 60 Hz, kept until the
/// device is profiled.
pub const FRAME_BUDGET: u64 = 279_620;

/// Cycles the main chunk plus `_init` may spend together: one second.
pub const INIT_BUDGET: u64 = 60 * FRAME_BUDGET;

/// Instructions between count-hook fires.
pub const HOOK_INTERVAL: u32 = 1000;

/// Cycles per Lua VM instruction.
pub const CYCLES_PER_INSTRUCTION: u64 = 2;

/// What one hook fire charges.
pub const HOOK_CHARGE: u64 = HOOK_INTERVAL as u64 * CYCLES_PER_INSTRUCTION;

/// Where cycles went. The order is the order of the profile arrays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// VM instructions, counted by the hook.
    Lua,
    /// Shapes, sprites and maps, by pixels touched.
    Draw,
    /// `print` in its drawing form.
    Text,
    /// Buffer allocation, fills and copies.
    Buf,
    /// Sheet and map decoding, `require` compilation.
    Asset,
    /// Priced native string and table functions.
    Str,
    /// Everything else: state changes, `btn`, `stat`, logging.
    Api,
}

pub const CATEGORY_COUNT: usize = 7;

impl Category {
    pub const ALL: [Category; CATEGORY_COUNT] = [
        Category::Lua,
        Category::Draw,
        Category::Text,
        Category::Buf,
        Category::Asset,
        Category::Str,
        Category::Api,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Category::Lua => "lua",
            Category::Draw => "draw",
            Category::Text => "text",
            Category::Buf => "buf",
            Category::Asset => "asset",
            Category::Str => "string",
            Category::Api => "api",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// The cycles one frame spent, by category, with the budget it had and
/// the guest heap after the step. Part of every frame's output.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct FrameProfile {
    pub cycles: [u64; CATEGORY_COUNT],
    pub budget: u64,
    /// Bytes the guest's own heap held after the step, 0 for a guest
    /// without one.
    pub lua_mem: u64,
}

impl FrameProfile {
    pub fn total(&self) -> u64 {
        self.cycles.iter().fold(0u64, |a, &b| a.saturating_add(b))
    }

    pub fn get(&self, c: Category) -> u64 {
        self.cycles[c.index()]
    }
}

/// The budget was spent. Carries what a fault message needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetExceeded {
    pub callback: &'static str,
    pub used: u64,
    pub budget: u64,
}

impl BudgetExceeded {
    pub fn message(&self) -> String {
        format!(
            "{} used {} of {} cycles",
            self.callback, self.used, self.budget
        )
    }
}

impl std::fmt::Display for BudgetExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", Fault::BUDGET_EXCEEDED, self.message())
    }
}

impl std::error::Error for BudgetExceeded {}

/// Per-frame accounting. `charge` is the only way cycles are spent; it
/// fails exactly once, at the charge that crosses the budget, and the
/// meter is dead from then on.
#[derive(Debug, Clone)]
pub struct Meter {
    used: [u64; CATEGORY_COUNT],
    budget: u64,
    callback: &'static str,
    dead: Option<Fault>,
}

impl Default for Meter {
    fn default() -> Meter {
        Meter::new()
    }
}

impl Meter {
    pub fn new() -> Meter {
        Meter {
            used: [0; CATEGORY_COUNT],
            budget: FRAME_BUDGET,
            callback: "",
            dead: None,
        }
    }

    /// Start a frame: zero the counters and set its budget.
    pub fn begin_frame(&mut self, budget: u64) {
        self.used = [0; CATEGORY_COUNT];
        self.budget = budget;
    }

    /// Name the entry point being run, for the fault message.
    pub fn set_callback(&mut self, name: &'static str) {
        self.callback = name;
    }

    pub fn callback(&self) -> &'static str {
        self.callback
    }

    pub fn budget(&self) -> u64 {
        self.budget
    }

    pub fn total(&self) -> u64 {
        self.used.iter().fold(0u64, |a, &b| a.saturating_add(b))
    }

    /// Spend `n` cycles (at least 1) in `cat`. The error is returned
    /// once; afterwards the meter is dead and callers must check
    /// [`Meter::fault`] before doing work.
    pub fn charge(&mut self, cat: Category, n: u64) -> Result<(), BudgetExceeded> {
        let n = n.max(1);
        let slot = &mut self.used[cat.index()];
        *slot = slot.saturating_add(n);
        let total = self.total();
        if total > self.budget && self.dead.is_none() {
            return Err(BudgetExceeded {
                callback: self.callback,
                used: total,
                budget: self.budget,
            });
        }
        Ok(())
    }

    /// Whether a terminal fault has been recorded.
    pub fn is_dead(&self) -> bool {
        self.dead.is_some()
    }

    /// Record the terminal fault. The first one wins.
    pub fn kill(&mut self, fault: Fault) {
        if self.dead.is_none() {
            self.dead = Some(fault);
        }
    }

    pub fn fault(&self) -> Option<&Fault> {
        self.dead.as_ref()
    }

    pub fn profile(&self, lua_mem: u64) -> FrameProfile {
        FrameProfile {
            cycles: self.used,
            budget: self.budget,
            lua_mem,
        }
    }
}

// ----- the price list --------------------------------------------------

/// Prices in cycles for work measured in pixels, bytes or elements. Every
/// binding also pays the minimum of one cycle through `charge`.
pub mod price {
    /// `cls` pixels per cycle: the clip area is cheap because it is
    /// one fill.
    pub const CLS_PIXELS_PER_CYCLE: u64 = 64;
    /// Pixels per cycle for shapes, text and blits.
    pub const PIXELS_PER_CYCLE: u64 = 3;
    /// `map`: cycles per cell requested, on top of the pixels.
    pub const MAP_CYCLES_PER_CELL: u64 = 2;
    /// Bytes per cycle for log lines and other byte-sized API traffic.
    pub const BYTES8: u64 = 8;
    /// Bytes per cycle for buffer memory moved or allocated.
    pub const BYTES128: u64 = 128;
    /// Source bytes per cycle compiled by `require`.
    pub const COMPILE_BYTES_PER_CYCLE: u64 = 4;
    /// Divisor of the pattern-matching work estimate.
    pub const PATTERN_DIVISOR: u64 = 256;
    /// Cap on the exponent of the pattern-matching work estimate.
    pub const PATTERN_MAX_EXPONENT: u32 = 8;
    /// Pixels' worth of work per unit of radius a circle costs before
    /// clipping (`raster::circle_work`).
    pub const CIRCLE_WORK_PER_RADIUS: u64 = 6;

    /// `cls`: the clip area is cheap because it is one fill.
    pub fn cls(pixels: u64) -> u64 {
        1 + pixels / CLS_PIXELS_PER_CYCLE
    }

    /// Shapes and blits: pixels touched after clipping.
    pub fn pixels(touched: u64) -> u64 {
        1 + touched / PIXELS_PER_CYCLE
    }

    /// `map`: per cell requested on top of the pixels.
    pub fn map(cells: u64, touched: u64) -> u64 {
        cells
            .saturating_mul(MAP_CYCLES_PER_CELL)
            .saturating_add(pixels(touched))
    }

    /// `print` drawing: per character plus the pixels.
    pub fn text(chars: u64, touched: u64) -> u64 {
        chars.max(1).saturating_add(touched / PIXELS_PER_CYCLE)
    }

    /// Log lines and other byte-sized API traffic.
    pub fn bytes8(bytes: u64) -> u64 {
        1 + bytes / BYTES8
    }

    /// Buffer memory moved or allocated.
    pub fn bytes128(bytes: u64) -> u64 {
        1 + bytes / BYTES128
    }

    /// Source compiled by `require`.
    pub fn compile(bytes: u64) -> u64 {
        1 + bytes / COMPILE_BYTES_PER_CYCLE
    }

    /// Pattern matching, priced by its worst case: each backtracking
    /// quantifier (`*`, `+`, `-`, `?`) can rescan the subject, and an
    /// unanchored pattern is tried from every position. The work is
    /// `pattern * subject ^ (quantifiers + 1 if unanchored)`, divided by
    /// 256 so ordinary matching on short strings stays cheap while a
    /// backtracking bomb on a long subject faults before it runs.
    pub fn pattern(subject: u64, pattern: u64, quantifiers: u32, anchored: bool) -> u64 {
        let exp = quantifiers
            .saturating_add(if anchored { 0 } else { 1 })
            .min(PATTERN_MAX_EXPONENT);
        let work = (pattern.max(1) as f64) * (subject.max(1) as f64).powi(exp as i32)
            / PATTERN_DIVISOR as f64;
        if work >= u64::MAX as f64 {
            u64::MAX
        } else {
            1 + work as u64
        }
    }

    /// Backtracking quantifiers in a Lua pattern and whether it is
    /// anchored, for [`pattern`]. A `-` inside a set (`[a-z]`) is a
    /// range, not a quantifier, so set bodies are skipped.
    pub fn pattern_shape(p: &[u8]) -> (u32, bool) {
        let anchored = p.first() == Some(&b'^');
        let mut quantifiers = 0;
        let mut i = 0;
        while i < p.len() {
            match p[i] {
                b'%' => i += 1,
                b'[' => {
                    // `[]...]` and `[^]...]` start with a literal `]`.
                    i += 1;
                    if p.get(i) == Some(&b'^') {
                        i += 1;
                    }
                    if p.get(i) == Some(&b']') {
                        i += 1;
                    }
                    while i < p.len() && p[i] != b']' {
                        if p[i] == b'%' {
                            i += 1;
                        }
                        i += 1;
                    }
                }
                b'*' | b'+' | b'-' | b'?' => quantifiers += 1,
                _ => {}
            }
            i += 1;
        }
        (quantifiers, anchored)
    }

    /// `table.sort`: n log2 n comparisons.
    pub fn sort(n: u64) -> u64 {
        let log = if n <= 1 {
            0
        } else {
            64 - n.leading_zeros() as u64
        };
        1 + n.saturating_mul(log)
    }

    /// A new coroutine starts with a reset hook count, so it is charged
    /// the interval it could hide.
    pub fn coroutine() -> u64 {
        super::HOOK_CHARGE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charge_adds_and_trips_once() {
        let mut m = Meter::new();
        m.begin_frame(100);
        m.set_callback("_update");
        assert!(m.charge(Category::Draw, 0).is_ok(), "minimum of one");
        assert_eq!(m.total(), 1);
        assert!(
            m.charge(Category::Lua, 99).is_ok(),
            "exactly the budget is fine"
        );
        let err = m.charge(Category::Api, 1).unwrap_err();
        assert_eq!(err.callback, "_update");
        assert_eq!((err.used, err.budget), (101, 100));
        assert_eq!(err.message(), "_update used 101 of 100 cycles");
        assert!(!m.is_dead(), "the caller records the fault");
        m.kill(Fault::new(
            Fault::BUDGET_EXCEEDED,
            "main.lua",
            None,
            err.message(),
        ));
        assert!(
            m.charge(Category::Api, 1).is_ok(),
            "dead meters count silently"
        );
        assert!(m.is_dead());
        m.kill(Fault::new("later", "x", None, ""));
        assert_eq!(m.fault().unwrap().code, "budget_exceeded", "first wins");
        let p = m.profile(7);
        assert_eq!(p.get(Category::Lua), 99);
        assert_eq!(p.total(), 102);
        assert_eq!(p.lua_mem, 7);
        assert_eq!(p.budget, 100);
    }

    #[test]
    fn begin_frame_resets_the_counters_but_not_death() {
        let mut m = Meter::new();
        m.charge(Category::Lua, 5).unwrap();
        m.kill(Fault::new("x", "f", None, ""));
        m.begin_frame(FRAME_BUDGET);
        assert_eq!(m.total(), 0);
        assert!(m.is_dead());
        assert_eq!(m.budget(), FRAME_BUDGET);
    }

    #[test]
    fn prices_are_monotonic_and_never_free() {
        assert_eq!(price::cls(0), 1);
        assert_eq!(price::cls(640 * 480), 1 + 4800);
        assert_eq!(price::pixels(0), 1);
        assert_eq!(price::pixels(300), 101);
        assert_eq!(price::map(10, 30), 20 + 11);
        assert_eq!(price::text(0, 0), 1);
        assert_eq!(price::text(5, 60), 25);
        assert_eq!(price::bytes128(u64::MAX), 1 + u64::MAX / 128);
        // Plain search on a short string is a few cycles; a backtracking
        // bomb on a long one is more than any budget.
        assert_eq!(price::pattern(10, 3, 0, false), 1);
        assert_eq!(price::pattern(100, 3, 1, false), 1 + 3 * 100 * 100 / 256);
        assert_eq!(price::pattern(100, 3, 1, true), 1 + 300 / 256);
        assert!(price::pattern(100_000, 9, 4, false) > INIT_BUDGET);
        assert_eq!(price::pattern(u64::MAX, u64::MAX, 8, false), u64::MAX);
        assert_eq!(price::pattern_shape(b".-.-.-.-b"), (4, false));
        assert_eq!(price::pattern_shape(b"^%d+%.%d+$"), (2, true));
        assert_eq!(price::pattern_shape(b"a%+b%%-c"), (1, false));
        assert_eq!(price::pattern_shape(b""), (0, false));
        // A range dash inside a set is not a quantifier.
        assert_eq!(price::pattern_shape(b"[a-z]+"), (1, false));
        assert_eq!(price::pattern_shape(b"^[^-]*[]-]?$"), (2, true));
        assert_eq!(price::pattern_shape(b"[%]-]+"), (1, false));
        assert_eq!(price::pattern_shape(b"[a-"), (0, false));
        assert_eq!(price::sort(0), 1);
        assert_eq!(price::sort(1), 1);
        assert_eq!(price::sort(1024), 1 + 1024 * 11);
        assert_eq!(price::coroutine(), 2000);
        assert!(
            price::pattern(u64::MAX, u64::MAX, 0, false) > 0,
            "no overflow panic"
        );
    }

    #[test]
    fn category_names_follow_the_order() {
        let names: Vec<_> = Category::ALL.iter().map(|c| c.name()).collect();
        assert_eq!(
            names,
            ["lua", "draw", "text", "buf", "asset", "string", "api"]
        );
        for (i, c) in Category::ALL.iter().enumerate() {
            assert_eq!(c.index(), i);
        }
    }
}
