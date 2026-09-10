//! Frame pacing on an absolute schedule: deadline `n` is `t0 + n * period`,
//! so a late frame does not push later deadlines and drift does not
//! accumulate. Only if the loop falls more than [`Scheduler::RESYNC_AFTER`]
//! behind (the window was being dragged, the machine slept) does the
//! schedule restart from now instead of running a burst of catch-up
//! frames.

use std::time::{Duration, Instant};

/// A monotonic clock the scheduler can sleep on. Tests supply a fake.
pub trait Clock {
    fn now(&self) -> Duration;
    fn sleep_until(&mut self, deadline: Duration);
}

/// The process clock, measured from construction.
pub struct RealClock {
    start: Instant,
}

impl RealClock {
    pub fn new() -> RealClock {
        RealClock {
            start: Instant::now(),
        }
    }
}

impl Default for RealClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for RealClock {
    fn now(&self) -> Duration {
        self.start.elapsed()
    }

    /// Sleep in coarse steps, then spin the last stretch. OS sleep
    /// granularity on Windows is around a millisecond once SDL has set the
    /// timer resolution, so leave a margin and busy-wait the remainder.
    fn sleep_until(&mut self, deadline: Duration) {
        const MARGIN: Duration = Duration::from_micros(1500);
        loop {
            let now = self.now();
            if now >= deadline {
                return;
            }
            let remaining = deadline - now;
            if remaining > MARGIN {
                std::thread::sleep(remaining - MARGIN);
            } else {
                std::thread::yield_now();
            }
        }
    }
}

pub struct Scheduler {
    period: Duration,
    next: Duration,
}

impl Scheduler {
    /// How far behind the schedule may fall before it restarts from now.
    pub const RESYNC_AFTER: Duration = Duration::from_millis(500);

    pub fn new(now: Duration, period: Duration) -> Scheduler {
        Scheduler {
            period,
            next: now + period,
        }
    }

    /// The deadline the next `wait` will target.
    pub fn next_deadline(&self) -> Duration {
        self.next
    }

    /// Block until the next deadline and advance the schedule. Returns the
    /// deadline that was waited for.
    pub fn wait(&mut self, clock: &mut impl Clock) -> Duration {
        let deadline = self.next;
        let now = clock.now();
        if now < deadline {
            clock.sleep_until(deadline);
        } else if now - deadline > Self::RESYNC_AFTER {
            self.next = now;
        }
        self.next += self.period;
        deadline
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeClock {
        now: Duration,
        sleeps: Vec<Duration>,
    }

    impl FakeClock {
        fn new() -> FakeClock {
            FakeClock {
                now: Duration::from_secs(10),
                sleeps: Vec::new(),
            }
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> Duration {
            self.now
        }
        fn sleep_until(&mut self, deadline: Duration) {
            self.sleeps.push(deadline);
            self.now = deadline;
        }
    }

    const PERIOD: Duration = Duration::from_nanos(1_000_000_000 / 60);

    #[test]
    fn sixty_frames_land_within_one_period_of_one_second() {
        let mut clock = FakeClock::new();
        let t0 = clock.now();
        let mut s = Scheduler::new(t0, PERIOD);
        let mut last = t0;
        for _ in 0..60 {
            // Simulate a frame that takes 3 ms of work.
            clock.now += Duration::from_millis(3);
            last = s.wait(&mut clock);
        }
        let target = t0 + Duration::from_secs(1);
        let diff = last.abs_diff(target);
        assert!(diff <= PERIOD, "last deadline {last:?} vs {target:?}");
        assert_eq!(clock.sleeps.len(), 60, "every frame slept");
        assert!(clock.now <= target && target - clock.now <= PERIOD);
    }

    #[test]
    fn a_late_frame_does_not_shift_later_deadlines() {
        let mut clock = FakeClock::new();
        let t0 = clock.now();
        let mut s = Scheduler::new(t0, PERIOD);
        s.wait(&mut clock);
        // Frame two runs 30 ms, blowing through its deadline.
        clock.now += Duration::from_millis(30);
        let late = s.wait(&mut clock);
        assert_eq!(late, t0 + 2 * PERIOD);
        assert_eq!(clock.sleeps.len(), 1, "no sleep when late");
        // The next deadlines are still on the original grid.
        assert_eq!(s.next_deadline(), t0 + 3 * PERIOD);
        let third = s.wait(&mut clock);
        assert_eq!(third, t0 + 3 * PERIOD);
        assert_eq!(s.next_deadline(), t0 + 4 * PERIOD);
    }

    #[test]
    fn falling_far_behind_restarts_the_schedule() {
        let mut clock = FakeClock::new();
        let t0 = clock.now();
        let mut s = Scheduler::new(t0, PERIOD);
        clock.now += Duration::from_secs(2);
        s.wait(&mut clock);
        assert_eq!(s.next_deadline(), clock.now + PERIOD);
    }
}
