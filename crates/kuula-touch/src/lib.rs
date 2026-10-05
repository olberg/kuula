//! On-screen controls for a touch screen: where the picture and the
//! controls go for a window ([`Layout`]), which button each finger holds
//! ([`Pad`]), and drawing the console's frame ([`present`]) and the
//! controls over it ([`draw_controls`]). No windowing or platform code is
//! in here: the host gives it a pixel buffer ([`Canvas`]) and the touches.
//!
//! Pixels are four bytes, R, G, B, X, which is a window buffer's own
//! format. Nothing allocates per frame.

mod canvas;
mod draw;
mod layout;
mod pad;
mod present;

pub use canvas::Canvas;
pub use draw::{draw_controls, HELD_ALPHA, IDLE_ALPHA, OVER_PICTURE};
pub use layout::{
    Disc, Insets, Layout, Rect, BUTTON_DP, BUTTON_HIT, DPAD_CLAIM, DPAD_DEAD_ZONE, DPAD_DP, GAP_DP,
    KEY_GROW_DP, MARGIN_DP, MENU_GROW_DP, MENU_H_DP, MENU_W_DP, SHOULDER_H_DP, SHOULDER_W_DP,
    START_W_DP,
};
pub use pad::{Pad, MAX_FINGERS};
pub use present::present;
