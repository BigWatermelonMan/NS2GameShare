//! Slow capacity recovery and fast backoff keep USB bursts from corrupting references.
use std::time::{Duration, Instant};
/// Relative USB transit time, without requiring synchronized PC/device clocks.
/// Keep two overlapping minute windows so oscillator drift cannot accumulate.
pub struct TransitDelay {
    start: Instant,
    window: Instant,
    minimum: i128,
    previous_minimum: Option<i128>,
    previous_device: Option<u64>,
    peak: Duration,
}
impl TransitDelay {
    pub fn new(now: Instant) -> Self {
        Self {
            start: now,
            window: now,
            minimum: i128::MAX,
            previous_minimum: None,
            previous_device: None,
            peak: Duration::ZERO,
        }
    }
    pub fn observe(&mut self, device_us: u64, now: Instant) {
        if self
            .previous_device
            .is_some_and(|previous| device_us < previous)
        {
            *self = Self::new(now);
        }
        self.previous_device = Some(device_us);
        if now.saturating_duration_since(self.window) >= Duration::from_secs(60) {
            self.previous_minimum = Some(self.minimum);
            self.minimum = i128::MAX;
            self.window = now;
        }
        let offset =
            now.saturating_duration_since(self.start).as_micros() as i128 - i128::from(device_us);
        self.minimum = self.minimum.min(offset);
        let minimum = self
            .previous_minimum
            .map_or(self.minimum, |previous| previous.min(self.minimum));
        self.peak = self.peak.max(Duration::from_micros(
            (offset - minimum).clamp(0, u64::MAX as i128) as u64,
        ));
    }
    pub fn take_peak(&mut self) -> Duration {
        std::mem::take(&mut self.peak)
    }
}
pub struct Recovery {
    previous: Option<u64>,
    missing: Option<(u64, u64)>,
    next_request: Instant,
}
impl Recovery {
    pub fn new(now: Instant) -> Self {
        Self {
            previous: None,
            missing: None,
            next_request: now,
        }
    }
    pub fn observe(&mut self, sequence: u64, keyframe: bool) {
        if keyframe {
            self.missing = None;
        } else if let Some(previous) = self.previous
            && sequence > previous.saturating_add(1)
        {
            let first = self.missing.map_or(previous + 1, |(first, _)| first);
            self.missing = Some((first, sequence - 1));
        }
        self.previous = Some(sequence);
    }
    pub fn dropped(&mut self, sequence: u64) {
        let first = self.missing.map_or(sequence, |(first, _)| first);
        self.missing = Some((first, sequence));
    }
    pub fn request(&mut self, now: Instant) -> Option<(u64, u64)> {
        if now < self.next_request {
            return None;
        }
        let missing = self.missing.take()?;
        self.next_request = now + Duration::from_secs(2);
        Some(missing)
    }
}
pub struct Controller {
    ceiling: u32,
    capacity: u32,
    updates: u32,
    clean: u32,
    cooldown: u32,
}
impl Controller {
    pub fn new(ceiling_kbps: u32) -> Self {
        Self {
            ceiling: ceiling_kbps,
            capacity: ceiling_kbps.min(1200),
            updates: 0,
            clean: 0,
            cooldown: 0,
        }
    }
    /// Called once per 500 ms of active media; startup loss is treated separately.
    pub fn update(&mut self, missing: u64, delay: Duration) -> u32 {
        self.updates = self.updates.saturating_add(1);
        self.cooldown = self.cooldown.saturating_sub(1);
        if missing > 0 || delay >= Duration::from_millis(60) {
            self.clean = 0;
            if (missing >= 3 || delay >= Duration::from_millis(80))
                && self.updates > 10
                && self.cooldown == 0
            {
                self.capacity = self.capacity.saturating_sub(100).max(self.ceiling.min(600));
                self.cooldown = 4;
            }
        } else if delay < Duration::from_millis(30) {
            self.clean += 1;
            if self.clean >= 20 {
                self.capacity = (self.capacity + 25).min(self.ceiling);
                self.clean = 0;
            }
        } else {
            self.clean = 0;
        }
        self.capacity * 1000
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transit_delay_uses_device_time_and_survives_reset_and_clock_drift() {
        let start = Instant::now();
        let mut delay = TransitDelay::new(start);
        delay.observe(1_000_000, start);
        delay.observe(1_010_000, start + Duration::from_millis(110));
        assert_eq!(delay.take_peak(), Duration::from_millis(100));
        assert_eq!(delay.take_peak(), Duration::ZERO);
        delay.observe(1_120_000, start + Duration::from_millis(120));
        assert_eq!(delay.take_peak(), Duration::ZERO);
        delay.observe(0, start + Duration::from_secs(1));
        assert_eq!(delay.take_peak(), Duration::ZERO);
        // A small constant rate mismatch ages out instead of growing forever.
        for second in 1..=600 {
            delay.observe(second * 999_900, start + Duration::from_secs(second + 1));
            assert!(delay.take_peak() <= Duration::from_millis(12));
        }
    }
    #[test]
    fn delay_reduces_capacity_before_loss_and_prevents_premature_recovery() {
        let mut c = Controller::new(1400);
        for _ in 0..10 {
            c.update(0, Duration::ZERO);
        }
        assert_eq!(c.update(0, Duration::from_millis(100)), 1_100_000);
        for _ in 0..3 {
            assert_eq!(c.update(100, Duration::from_millis(100)), 1_100_000);
        }
        assert_eq!(c.update(0, Duration::from_millis(100)), 1_000_000);
        for _ in 0..100 {
            c.update(0, Duration::from_millis(40));
        }
        assert_eq!(c.update(0, Duration::from_millis(40)), 1_000_000);
    }
    #[test]
    fn keyframes_cancel_stale_requests_and_retries_are_bounded() {
        let now = Instant::now();
        let mut r = Recovery::new(now);
        r.observe(1, true);
        r.observe(4, false);
        r.observe(7, true);
        assert_eq!(r.request(now), None);
        r.observe(10, false);
        assert_eq!(r.request(now), Some((8, 9)));
        r.observe(13, false);
        assert_eq!(r.request(now + Duration::from_millis(500)), None);
        assert_eq!(r.request(now + Duration::from_secs(2)), Some((11, 12)));
    }
    #[test]
    fn backoff_and_bounded_recovery() {
        let mut c = Controller::new(1400);
        assert_eq!(c.update(100, Duration::ZERO), 1_200_000);
        for _ in 0..20 {
            c.update(0, Duration::ZERO);
        }
        assert_eq!(c.update(8, Duration::ZERO), 1_125_000);
        for _ in 0..1000 {
            c.update(0, Duration::ZERO);
        }
        assert_eq!(c.update(0, Duration::ZERO), 1_400_000);
        for _ in 0..100 {
            c.update(10, Duration::ZERO);
        }
        assert_eq!(c.update(10, Duration::ZERO), 600_000);
    }
    #[test]
    fn low_explicit_ceiling_is_never_exceeded() {
        let mut c = Controller::new(600);
        for _ in 0..100 {
            assert!(c.update(100, Duration::ZERO) <= 600_000);
        }
    }
}
