use crate::wire;
use anyhow::{Context, Result, bail};
use std::{
    collections::VecDeque,
    io::{Read, Write},
    time::{Duration, Instant},
};
enum Link {
    Serial(Box<dyn serialport::SerialPort>),
    Tcp(std::net::TcpStream),
    #[cfg(windows)]
    Fx2(crate::fx2::Device),
}
impl Read for Link {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Serial(port) => port.read(bytes),
            Self::Tcp(stream) => match stream.read(bytes)? {
                0 => Err(std::io::ErrorKind::UnexpectedEof.into()),
                count => Ok(count),
            },
            #[cfg(windows)]
            Self::Fx2(port) => port.read(bytes),
        }
    }
}
impl Write for Link {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Serial(port) => port.write(bytes),
            Self::Tcp(stream) => stream.write(bytes),
            #[cfg(windows)]
            Self::Fx2(port) => port.write(bytes),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Serial(port) => port.flush(),
            Self::Tcp(stream) => stream.flush(),
            #[cfg(windows)]
            Self::Fx2(port) => port.flush(),
        }
    }
}
impl Link {
    fn baud_rate(&self) -> Result<u32> {
        match self {
            Self::Serial(port) => Ok(port.baud_rate()?),
            Self::Tcp(_) => bail!("TCP has no UART baud setting"),
            #[cfg(windows)]
            Self::Fx2(_) => bail!("FX2 has no UART baud setting"),
        }
    }
    fn set_baud_rate(&mut self, rate: u32) -> Result<()> {
        match self {
            Self::Serial(port) => Ok(port.set_baud_rate(rate)?),
            Self::Tcp(_) => bail!("TCP has no UART baud setting"),
            #[cfg(windows)]
            Self::Fx2(_) => bail!("FX2 has no UART baud setting"),
        }
    }
}
pub struct Radio {
    port: Link,
    decoder: wire::Decoder,
    seq: u32,
    pub rx_missing: u64,
    last_rx: Option<u32>,
    pending: VecDeque<(u8, Vec<u8>)>,
    baseline_baud: u32,
    restore_baud: Option<u32>,
    transit: crate::quality::TransitDelay,
}
impl Radio {
    pub fn open(port: &str, baud: u32) -> Result<Self> {
        if let Some(port) = port.strip_prefix("tcp:") {
            let port = port.parse::<u16>().context("expected tcp:<local port>")?;
            let endpoint = std::net::SocketAddr::from(([127, 0, 0, 1], port));
            let stream = std::net::TcpStream::connect_timeout(&endpoint, Duration::from_secs(2))
                .context("connecting to local Wi-Fi bridge tunnel")?;
            stream.set_nodelay(true)?;
            stream.set_read_timeout(Some(Duration::from_millis(2)))?;
            stream.set_write_timeout(Some(Duration::from_secs(1)))?;
            return Ok(Self::from_link(Link::Tcp(stream), baud));
        }
        #[cfg(windows)]
        if port == "fx2" || port.starts_with("fx2:") {
            let path = port.strip_prefix("fx2:").filter(|s| !s.is_empty());
            let link = crate::fx2::Device::open(path)?;
            println!("接続ポート FX2LP USB High Speed");
            return Ok(Self::from_link(Link::Fx2(link), baud));
        }
        let detected;
        let port = if port.eq_ignore_ascii_case("auto") {
            let ports: Vec<_> = serialport::available_ports()?.into_iter().filter(|p| {
                matches!(&p.port_type, serialport::SerialPortType::UsbPort(u) if u.vid == 0x303a && u.pid == 0x1001)
            }).collect();
            anyhow::ensure!(
                ports.len() == 1,
                "native ESP32 USB port is absent or ambiguous; specify --port explicitly"
            );
            detected = ports[0].port_name.clone();
            detected.as_str()
        } else {
            port
        };
        println!("接続ポート {port}");
        let builder = serialport::new(port, baud)
            .dtr_on_open(false)
            .timeout(Duration::from_millis(2));
        #[cfg(windows)]
        let p: Box<dyn serialport::SerialPort> = {
            use std::os::windows::io::AsRawHandle;
            let port = builder.open_native()?;
            unsafe {
                windows::Win32::Devices::Communication::SetupComm(
                    windows::Win32::Foundation::HANDLE(port.as_raw_handle()),
                    256 * 1024,
                    64 * 1024,
                )?;
                // The crate applies one timeout to both directions. USB writes
                // can legitimately take longer than the 2 ms input polling wait.
                windows::Win32::Devices::Communication::SetCommTimeouts(
                    windows::Win32::Foundation::HANDLE(port.as_raw_handle()),
                    &windows::Win32::Devices::Communication::COMMTIMEOUTS {
                        ReadIntervalTimeout: u32::MAX,
                        ReadTotalTimeoutMultiplier: u32::MAX,
                        ReadTotalTimeoutConstant: 2,
                        WriteTotalTimeoutMultiplier: 0,
                        WriteTotalTimeoutConstant: 1000,
                    },
                )?;
            }
            Box::new(port)
        };
        #[cfg(not(windows))]
        let p = builder.open()?;
        Ok(Self::from_link(Link::Serial(p), baud))
    }
    fn from_link(port: Link, baud: u32) -> Self {
        Self {
            port,
            decoder: Default::default(),
            seq: 0,
            rx_missing: 0,
            last_rx: None,
            pending: VecDeque::new(),
            baseline_baud: baud,
            restore_baud: None,
            transit: crate::quality::TransitDelay::new(Instant::now()),
        }
    }
    pub fn send(&mut self, line: &str) -> Result<()> {
        let mut bytes = line.as_bytes().to_vec();
        bytes.push(b'\n');
        self.port
            .write_all(&bytes)
            .context("bridge command write")?;
        Ok(())
    }
    pub fn greet(&mut self) -> Result<String> {
        // No implicit rate changes: the test PC experienced a CH343 driver
        // BSOD during sustained UART traffic. Use only the configured rate.
        for _ in 0..2 {
            self.send("\nhello")?;
            let until = Instant::now() + Duration::from_millis(500);
            while Instant::now() < until {
                for (kind, p) in self.read_frames()? {
                    if kind == wire::TEXT {
                        let s = String::from_utf8_lossy(&p).trim().to_owned();
                        if s.starts_with("ok hello") {
                            return Ok(s);
                        }
                    }
                }
            }
        }
        bail!("bridge hello timeout")
    }
    pub fn switch_baud(&mut self, baud: u32) -> Result<()> {
        anyhow::ensure!(
            (1_000_000..=4_000_000).contains(&baud),
            "unsupported UART rate"
        );
        let old = self.port.baud_rate()?;
        if old == baud {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
        let expected = format!("ok baud {baud}");
        let reply = self.command(&format!("baud {baud}"))?;
        anyhow::ensure!(reply == expected, "unexpected baud acknowledgement");
        self.port.set_baud_rate(baud)?;
        std::thread::sleep(Duration::from_millis(10));
        self.send("hello")?;
        let until = Instant::now() + Duration::from_secs(1);
        while Instant::now() < until {
            for (kind, p) in self.read_frames()? {
                if kind == wire::TEXT && p.starts_with(b"ok hello") {
                    self.restore_baud = Some(self.baseline_baud);
                    println!("UART link {} baud", baud);
                    return Ok(());
                }
            }
        }
        self.port.set_baud_rate(old)?;
        bail!("UART rate change was not confirmed")
    }
    fn read_frames(&mut self) -> Result<Vec<(u8, Vec<u8>)>> {
        let mut b = [0; 16384];
        match self.port.read(&mut b) {
            Ok(n) => Ok(self.decoder.push(&b[..n])),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                Ok(Vec::new())
            }
            Err(e) => Err(e).context("bridge read"),
        }
    }
    pub fn poll(&mut self) -> Result<Vec<(u8, Vec<u8>)>> {
        if !self.pending.is_empty() {
            return Ok(self.pending.drain(..).collect());
        }
        self.read_frames()
    }
    pub fn command(&mut self, line: &str) -> Result<String> {
        self.send(line)?;
        let until = Instant::now() + Duration::from_secs(4);
        while Instant::now() < until {
            let mut reply = None;
            for (kind, p) in self.read_frames()? {
                if kind == wire::TEXT {
                    let s = String::from_utf8_lossy(&p).trim().to_owned();
                    if s.starts_with("err") {
                        bail!("device command rejected")
                    };
                    if s.starts_with("ok") {
                        reply = Some(s);
                    }
                } else {
                    if self.pending.len() == 256 {
                        bail!("device event queue full")
                    };
                    self.pending.push_back((kind, p));
                }
            }
            if let Some(s) = reply {
                return Ok(s);
            }
        }
        bail!("device command timeout")
    }
    pub fn ethernet(&mut self, eth: &[u8]) -> Result<()> {
        self.seq = self.seq.wrapping_add(1);
        self.port
            .write_all(&wire::ethernet_tx(eth, self.seq)?)
            .context("bridge Ethernet write")?;
        Ok(())
    }
    pub fn receive_ethernet<'a>(&mut self, p: &'a [u8]) -> Result<&'a [u8]> {
        let (seq, eth) = wire::ethernet_rx(p)?;
        self.transit
            .observe(u64::from_le_bytes(p[..8].try_into()?), Instant::now());
        if let Some(last) = self.last_rx {
            let delta = seq.wrapping_sub(last);
            if delta > 0 && delta < 0x80000000 {
                self.rx_missing += (delta - 1) as u64;
                self.last_rx = Some(seq);
            }
        } else {
            self.last_rx = Some(seq);
        }
        Ok(eth)
    }
    pub fn take_transit_delay(&mut self) -> Duration {
        self.transit.take_peak()
    }
    pub fn stats(&self) -> String {
        format!(
            "D2H欠番={} CRC不正={} 不正ヘッダ={}",
            self.rx_missing, self.decoder.bad_crc, self.decoder.malformed
        )
    }
}
impl Drop for Radio {
    fn drop(&mut self) {
        for line in ["sta leave", "mode monitor", "stop"] {
            let _ = self.send(line);
        }
        let _ = self.port.flush();
        if let Some(baud) = self.restore_baud.take() {
            std::thread::sleep(Duration::from_millis(100));
            if self.send(&format!("baud {baud}")).is_ok() {
                let expected = format!("ok baud {baud}");
                let until = Instant::now() + Duration::from_secs(1);
                'restore: while Instant::now() < until {
                    if let Ok(frames) = self.read_frames() {
                        for (kind, p) in frames {
                            if kind == wire::TEXT && p == expected.as_bytes() {
                                let _ = self.port.set_baud_rate(baud);
                                break 'restore;
                            }
                        }
                    } else {
                        break;
                    }
                }
            }
        }
    }
}
