//! DirectInput 8 backend. COM/device ownership stays on the sampling thread.
use crate::input::State;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    mem::size_of,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Devices::HumanInterfaceDevice::*,
        Foundation::HWND,
        System::{Com::*, LibraryLoader::GetModuleHandleW},
    },
    core::{GUID, Interface},
};
const OPTIONAL: u32 = 0x8000_0000;
// DirectInput's MAKEDIPROP(4) is an integer token, never a dereferenced GUID.
const RANGE_PROPERTY: *const GUID = std::ptr::without_provenance(4);
#[derive(Clone, Debug, Serialize)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub vendor: u16,
    pub product: u16,
    pub profile: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Mapping {
    pub enabled: bool,
    pub pov_buttons: [Option<u8>; 4], // Left, Up, Right, Down; old files keep defaults.
    pub device: Option<String>,
    pub buttons: Option<Vec<Option<u8>>>,
    pub axes: [usize; 4],
    pub invert: [bool; 4],
    pub deadzone: f32,
}
impl Default for Mapping {
    fn default() -> Self {
        Self {
            enabled: true,
            pov_buttons: [Some(12), Some(13), Some(14), Some(15)],
            device: None,
            buttons: None,
            axes: [0, 1, 2, 5],
            invert: [false, true, false, true],
            deadzone: 0.15,
        }
    }
}
impl Mapping {
    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.pov_buttons.iter().flatten().all(|b| *b < 32),
            "Invalid DirectInput POV mapping"
        );
        anyhow::ensure!(
            self.axes.iter().all(|&a| a < 8)
                && self.deadzone.is_finite()
                && (0.0..1.0).contains(&self.deadzone),
            "invalid DirectInput axes/deadzone"
        );
        if let Some(b) = &self.buttons {
            anyhow::ensure!(
                b.len() <= 128 && b.iter().flatten().all(|&b| b < 32),
                "invalid DirectInput button mapping"
            );
        }
        Ok(())
    }
}
#[repr(C)]
#[derive(Clone)]
struct Raw {
    axes: [i32; 8],
    pov: [u32; 4],
    buttons: [u8; 128],
}
impl Default for Raw {
    fn default() -> Self {
        Self {
            axes: [0; 8],
            pov: [u32::MAX; 4],
            buttons: [0; 128],
        }
    }
}
struct Com;
impl Com {
    fn new() -> Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        }
        Ok(Self)
    }
}
impl Drop for Com {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}
unsafe extern "system" fn enumerate(
    info: *mut DIDEVICEINSTANCEW,
    context: *mut std::ffi::c_void,
) -> windows::core::BOOL {
    unsafe {
        if info.is_null() || context.is_null() {
            return false.into();
        }
        let list = &mut *(context as *mut Vec<DIDEVICEINSTANCEW>);
        if list.len() < 32 {
            list.push(*info);
            true.into()
        } else {
            false.into()
        }
    }
}
fn name(b: &[u16]) -> String {
    String::from_utf16_lossy(&b[..b.iter().position(|&v| v == 0).unwrap_or(b.len())])
}
fn info(i: &DIDEVICEINSTANCEW) -> DeviceInfo {
    let vendor = i.guidProduct.data1 as u16;
    let product = (i.guidProduct.data1 >> 16) as u16;
    DeviceInfo {
        id: format!("{:?}", i.guidInstance),
        name: name(&i.tszProductName),
        vendor,
        product,
        profile: if vendor == 0x054c {
            "DualShock 4 / Sony"
        } else if vendor == 0x057e && product == 0x2009 {
            "Switch Pro Controller"
        } else {
            "Generic DirectInput"
        }
        .into(),
    }
}
fn create() -> Result<IDirectInput8W> {
    unsafe {
        let mut raw = std::ptr::null_mut();
        DirectInput8Create(
            GetModuleHandleW(None)?.into(),
            0x0800,
            &IDirectInput8W::IID,
            &mut raw,
            None,
        )?;
        Ok(IDirectInput8W::from_raw(raw))
    }
}
fn devices(di: &IDirectInput8W) -> Result<Vec<DIDEVICEINSTANCEW>> {
    let mut list = Vec::new();
    unsafe {
        di.EnumDevices(
            DI8DEVCLASS_GAMECTRL,
            Some(enumerate),
            &mut list as *mut _ as *mut _,
            DIEDFL_ATTACHEDONLY,
        )?;
    }
    Ok(list)
}
pub fn list() -> Result<Vec<DeviceInfo>> {
    let _com = Com::new()?;
    let di = create()?;
    Ok(devices(&di)?.iter().map(info).collect())
}
struct Device {
    device: IDirectInputDevice8W,
    instance: GUID,
    pub info: DeviceInfo,
    ranges: [(i32, i32); 8],
    acquired: bool,
}
impl Device {
    fn new(di: &IDirectInput8W, i: &DIDEVICEINSTANCEW, window: HWND) -> Result<Self> {
        unsafe {
            let mut d = None;
            di.CreateDevice(&i.guidInstance, &mut d, None)?;
            let device = d.context("DirectInput device missing")?;
            let guids = [
                GUID_XAxis,
                GUID_YAxis,
                GUID_ZAxis,
                GUID_RxAxis,
                GUID_RyAxis,
                GUID_RzAxis,
                GUID_Slider,
                GUID_Slider,
            ];
            let mut objects = Vec::with_capacity(140);
            for (n, g) in guids.iter().enumerate() {
                objects.push(DIOBJECTDATAFORMAT {
                    pguid: g,
                    dwOfs: (n * 4) as u32,
                    dwType: DIDFT_ABSAXIS | ((n as u32) << 8) | OPTIONAL,
                    dwFlags: 0,
                });
            }
            for n in 0..4 {
                objects.push(DIOBJECTDATAFORMAT {
                    pguid: &GUID_POV,
                    dwOfs: 32 + n * 4,
                    dwType: DIDFT_POV | (n << 8) | OPTIONAL,
                    dwFlags: 0,
                });
            }
            for n in 0..128 {
                objects.push(DIOBJECTDATAFORMAT {
                    pguid: &GUID_Button,
                    dwOfs: 48 + n,
                    dwType: DIDFT_BUTTON | (n << 8) | OPTIONAL,
                    dwFlags: 0,
                });
            }
            let mut format = DIDATAFORMAT {
                dwSize: size_of::<DIDATAFORMAT>() as u32,
                dwObjSize: size_of::<DIOBJECTDATAFORMAT>() as u32,
                dwFlags: 1,
                dwDataSize: size_of::<Raw>() as u32,
                dwNumObjs: objects.len() as u32,
                rgodf: objects.as_mut_ptr(),
            };
            device.SetDataFormat(&mut format)?;
            device.SetCooperativeLevel(window, DISCL_FOREGROUND | DISCL_NONEXCLUSIVE)?;
            let mut range = DIPROPRANGE {
                diph: DIPROPHEADER {
                    dwSize: size_of::<DIPROPRANGE>() as u32,
                    dwHeaderSize: size_of::<DIPROPHEADER>() as u32,
                    dwObj: 0,
                    dwHow: DIPH_DEVICE,
                },
                lMin: -32768,
                lMax: 32767,
            };
            // DIPROP_* is MAKEDIPROP(n), a pointer-sized integer sentinel, not a GUID object.
            let configured = device.SetProperty(RANGE_PROPERTY, &mut range.diph).is_ok();
            let mut ranges = [if configured {
                (-32768, 32767)
            } else {
                (0, 65535)
            }; 8];
            for (n, r) in ranges.iter_mut().enumerate() {
                range.diph.dwObj = n as u32 * 4;
                range.diph.dwHow = DIPH_BYOFFSET;
                if device.GetProperty(RANGE_PROPERTY, &mut range.diph).is_ok()
                    && range.lMin < range.lMax
                {
                    *r = (range.lMin, range.lMax);
                }
            }
            Ok(Self {
                device,
                instance: i.guidInstance,
                info: info(i),
                ranges,
                acquired: false,
            })
        }
    }
    fn sample(
        &mut self,
        active: bool,
        mapping: &Mapping,
    ) -> Option<(State, crate::binding::Physical)> {
        unsafe {
            if !active {
                if self.acquired {
                    let _ = self.device.Unacquire();
                    self.acquired = false;
                }
                return None;
            }
            if !self.acquired {
                self.device.Acquire().ok()?;
                self.acquired = true;
            }
            if self.device.Poll().is_err() {
                self.acquired = false;
                self.device.Acquire().ok()?;
                self.acquired = true;
                self.device.Poll().ok()?;
            }
            let mut raw = Raw::default();
            if self
                .device
                .GetDeviceState(size_of::<Raw>() as u32, &mut raw as *mut _ as *mut _)
                .is_err()
            {
                self.acquired = false;
                return None;
            }
            Some((
                map(&raw, &self.ranges, self.info.vendor, mapping),
                physical(&raw, self.info.vendor),
            ))
        }
    }
}
impl Drop for Device {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.Unacquire();
        }
    }
}
pub struct Backend {
    pub physical: crate::binding::Physical,
    raw_ds4_allowed: bool,
    devices: Vec<Device>,
    di: IDirectInput8W,
    window: HWND,
    next_refresh: Instant,
    mapping: Mapping,
    _com: Com,
}
impl Backend {
    pub fn new(window: HWND, mapping: Mapping) -> Result<Self> {
        mapping.validate()?;
        let com = Com::new()?;
        let di = create()?;
        Ok(Self {
            physical: Default::default(),
            raw_ds4_allowed: mapping.device.is_none(),
            devices: vec![],
            di,
            window,
            next_refresh: Instant::now(),
            mapping,
            _com: com,
        })
    }
    pub fn sample(&mut self, active: bool) -> State {
        self.physical = Default::default();
        if Instant::now() >= self.next_refresh {
            self.next_refresh = Instant::now() + Duration::from_secs(1);
            if let Ok(list) = devices(&self.di) {
                let sony: Vec<_> = list
                    .iter()
                    .filter(|i| {
                        let d = info(i);
                        d.vendor == 0x054c && matches!(d.product, 0x05c4 | 0x09cc)
                    })
                    .collect();
                self.raw_ds4_allowed = self.mapping.device.as_ref().is_none_or(|id| {
                    sony.len() == 1
                        && id.eq_ignore_ascii_case(&format!("{:?}", sony[0].guidInstance))
                });
                self.devices
                    .retain(|d| list.iter().any(|i| i.guidInstance == d.instance));
                for i in list {
                    if !self.devices.iter().any(|d| d.instance == i.guidInstance)
                        && self.mapping.device.as_ref().is_none_or(|s| {
                            s.eq_ignore_ascii_case(&format!("{:?}", i.guidInstance))
                        })
                    {
                        match Device::new(&self.di, &i, self.window) {
                            Ok(d) => {
                                eprintln!("DirectInput: {} ({})", d.info.name, d.info.profile);
                                self.devices.push(d);
                            }
                            Err(e) => eprintln!("DirectInput open failed: {e}"),
                        }
                    }
                }
            }
        }
        let mut state = State::default();
        self.devices.retain_mut(|d| {
            if let Some((s, p)) = d.sample(active, &self.mapping) {
                self.physical.merge(p);
                merge(&mut state, s);
                true
            } else if active {
                // USB/Bluetooth reconnection can leave an enumerated instance
                // with a dead handle. Recreate it at the next bounded refresh
                // instead of retrying Acquire forever on the old interface.
                eprintln!("DirectInput reconnect pending: {}", d.info.name);
                false
            } else {
                true // Losing foreground focus is not a disconnection.
            }
        });
        state
    }
    pub fn raw_ds4_allowed(&self) -> bool {
        self.raw_ds4_allowed
    }
}
pub fn merge(a: &mut State, b: State) {
    a.buttons |= b.buttons;
    a.controller |= b.controller;
    for (x, y) in [
        (&mut a.lx, b.lx),
        (&mut a.ly, b.ly),
        (&mut a.rx, b.rx),
        (&mut a.ry, b.ry),
    ] {
        if y.abs() > x.abs() {
            *x = y
        }
    }
}
fn normalize(v: i32, range: (i32, i32), deadzone: f32) -> f32 {
    let span = range.1 as f64 - range.0 as f64;
    if span <= 0.0 {
        return 0.0;
    }
    let v = ((v as f64 - range.0 as f64) / span * 2.0 - 1.0).clamp(-1.0, 1.0) as f32;
    if v.abs() <= deadzone {
        0.0
    } else {
        v.signum() * (v.abs() - deadzone) / (1.0 - deadzone)
    }
}
fn map(raw: &Raw, ranges: &[(i32, i32); 8], vendor: u16, mapping: &Mapping) -> State {
    let mut s = State {
        controller: true,
        ..Default::default()
    };
    // Home/Capture/PS are system buttons, not Npad stick-direction bits 16/17.
    let sony = [3, 1, 0, 2, 6, 7, 8, 9, 11, 10, 4, 5];
    let nintendo = [1, 0, 3, 2, 6, 7, 8, 9, 11, 10, 4, 5];
    for (n, &button) in raw.buttons.iter().enumerate() {
        if button & 0x80 == 0 {
            continue;
        }
        let bit = if let Some(m) = &mapping.buttons {
            m.get(n).copied().flatten()
        } else {
            (if vendor == 0x054c { &sony } else { &nintendo })
                .get(n)
                .copied()
        };
        if let Some(bit) = bit {
            s.buttons |= 1 << bit;
        }
    }
    let mut axes = [0.0; 4];
    for (n, value) in axes.iter_mut().enumerate() {
        let a = mapping.axes[n];
        *value = normalize(raw.axes[a], ranges[a], mapping.deadzone)
            * if mapping.invert[n] { -1.0 } else { 1.0 };
    }
    s.lx = axes[0];
    s.ly = axes[1];
    s.rx = axes[2];
    s.ry = axes[3];
    let pov = raw.pov[0];
    if pov & 0xffff != 0xffff && pov < 36000 {
        let sector = ((pov + 2250) / 4500) % 8;
        for (direction, on) in [
            (1, matches!(sector, 0 | 1 | 7)),
            (2, matches!(sector, 1..=3)),
            (3, matches!(sector, 3..=5)),
            (0, matches!(sector, 5..=7)),
        ] {
            if on && let Some(bit) = mapping.pov_buttons[direction] {
                s.buttons |= 1 << bit;
            }
        }
    }
    s
}
fn ds4_raw(bytes: &[u8]) -> Option<Raw> {
    if bytes.len() < 9 {
        return None;
    }
    let mut raw = Raw {
        axes: [
            bytes[0] as i32,
            bytes[1] as i32,
            bytes[2] as i32,
            bytes[7] as i32,
            bytes[8] as i32,
            bytes[3] as i32,
            128,
            128,
        ],
        ..Default::default()
    };
    for i in 0..4 {
        raw.buttons[i] = if bytes[4] & (0x10 << i) != 0 { 0x80 } else { 0 };
    }
    for i in 0..8 {
        raw.buttons[4 + i] = if bytes[5] & (1 << i) != 0 { 0x80 } else { 0 };
    }
    let hat = bytes[4] & 15;
    raw.buttons[12] = if bytes[6] & 1 != 0 { 0x80 } else { 0 };
    raw.buttons[13] = if bytes[6] & 2 != 0 { 0x80 } else { 0 };
    if hat < 8 {
        raw.pov[0] = u32::from(hat) * 4500;
    }
    Some(raw)
}
pub(crate) fn ds4_common(bytes: &[u8], mapping: &Mapping) -> Option<State> {
    Some(map(&ds4_raw(bytes)?, &[(0, 255); 8], 0x054c, mapping))
}
pub(crate) fn ds4_physical(bytes: &[u8]) -> Option<crate::binding::Physical> {
    Some(physical(&ds4_raw(bytes)?, 0x054c))
}
fn physical(raw: &Raw, vendor: u16) -> crate::binding::Physical {
    let buttons = raw
        .buttons
        .iter()
        .enumerate()
        .filter(|(_, v)| **v & 0x80 != 0)
        .fold(0u128, |bits, (i, _)| bits | (1 << i));
    let mut hat = 0;
    let pov = raw.pov[0];
    if pov < 36000 {
        let sector = ((pov + 2250) / 4500) % 8;
        for (i, on) in [
            matches!(sector, 5..=7),
            matches!(sector, 0 | 1 | 7),
            matches!(sector, 1..=3),
            matches!(sector, 3..=5),
        ]
        .into_iter()
        .enumerate()
        {
            if on {
                hat |= 1 << i;
            }
        }
    }
    crate::binding::Physical {
        buttons,
        hat,
        connected: true,
        vendor,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pov_can_be_assigned_without_changing_physical_capture() {
        let mut raw = Raw::default();
        raw.pov[0] = 27000;
        let mut mapping = Mapping::default();
        mapping.pov_buttons[0] = Some(0);
        assert_eq!(map(&raw, &[(0, 255); 8], 0x054c, &mapping).buttons, 1);
        assert_eq!(physical(&raw, 0x054c).hat, 1);
        mapping.pov_buttons[0] = None;
        assert_eq!(map(&raw, &[(0, 255); 8], 0x054c, &mapping).buttons, 0);
    }
    #[test]
    fn sony_face_buttons_and_hat() {
        let mut r = Raw::default();
        r.buttons[1] = 0x80;
        r.buttons[2] = 0x80;
        r.pov[0] = 4500;
        let s = map(&r, &[(-32768, 32767); 8], 0x054c, &Mapping::default());
        assert_eq!(s.buttons, (1 << 1) | 1 | (1 << 13) | (1 << 14));
    }
    #[test]
    fn signed_axis_deadzone() {
        assert_eq!(normalize(0, (-32768, 32767), 0.15), 0.0);
        assert_eq!(normalize(32767, (-32768, 32767), 0.15), 1.0);
        assert_eq!(normalize(-32768, (-32768, 32767), 0.15), -1.0);
    }
    #[test]
    fn invalid_mapping_rejected() {
        let m = Mapping {
            axes: [0, 1, 8, 5],
            ..Default::default()
        };
        assert!(m.validate().is_err());
    }
}
