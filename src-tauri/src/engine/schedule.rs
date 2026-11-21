use std::time::{Duration, Instant};

use crate::settings::Cadence;

// Each value is the wait after the previous check. Existing tray limits cap this sequence.
const IDLE_MINUTES: [u64; 6] = [1, 3, 5, 10, 20, 30];

#[derive(Clone, Copy)]
pub(crate) enum Interaction
{
    Window(bool),
    Tray(bool),
    Refresh,
}

pub(super) struct Schedule
{
    window: bool,
    pub(super) hovered: bool,
    step: usize,
    idle_at: Instant,
}

impl Schedule
{
    pub(super) fn new(now: Instant) -> Self
    {
        Self { window: false, hovered: false, step: 0, idle_at: now + Duration::from_secs(60) }
    }

    pub(super) fn active(&self) -> bool { self.window || self.hovered }

    pub(super) fn restart(&mut self, now: Instant)
    {
        self.step = 0;
        self.idle_at = now + Duration::from_secs(60);
    }

    pub(super) fn interact(&mut self, event: Interaction, now: Instant) -> bool
    {
        let active = self.active();
        let refresh = match event
        {
            Interaction::Window(open) =>
            {
                let opening = open && !self.window;
                self.window = open;
                opening
            }
            Interaction::Tray(inside) =>
            {
                let entering = inside && !self.hovered;
                self.hovered = inside;
                entering
            }
            Interaction::Refresh => true,
        };
        if refresh || (active && !self.active()) { self.restart(now); }
        refresh
    }

    pub(super) fn idle_deadline(&self) -> Option<Instant>
    {
        (!self.active()).then_some(self.idle_at)
    }

    pub(super) fn take_idle_check(&mut self, now: Instant, cadence: Cadence) -> bool
    {
        if self.active() || now < self.idle_at { return false; }
        self.step = (self.step + 1).min(IDLE_MINUTES.len() - 1);
        let delay = Duration::from_secs(IDLE_MINUTES[self.step] * 60).min(cadence.tray_limit());
        self.idle_at = now + delay;
        true
    }
}

#[cfg(test)]
mod tests
{
    use super::*;

    fn minutes(n: u64) -> Duration { Duration::from_secs(n * 60) }

    #[test]
    fn idle_intervals_advance_and_stay_at_the_saved_limit()
    {
        for (saved, intervals) in [
            ("live", [1, 3, 5, 10, 20, 20, 20]),
            ("5m", [1, 3, 5, 5, 5, 5, 5]),
            ("15m", [1, 3, 5, 10, 15, 15, 15]),
            ("30m", [1, 3, 5, 10, 20, 30, 30]),
        ]
        {
            let cadence = serde_json::from_value(serde_json::json!(saved)).unwrap();
            let mut at = Instant::now();
            let mut schedule = Schedule::new(at);
            for wait in intervals
            {
                at += minutes(wait);
                assert_eq!(schedule.idle_deadline(), Some(at));
                assert!(!schedule.take_idle_check(at - Duration::from_secs(1), cadence));
                assert!(schedule.take_idle_check(at, cadence));
            }
        }
    }

    #[test]
    fn hover_and_window_each_refresh_once_and_only_the_last_exit_starts_backoff()
    {
        let start = Instant::now();
        let mut schedule = Schedule::new(start);
        assert!(schedule.take_idle_check(start + minutes(1), Cadence::default()));
        let enter = start + minutes(2);
        assert!(schedule.interact(Interaction::Tray(true), enter));
        assert!(!schedule.interact(Interaction::Tray(true), enter));
        assert_eq!(schedule.idle_deadline(), None);
        assert!(schedule.interact(Interaction::Window(true), enter));
        assert!(!schedule.interact(Interaction::Window(true), enter));
        assert!(!schedule.interact(Interaction::Tray(false), enter));
        assert!(!schedule.take_idle_check(enter + minutes(30), Cadence::default()));
        let close = enter + minutes(31);
        assert!(schedule.interact(Interaction::Tray(true), close));
        assert!(!schedule.interact(Interaction::Window(false), close));
        assert_eq!(schedule.idle_deadline(), None);
        let leave = close + minutes(1);
        assert!(!schedule.interact(Interaction::Tray(false), leave));
        assert_eq!(schedule.idle_deadline(), Some(leave + minutes(1)));
        assert!(schedule.take_idle_check(leave + minutes(1), Cadence::default()));
        let manual = leave + minutes(2);
        assert!(schedule.interact(Interaction::Refresh, manual));
        assert_eq!(schedule.idle_deadline(), Some(manual + minutes(1)));
    }
}
