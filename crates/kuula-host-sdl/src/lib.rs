//! SDL2 desktop host: a window, an integer scaler, 60 Hz pacing and a
//! keyboard mapped to the logical controller. The core never learns the
//! scale.
//!
//! When the cart faults the host draws the core's error screen over the
//! last complete frame and offers A to restart
//! the cart from scratch and B to quit; there is no shell yet to do it.
//!
//! SDL2 is compiled from source through `sdl2-sys`'s `bundled` and
//! `static-link` features, so the executable carries no DLL. On Windows
//! this needs MSVC and CMake; `.cargo/config.toml` sets the CMake policy
//! version SDL's old build files require.

pub mod audio;
pub mod convert;
pub mod keys;
pub mod pacing;
pub mod scale;

use std::time::Duration;

use kuula_core::shell::SysRequest;
use kuula_core::{error_screen, Console, ConsoleState, FRAME_RATE};
use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use sdl2::pixels::PixelFormatEnum;

use keys::KeyState;
use pacing::{Clock, RealClock, Scheduler};

pub struct HostOptions {
    /// Integer scale, 1 to 4.
    pub scale: u32,
    pub title: String,
}

/// Why the loop ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    WindowClosed,
    /// B on the error screen.
    Quit,
}

/// Open a window and run consoles from `make` until the window is
/// closed or the player quits from the error screen. A fault is printed
/// to stderr once and the error screen stays up until A rebuilds the
/// console through `make` or B quits. Returns how the loop ended and the
/// last console's state.
pub fn run(
    make: &mut dyn FnMut() -> Console,
    opts: HostOptions,
) -> Result<(Exit, ConsoleState), String> {
    let scale = scale::clamp(opts.scale);
    let mut console = make();
    // The cart's declared mode: the texture is that size, and the
    // window is the primary mode times the scale whatever the cart
    // declares, so a 320x240 cart is stretched an exact 2x more.
    let (w, h) = {
        let out = console.output();
        (out.width, out.height)
    };
    let (win_w, win_h) = scale::window_size(scale);

    let sdl = sdl2::init()?;
    let video = sdl.video()?;
    let window = video
        .window(&opts.title, win_w, win_h)
        .position_centered()
        .build()
        .map_err(|e| e.to_string())?;
    let mut canvas = window.into_canvas().build().map_err(|e| e.to_string())?;
    let creator = canvas.texture_creator();
    let mut texture = creator
        .create_texture_streaming(PixelFormatEnum::RGB24, w, h)
        .map_err(|e| e.to_string())?;
    let mut events = sdl.event_pump()?;
    // No device is not fatal: the console still renders its PCM.
    let audio = match sdl.audio() {
        Ok(subsystem) => audio::open(&subsystem),
        Err(e) => {
            eprintln!("audio: {e}; running silent");
            None
        }
    };

    let mut keys = KeyState::default();
    let mut clock = RealClock::new();
    let mut scheduler = Scheduler::new(
        clock.now(),
        Duration::from_nanos(1_000_000_000 / FRAME_RATE as u64),
    );
    let mut reported_fault = false;
    let mut reported_shell_fault = false;
    let mut overlay: Vec<u8> = Vec::new();

    let (mut w, mut h) = (w, h);
    let exit = 'main: loop {
        // Without a working shell the host owns the error screen and its
        // two keys; with one, the shell draws it and A/B reach it as
        // buttons.
        let host_error_screen = console.state().fault().is_some()
            && (!console.has_shell() || console.shell_fault().is_some());
        let faulted = host_error_screen;
        for event in events.poll_iter() {
            match event {
                Event::Quit { .. } => break 'main Exit::WindowClosed,
                Event::KeyDown {
                    keycode: Some(key),
                    keymod,
                    repeat,
                    ..
                } => {
                    if let Some(new_scale) = keys::scale_hotkey(key, keymod) {
                        let (win_w, win_h) = scale::window_size(new_scale);
                        canvas
                            .window_mut()
                            .set_size(win_w, win_h)
                            .map_err(|e| e.to_string())?;
                    } else if faulted && key == Keycode::Z && !repeat {
                        // A: a fresh console from the same cart; nothing
                        // from the dead one survives, not even held keys.
                        console = make();
                        keys = KeyState::default();
                        reported_fault = false;
                        if let Some(a) = &audio {
                            a.ring.clear();
                        }
                    } else if faulted && key == Keycode::X && !repeat {
                        break 'main Exit::Quit;
                    } else if !repeat {
                        keys.press(key);
                    }
                }
                Event::KeyUp {
                    keycode: Some(key), ..
                } => keys.release(key),
                _ => {}
            }
        }

        // Step, then read the frame back through the shared borrow so
        // the state can be inspected beside it.
        console.step(keys.input());
        for e in console.take_save_failures() {
            eprintln!("save: {e}");
        }
        for request in console.take_host_requests() {
            if let SysRequest::SetScale(new_scale) = request {
                let (win_w, win_h) = scale::window_size(new_scale);
                canvas
                    .window_mut()
                    .set_size(win_w, win_h)
                    .map_err(|e| e.to_string())?;
            }
        }
        let out = console.output();
        if (out.width, out.height) != (w, h) {
            // The shell ran a cart in another screen mode: only the
            // texture follows, the window keeps its size.
            (w, h) = (out.width, out.height);
            texture = creator
                .create_texture_streaming(PixelFormatEnum::RGB24, w, h)
                .map_err(|e| e.to_string())?;
        }
        if let Some(f) = console.shell_fault() {
            if !reported_shell_fault {
                reported_shell_fault = true;
                eprintln!("shell: {f}");
            }
        }
        if let Some(a) = &audio {
            a.ring.push(out.audio);
        }
        for line in out.log {
            println!("{line}");
        }
        let pixels: &[u8] = match console.state() {
            ConsoleState::Faulted(fault) if host_error_screen => {
                if !reported_fault {
                    reported_fault = true;
                    eprintln!("{fault}");
                }
                overlay.clear();
                overlay.extend_from_slice(out.screen);
                error_screen::compose(&mut overlay, out.width, out.height, fault);
                &overlay
            }
            _ => out.screen,
        };
        texture.with_lock(None, |buf, pitch| {
            convert::blit_rows(pixels, out.palette, w as usize, buf, pitch)
        })?;

        canvas.clear();
        canvas.copy(&texture, None, None)?;
        canvas.present();
        scheduler.wait(&mut clock);
    };

    Ok((exit, console.state().clone()))
}
