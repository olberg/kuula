//! Integer window scale, 1x to 4x, counted in the system's primary
//! 640x480 mode. The window is the same size whatever the cart's mode:
//! a 320x240 cart is drawn at twice the scale, invisibly, so switching
//! carts never resizes the window and the scale setting means one thing.

use kuula_core::ScreenMode;

pub const MIN_SCALE: u32 = 1;
pub const MAX_SCALE: u32 = 4;
pub const DEFAULT_SCALE: u32 = 2;

/// The window's pixel size at `scale`, independent of the cart's mode.
pub fn window_size(scale: u32) -> (u32, u32) {
    let (w, h) = ScreenMode::High.size();
    (w * scale, h * scale)
}

/// Parse a `--scale` argument. Accepts 1 to 4 only.
pub fn parse(text: &str) -> Result<u32, String> {
    let n: u32 = text.trim().parse().map_err(|_| {
        format!("scale must be a whole number from {MIN_SCALE} to {MAX_SCALE}, got {text:?}")
    })?;
    if (MIN_SCALE..=MAX_SCALE).contains(&n) {
        Ok(n)
    } else {
        Err(format!(
            "scale must be from {MIN_SCALE} to {MAX_SCALE}, got {n}"
        ))
    }
}

/// Force a scale into range.
pub fn clamp(n: u32) -> u32 {
    n.clamp(MIN_SCALE, MAX_SCALE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_one_to_four() {
        for n in 1..=4 {
            assert_eq!(parse(&n.to_string()), Ok(n));
        }
        assert_eq!(parse(" 3 "), Ok(3));
    }

    #[test]
    fn rejects_out_of_range_and_non_numbers() {
        assert!(parse("0").is_err());
        assert!(parse("5").is_err());
        assert!(parse("-1").is_err());
        assert!(parse("two").is_err());
        assert!(parse("2.0").is_err());
        assert!(parse("").is_err());
    }

    #[test]
    fn window_size_is_the_primary_mode_times_scale() {
        assert_eq!(window_size(1), (640, 480));
        assert_eq!(window_size(3), (1920, 1440));
    }

    #[test]
    fn clamp_stays_in_range() {
        assert_eq!(clamp(0), 1);
        assert_eq!(clamp(9), 4);
        assert_eq!(clamp(3), 3);
    }
}
