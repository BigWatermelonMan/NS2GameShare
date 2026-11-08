//! Virtual IMU model adapted from mizuyoukanao/nxic-pico (MIT, 2026 mizuyoukanao).
//! See licenses/third-party/nxic-pico-MIT.txt. Counts represent angles, not rates.
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseMode {
    #[default]
    RightStick,
    Gyro,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub yaw_counts_per_turn: f64,
    pub pitch_counts_per_turn: f64,
    pub pitch_limit_deg: f64,
    pub invert_yaw: bool,
    pub invert_pitch: bool,
    pub reset_key: u16,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            yaw_counts_per_turn: 8000.0,
            pitch_counts_per_turn: 8000.0,
            pitch_limit_deg: 80.0,
            invert_yaw: false,
            invert_pitch: false,
            reset_key: 0x76,
        }
    }
}
impl Config {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            [self.yaw_counts_per_turn, self.pitch_counts_per_turn]
                .iter()
                .all(|x| x.is_finite() && (500.0..=200000.0).contains(x))
                && self.pitch_limit_deg.is_finite()
                && (5.0..=89.0).contains(&self.pitch_limit_deg)
                && (1..=254).contains(&self.reset_key)
                && ![27, 0x78].contains(&self.reset_key),
            "Invalid mouse gyro settings"
        );
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Pose {
    pub yaw: f64,
    pub pitch: f64,
    pub active: bool,
    pub reset: u64,
}
pub struct Integrator {
    config: Config,
    pose: Pose,
    reset_held: bool,
}
impl Integrator {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            pose: Pose::default(),
            reset_held: false,
        }
    }
    pub fn sample(&mut self, dx: i32, dy: i32, active: bool, reset: bool) -> Pose {
        if !active {
            self.pose.yaw = 0.0;
            self.pose.pitch = 0.0;
            self.pose.active = false;
            self.reset_held = false;
            return self.pose;
        }
        if reset && !self.reset_held {
            self.pose.pitch = 0.0;
            self.pose.yaw = 0.0;
            self.pose.reset = self.pose.reset.wrapping_add(1);
        }
        self.reset_held = reset;
        self.pose.active = true;
        self.pose.yaw += f64::from(dx) / self.config.yaw_counts_per_turn
            * if self.config.invert_yaw { 1.0 } else { -1.0 };
        self.pose.pitch = (self.pose.pitch
            + f64::from(dy) / self.config.pitch_counts_per_turn
                * if self.config.invert_pitch { 1.0 } else { -1.0 })
        .clamp(
            -self.config.pitch_limit_deg / 360.0,
            self.config.pitch_limit_deg / 360.0,
        );
        self.pose
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Sensor {
    pub time_ns: u64, // sample interval; one sample per handle in each feedback batch
    pub acceleration: [f32; 3],
    pub velocity: [f32; 3], // rotations per second, in controller body axes
    pub angle: [f32; 3],    // virtual Euler rotations; game interpretation is experimental
    pub direction: [f32; 9],
}
#[derive(Default, Clone)]
pub struct Encoder {
    pose: Pose,
    last_ns: Option<u64>,
}
impl Encoder {
    pub fn sample(&mut self, target: Option<Pose>, now_ns: u64) -> Option<Sensor> {
        let Some(target) = target else {
            *self = Self::default();
            return None;
        };
        let dt = self
            .last_ns
            .map_or(0.008, |last| now_ns.saturating_sub(last) as f64 / 1e9)
            .clamp(0.001, 0.1);
        self.last_ns = Some(now_ns);
        let mut yaw_rate = 0.0;
        let mut pitch_rate = 0.0;
        if !target.active || target.reset != self.pose.reset {
            self.pose = Pose {
                reset: target.reset,
                ..Pose::default()
            };
        } else {
            // Carry un-emitted angle forward instead of dropping fast movement.
            let max = 2240.0 / 360.0 * dt;
            let dy = (target.yaw - self.pose.yaw).clamp(-max, max);
            let dp = (target.pitch - self.pose.pitch).clamp(-max, max);
            self.pose.yaw += dy;
            self.pose.pitch += dp;
            yaw_rate = dy / dt;
            pitch_rate = dp / dt;
        }
        self.pose.active = target.active;
        let (s, c) = (self.pose.pitch * std::f64::consts::TAU).sin_cos();
        let (sy, cy) = (self.pose.yaw.rem_euclid(1.0) * std::f64::consts::TAU).sin_cos();
        Some(Sensor {
            // Peer batches use 5, 10, ... ms offsets for 5 ms samples. Our batch
            // has one sample per handle, so its offset is this send interval.
            time_ns: (dt * 1e9).round() as u64,
            // GameShare's HID acceleration is opposite the raw Pro Controller IMU convention.
            acceleration: [-s as f32, 0.0, -c as f32],
            velocity: [
                (yaw_rate * s) as f32,
                -pitch_rate as f32,
                (yaw_rate * c) as f32,
            ],
            angle: [0.0, -self.pose.pitch as f32, self.pose.yaw as f32],
            direction: [
                (cy * c) as f32,
                (sy * c) as f32,
                s as f32,
                -sy as f32,
                cy as f32,
                0.0,
                (-cy * s) as f32,
                (-sy * s) as f32,
                c as f32,
            ],
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn single_sample_offsets_use_send_interval_not_session_age() {
        let mut e = Encoder::default();
        let pose = Some(Pose::default());
        assert_eq!(e.sample(pose, 1_000_000_000).unwrap().time_ns, 8_000_000);
        assert_eq!(e.sample(pose, 1_008_000_000).unwrap().time_ns, 8_000_000);
        assert_eq!(e.sample(pose, 1_020_000_000).unwrap().time_ns, 12_000_000);
    }
    #[test]
    fn counts_are_independent_of_mouse_report_rate() {
        let mut a = Integrator::new(Config::default());
        let mut b = Integrator::new(Config::default());
        let whole = a.sample(800, 400, true, false);
        let mut split = Pose::default();
        for _ in 0..100 {
            split = b.sample(8, 4, true, false);
        }
        assert!((whole.yaw - split.yaw).abs() < 1e-12 && (whole.pitch - split.pitch).abs() < 1e-12);
        let mut encoder = Encoder::default();
        let mut s = encoder.sample(Some(whole), 0).unwrap();
        for i in 1..100 {
            s = encoder.sample(Some(whole), i * 8_000_000).unwrap();
        }
        assert!((s.angle[2] + 0.1).abs() < 1e-6);
        assert!(s.velocity.iter().all(|v| v.abs() < 1e-6));
        assert!((s.acceleration.iter().map(|x| x * x).sum::<f32>() - 1.0).abs() < 1e-6);
    }
    #[test]
    fn release_and_reset_do_not_emit_return_rotation() {
        let mut e = Encoder::default();
        e.sample(
            Some(Pose {
                yaw: 0.1,
                pitch: 0.1,
                active: true,
                reset: 0,
            }),
            0,
        );
        let reset = e
            .sample(
                Some(Pose {
                    reset: 1,
                    ..Pose::default()
                }),
                8_000_000,
            )
            .unwrap();
        assert_eq!(reset.velocity, [0.0; 3]);
        assert_eq!(reset.acceleration, [0.0, 0.0, -1.0]);
        assert!(e.sample(None, 9_000_000).is_none());
    }
    #[test]
    fn pitch_stop_discards_outward_motion_and_reverses_immediately() {
        let mut i = Integrator::new(Config::default());
        i.sample(0, -20000, true, false);
        let p = i.sample(0, 1, true, false);
        assert!(p.pitch < 80.0 / 360.0);
    }
}
