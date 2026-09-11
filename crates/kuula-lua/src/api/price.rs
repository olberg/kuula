//! Typed references to the core's price list, rendered as the formulas
//! the reference shows. The numbers in a rendered formula are the
//! constants in `kuula_core::meter::price`; nothing here is a second
//! copy of a price, and nothing here runs when a binding charges.

use kuula_core::meter::price;

/// What a binding charges, on top of nothing: every binding's minimum
/// of one cycle is part of the formula.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Price {
    /// The one-cycle minimum only.
    One,
    /// `price::cls` over the clip area.
    Cls,
    /// `price::pixels` over the pixels touched after clipping.
    Pixels,
    /// `price::pixels` over the pixels touched or the circle's midpoint
    /// walk, whichever is more.
    Circle,
    /// `price::text`: characters plus pixels.
    Text,
    /// `price::map`: per cell requested plus pixels.
    Map,
    /// `price::bytes8` over the named quantity.
    Bytes8(&'static str),
    /// `price::bytes128` over the named quantity.
    Bytes128(&'static str),
    /// A buffer allocation: one cycle once the ledger accepted it, then
    /// `price::bytes128` of the buffer (`buf`, and each buffer `load`
    /// recreates).
    Alloc,
    /// A named asset load: one cycle, plus `price::bytes8` of the
    /// decoded bytes when the name was not live.
    Asset,
    /// `require`: `price::compile` over the source once, one cycle
    /// afterwards.
    Compile,
    /// `save`: the flat cost, then `price::bytes8` of the walked value
    /// and `price::bytes8` of the encoded text.
    Save,
    /// `load`: the flat cost, then `price::bytes8` of the stored text
    /// when the slot holds one.
    Load,
    /// `price::pattern`, the worst case of a Lua pattern match.
    Pattern,
    /// `Pattern` plus `price::bytes8` of the result.
    Gsub,
    /// `price::sort`.
    Sort,
    /// `price::coroutine`.
    Coroutine,
    /// `collectgarbage("collect")`: `price::bytes128` of the heap.
    Gc,
    /// A native routed through the runtime for its semantics, not its
    /// price: only the instruction hook charges.
    Native,
    /// Not a call.
    Value,
}

/// A linear price: `flat + sum(quantity / per)`. A flat part that is
/// made of several charges is computed from the price functions at
/// zero, so it follows them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Formula {
    pub flat: u64,
    pub terms: Vec<(&'static str, u64)>,
}

impl Formula {
    pub fn eval(&self, inputs: &[u64]) -> u64 {
        assert_eq!(inputs.len(), self.terms.len(), "one input per term");
        self.terms
            .iter()
            .zip(inputs)
            .fold(self.flat, |acc, ((_, per), x)| acc + x / per)
    }

    pub fn render(&self) -> String {
        let mut out = self.flat.to_string();
        for (unit, per) in &self.terms {
            out.push_str(&format!(" + {unit} / {per}"));
        }
        out
    }
}

impl Price {
    /// The linear formula, for the prices that are one.
    pub fn formula(self) -> Option<Formula> {
        let linear = |flat, terms: Vec<(&'static str, u64)>| Some(Formula { flat, terms });
        match self {
            Price::One => linear(1, vec![]),
            Price::Cls => linear(1, vec![("clip pixels", price::CLS_PIXELS_PER_CYCLE)]),
            Price::Pixels => linear(1, vec![("touched", price::PIXELS_PER_CYCLE)]),
            Price::Bytes8(what) => linear(1, vec![(what, price::BYTES8)]),
            Price::Bytes128(what) => linear(1, vec![(what, price::BYTES128)]),
            Price::Alloc => linear(1 + price::bytes128(0), vec![("bytes", price::BYTES128)]),
            Price::Gc => linear(1, vec![("heap bytes", price::BYTES128)]),
            Price::Save => linear(
                crate::codec::SAVE_FLAT_CYCLES + 2 * price::bytes8(0),
                vec![
                    ("value bytes", price::BYTES8),
                    ("encoded bytes", price::BYTES8),
                ],
            ),
            _ => None,
        }
    }

    /// What `load` charges for a slot holding `stored` bytes, or an
    /// empty one.
    pub fn load(stored: Option<u64>) -> u64 {
        match stored {
            Some(n) => crate::codec::SAVE_FLAT_CYCLES + price::bytes8(n),
            None => crate::codec::SAVE_FLAT_CYCLES,
        }
    }

    /// What a named asset load charges: `decoded` bytes on the first
    /// call, `None` when the name is already live.
    pub fn asset(decoded: Option<u64>) -> u64 {
        match decoded {
            Some(n) => 1 + price::bytes8(n),
            None => 1,
        }
    }

    /// The formula as the reference shows it.
    pub fn render(self) -> String {
        if let Some(f) = self.formula() {
            return f.render();
        }
        let px = price::PIXELS_PER_CYCLE;
        match self {
            Price::Circle => format!(
                "1 + max(touched, {} r) / {px}",
                price::CIRCLE_WORK_PER_RADIUS
            ),
            Price::Text => format!("characters + touched / {px}"),
            Price::Map => format!(
                "{} per cell requested + 1 + touched / {px}",
                price::MAP_CYCLES_PER_CELL
            ),
            Price::Asset => format!(
                "{} + decoded bytes / {}; {} when already live",
                Price::asset(Some(0)),
                price::BYTES8,
                Price::asset(None)
            ),
            Price::Compile => format!(
                "1 + source bytes / {} on first load, 1 afterwards",
                price::COMPILE_BYTES_PER_CYCLE
            ),
            Price::Load => format!(
                "{} + stored bytes / {}; {} when the slot is empty",
                Price::load(Some(0)),
                price::BYTES8,
                Price::load(None)
            ),
            Price::Pattern => format!(
                "pattern bytes x subject bytes ^ (quantifiers + 1 if unanchored) / {}",
                price::PATTERN_DIVISOR
            ),
            Price::Gsub => format!(
                "{}, then result bytes / {}",
                Price::Pattern.render(),
                price::BYTES8
            ),
            Price::Sort => "n log2 n".to_string(),
            Price::Coroutine => price::coroutine().to_string(),
            Price::Native => "instructions only".to_string(),
            Price::Value => String::new(),
            Price::One
            | Price::Cls
            | Price::Pixels
            | Price::Bytes8(_)
            | Price::Bytes128(_)
            | Price::Alloc
            | Price::Save
            | Price::Gc => unreachable!("linear prices render through their formula"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rendered formulas are built from the same constants the
    /// price functions use; the linear ones evaluate to the same
    /// cycles on sample inputs.
    #[test]
    fn linear_formulas_agree_with_the_price_list() {
        for n in [0, 1, 7, 63, 64, 1000, 640 * 480] {
            assert_eq!(Price::Cls.formula().unwrap().eval(&[n]), price::cls(n));
            assert_eq!(
                Price::Pixels.formula().unwrap().eval(&[n]),
                price::pixels(n)
            );
            assert_eq!(
                Price::Bytes8("b").formula().unwrap().eval(&[n]),
                price::bytes8(n)
            );
            assert_eq!(
                Price::Bytes128("b").formula().unwrap().eval(&[n]),
                price::bytes128(n)
            );
            assert_eq!(Price::Gc.formula().unwrap().eval(&[n]), price::bytes128(n));
        }
        assert_eq!(Price::One.formula().unwrap().eval(&[]), 1);
        assert_eq!(Price::Pixels.render(), "1 + touched / 3");
        assert_eq!(Price::Cls.render(), "1 + clip pixels / 64");
        assert_eq!(Price::Bytes8("bytes").render(), "1 + bytes / 8");
    }

    /// The composed prices: a flat charge and then a price function,
    /// as the bindings charge them, rendered with the sum.
    #[test]
    fn composed_formulas_carry_their_flat_charges() {
        assert_eq!(Price::Alloc.render(), "2 + bytes / 128");
        assert_eq!(
            Price::Alloc.formula().unwrap().eval(&[4096]),
            1 + price::bytes128(4096)
        );
        assert_eq!(
            Price::Asset.render(),
            "2 + decoded bytes / 8; 1 when already live"
        );
        assert_eq!(Price::asset(Some(128)), 1 + price::bytes8(128));
        assert_eq!(
            Price::Save.render(),
            "66 + value bytes / 8 + encoded bytes / 8"
        );
        assert_eq!(
            Price::Save.formula().unwrap().eval(&[80, 40]),
            64 + price::bytes8(80) + price::bytes8(40)
        );
        assert_eq!(
            Price::Load.render(),
            "65 + stored bytes / 8; 64 when the slot is empty"
        );
        assert_eq!(Price::load(Some(40)), 64 + price::bytes8(40));
    }

    /// The shaped prices name the same constants the functions use.
    #[test]
    fn shaped_formulas_carry_the_price_constants() {
        assert_eq!(Price::Circle.render(), "1 + max(touched, 6 r) / 3");
        assert_eq!(price::pixels(6 * 100), 1 + 600 / 3);
        assert_eq!(Price::Text.render(), "characters + touched / 3");
        assert_eq!(price::text(5, 30), 5 + 10);
        assert_eq!(price::text(0, 0), 1, "at least one character");
        assert_eq!(
            Price::Map.render(),
            "2 per cell requested + 1 + touched / 3"
        );
        assert_eq!(price::map(10, 30), 20 + 1 + 10);
        assert_eq!(
            Price::Compile.render(),
            "1 + source bytes / 4 on first load, 1 afterwards"
        );
        assert_eq!(price::compile(400), 101);
        assert!(Price::Pattern.render().ends_with("/ 256"));
        assert_eq!(Price::Coroutine.render(), price::coroutine().to_string());
        assert_eq!(Price::Sort.render(), "n log2 n");
        assert_eq!(price::sort(8), 1 + 8 * 4);
        assert_eq!(Price::Value.render(), "");
    }
}
