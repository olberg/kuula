//! The activity's loop. It follows the SDL host's (`kuula-host-sdl`): the
//! console is stepped on an absolute 60 Hz schedule, with a step more or
//! fewer now and then to keep to the audio device's clock, and the shell's
//! requests are served after each step. What differs is the platform: the
//! picture and the controls are drawn into the window's buffer, the
//! activity's own lifecycle decides whether anything runs, and waiting for
//! the next frame is polling the activity's events, so a touch is taken as
//! it comes and a paused app blocks and does nothing.

use std::time::Duration;

use android_activity::input::Axis;
use android_activity::{AndroidApp, MainEvent, PollEvent, WindowManagerFlags};
use kuula_core::shell::{Settings, SysRequest};
use kuula_core::{error_screen, Console, ConsoleState, Fault, FrameInput, FRAME_RATE};
use kuula_host_common::audio::{report, trim_steps, Ring};
use kuula_host_common::devlog::{self, Outcome, Watch};
use kuula_host_common::pacing::{Clock, RealClock, Scheduler};
use kuula_touch::{Canvas, Layout};
use ndk::hardware_buffer_format::HardwareBufferFormat;
use ndk::native_window::NativeWindow;

use crate::audio::Audio;
use crate::input::{self, Input};
use crate::lifecycle::{Event, Lifecycle};
use crate::shell::{self, Shell};
use crate::storage::{self, Dirs};
use crate::view::{self, LayoutKey};
use crate::{activity, carts, settings};

/// How long before a device that failed is tried again.
const AUDIO_RETRY: Duration = Duration::from_secs(2);

/// A clock that does not sleep: the schedule is kept by `Scheduler`, and the
/// wait for its deadline is spent polling the activity's events.
struct NoSleep<'a>(&'a RealClock);

impl Clock for NoSleep<'_> {
    fn now(&self) -> Duration {
        self.0.now()
    }

    fn sleep_until(&mut self, _deadline: Duration) {}
}

pub fn run(app: &AndroidApp) -> Result<(), String> {
    let dirs = storage::prepare(app).ok_or("the app has no internal data directory")?;
    let stored = settings::load(&dirs.settings, Settings::default());
    let listed = carts::list(&dirs.own_carts, &dirs.builtin_carts);
    log::info!("{} carts listed", listed.len());
    let shell = Shell::new(listed, dirs.saves.clone(), stored);

    // The buttons of a controller that a hat reports as axes are not
    // delivered unless asked for, nor are its triggers.
    for axis in [
        Axis::HatX,
        Axis::HatY,
        Axis::Ltrigger,
        Axis::Rtrigger,
        Axis::Brake,
        Axis::Gas,
    ] {
        app.enable_motion_axis(axis);
    }
    // A controller's buttons do not keep a screen awake the way a finger does.
    app.set_window_flags(
        WindowManagerFlags::KEEP_SCREEN_ON,
        WindowManagerFlags::empty(),
    );

    let mut host = Host::new(app.clone(), shell, dirs);
    if let Some(name) = activity::launch_cart(app) {
        host.start_on(&name);
    }
    host.main_loop();
    Ok(())
}

struct Host {
    app: AndroidApp,
    shell: Shell,
    dirs: Dirs,
    console: Console,
    lifecycle: Lifecycle,
    window: Option<NativeWindow>,
    layout: Option<(LayoutKey, Layout)>,
    /// Where the display's cutout leaves the window clear, as last told.
    cutout: activity::Cutout,
    input: Input,
    input_pending: bool,
    ring: Ring,
    audio: Option<Audio>,
    audio_retry_at: Option<Duration>,
    clock: RealClock,
    period: Duration,
    scheduler: Scheduler,
    presented: u64,
    /// The buttons at the last step, for what was pressed just now.
    previous_buttons: u16,
    overlay: Vec<u8>,
    last_fault: Option<Fault>,
    last_shell_fault: Option<Fault>,
    reported_bad_format: bool,
    was_active: bool,
    /// The activity was asked to finish: nothing more is stepped.
    leaving: bool,
    /// The cart the activity was started on, until the log says how it
    /// fared.
    deployed: Option<Watch>,
}

