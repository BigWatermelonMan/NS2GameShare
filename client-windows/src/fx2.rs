//! Native WinUSB FX2LP link. RAM loading uses Cypress request A0, never EEPROM.
use anyhow::{Context, Result, ensure};
use std::{io, mem::size_of, path::Path};
use windows::{
    Win32::{
        Devices::{DeviceAndDriverInstallation::*, Usb::*},
        Foundation::*,
        Storage::FileSystem::*,
    },
    core::{GUID, PCWSTR},
};
const INTERFACE: GUID = GUID::from_u128(0x3dfb6a74_cb5c_4a90_ae41_b47f6d3d6947);
const USB_INTERFACE: GUID = GUID::from_u128(0xa5dcbf10_6530_11d2_901f_00c04fb951ed);
struct DeviceSet(HDEVINFO);
impl Drop for DeviceSet {
    fn drop(&mut self) {
        unsafe {
            let _ = SetupDiDestroyDeviceInfoList(self.0);
        }
    }
}
pub fn paths() -> Result<Vec<String>> {
    let mut result = Vec::new();
    for guid in [INTERFACE, USB_INTERFACE] {
        unsafe {
            let set = DeviceSet(SetupDiGetClassDevsW(
                Some(&guid),
                PCWSTR::null(),
                None,
                DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
            )?);
            for index in 0..4096 {
                let mut interface = SP_DEVICE_INTERFACE_DATA {
                    cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
                    ..Default::default()
                };
                if let Err(e) =
                    SetupDiEnumDeviceInterfaces(set.0, None, &guid, index, &mut interface)
                {
                    if e.code() == windows::core::HRESULT::from_win32(ERROR_NO_MORE_ITEMS.0) {
                        break;
                    }
                    return Err(e.into());
                }
                let mut required = 0;
                let _ = SetupDiGetDeviceInterfaceDetailW(
                    set.0,
                    &interface,
                    None,
                    0,
                    Some(&mut required),
                    None,
                );
                ensure!((8..=65536).contains(&required), "invalid USB path length");
                // u64 allocation provides the alignment required by SetupAPI.
                let mut storage = vec![0u64; (required as usize).div_ceil(8)];
                let detail = storage
                    .as_mut_ptr()
                    .cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
                (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
                SetupDiGetDeviceInterfaceDetailW(
                    set.0,
                    &interface,
                    Some(detail),
                    required,
                    None,
                    None,
                )?;
                let start = (*detail).DevicePath.as_ptr();
                let maximum = (required as usize - 4) / 2;
                let slice = std::slice::from_raw_parts(start, maximum);
                let end = slice
                    .iter()
                    .position(|v| *v == 0)
                    .context("unterminated USB path")?;
                let path = String::from_utf16(&slice[..end])?;
                if path.to_ascii_lowercase().contains("vid_04b4&pid_8613") {
                    result.push(path);
                }
            }
        }
    }
    let mut identities = std::collections::BTreeSet::new();
    // The custom and generic interface GUIDs can refer to the same USB device.
    // Prefer the custom interface already collected in the first pass.
    result.retain(|path| {
        identities.insert(
            path.rsplit_once('#')
                .map_or(path.as_str(), |(device, _)| device)
                .to_ascii_lowercase(),
        )
    });
    result.sort();
    Ok(result)
}
pub struct Device {
    file: HANDLE,
    usb: WINUSB_INTERFACE_HANDLE,
}
impl Device {
    pub fn open_raw(path: Option<&str>) -> Result<Self> {
        let detected;
        let path = if let Some(path) = path {
            path
        } else {
            detected = paths()?;
            ensure!(
                detected.len() == 1,
                "FX2 absent or ambiguous; list with fx2-list and specify --device"
            );
            &detected[0]
        };
        ensure!(
            path.to_ascii_lowercase().contains("vid_04b4&pid_8613"),
            "device must be FX2LP 04b4:8613"
        );
        let wide: Vec<_> = path.encode_utf16().chain(Some(0)).collect();
        unsafe {
            let file = CreateFileW(
                PCWSTR(wide.as_ptr()),
                GENERIC_READ.0 | GENERIC_WRITE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                None,
            )
            .context("FX2 open failed; this device requires WinUSB")?;
            let mut usb = WINUSB_INTERFACE_HANDLE::default();
            if let Err(e) = WinUsb_Initialize(file, &mut usb) {
                let _ = CloseHandle(file);
                return Err(e).context("FX2 requires WinUSB driver binding");
            }
            let device = Self { file, usb };
            let timeout = 1000u32;
            WinUsb_SetPipePolicy(
                device.usb,
                0,
                PIPE_TRANSFER_TIMEOUT,
                4,
                (&timeout as *const u32).cast(),
            )?;
            Ok(device)
        }
    }
    pub fn open(path: Option<&str>) -> Result<Self> {
        let mut device = Self::open_raw(path)?;
        let mut status = [0; 8];
        let n = device.control(0xc0, 0xb0, 0, 0, &mut status)?;
        ensure!(
            n == 8 && status[..5] == *b"B5FX\x01",
            "FX2 bridge firmware missing; load its RAM image first"
        );
        ensure!(
            status[5] == 1,
            "FX2 requires USB High Speed; check cable and hub"
        );
        ensure!(status[6] == 1, "FX2 USB configuration is inactive");
        let mut found = [false; 2];
        unsafe {
            for index in 0..2 {
                let mut pipe = WINUSB_PIPE_INFORMATION::default();
                WinUsb_QueryPipe(device.usb, 0, index, &mut pipe)?;
                ensure!(
                    pipe.PipeType == UsbdPipeTypeBulk && pipe.MaximumPacketSize == 512,
                    "unexpected FX2 pipe configuration"
                );
                let slot = match pipe.PipeId {
                    2 => 0,
                    0x86 => 1,
                    _ => anyhow::bail!("unexpected FX2 endpoint"),
                };
                found[slot] = true;
                let timeout = if slot == 1 { 20u32 } else { 1000u32 };
                WinUsb_SetPipePolicy(
                    device.usb,
                    pipe.PipeId,
                    PIPE_TRANSFER_TIMEOUT,
                    4,
                    (&timeout as *const u32).cast(),
                )?;
            }
        }
        ensure!(found == [true, true], "FX2 endpoints 02/86 required");
        Ok(device)
    }
    pub fn diagnostics(&mut self) -> Result<()> {
        let mut bytes = [0; 24];
        self.control(0xc0, 0xb1, 0, 0, &mut bytes)?;
        println!(
            "FX2 CPUCS={:02x} IFCONFIG={:02x} EP2CS={:02x} EP6CS={:02x} IOA={:02x} OEA={:02x} PINFLAGSAB={:02x} POL={:02x} uart_bytes={} out_packets={}",
            bytes[0],
            bytes[1],
            bytes[2],
            bytes[3],
            bytes[4],
            bytes[5],
            bytes[6],
            bytes[7],
            u16::from_le_bytes([bytes[8], bytes[9]]),
            u16::from_le_bytes([bytes[10], bytes[11]])
        );
        println!(
            "FX2 EP6 byte count={} flags={:02x} FD={:02x}",
            u16::from_be_bytes([bytes[12] & 15, bytes[13]]),
            bytes[14],
            bytes[15]
        );
        println!(
            "FX2 EP6FIFOCFG={:02x} AUTOINLEN={} EP6CFG={:02x}",
            bytes[16],
            u16::from_be_bytes([bytes[17], bytes[18]]),
            bytes[19]
        );
        println!("FX2 RDY={:02x} PORTACFG={:02x}", bytes[20], bytes[21]);
        Ok(())
    }
    fn control(
        &mut self,
        kind: u8,
        request: u8,
        value: u16,
        index: u16,
        bytes: &mut [u8],
    ) -> Result<usize> {
        ensure!(
            bytes.len() <= u16::MAX as usize,
            "USB control transfer too large"
        );
        let setup = WINUSB_SETUP_PACKET {
            RequestType: kind,
            Request: request,
            Value: value,
            Index: index,
            Length: bytes.len() as u16,
        };
        let mut count = 0;
        unsafe {
            WinUsb_ControlTransfer(self.usb, setup, Some(bytes), Some(&mut count), None)?;
        }
        ensure!(count as usize == bytes.len(), "short USB control transfer");
        Ok(count as usize)
    }
}
impl Drop for Device {
    fn drop(&mut self) {
        unsafe {
            let _ = WinUsb_Free(self.usb);
            let _ = CloseHandle(self.file);
        }
    }
}
impl io::Read for Device {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let n = bytes.len().min(512);
        let mut count = 0;
        match unsafe {
            WinUsb_ReadPipe(
                self.usb,
                0x86,
                Some(&mut bytes[..n]),
                Some(&mut count),
                None,
            )
        } {
            Ok(()) => Ok(count as usize),
            Err(e)
                if e.code() == windows::core::HRESULT::from_win32(ERROR_SEM_TIMEOUT.0)
                    || e.code() == windows::core::HRESULT::from_win32(ERROR_TIMEOUT.0) =>
            {
                Err(io::ErrorKind::TimedOut.into())
            }
            Err(e) => Err(io::Error::other(e.to_string())),
        }
    }
}
impl io::Write for Device {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut count = 0;
        unsafe {
            WinUsb_WritePipe(
                self.usb,
                2,
                &bytes[..bytes.len().min(512)],
                Some(&mut count),
                None,
            )
        }
        .map_err(|e| io::Error::other(e.to_string()))?;
        Ok(count as usize)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
pub fn load(path: &Path, device: Option<&str>) -> Result<()> {
    let records = crate::ihex::parse(&std::fs::read_to_string(path)?)?;
    let mut device = Device::open_raw(device)?;
    device.control(0x40, 0xa0, 0xe600, 0, &mut [1])?;
    let mut total = 0;
    // A failed write leaves the CPU halted; unplugging restores EEPROM/ROM boot.
    for (address, data) in records {
        for (index, chunk) in data.chunks(256).enumerate() {
            let mut chunk = chunk.to_vec();
            device.control(0x40, 0xa0, address + (index * 256) as u16, 0, &mut chunk)?;
            total += chunk.len();
        }
    }
    device.control(0x40, 0xa0, 0xe600, 0, &mut [0])?;
    println!("FX2 RAM firmware loaded: {total} bytes; EEPROM unchanged");
    Ok(())
}
pub fn probe(seconds: u64, bench: bool, raw: Option<&Path>) -> Result<()> {
    use std::{
        io::{Read, Write},
        time::{Duration, Instant},
    };
    ensure!((1..=30).contains(&seconds), "probe seconds must be 1..30");
    let mut device = Device::open(None)?;
    let mut raw = raw
        .map(std::fs::File::create)
        .transpose()?
        .map(io::BufWriter::new);
    device.diagnostics()?;
    device.write_all(b"\nhello\n")?;
    if bench {
        device.write_all(b"bench 1000\n")?;
    }
    let until = Instant::now() + Duration::from_secs(seconds);
    let mut decoder = crate::wire::Decoder::default();
    let mut received = 0;
    let mut blocks = 0;
    while Instant::now() < until {
        let mut bytes = [0; 512];
        match device.read(&mut bytes) {
            Ok(n) => {
                if let Some(raw) = &mut raw {
                    raw.write_all(&bytes[..n])?;
                }
                received += n;
                if blocks < 4 {
                    println!("FIFO {n}B {:02x?}", &bytes[..n.min(48)]);
                }
                blocks += 1;
                for (kind, payload) in decoder.push(&bytes[..n]) {
                    if kind == crate::wire::TEXT || kind == crate::wire::EVENT {
                        println!("frame type={kind} {}", String::from_utf8_lossy(&payload));
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::TimedOut => {}
            Err(e) => return Err(e.into()),
        }
    }
    println!(
        "FIFO total={received} CRC={} headers={}",
        decoder.bad_crc, decoder.malformed
    );
    if let Some(raw) = &mut raw {
        raw.flush()?;
    }
    device.diagnostics()?;
    Ok(())
}
