//! DS4 Raw Input fallback for Bluetooth full reports not exposed by DirectInput.
//! Report layouts / Bluetooth input CRC: Sony's Linux hid-playstation driver.
use crate::{dinput, input::State};
use anyhow::Result;
use std::{
    collections::BTreeMap,
    mem::size_of,
    sync::{
        Mutex,
        atomic::{AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};
use windows::Win32::{Foundation::HWND, UI::Input::*};

static DEVICES: Mutex<BTreeMap<usize, bool>> = Mutex::new(BTreeMap::new());
static LATEST: Mutex<Option<(Instant, Vec<u8>)>> = Mutex::new(None);
static TRACE_REPORTS: AtomicU32 = AtomicU32::new(0);

pub fn register(window: HWND) -> Result<()> {
    let devices = [4, 5].map(|usage| RAWINPUTDEVICE {
        usUsagePage: 1,
        usUsage: usage,
        dwFlags: RIDEV_DEVNOTIFY,
        hwndTarget: window,
    });
    unsafe {
        RegisterRawInputDevices(&devices, size_of::<RAWINPUTDEVICE>() as u32)?;
    }
    Ok(())
}
pub fn clear() {
    *LATEST.lock().unwrap() = None;
}
pub fn changed() {
    DEVICES.lock().unwrap().clear();
    clear();
}
pub fn raw(handle: HRAWINPUT) {
    unsafe {
        let header_size = size_of::<RAWINPUTHEADER>() as u32;
        let mut size = 0;
        if GetRawInputData(handle, RID_INPUT, None, &mut size, header_size) == u32::MAX
            || !(header_size + 8..=65536).contains(&size)
        {
            return;
        }
        // RAWINPUT requires pointer alignment; a byte Vec alone does not provide it.
        let mut storage = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
        let n = GetRawInputData(
            handle,
            RID_INPUT,
            Some(storage.as_mut_ptr().cast()),
            &mut size,
            header_size,
        );
        if n == u32::MAX || n != size {
            return;
        }
        let header = &*storage.as_ptr().cast::<RAWINPUTHEADER>();
        if header.dwType != RIM_TYPEHID.0 {
            return;
        }
        let key = header.hDevice.0 as usize;
        let sony = *DEVICES.lock().unwrap().entry(key).or_insert_with(|| {
            let mut info = RID_DEVICE_INFO {
                cbSize: size_of::<RID_DEVICE_INFO>() as u32,
                ..Default::default()
            };
            let mut count = info.cbSize;
            if GetRawInputDeviceInfoW(
                Some(header.hDevice),
                RIDI_DEVICEINFO,
                Some((&mut info as *mut RID_DEVICE_INFO).cast()),
                &mut count,
            ) == u32::MAX
                || info.dwType != RIM_TYPEHID
            {
                return false;
            }
            let hid = info.Anonymous.hid;
            let matched = hid.dwVendorId == 0x054c && matches!(hid.dwProductId, 0x05c4 | 0x09cc);
            if matched {
                eprintln!("DS4 Raw HID connected (USB/Bluetooth)");
            }
            matched
        });
        if !sony {
            return;
        }
        let bytes = std::slice::from_raw_parts(storage.as_ptr().cast::<u8>(), n as usize);
        let offset = header_size as usize;
        let report_size =
            u32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        let count = u32::from_ne_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let reports = &bytes[offset + 8..];
        if TRACE_REPORTS.fetch_add(1, Ordering::Relaxed) < 3 {
            eprintln!(
                "DS4 Raw HID size={report_size} count={count} report={:02x}",
                reports.first().copied().unwrap_or(0)
            );
        }
        if !(10..=4096).contains(&report_size)
            || count == 0
            || count
                .checked_mul(report_size)
                .is_none_or(|n| n > reports.len())
        {
            return;
        }
        for report in reports.chunks_exact(report_size).take(count) {
            if parse(report, &dinput::Mapping::default()).is_some() {
                *LATEST.lock().unwrap() = Some((Instant::now(), report.to_vec()));
            }
        }
    }
}
pub fn sample(active: bool, mapping: &dinput::Mapping) -> State {
    if !active {
        return State::default();
    }
    LATEST
        .lock()
        .unwrap()
        .as_ref()
        .filter(|(time, _)| time.elapsed() <= Duration::from_millis(100))
        .and_then(|(_, report)| parse(report, mapping))
        .unwrap_or_default()
}
fn crc32(bytes: impl Iterator<Item = u8>) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}
fn common(report: &[u8]) -> Option<&[u8]> {
    let offset = match report.first()? {
        1 if (10..=4096).contains(&report.len()) => 1,
        0x11 if (78..=4096).contains(&report.len()) => {
            // Raw Input pads to the descriptor's maximum report size (547B
            // on the observed BT DS4). Its actual report and CRC are 78B.
            let expected = u32::from_le_bytes(report[74..78].try_into().ok()?);
            if crc32(std::iter::once(0xa1).chain(report[..74].iter().copied())) != expected {
                return None;
            }
            3
        }
        _ => return None,
    };
    report.get(offset..offset + 9)
}
fn parse(report: &[u8], mapping: &dinput::Mapping) -> Option<State> {
    dinput::ds4_common(common(report)?, mapping)
}
pub fn physical(active: bool) -> crate::binding::Physical {
    if !active {
        return Default::default();
    }
    LATEST
        .lock()
        .unwrap()
        .as_ref()
        .filter(|(time, _)| time.elapsed() <= Duration::from_millis(100))
        .and_then(|(_, report)| dinput::ds4_physical(common(report)?))
        .unwrap_or_default()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn raw_capture_ignores_game_mapping_and_bt_padding() {
        let mut report = vec![0; 547];
        report[0] = 0x11;
        report[3..12].copy_from_slice(&[128, 128, 128, 128, 0x28, 0, 0, 0, 0]);
        let crc = crc32(std::iter::once(0xa1).chain(report[..74].iter().copied()));
        report[74..78].copy_from_slice(&crc.to_le_bytes());
        let physical = dinput::ds4_physical(common(&report).unwrap()).unwrap();
        assert_eq!(physical.buttons, 2);
        assert_eq!(physical.hat, 0);
        let mapping = dinput::Mapping {
            buttons: Some(vec![None; 128]),
            ..Default::default()
        };
        assert_eq!(parse(&report, &mapping).unwrap().buttons, 0);
        assert_eq!(physical.vendor, 0x054c);
    }
    #[test]
    fn usb_and_bluetooth_have_identical_controls_and_crc_is_checked() {
        let common = [255, 128, 128, 0, 0x20 | 1, 0x53, 0, 0, 0];
        let mut usb = vec![1];
        usb.extend_from_slice(&common);
        let mut bt = vec![0; 78];
        bt[0] = 0x11;
        bt[3..12].copy_from_slice(&common);
        let crc = crc32(std::iter::once(0xa1).chain(bt[..74].iter().copied()));
        bt[74..].copy_from_slice(&crc.to_le_bytes());
        let s = parse(&usb, &dinput::Mapping::default()).unwrap();
        assert_eq!(parse(&bt, &dinput::Mapping::default()), Some(s));
        bt.resize(547, 0);
        assert_eq!(parse(&bt, &dinput::Mapping::default()), Some(s));
        assert_eq!(s.lx, 1.0);
        assert_eq!(s.ry, 1.0);
        assert_ne!(s.buttons & ((1 << 1) | (1 << 13) | (1 << 14)), 0);
        bt[4] ^= 1;
        assert!(parse(&bt, &dinput::Mapping::default()).is_none());
        assert!(parse(&usb[..9], &dinput::Mapping::default()).is_none());
        assert_eq!(crc32(b"123456789".iter().copied()), 0xcbf4_3926);
    }
    #[test]
    fn custom_mapping_and_stale_release_are_preserved() {
        let mut report = vec![1, 128, 128, 128, 128, 0x20 | 8, 0, 0, 0, 0];
        let m = dinput::Mapping {
            buttons: Some(vec![None, Some(10)]),
            ..Default::default()
        };
        assert_eq!(parse(&report, &m).unwrap().buttons, 1 << 10);
        report[5] = 8;
        assert_eq!(parse(&report, &m).unwrap().buttons, 0);
        *LATEST.lock().unwrap() = Some((Instant::now() - Duration::from_secs(1), report));
        assert_eq!(sample(true, &m), State::default());
        clear();
    }
}