impl Host {
    fn new(app: AndroidApp, shell: Shell, dirs: Dirs) -> Host {
        let clock = RealClock::new();
        let period = Duration::from_nanos(1_000_000_000 / FRAME_RATE as u64);
        let scheduler = Scheduler::new(clock.now(), period);
        let console = shell.make();
        Host {
            app,
            shell,
            dirs,
            console,
            lifecycle: Lifecycle::default(),
            window: None,
            layout: None,
            cutout: activity::Cutout::default(),
            input: Input::new(),
            input_pending: false,
            ring: Ring::default(),
            audio: None,
            audio_retry_at: None,
            clock,
            period,
            scheduler,
            presented: 0,
            previous_buttons: 0,
            overlay: Vec::new(),
            last_fault: None,
            last_shell_fault: None,
            reported_bad_format: false,
            was_active: false,
            leaving: false,
            deployed: None,
        }
    }

    /// Open the listed cart `name` at once, as the shell would on a
    /// person's choice: the activity was started on it (a cart pushed over
    /// `adb`). The log says how it fared, for the desktop that pushed it.
    fn start_on(&mut self, name: &str) {
        match self.shell.open(name) {
            Ok((source, store)) => {
                log::info!("started on {name}");
                self.console.host_load_cart(source, store);
                self.deployed = Some(Watch::new(name));
            }
            Err(fault) => {
                log::warn!(
                    "{}",
                    devlog::line(name, &Outcome::NotRun(fault.to_string()))
                );
            }
        }
    }

    fn main_loop(&mut self) {
        loop {
            if self.lifecycle.destroyed() {
                break;
            }
            let active = self.lifecycle.active() && !self.leaving;
            if active != self.was_active {
                self.was_active = active;
                if active {
                    self.activate();
                } else {
                    self.deactivate();
                }
            }
            if active {
                self.frame();
            } else {
                // Nothing to step, nothing to draw, nothing to wake for but
                // an event: block.
                self.poll(None);
            }
        }
        // The window is going away with the activity.
        self.deactivate();
    }

    /// The app came to the front.
    fn activate(&mut self) {
        log::info!("in front");
        activity::hide_system_bars(&self.app);
        activity::ask_cutout(&self.app, &self.cutout);
        match &self.audio {
            Some(audio) => audio.resume(),
            None => self.open_audio(),
        }
        // No catch-up for the time spent away.
        self.scheduler = Scheduler::new(self.clock.now(), self.period);
        self.presented = 0;
    }

    /// Open the audio device, at the start and again after a stream died.
    /// A device may take audio faster than it plays it while it fills a
    /// queue of its own; that is let pass on silence, before a frame is
    /// pushed to the stream, for every stream that is opened: the one a
    /// retry opens fills its queue like the first. The ring starts empty,
    /// so nothing from before is played late, and the schedule starts
    /// after the wait.
    fn open_audio(&mut self) {
        self.ring.clear();
        self.audio = Audio::open(self.ring.clone());
        if let Some(audio) = &self.audio {
            audio.pace.settle(&self.ring);
        }
        let now = self.clock.now();
        self.audio_retry_at = Some(now + AUDIO_RETRY);
        self.scheduler = Scheduler::new(now, self.period);
    }

    /// The app went to the back, or is ending.
    fn deactivate(&mut self) {
        log::info!("not in front");
        if let Some(audio) = &self.audio {
            audio.pause();
            // A line in the log when the ring ran dry or overflowed.
            report(&audio.ring, &audio.pace, false);
        }
        self.input.release_all();
    }

    /// Wait up to `timeout` for the activity's events (for ever on `None`),
    /// take them, and take the input they announce.
    fn poll(&mut self, timeout: Option<Duration>) {
        let app = self.app.clone();
        app.poll_events(timeout, |event| self.on_event(event));
        if self.input_pending {
            self.input_pending = false;
            input::drain(
                &self.app,
                &mut self.input,
                self.layout.as_ref().map(|(_, l)| l),
            );
        }
    }

