//! SDL2 desktop host: a window, an integer scaler, 60 Hz pacing and a
//! keyboard and controllers mapped to the logical controller. The core
//! never learns the scale.
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
pub mod device;
mod gamepad;
pub mod keys;
pub use kuula_host_common::pacing;
pub mod scale;
mod text_input;

pub type ShellService = Box<dyn FnMut(&mut Console, &[SysRequest])>;

use std::time::Duration;

use kuula_core::input::BTN_MENU;
use kuula_core::net::Link;
use kuula_core::shell::{Settings, SysRequest};
use kuula_core::{error_screen, Console, ConsoleState, FRAME_RATE};
use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use sdl2::pixels::PixelFormatEnum;

use device::{HoldToExit, Profile};
use keys::{KeyState, Layout};
use kuula_host_common::chord::MenuChord;
use pacing::{Clock, RealClock, Scheduler};

pub struct HostOptions {
    /// Integer scale, 1 to 4.
    pub scale: u32,
    pub title: String,
    /// The network link the console steps through, for a cart that
    /// may use the network; `None` steps without one.
    pub link: Option<Link>,
    /// Told the shell's settings after every change, so the host can
    /// persist them.
    pub on_settings: Option<Box<dyn FnMut(Settings)>>,
    pub shell_service: Option<ShellService>,
}

impl HostOptions {
    pub fn new(scale: u32, title: impl Into<String>) -> HostOptions {
        HostOptions {
            scale,
            title: title.into(),
            link: None,
            on_settings: None,
            shell_service: None,
        }
    }
}

