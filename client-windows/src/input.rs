//! Latest-state native input sampling; feedback.rs serializes the GameShare records.
use serde::{Deserialize, Serialize};
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub keyboard: Keyboard,
    pub xinput_enabled: bool,
    pub xinput_index: Option<u32>,
    pub xinput_deadzone: [f32; 2],
    pub xinput_buttons: Option<[Option<u8>; 16]>,
    pub directinput: crate::dinput::Mapping,
    pub mouse_enabled: bool,
    pub mouse_capture_key: u16,
    pub mouse_buttons: [Option<u8>; 5],
    pub mouse_sensitivity: f32,
    pub mouse_smoothing_ms: f32,
    pub mouse_mode: crate::motion::MouseMode,
    pub mouse_gyro: crate::motion::Config,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            keyboard: Keyboard::default(),
            xinput_enabled: true,
            xinput_index: None,
            xinput_deadzone: [7849.0 / 32767.0, 8689.0 / 32767.0],
            xinput_buttons: None,
            mouse_enabled: true,
            mouse_capture_key: 0x77,
            mouse_buttons: [Some(9), Some(8), Some(5), Some(6), Some(7)],
            directinput: Default::default(),
            mouse_sensitivity: 0.025,
            mouse_smoothing_ms: 8.0,
            mouse_mode: Default::default(),
            mouse_gyro: Default::default(),
        }
    }
}
impl Config {
    pub fn load(path: Option<&std::path::Path>) -> anyhow::Result<Self> {
        let c: Self = if let Some(path) = path {
            serde_json::from_slice(&std::fs::read(path)?)?
        } else {
            Self::default()
        };
        c.validate()?;
        Ok(c)
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        let c = self;
        c.mouse_gyro.validate()?;
        anyhow::ensure!(
            c.mouse_mode != crate::motion::MouseMode::Gyro
                || !c.mouse_enabled
                || (c.mouse_gyro.reset_key != c.mouse_capture_key
                    && !c
                        .keyboard
                        .buttons
                        .iter()
                        .chain(c.keyboard.left_stick.iter())
                        .flatten()
                        .any(|k| *k == c.mouse_gyro.reset_key)),
            "ジャイロのリセットキーが他の割り当てと重複しています。"
        );
        c.directinput.validate()?;
        c.keyboard.validate()?;
        anyhow::ensure!(
            c.xinput_buttons
                .as_ref()
                .is_none_or(|b| b.iter().flatten().all(|n| *n < 16)),
            "invalid XInput button mapping"
        );
        anyhow::ensure!(
            c.xinput_index.is_none_or(|i| i < 4)
                && c.xinput_deadzone
                    .iter()
                    .all(|n| n.is_finite() && (0.0..1.0).contains(n)),
            "invalid XInput settings"
        );
        anyhow::ensure!(
            valid_key(c.mouse_capture_key)
                && c.mouse_capture_key != 0x78
                && c.mouse_buttons.iter().flatten().all(|b| *b < 16),
            "invalid mouse bindings"
        );
        anyhow::ensure!(
            !c.mouse_enabled
                || !c
                    .keyboard
                    .buttons
                    .iter()
                    .chain(c.keyboard.left_stick.iter())
                    .flatten()
                    .any(|k| *k == c.mouse_capture_key),
            "マウス取り込みキーとゲーム操作キーが重複しています。"
        );
        anyhow::ensure!(
            c.mouse_sensitivity.is_finite()
                && (0.0001..=1.0).contains(&c.mouse_sensitivity)
                && c.mouse_smoothing_ms.is_finite()
                && (1.0..=100.0).contains(&c.mouse_smoothing_ms),
            "invalid mouse settings"
        );
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Keyboard {
    pub buttons: [Option<u16>; 16],
    pub left_stick: [Option<u16>; 4], // Left, Up, Right, Down
}
impl Default for Keyboard {
    fn default() -> Self {
        Self {
            buttons: [
                0x58, 0x5a, 0x53, 0x41, 0, 0, 0x51, 0x57, 0x31, 0x32, 13, 8, 37, 38, 39, 40,
            ]
            .map(|k| if k == 0 { None } else { Some(k) }),
            left_stick: [Some(37), Some(38), Some(39), Some(40)],
        }
    }
}
pub fn valid_key(key: u16) -> bool {
    (1..=254).contains(&key) && key != 27 && key != 0x78
}
impl Keyboard {
    pub fn mask(&self, key: u16) -> u32 {
        self.buttons
            .iter()
            .enumerate()
            .filter(|(_, v)| **v == Some(key))
            .fold(0, |bits, (i, _)| bits | (1 << i))
    }
    fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.buttons
                .iter()
                .chain(self.left_stick.iter())
                .flatten()
                .all(|k| valid_key(*k)),
            "キーには Esc と F9 を使用できません。"
        );
        Ok(())
    }
}
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::{
    KeyboardAndMouse::GetAsyncKeyState,
    XboxController::{XINPUT_STATE, XInputGetState},
};
#[derive(Default, Debug, Clone, Copy, PartialEq, Serialize)]
pub struct State {
    pub motion: Option<crate::motion::Pose>,
    pub buttons: u32,
    pub lx: f32,
    pub ly: f32,
    pub rx: f32,
    pub ry: f32,
    pub controller: bool,
}
fn axis(v: i16, deadzone: i32) -> f32 {
    let n = v as i32;
    if n.abs() <= deadzone {
        0.0
    } else {
        (n.signum() as f32 * ((n.abs() - deadzone) as f32 / (32767 - deadzone) as f32))
            .clamp(-1.0, 1.0)
    }
}
fn pressed(vk: i32) -> bool {
    unsafe { GetAsyncKeyState(vk) < 0 }
}
static KEY_EDGES: AtomicU32 = AtomicU32::new(0);
static KEYBOARD: Mutex<Option<Keyboard>> = Mutex::new(None);
static CAPTURE_KEY: AtomicU32 = AtomicU32::new(0x77);
pub fn mouse_capture_key() -> u32 {
    CAPTURE_KEY.load(Ordering::Relaxed)
}
pub const XINPUT_DEFAULT: [u8; 16] = [1, 0, 3, 2, 4, 5, 6, 7, 10, 11, 12, 13, 14, 15, 8, 9];
fn xinput_buttons(
    buttons: u16,
    left_trigger: u8,
    right_trigger: u8,
    mapping: Option<&[Option<u8>; 16]>,
) -> u32 {
    let masks = [
        0x1000, 0x2000, 0x4000, 0x8000, 0x40, 0x80, 0x100, 0x200, 0x10, 0x20, 4, 1, 8, 2,
    ];
    let mut out = 0;
    for (i, on) in masks
        .into_iter()
        .map(|mask| buttons & mask != 0)
        .chain([left_trigger > 30, right_trigger > 30])
        .enumerate()
    {
        if on && let Some(bit) = mapping.map_or(Some(XINPUT_DEFAULT[i]), |m| m[i]) {
            out |= 1 << bit;
        }
    }
    out
}
/// Preserve a short key press even if keydown and keyup fall between two polls.
pub fn key_down(vk: u32) {
    if let Some(k) = &*KEYBOARD.lock().unwrap() {
        KEY_EDGES.fetch_or(k.mask(vk as u16), Ordering::Relaxed);
    }
}
fn sample(config: &Config, physical: &mut crate::binding::Physical) -> State {
    let mut s = State::default();
    let mut x = XINPUT_STATE::default();
    for i in 0..4 {
        if !config.xinput_enabled || config.xinput_index.is_some_and(|index| index != i) {
            continue;
        }
        if unsafe { XInputGetState(i, &mut x) } == 0 {
            let g = x.Gamepad;
            let masks = [
                0x1000, 0x2000, 0x4000, 0x8000, 0x40, 0x80, 0x100, 0x200, 0x10, 0x20, 4, 1, 8, 2,
            ];
            physical.connected = true;
            for (n, on) in masks
                .into_iter()
                .map(|m| g.wButtons.0 & m != 0)
                .chain([g.bLeftTrigger > 30, g.bRightTrigger > 30])
                .enumerate()
            {
                if on {
                    physical.buttons |= 1 << n;
                }
            }
            s.controller = true;
            s.buttons |= xinput_buttons(
                g.wButtons.0,
                g.bLeftTrigger,
                g.bRightTrigger,
                config.xinput_buttons.as_ref(),
            );
            s.lx = axis(
                g.sThumbLX,
                (config.xinput_deadzone[0] * 32767.0).round() as i32,
            );
            s.ly = axis(
                g.sThumbLY,
                (config.xinput_deadzone[0] * 32767.0).round() as i32,
            );
            s.rx = axis(
                g.sThumbRX,
                (config.xinput_deadzone[1] * 32767.0).round() as i32,
            );
            s.ry = axis(
                g.sThumbRY,
                (config.xinput_deadzone[1] * 32767.0).round() as i32,
            );
            break;
        }
    }
    for (bit, vk) in config.keyboard.buttons.iter().enumerate() {
        if vk.is_some_and(|vk| pressed(vk as i32)) {
            s.buttons |= 1 << bit;
        }
    }
    s.buttons |= KEY_EDGES.swap(0, Ordering::Relaxed);
    let keys = config
        .keyboard
        .left_stick
        .map(|k| k.is_some_and(|k| pressed(k as i32)));
    let horizontal = i32::from(keys[2]) - i32::from(keys[0]);
    let vertical = i32::from(keys[1]) - i32::from(keys[3]);
    if horizontal != 0 {
        s.lx = horizontal as f32
    }
    if vertical != 0 {
        s.ly = vertical as f32
    }
    s
}
pub struct Sampler {
    physical: Arc<Mutex<crate::binding::Snapshot>>,
    pub active: Arc<AtomicBool>,
    latest: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
    events: mpsc::Receiver<(u128, State)>,
}
impl Sampler {
    pub fn start(window: HWND) -> Self {
        Self::start_with_config(window, Config::default())
    }
    pub fn start_with_config(window: HWND, config: Config) -> Self {
        *KEYBOARD.lock().unwrap() = Some(config.keyboard.clone());
        CAPTURE_KEY.store(
            if config.mouse_enabled {
                config.mouse_capture_key as u32
            } else {
                0
            },
            Ordering::Relaxed,
        );
        KEY_EDGES.store(0, Ordering::Relaxed);
        let active = Arc::new(AtomicBool::new(false));
        let latest = Arc::new(Mutex::new(State::default()));
        let physical = Arc::new(Mutex::new(crate::binding::Snapshot::default()));
        let physical_thread = physical.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let (a, l, s) = (active.clone(), latest.clone(), stop.clone());
        let (sender, events) = mpsc::sync_channel(128);
        let window = window.0 as usize;
        let thread = thread::spawn(move || {
            let raw_mapping = config.directinput.clone();
            let mut dinput = if config.directinput.enabled {
                match crate::dinput::Backend::new(
                    HWND(window as *mut _),
                    config.directinput.clone(),
                ) {
                    Ok(d) => Some(d),
                    Err(e) => {
                        eprintln!("DirectInput initialization failed: {e}");
                        None
                    }
                }
            } else {
                None
            };
            let mut mouse =
                crate::mouse::Filter::new(config.mouse_sensitivity, config.mouse_smoothing_ms);
            mouse.set_buttons(config.mouse_buttons);
            mouse.set_mode(
                if config.mouse_enabled {
                    config.mouse_mode
                } else {
                    crate::motion::MouseMode::RightStick
                },
                config.mouse_gyro,
            );
            let mut previous_sample = Instant::now();
            let epoch = Instant::now();
            let mut previous = State::default();
            while !s.load(Ordering::Relaxed) {
                let start = Instant::now();
                let active = a.load(Ordering::Relaxed);
                let mut raw = crate::binding::Snapshot::default();
                let mut value = if active {
                    sample(&config, &mut raw.xinput)
                } else {
                    KEY_EDGES.store(0, Ordering::Relaxed);
                    State::default()
                };
                if let Some(d) = &mut dinput {
                    crate::dinput::merge(&mut value, d.sample(active));
                    raw.directinput = d.physical;
                }
                let allow_raw = active
                    && config.directinput.enabled
                    && dinput
                        .as_ref()
                        .map_or(config.directinput.device.is_none(), |d| d.raw_ds4_allowed());
                raw.directinput.merge(crate::ds4::physical(allow_raw));
                *physical_thread.lock().unwrap() = raw;
                crate::dinput::merge(&mut value, crate::ds4::sample(allow_raw, &raw_mapping));
                let time = Instant::now();
                mouse.sample(&mut value, active, time.duration_since(previous_sample));
                previous_sample = time;
                *l.lock().unwrap() = value;
                if value != previous {
                    let _ = sender.try_send((epoch.elapsed().as_micros(), value));
                    previous = value;
                }
                if let Some(wait) = Duration::from_millis(2).checked_sub(start.elapsed()) {
                    thread::sleep(wait);
                }
            }
        });
        Self {
            physical,
            active,
            latest,
            stop,
            thread: Some(thread),
            events,
        }
    }
    pub fn physical(&self) -> crate::binding::Snapshot {
        *self.physical.lock().unwrap()
    }
    pub fn latest(&self) -> State {
        *self.latest.lock().unwrap()
    }
    pub fn events(&self) -> impl Iterator<Item = (u128, State)> + '_ {
        self.events.try_iter()
    }
}
impl Drop for Sampler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_config_keeps_stick_mode_and_gyro_reset_conflicts_are_checked() {
        let mut config: Config =
            serde_json::from_str(r#"{"mouse_sensitivity":0.03,"directinput":{"buttons":null}}"#)
                .unwrap();
        assert_eq!(config.mouse_mode, crate::motion::MouseMode::RightStick);
        assert!(config.directinput.enabled);
        assert_eq!(
            config.directinput.pov_buttons,
            [Some(12), Some(13), Some(14), Some(15)]
        );
        config.mouse_mode = crate::motion::MouseMode::Gyro;
        config.mouse_gyro.reset_key = config.mouse_capture_key;
        assert!(config.validate().is_err());
        config.mouse_gyro.reset_key = 0x76;
        config.validate().unwrap();
    }
    #[test]
    fn xinput_remapping_supports_disabled_buttons_and_triggers() {
        assert_eq!(xinput_buttons(0x1000, 255, 0, None), (1 << 1) | (1 << 8));
        let mut mapping = XINPUT_DEFAULT.map(Some);
        mapping[0] = Some(10);
        mapping[14] = None;
        assert_eq!(xinput_buttons(0x1000, 255, 0, Some(&mapping)), 1 << 10);
    }
    #[test]
    fn custom_bindings_preserve_shared_keys_and_reserved_keys() {
        let mut k = Keyboard::default();
        k.buttons[0] = Some(0x43);
        k.buttons[1] = Some(0x43);
        assert_eq!(k.mask(0x43), 3);
        assert_eq!(k.mask(0x58), 0);
        k.buttons[1] = None;
        assert_eq!(k.mask(0x43), 1);
        k.buttons[0] = Some(27);
        assert!(k.validate().is_err());
    }
    #[test]
    fn deadzone_and_limits() {
        assert_eq!(axis(7849, 7849), 0.0);
        assert_eq!(axis(-7849, 7849), 0.0);
        assert_eq!(axis(32767, 7849), 1.0);
        assert_eq!(axis(-32768, 7849), -1.0);
        assert!(axis(16000, 7849) > 0.0);
    }
}
