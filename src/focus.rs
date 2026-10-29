//! Monk mode: a pomodoro timer. Start → the monk settles into his focus pose (the clock does not run yet)
//! → focus countdown → break countdown → idle again.

use std::time::{Duration, Instant};

/// How long the monk takes to sit down before the focus countdown starts.
pub const SETTLE: Duration = Duration::from_millis(1500);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Idle,
    /// Sitting down; the focus countdown starts when this ends.
    Settling,
    Focus,
    Break,
}

/// What `tick` reports when a countdown ends.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event {
    FocusDone,
    BreakDone,
}

pub struct Timer {
    pub focus: Duration,
    pub rest: Duration,
    pub phase: Phase,
    /// Focus sessions finished since the app started.
    pub sessions: u32,
    /// Start of the running stretch of the current phase; None while paused.
    since: Option<Instant>,
    /// Time already spent in the current phase before `since` (earlier stretches, before a pause).
    spent: Duration,
}

impl Default for Timer {
    fn default() -> Self {
        Self {
            focus: Duration::from_secs(25 * 60),
            rest: Duration::from_secs(5 * 60),
            phase: Phase::Idle,
            sessions: 0,
            since: None,
            spent: Duration::ZERO,
        }
    }
}

impl Timer {
    pub fn start(&mut self, now: Instant) {
        self.enter(Phase::Settling, now);
    }

    pub fn paused(&self) -> bool {
        self.phase != Phase::Idle && self.since.is_none()
    }

    pub fn pause(&mut self, now: Instant) {
        if let Some(since) = self.since.take() {
            self.spent += now - since;
        }
    }

    pub fn resume(&mut self, now: Instant) {
        if self.paused() {
            self.since = Some(now);
        }
    }

    pub fn reset(&mut self) {
        self.phase = Phase::Idle;
        self.since = None;
        self.spent = Duration::ZERO;
    }

    pub fn skip_break(&mut self) {
        if self.phase == Phase::Break {
            self.reset();
        }
    }

    fn enter(&mut self, phase: Phase, now: Instant) {
        self.phase = phase;
        self.since = Some(now);
        self.spent = Duration::ZERO;
    }

    fn elapsed(&self, now: Instant) -> Duration {
        self.spent + self.since.map_or(Duration::ZERO, |s| now.saturating_duration_since(s))
    }

    /// Length of the current phase.
    pub fn length(&self) -> Duration {
        match self.phase {
            Phase::Idle | Phase::Focus => self.focus,
            Phase::Settling => SETTLE,
            Phase::Break => self.rest,
        }
    }

    /// Time left on the clock. While settling the full focus time is shown: the clock has not started.
    pub fn remaining(&self, now: Instant) -> Duration {
        match self.phase {
            Phase::Idle | Phase::Settling => self.focus,
            _ => self.length().saturating_sub(self.elapsed(now)),
        }
    }

    /// 0 → 1 through the current phase.
    pub fn progress(&self, now: Instant) -> f32 {
        match self.phase {
            Phase::Idle => 0.0,
            _ => (self.elapsed(now).as_secs_f32() / self.length().as_secs_f32().max(0.001)).min(1.0),
        }
    }

    /// Moves to the next phase when the current one is over. Call it often (every frame is fine).
    pub fn tick(&mut self, now: Instant) -> Option<Event> {
        if self.since.is_none() || self.elapsed(now) < self.length() {
            return None;
        }
        // The next phase starts when this one ended, not when tick noticed (the window may have been hidden).
        let ended = now - (self.elapsed(now) - self.length());
        match self.phase {
            Phase::Settling => {
                self.enter(Phase::Focus, ended);
                self.tick(now)
            }
            Phase::Focus => {
                self.sessions += 1;
                self.enter(Phase::Break, ended);
                Some(Event::FocusDone)
            }
            Phase::Break => {
                self.reset();
                Some(Event::BreakDone)
            }
            Phase::Idle => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_starts_after_settling_and_survives_pause() {
        let t0 = Instant::now();
        let s = Duration::from_secs;
        let mut timer = Timer { focus: s(60), rest: s(10), ..Default::default() };
        timer.start(t0);
        // Settling: the clock still shows the full focus time.
        assert_eq!(timer.tick(t0 + SETTLE / 2), None);
        assert_eq!(timer.remaining(t0 + SETTLE / 2), s(60));
        let focus_start = t0 + SETTLE;
        assert_eq!(timer.tick(focus_start + s(20)), None);
        assert_eq!(timer.phase, Phase::Focus);
        assert_eq!(timer.remaining(focus_start + s(20)), s(40));
        // A 100 s pause does not count.
        timer.pause(focus_start + s(20));
        assert_eq!(timer.remaining(focus_start + s(120)), s(40));
        timer.resume(focus_start + s(120));
        assert_eq!(timer.tick(focus_start + s(159)), None);
        assert_eq!(timer.tick(focus_start + s(161)), Some(Event::FocusDone));
        assert_eq!((timer.phase, timer.sessions), (Phase::Break, 1));
        // The break started when focus ended (at 160 s), not when tick ran.
        assert_eq!(timer.remaining(focus_start + s(161)), s(9));
        assert_eq!(timer.tick(focus_start + s(170)), Some(Event::BreakDone));
        assert_eq!(timer.phase, Phase::Idle);
    }
}
