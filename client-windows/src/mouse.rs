//! Foreground Raw Input mouse -> right stick. F8 captures; focus loss always releases.
use anyhow::{Result, ensure};
use std::{
    mem::size_of,
    sync::atomic::{AtomicBool, AtomicI32, Ordering},
    time::Duration,
};
use windows::Win32::{
    Foundation::*,
    Graphics::Gdi::ClientToScreen,
    UI::{
        Input::{
            KeyboardAndMouse::{GetAsyncKeyState, ReleaseCapture, SetCapture},
            *,
        },
        WindowsAndMessaging::*,
    },
};
static ENABLED: AtomicBool = AtomicBool::new(false);
static DX: AtomicI32 = AtomicI32::new(0);
static DY: AtomicI32 = AtomicI32::new(0);
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}
pub fn register(window: HWND) -> Result<()> {
    unsafe {
        RegisterRawInputDevices(
            &[RAWINPUTDEVICE {
                usUsagePage: 1,
                usUsage: 2,
                dwFlags: RAWINPUTDEVICE_FLAGS(0),
                hwndTarget: window,
            }],
            size_of::<RAWINPUTDEVICE>() as u32,
        )?;
    }
    Ok(())
}
pub fn toggle(window: HWND) -> Result<()> {
    if enabled() {
        release();
        return Ok(());
    }
    unsafe {
        let mut rect = RECT::default();
        GetClientRect(window, &mut rect)?;
        let mut top = POINT {
            x: rect.left,
            y: rect.top,
        };
        let mut bottom = POINT {
            x: rect.right,
            y: rect.bottom,
        };
        ensure!(
            ClientToScreen(window, &mut top).as_bool()
                && ClientToScreen(window, &mut bottom).as_bool(),
            "mouse capture rectangle unavailable"
        );
        ClipCursor(Some(&RECT {
            left: top.x,
            top: top.y,
            right: bottom.x,
            bottom: bottom.y,
        }))?;
        let _ = SetCapture(window);
        ShowCursor(false);
    }
    DX.store(0, Ordering::Relaxed);
    DY.store(0, Ordering::Relaxed);
    ENABLED.store(true, Ordering::Relaxed);
    Ok(())
}
pub fn release() {
    if ENABLED.swap(false, Ordering::Relaxed) {
        unsafe {
            let _ = ClipCursor(None);
            let _ = ReleaseCapture();
            ShowCursor(true);
        }
    }
    DX.store(0, Ordering::Relaxed);
    DY.store(0, Ordering::Relaxed);
}
pub fn raw(handle: HRAWINPUT) {
    if !enabled() {
        return;
    }
    unsafe {
        let mut data = RAWINPUT::default();
        let mut size = size_of::<RAWINPUT>() as u32;
        let n = GetRawInputData(
            handle,
            RID_INPUT,
            Some(&mut data as *mut _ as *mut _),
            &mut size,
            size_of::<RAWINPUTHEADER>() as u32,
        );
        if n == u32::MAX || data.header.dwType != RIM_TYPEMOUSE.0 {
            return;
        }
        let m = data.data.mouse;
        if m.usFlags.0 & MOUSE_MOVE_ABSOLUTE.0 != 0 {
            return;
        }
        let _ = DX.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
            Some(n.saturating_add(m.lLastX).clamp(-16384, 16384))
        });
        let _ = DY.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
            Some(n.saturating_add(m.lLastY).clamp(-16384, 16384))
        });
    }
}
pub struct Filter {
    mode: crate::motion::MouseMode,
    gyro: crate::motion::Integrator,
    reset_key: u16,
    buttons: [Option<u8>; 5],
    x: f32,
    y: f32,
    sensitivity: f32,
    smoothing_ms: f32,
}
impl Filter {
    pub fn new(sensitivity: f32, smoothing_ms: f32) -> Self {
        Self {
            mode: Default::default(),
            gyro: crate::motion::Integrator::new(Default::default()),
            reset_key: 0x76,
            buttons: [Some(9), Some(8), Some(5), Some(6), Some(7)],
            x: 0.0,
            y: 0.0,
            sensitivity,
            smoothing_ms,
        }
    }
    pub fn set_buttons(&mut self, buttons: [Option<u8>; 5]) {
        self.buttons = buttons;
    }
    pub fn set_mode(&mut self, mode: crate::motion::MouseMode, config: crate::motion::Config) {
        self.mode = mode;
        self.gyro = crate::motion::Integrator::new(config);
        self.reset_key = config.reset_key;
    }
    pub fn sample(&mut self, state: &mut crate::input::State, active: bool, dt: Duration) {
        let captured = active && enabled();
        if self.mode == crate::motion::MouseMode::Gyro {
            let dx = DX.swap(0, Ordering::Relaxed);
            let dy = DY.swap(0, Ordering::Relaxed);
            state.motion = Some(self.gyro.sample(
                dx,
                dy,
                captured,
                unsafe { GetAsyncKeyState(self.reset_key as i32) } < 0,
            ));
        }
        if !active || !enabled() {
            self.x = 0.0;
            self.y = 0.0;
            DX.store(0, Ordering::Relaxed);
            DY.store(0, Ordering::Relaxed);
            return;
        }
        let decay = (-dt.as_secs_f32() * 1000.0 / self.smoothing_ms).exp();
        if self.mode == crate::motion::MouseMode::RightStick {
            self.x = (self.x * decay + DX.swap(0, Ordering::Relaxed) as f32 * self.sensitivity)
                .clamp(-1.0, 1.0);
            self.y = (self.y * decay - DY.swap(0, Ordering::Relaxed) as f32 * self.sensitivity)
                .clamp(-1.0, 1.0);
            if self.x.abs() < 0.005 {
                self.x = 0.0
            }
            if self.y.abs() < 0.005 {
                self.y = 0.0
            }
            if self.x.abs() > state.rx.abs() {
                state.rx = self.x
            }
            if self.y.abs() > state.ry.abs() {
                state.ry = self.y
            }
        }
        for (vk, bit) in [1, 2, 4, 5, 6].into_iter().zip(self.buttons) {
            if let Some(bit) = bit
                && unsafe { GetAsyncKeyState(vk) } < 0
            {
                state.buttons |= 1 << bit;
            }
        }
    }
}