    fn on_event(&mut self, event: PollEvent<'_>) {
        let PollEvent::Main(event) = event else {
            return;
        };
        match event {
            MainEvent::InputAvailable => self.input_pending = true,
            MainEvent::InitWindow { .. } => {
                self.lifecycle.event(Event::InitWindow);
                self.take_window();
            }
            MainEvent::TerminateWindow { .. } => {
                self.lifecycle.event(Event::TerminateWindow);
                self.window = None;
                self.layout = None;
                self.input.release_all();
            }
            // A rotation, a split screen, a keyboard or a bar coming and
            // going. The layout is rebuilt at the next frame from what the
            // window is then; the buffers are asked for again, and fingers
            // that were down belonged to the old shape.
            MainEvent::WindowResized { .. }
            | MainEvent::ConfigChanged { .. }
            | MainEvent::ContentRectChanged { .. }
            | MainEvent::InsetsChanged { .. } => {
                self.take_window();
                self.input.pad.cancel();
                // The cutout is on another side after a turn.
                activity::ask_cutout(&self.app, &self.cutout);
            }
            MainEvent::GainedFocus => self.lifecycle.event(Event::GainedFocus),
            MainEvent::LostFocus => {
                self.lifecycle.event(Event::LostFocus);
                self.input.release_all();
            }
            MainEvent::Resume { .. } => self.lifecycle.event(Event::Resume),
            MainEvent::Pause => self.lifecycle.event(Event::Pause),
            MainEvent::Stop => self.lifecycle.event(Event::Stop),
            MainEvent::Destroy => self.lifecycle.event(Event::Destroy),
            _ => {}
        }
    }

    /// The window as the activity has it now, with a 32-bit buffer the size
    /// of the window.
    fn take_window(&mut self) {
        self.window = self.app.native_window();
        if let Some(window) = &self.window {
            // Zero for both sizes: the window's own, whatever it becomes.
            if let Err(e) =
                window.set_buffers_geometry(0, 0, Some(HardwareBufferFormat::R8G8B8X8_UNORM))
            {
                log::warn!("cannot set the window's buffer format: {e}");
            }
        }
    }

    /// What the layout is to be built from now.
    fn wanted_layout(&self, picture: (u32, u32)) -> Option<LayoutKey> {
        let window = self.window.as_ref()?;
        let size = (window.width(), window.height());
        if size.0 <= 0 || size.1 <= 0 {
            return None;
        }
        let rect = self.app.content_rect();
        let dpi = self.app.config().density();
        let content =
            view::insets_from_content(size, (rect.left, rect.top, rect.right, rect.bottom));
        Some(LayoutKey {
            window: size,
            insets: view::widest(content, self.cutout.insets(), size),
            density: view::density_scale(dpi),
            picture,
        })
    }

    /// Build the layout again when what it is made from has changed.
    fn update_layout(&mut self, picture: (u32, u32)) {
        let Some(key) = self.wanted_layout(picture) else {
            self.layout = None;
            return;
        };
        if self.layout.as_ref().is_some_and(|(k, _)| *k == key) {
            return;
        }
        self.set_layout(key);
    }

    fn set_layout(&mut self, key: LayoutKey) {
        log::info!(
            "layout: window {}x{}, insets {:?}, density {}, picture {}x{}",
            key.window.0,
            key.window.1,
            key.insets,
            key.density,
            key.picture.0,
            key.picture.1
        );
        let layout = Layout::new(key.window, key.insets, key.density, key.picture);
        self.layout = Some((key, layout));
        // The fingers held belonged to the old layout.
        self.input.pad.cancel();
    }

