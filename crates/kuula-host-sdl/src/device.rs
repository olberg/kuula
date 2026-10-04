//! What the host asks of SDL on a handheld. The profile follows from the
//! video driver SDL picked, so one binary runs on both: on a desktop
//! nothing changes, on the Miyoo Mini (Plus) the host adapts to the
//! fork of SDL2 that ships with its ports (steward-fu's, driver name
//! `Mini`).
//!
//! That fork has no software or GL path worth using. Its renderer keeps
//! the pointer of the last `SDL_UpdateTexture`, and `SDL_RenderCopy`
//! hands it to the SigmaStar GFX blitter, which scales and rotates it
//! into the 640x480 framebuffer. It knows two texture formats (RGB565
//! and ARGB8888), computes the destination from the window size
//! (integer scale of the window into the framebuffer, so a window
//! larger than 640x480 draws nothing), and is only chosen when the
//! caller asks for an accelerated renderer: the software renderer is
//! ahead of it in the list and shows a black screen.

use sdl2::pixels::PixelFormatEnum;

/// Name the fork's video driver reports through `SDL_GetCurrentVideoDriver`.
pub const MINI_VIDEO_DRIVER: &str = "Mini";

/// Frames Menu is held to leave the shell: the handheld has no window to
/// close. 90 frames is a second and a half.
pub const HOLD_TO_EXIT_FRAMES: u32 = 90;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Desktop,
    MiyooMini,
}

impl Profile {
    pub fn for_video_driver(name: &str) -> Profile {
        if name.eq_ignore_ascii_case(MINI_VIDEO_DRIVER) {
            Profile::MiyooMini
        } else {
            Profile::Desktop
        }
    }

    /// The streaming texture's pixel format. The Mini renderer offers
    /// RGB565 and ARGB8888 only; anything else would be converted by SDL
    /// on every frame.
    pub fn pixel_format(self) -> PixelFormatEnum {
        match self {
            Profile::Desktop => PixelFormatEnum::RGB24,
            Profile::MiyooMini => PixelFormatEnum::ARGB8888,
        }
    }

    /// Whether to ask for an accelerated renderer. Elsewhere SDL's own
    /// choice stands.
    pub fn accelerated_renderer(self) -> bool {
        self == Profile::MiyooMini
    }

    /// The largest window scale. The panel is the primary 640x480 mode,
    /// and the Mini driver draws nothing into a larger window.
    pub fn max_scale(self) -> u32 {
        match self {
            Profile::Desktop => crate::scale::MAX_SCALE,
            Profile::MiyooMini => 1,
        }
    }

    /// Whether holding Menu leaves the host.
    pub fn hold_menu_to_exit(self) -> bool {
        self == Profile::MiyooMini
    }

    /// Sample frames the audio device is asked to take at a time. The Mini
    /// driver sends a buffer and then sleeps `frames * 1000 / rate - 10`
    /// milliseconds, on a kernel whose timer ticks every 10 ms. With 441
    /// frames that is 10 ms of audio a tick, the pace of playback. With the
    /// usual 1024 it is 23.2 ms every 20, which outruns playback for two
    /// seconds, until the driver has queued all the 350 ms it can, and
    /// leaves the sound that far behind the picture. Fewer than 441 make
    /// the sleep negative.
    pub fn audio_buffer(self) -> u16 {
        match self {
            Profile::Desktop => 1024,
            Profile::MiyooMini => 441,
        }
    }
}

/// Counts consecutive frames a button is held.
#[derive(Debug, Default, Clone, Copy)]
pub struct HoldToExit {
    held: u32,
}

impl HoldToExit {
    /// Feed one frame; true once the button has been held for
    /// [`HOLD_TO_EXIT_FRAMES`] frames in a row.
    pub fn tick(&mut self, down: bool) -> bool {
        if down {
            self.held = self.held.saturating_add(1);
        } else {
            self.held = 0;
        }
        self.held >= HOLD_TO_EXIT_FRAMES
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_mini_driver_selects_the_handheld_profile() {
        assert_eq!(Profile::for_video_driver("Mini"), Profile::MiyooMini);
        assert_eq!(Profile::for_video_driver("mini"), Profile::MiyooMini);
        for name in ["windows", "x11", "wayland", "cocoa", "dummy", "KMSDRM", ""] {
            assert_eq!(Profile::for_video_driver(name), Profile::Desktop, "{name}");
        }
    }

    #[test]
    fn the_desktop_profile_keeps_the_desktop_behaviour() {
        let p = Profile::Desktop;
        assert_eq!(p.pixel_format(), PixelFormatEnum::RGB24);
        assert!(!p.accelerated_renderer());
        assert_eq!(p.max_scale(), 4);
        assert!(!p.hold_menu_to_exit());
        assert_eq!(p.audio_buffer(), 1024);
    }

    #[test]
    fn the_handheld_profile_fits_the_panel() {
        let p = Profile::MiyooMini;
        assert_eq!(p.pixel_format(), PixelFormatEnum::ARGB8888);
        assert!(p.accelerated_renderer());
        assert_eq!(crate::scale::window_size(p.max_scale()), (640, 480));
        assert!(p.hold_menu_to_exit());
        // Ten milliseconds, a tick of the device's timer: the Mini driver's
        // sleep after it is zero, and never negative.
        let frames = p.audio_buffer() as u32;
        assert_eq!(frames * 1000 / kuula_core::audio::SAMPLE_RATE, 10);
    }

    #[test]
    fn menu_must_be_held_unbroken_to_exit() {
        let mut hold = HoldToExit::default();
        for _ in 0..HOLD_TO_EXIT_FRAMES - 1 {
            assert!(!hold.tick(true));
        }
        assert!(!hold.tick(false), "releasing restarts the count");
        for _ in 0..HOLD_TO_EXIT_FRAMES - 1 {
            assert!(!hold.tick(true));
        }
        assert!(hold.tick(true));
    }
}
