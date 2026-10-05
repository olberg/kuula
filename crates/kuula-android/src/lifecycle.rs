//! Whether the app is in front: the console is stepped, and audio plays,
//! only while the activity is resumed, has a window and has focus. The
//! window going away and coming back (the app sent to the background, a
//! rotation on some devices) is a pause and a resume, not an end.

/// The lifecycle events the host reacts to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Resume,
    Pause,
    Stop,
    InitWindow,
    TerminateWindow,
    GainedFocus,
    LostFocus,
    Destroy,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Lifecycle {
    paused: bool,
    window: bool,
    focused: bool,
    destroyed: bool,
}

impl Lifecycle {
    pub fn event(&mut self, event: Event) {
        match event {
            Event::Resume => self.paused = false,
            Event::Pause | Event::Stop => self.paused = true,
            Event::InitWindow => self.window = true,
            Event::TerminateWindow => self.window = false,
            Event::GainedFocus => self.focused = true,
            Event::LostFocus => self.focused = false,
            Event::Destroy => self.destroyed = true,
        }
    }

    /// Whether to step and play: in front, with a window, with focus, and
    /// not on its way out.
    pub fn active(&self) -> bool {
        !self.paused && self.window && self.focused && !self.destroyed
    }

    /// The activity is being destroyed: leave the loop.
    pub fn destroyed(&self) -> bool {
        self.destroyed
    }

    pub fn has_window(&self) -> bool {
        self.window
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_runs_only_with_a_window_and_focus_and_not_paused() {
        let mut l = Lifecycle::default();
        assert!(!l.active());
        l.event(Event::Resume);
        l.event(Event::InitWindow);
        assert!(!l.active(), "no focus yet");
        l.event(Event::GainedFocus);
        assert!(l.active());
        l.event(Event::LostFocus);
        assert!(!l.active());
        l.event(Event::GainedFocus);
        l.event(Event::Pause);
        assert!(!l.active());
        l.event(Event::Resume);
        assert!(l.active());
    }

    #[test]
    fn the_window_going_and_coming_back_is_survived() {
        let mut l = Lifecycle::default();
        for e in [Event::Resume, Event::InitWindow, Event::GainedFocus] {
            l.event(e);
        }
        l.event(Event::TerminateWindow);
        assert!(!l.active());
        assert!(!l.has_window());
        l.event(Event::InitWindow);
        assert!(l.active());
    }

    #[test]
    fn destroy_ends_it_whatever_else_holds() {
        let mut l = Lifecycle::default();
        for e in [Event::Resume, Event::InitWindow, Event::GainedFocus] {
            l.event(e);
        }
        l.event(Event::Destroy);
        assert!(l.destroyed());
        assert!(!l.active());
    }
}