    /// One frame: the steps the schedule asks for, the picture, and the wait
    /// for the next.
    fn frame(&mut self) {
        let out = self.console.output();
        let picture = (out.width, out.height);
        self.update_layout(picture);
        // The controls of whoever has the buttons: the shell's six, or
        // what the cart's manifest declares. The fingers stay as they
        // are, since what both have is in the same places.
        let shown = self.console.buttons_in_use();
        if let Some((_, layout)) = self.layout.as_mut() {
            *layout = layout.showing(shown);
        }
        self.retry_audio();

        let steps = self.scheduler.steps_due(self.clock.now());
        // And a frame more or fewer now and then to follow the audio
        // device's clock, which is the one the sound is played by.
        let steps = match &self.audio {
            Some(a) => trim_steps(steps, a.ring.playing_level(), self.presented),
            None => steps,
        };
        if let (true, Some(a)) = (steps != 1, &self.audio) {
            a.ring.note("frames stepped for one shown", steps as u64);
        }
        for _ in 0..steps {
            self.step();
            if self.leaving {
                return;
            }
        }
        self.present();
        self.presented += 1;
        // Once, a few seconds in: what the audio device settled on.
        if self.presented == 300 {
            if let Some(audio) = &self.audio {
                let (frames, held, ahead) = audio.latency.read();
                let ring = self.ring.playing_level().unwrap_or(0) / 2;
                log::info!(
                    "audio: {} Hz, {frames} sample frames a callback, the stream holding {held}, written {ahead} ms ahead of the speaker, {ring} sample frames of the console's waiting in the ring",
                    audio.rate
                );
            }
        }
        if self.presented == 1 {
            // A console's first frame loads what the cart starts with and
            // can take several periods. The schedule begins after it,
            // instead of opening the run with a burst of catch-up frames.
            self.scheduler = Scheduler::new(self.clock.now(), self.period);
        }
        let deadline = self.scheduler.wait(&mut NoSleep(&self.clock));
        loop {
            let now = self.clock.now();
            if now >= deadline || !self.lifecycle.active() {
                break;
            }
            self.poll(Some(deadline - now));
        }
    }

    /// A stream that died is opened again: at once when it had played for
    /// a while (headphones in or out, and the sound moves to them), after
    /// a wait when it died young, so a device that keeps failing is not
    /// opened every frame.
    fn retry_audio(&mut self) {
        let now = self.clock.now();
        if self.audio.as_ref().is_some_and(Audio::failed) {
            self.audio = None;
            self.audio_retry_at = Some(self.audio_retry_at.map_or(now, |at| at.max(now)));
        }
        if self.audio.is_none() && self.audio_retry_at.is_some_and(|at| now >= at) {
            self.open_audio();
        }
    }

    /// Whether the host's own error screen is up: the cart has faulted and
    /// there is no working shell to show it.
    fn host_error_screen(&self) -> bool {
        self.console.state().fault().is_some()
            && (!self.console.has_shell() || self.console.shell_fault().is_some())
    }

    fn step(&mut self) {
        let buttons = self.input.sample();
        let pressed = buttons & !self.previous_buttons;
        self.previous_buttons = buttons;
        if self.host_error_screen() {
            // A starts the cart again from scratch and B ends the app; nothing
            // from the dead console survives, not even held buttons.
            if pressed & kuula_core::input::BTN_A != 0 {
                self.console = self.shell.make();
                self.input.release_all();
                self.ring.clear();
                self.previous_buttons = 0;
                self.last_fault = None;
                self.last_shell_fault = None;
                return;
            }
            if pressed & kuula_core::input::BTN_B != 0 {
                self.leave();
                return;
            }
        }

        self.console
            .step_linked(&mut self.shell.link, FrameInput::new(buttons));
        for e in self.console.take_save_failures() {
            log::warn!("save: {}", devlog::one_line(&e.to_string()));
        }
        let requests = self.console.take_host_requests();
        let mut settings_changed = false;
        let mut exit = false;
        for request in &requests {
            match request {
                SysRequest::Exit => exit = true,
                SysRequest::SetNet(on) => {
                    self.shell.link.set_permitted(*on);
                    settings_changed = true;
                }
                SysRequest::SetVolume(_) => settings_changed = true,
                // The scale is the layout's.
                _ => {}
            }
        }
        shell::say_no_network(&mut self.console);
        if settings_changed {
            let s = self.console.settings();
            self.shell.settings_changed(s);
            settings::save(&self.dirs.settings, &s);
        }
        let out = self.console.output();
        if let Some(a) = &self.audio {
            a.ring.push(out.audio);
        }
        for line in out.log {
            log::info!("{}", devlog::cart_line(line));
        }
        self.report_faults();
        if let Some(watch) = &mut self.deployed {
            let fault = self.console.state().fault().map(|f| f.to_string());
            let answer = watch
                .after_step(fault.as_deref())
                .map(|outcome| devlog::line(watch.name(), &outcome));
            if let Some(line) = answer {
                log::info!("{line}");
                self.deployed = None;
            }
        }
        if exit {
            self.leave();
        }
    }

