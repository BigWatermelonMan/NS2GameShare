use anyhow::{Result, ensure};
use std::{
    fs::File,
    io::{Read, Write},
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
pub struct Reader {
    f: File,
    pub linktype: u32,
}
impl Reader {
    pub fn open(path: &Path) -> Result<Self> {
        let mut f = File::open(path)?;
        let mut h = [0; 24];
        f.read_exact(&mut h)?;
        ensure!(
            h[..4] == [0xd4, 0xc3, 0xb2, 0xa1],
            "only little-endian microsecond pcap supported"
        );
        let linktype = u32::from_le_bytes(h[20..24].try_into()?);
        ensure!(
            linktype == 1 || linktype == 127,
            "unsupported pcap link type"
        );
        Ok(Self { f, linktype })
    }
    pub fn next_packet(&mut self) -> Result<Option<Vec<u8>>> {
        Ok(self.next_timed_packet()?.map(|(_, packet)| packet))
    }
    pub fn next_timed_packet(&mut self) -> Result<Option<(Duration, Vec<u8>)>> {
        let mut h = [0; 16];
        let n = self.f.read(&mut h[..1])?;
        if n == 0 {
            return Ok(None);
        }
        self.f.read_exact(&mut h[1..])?;
        let n = u32::from_le_bytes(h[8..12].try_into()?) as usize;
        ensure!(n <= 65536, "pcap packet too large");
        let mut b = vec![0; n];
        self.f.read_exact(&mut b)?;
        let time = Duration::from_secs(u32::from_le_bytes(h[..4].try_into()?) as u64)
            + Duration::from_micros(u32::from_le_bytes(h[4..8].try_into()?) as u64);
        Ok(Some((time, b)))
    }
}
pub struct Writer {
    f: File,
}
impl Writer {
    pub fn create(path: &Path) -> Result<Self> {
        let mut f = File::create(path)?;
        f.write_all(&[
            0xd4, 0xc3, 0xb2, 0xa1, 2, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 0, 0, 1, 0, 0,
            0,
        ])?;
        Ok(Self { f })
    }
    pub fn packet(&mut self, p: &[u8]) -> Result<()> {
        let t = SystemTime::now().duration_since(UNIX_EPOCH)?;
        self.f.write_all(&(t.as_secs() as u32).to_le_bytes())?;
        self.f.write_all(&t.subsec_micros().to_le_bytes())?;
        self.f.write_all(&(p.len() as u32).to_le_bytes())?;
        self.f.write_all(&(p.len() as u32).to_le_bytes())?;
        self.f.write_all(p)?;
        Ok(())
    }
}
