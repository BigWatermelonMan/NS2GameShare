use std::time::{Duration, Instant};
#[derive(Default)]
pub struct Timeline {
    anchor: Option<(u64, Instant)>,
}
impl Timeline {
    pub fn start(&mut self, pts: u64, now: Instant) {
        self.anchor = Some((pts, now));
    }
    pub fn deadline(&self, pts: u64) -> Option<Instant> {
        self.anchor.map(|(base, now)| {
            if pts >= base {
                now + Duration::from_nanos((pts - base).min(5_000_000_000))
            } else {
                now.checked_sub(Duration::from_nanos(base - pts))
                    .unwrap_or(now)
            }
        })
    }
    /// Follow the actual audio queue rather than letting device clock drift accumulate.
    pub fn align(&mut self, pts: u64, now: Instant) {
        let Some(due) = self.deadline(pts) else {
            self.start(pts, now);
            return;
        };
        let shift = if now >= due {
            now.duration_since(due)
        } else {
            due.duration_since(now)
        };
        if shift > Duration::from_millis(100) {
            self.start(pts, now);
            return;
        }
        if let Some((_, anchor)) = &mut self.anchor {
            if now > due {
                *anchor += shift / 16;
            } else {
                *anchor = anchor.checked_sub(shift / 16).unwrap_or(*anchor);
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn audio_anchor_schedules_video() {
        let now = Instant::now();
        let mut t = Timeline::default();
        assert!(t.deadline(100).is_none());
        t.start(1_000_000, now);
        assert_eq!(
            t.deadline(21_000_000),
            Some(now + Duration::from_millis(20))
        );
    }
    #[test]
    fn follows_audio_drift_without_an_abrupt_small_jump() {
        let now = Instant::now();
        let mut t = Timeline::default();
        t.start(1000, now);
        let target = now + Duration::from_millis(20);
        t.align(1000, target);
        assert!(t.deadline(1000).unwrap() < now + Duration::from_millis(2));
        for _ in 0..64 {
            t.align(1000, target);
        }
        assert!(target.duration_since(t.deadline(1000).unwrap()) < Duration::from_millis(1));
    }
}