    /// Log a fault once, when it happens. Its message is the cart's to
    /// choose, line breaks and all, so it is logged as one line with
    /// `fault: ` before it: a second line of it would stand in the log
    /// with nothing before it, where the app's own word stands.
    fn report_faults(&mut self) {
        let fault = self.console.state().fault().cloned();
        if fault != self.last_fault {
            if let Some(f) = &fault {
                log::error!("fault: {}", devlog::one_line(&f.to_string()));
            }
            self.last_fault = fault;
        }
        let shell_fault = self.console.shell_fault().cloned();
        if shell_fault != self.last_shell_fault {
            if let Some(f) = &shell_fault {
                log::error!("shell fault: {}", devlog::one_line(&f.to_string()));
            }
            self.last_shell_fault = shell_fault;
        }
    }

    /// `sys.exit()`, or B on the error screen: finish the activity. The loop
    /// goes on, idle, until the activity is destroyed.
    fn leave(&mut self) {
        if !self.leaving {
            log::info!("finishing the activity");
            activity::finish(&self.app);
        }
        self.leaving = true;
    }

    /// The console's frame, and the controls over it, into the window.
    fn present(&mut self) {
        let Some(window) = &self.window else {
            return;
        };
        let Some((_, layout)) = &self.layout else {
            return;
        };
        let out = self.console.output();
        let pixels: &[u8] = match self.console.state() {
            ConsoleState::Faulted(fault) if self.host_error_screen() => {
                self.overlay.clear();
                self.overlay.extend_from_slice(out.screen);
                error_screen::compose(&mut self.overlay, out.width, out.height, fault);
                &self.overlay
            }
            _ => out.screen,
        };
        let mut guard = match window.lock(None) {
            Ok(guard) => guard,
            Err(e) => {
                log::warn!("cannot lock the window: {e}");
                return;
            }
        };
        if !matches!(
            guard.format(),
            HardwareBufferFormat::R8G8B8X8_UNORM | HardwareBufferFormat::R8G8B8A8_UNORM
        ) {
            if !self.reported_bad_format {
                self.reported_bad_format = true;
                log::error!(
                    "the window's buffer is not 32-bit RGB: {:?}",
                    guard.format()
                );
            }
            return;
        }
        let (width, height, stride) = (guard.width(), guard.height(), guard.stride());
        // The buffer can be of another size than the layout was built for:
        // the window turned while this frame was being made. It is drawn
        // as the layout stands, clipped, and the layout is built again at
        // the next frame from the window.
        let bits = guard.bits().cast::<u8>();
        if bits.is_null() {
            return;
        }
        // SAFETY: `lock` gave a buffer of `stride` pixels a row and `height`
        // rows of four bytes, writable until the guard is dropped, and
        // nothing else touches it meanwhile.
        let data = unsafe { std::slice::from_raw_parts_mut(bits, stride * height * 4) };
        let mut canvas = Canvas {
            data,
            width,
            height,
            stride,
        };
        kuula_touch::present(
            &mut canvas,
            layout,
            pixels,
            out.palette,
            out.width,
            out.height,
        );
        if self.input.controls.controls_visible() {
            kuula_touch::draw_controls(&mut canvas, layout, self.input.pad.buttons());
        }
    }
}