/// Why the loop ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    WindowClosed,
    /// B on the error screen, Menu held on a handheld, or the shell
    /// asking to end (`sys.exit()`).
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
    let mut link = opts.link;
    let mut on_settings = opts.on_settings;
    let mut shell_service = opts.shell_service;
    let mut console = make();
    // The cart's declared mode: the texture is that size, and the
    // window is the primary mode times the scale whatever the cart
    // declares, so a 320x240 cart is stretched an exact 2x more.
    let (w, h) = {
        let out = console.output();
        (out.width, out.height)
    };

    let sdl = sdl2::init()?;
    let video = sdl.video()?;
    let profile = Profile::for_video_driver(video.current_video_driver());
    let clamp_scale = |n: u32| scale::clamp(n).min(profile.max_scale());
    let mut window_scale = clamp_scale(opts.scale);
    console.set_effective_scale(window_scale);
    let (win_w, win_h) = scale::window_size(window_scale);
    let window = video
        .window(&opts.title, win_w, win_h)
        .position_centered()
        .build()
        .map_err(|e| e.to_string())?;
    let canvas_builder = window.into_canvas();
    let mut canvas = if profile.accelerated_renderer() {
        canvas_builder.accelerated()
    } else {
        canvas_builder
    }
    .build()
    .map_err(|e| e.to_string())?;
    let creator = canvas.texture_creator();
    let format = profile.pixel_format();
    let mut texture = creator
        .create_texture_streaming(format, w, h)
        .map_err(|e| e.to_string())?;
    let mut events = sdl.event_pump()?;
    // No device is not fatal: the console still renders its PCM.
    let audio = match sdl.audio() {
        Ok(subsystem) => audio::open(&subsystem, profile.audio_buffer()),
        Err(e) => {
            eprintln!("audio: {e}; running silent");
            None
        }
    };

    let mut gamepads = gamepad::Gamepads::new(&sdl).ok();
    let layout = Layout::for_profile(profile);
    let mut keys = KeyState::new(layout);
    let mut hold_menu = HoldToExit::default();
    let mut editor_keys = KeyState::default();
    let mut chord = MenuChord::default();
    // A device may take audio faster than it plays it while a queue of its
    // own fills; that passes on silence here, before the first frame.
    if let Some(a) = &audio {
        a.settle();
    }
    let mut clock = RealClock::new();
    let period = Duration::from_nanos(1_000_000_000 / FRAME_RATE as u64);
    let mut scheduler = Scheduler::new(clock.now(), period);
    // For what `KUULA_HOST_STATS` asks for at exit.
    let host_stats = std::env::var_os("KUULA_HOST_STATS").is_some();
    let began = clock.now();
    let (mut stepped, mut presented) = (0u64, 0u64);
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
        // A handheld has no keyboard: its buttons arrive as keys, and a
        // field is filled with the shell's on-screen keys. And while the
        // shell asks about a developer, over whatever it was showing, the
        // keys are the buttons that answer.
        let asking = console.dev_view().is_some_and(|d| !d.pending.is_empty());
        let editing = layout != Layout::Handheld
            && !asking
            && console
                .network_view()
                .is_some_and(|v| !v.editing.is_empty());
        if editing {
            video.text_input().start();
        } else {
            video.text_input().stop();
        }
        for event in events.poll_iter() {
            if let Some(pads) = gamepads.as_mut() {
                pads.event(&event);
            }
            if layout != Layout::Handheld
                && text_input::event(&event, &mut console, &video, &mut editor_keys)
            {
                continue;
            }
            match event {
                Event::Quit { .. } => break 'main Exit::WindowClosed,
                Event::KeyDown {
                    keycode: Some(key),
                    keymod,
                    repeat,
                    ..
                } => {
                    if let Some(new_scale) = keys::scale_hotkey(key, keymod) {
                        window_scale = clamp_scale(new_scale);
                        console.set_effective_scale(window_scale);
                        let (win_w, win_h) = scale::window_size(window_scale);
                        canvas
                            .window_mut()
                            .set_size(win_w, win_h)
                            .map_err(|e| e.to_string())?;
                    } else if faulted && key == Keycode::Z && !repeat {
                        // A: a fresh console from the same cart; nothing
                        // from the dead one survives, not even held keys.
                        console = make();
                        console.set_effective_scale(window_scale);
                        keys = KeyState::new(layout);
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

        // A frame is a sixtieth of a second whatever the display does: when
        // the schedule is a whole period behind (a display that presents
        // slower than 60 Hz, a slow frame), step again before presenting, so
        // the cart and its audio keep time and only shown frames are lost.
        let loop_began = clock.now();
        let steps = scheduler.steps_due(loop_began);
        // And a frame more or fewer now and then to follow the audio
        // device's clock, which is the one the sound is played by.
        let steps = match &audio {
            Some(a) => audio::trim_steps(steps, a.ring.playing_level(), presented),
            None => steps,
        };
        if let (true, Some(a)) = (steps != 1, &audio) {
            a.ring.note("frames stepped for one shown", steps as u64);
        }
        for _ in 0..steps {
            // Step, then read the frame back through the shared borrow so
            // the state can be inspected beside it.
            let input = kuula_core::FrameInput::new(chord.apply(
                keys.input().buttons
                    | editor_keys.input().buttons
                    | gamepads.as_mut().map(|p| p.buttons()).unwrap_or(0),
            ));
            if profile.hold_menu_to_exit() && hold_menu.tick(input.buttons & BTN_MENU != 0) {
                break 'main Exit::Quit;
            }
            match link.as_mut() {
                Some(l) => console.step_linked(l, input),
                None => {
                    console.step(input);
                }
            }
            stepped += 1;
            for e in console.take_save_failures() {
                eprintln!("save: {e}");
            }
            let requests = console.take_host_requests();
            let mut leave = false;
            for request in &requests {
                match request {
                    SysRequest::Exit => leave = true,
                    SysRequest::SetScale(new_scale) => {
                        window_scale = clamp_scale(*new_scale);
                        console.set_effective_scale(window_scale);
                        let (win_w, win_h) = scale::window_size(window_scale);
                        canvas
                            .window_mut()
                            .set_size(win_w, win_h)
                            .map_err(|e| e.to_string())?;
                    }
                    SysRequest::SetNet(on) => {
                        if let Some(l) = link.as_mut() {
                            l.set_permitted(*on);
                        }
                    }
                    _ => {}
                }
            }
            text_input::requests(&requests, &mut console, &video);
            if let Some(service) = shell_service.as_mut() {
                service(&mut console, &requests);
            }
            if leave {
                break 'main Exit::Quit;
            }
            if requests.iter().any(|r| {
                matches!(
                    r,
                    SysRequest::SetScale(_) | SysRequest::SetVolume(_) | SysRequest::SetNet(_)
                )
            }) {
                if let Some(f) = on_settings.as_mut() {
                    f(console.settings());
                }
            }
            let out = console.output();
            if let Some(a) = &audio {
                a.ring.push(out.audio);
            }
            for line in out.log {
                println!("{line}");
            }
        }
        let out = console.output();
        if (out.width, out.height) != (w, h) {
            // The shell ran a cart in another screen mode: only the
            // texture follows, the window keeps its size.
            (w, h) = (out.width, out.height);
            texture = creator
                .create_texture_streaming(format, w, h)
                .map_err(|e| e.to_string())?;
        }
        if let Some(f) = console.shell_fault() {
            if !reported_shell_fault {
                reported_shell_fault = true;
                eprintln!("shell: {f}");
            }
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
            if format == PixelFormatEnum::RGB24 {
                convert::blit_rows(pixels, out.palette, w as usize, buf, pitch)
            } else {
                convert::blit_rows_argb(pixels, out.palette, w as usize, buf, pitch)
            }
        })?;

        canvas.clear();
        canvas.copy(&texture, None, None)?;
        let drawn = clock.now();
        canvas.present();
        presented += 1;
        if host_stats {
            // A loop that took more than a frame and a half, and how much
            // of that the present was.
            let now = clock.now();
            let whole = now.saturating_sub(loop_began).as_millis() as u64;
            if let (true, Some(a)) = (whole > 25, &audio) {
                a.ring.note("ms in one loop", whole);
                a.ring.note(
                    "  of them in present",
                    now.saturating_sub(drawn).as_millis() as u64,
                );
            }
        }
        if presented == 1 {
            // A console's first frame loads what the cart starts with and
            // can take several periods. The schedule begins after it,
            // instead of opening the run with a burst of catch-up frames.
            scheduler = Scheduler::new(clock.now(), period);
        }
        scheduler.wait(&mut clock);
    };

    if let Some(a) = &audio {
        a.report(host_stats);
    }
    if host_stats {
        let seconds = clock.now().saturating_sub(began).as_secs_f64();
        eprintln!("host: {stepped} frames stepped and {presented} shown in {seconds:.1} s");
    }
    Ok((exit, console.state().clone()))
}
