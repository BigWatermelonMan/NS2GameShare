//! Bounded decoder for the observed LSP/MoLive2 bit-level fragment headers.
//! Lengths are read from headers; transport footers are never spliced into video.
use anyhow::{Context, Result, ensure};
use std::collections::BTreeMap;
const MAX_RECORD: usize = 4 * 1024 * 1024;
struct Bits<'a> {
    data: &'a [u8],
    bit: usize,
}
impl<'a> Bits<'a> {
    fn new(data: &'a [u8], pos: usize) -> Self {
        Self { data, bit: pos * 8 }
    }
    fn read(&mut self, n: usize) -> Result<u64> {
        ensure!(
            n <= 64
                && self
                    .bit
                    .checked_add(n)
                    .is_some_and(|x| x <= self.data.len() * 8),
            "truncated LSP bit field"
        );
        let mut v = 0;
        for _ in 0..n {
            v = (v << 1) | u64::from((self.data[self.bit / 8] >> (7 - self.bit % 8)) & 1);
            self.bit += 1;
        }
        Ok(v)
    }
    fn uint(&mut self) -> Result<u32> {
        let mut n = 1;
        while self.read(1)? == 0 {
            n += 1;
            ensure!(n <= 32, "LSP integer exceeds limit");
        }
        Ok(self.read(n)? as u32)
    }
    fn delta(&mut self) -> Result<i64> {
        let negative = self.read(1)? != 0;
        let mut n = 1;
        while self.read(1)? == 0 {
            n += 1;
            ensure!(n <= 18, "LSP time delta exceeds limit");
        }
        let v = self.read(26 + 2 * n)?;
        ensure!(v <= i64::MAX as u64, "LSP time overflow");
        Ok(if negative { -(v as i64) } else { v as i64 })
    }
    fn byte(&self) -> usize {
        self.bit.div_ceil(8)
    }
}
fn var(data: &[u8], pos: &mut usize) -> Result<u32> {
    let mut v = 0u32;
    for _ in 0..4 {
        let q = *data.get(*pos).context("truncated LSP extension integer")?;
        *pos += 1;
        v = (v << 7) | u32::from(q & 127);
        if q & 128 == 0 {
            return Ok(v);
        }
    }
    anyhow::bail!("LSP extension integer exceeds limit")
}
pub struct Fragment<'a> {
    pub stream: u32,
    pub final_part: bool,
    pub kind: Option<u32>,
    pub delta: Option<i64>,
    pub data: &'a [u8],
}
pub struct Packet<'a> {
    pub sequence: u32,
    pub connection_time: Option<u64>,
    pub fragments: Vec<Fragment<'a>>,
}
pub fn decode(data: &[u8]) -> Result<Packet<'_>> {
    ensure!(
        data.len() >= 16 && data[..2] == [0xec, 0xc2],
        "not an LSP datagram"
    );
    let sequence = (u32::from(data[2]) << 16) | (u32::from(data[3]) << 8) | u32::from(data[4]);
    // The BE16 length at byte 8 describes the inner transport record,
    // including its eight-byte header. An eight-byte outer prefix precedes
    // it and an eight-byte timestamp follows it. Full-sized packets need
    // not have a zero terminator: parsing into that timestamp discards an
    // otherwise complete datagram (and every dependent video reference).
    let record_len = usize::from(u16::from_be_bytes([data[8], data[9]]));
    ensure!(
        record_len >= 5 && data.len() == record_len + 16,
        "LSP transport length mismatch"
    );
    let data = &data[..8 + record_len];
    let mut pos = 12;
    let mut connection_time = None;
    if data.get(pos..pos + 2) == Some(b"L2") {
        ensure!(data.len() >= pos + 14, "truncated LSP connection header");
        let check = u16::from_be_bytes(data[pos + 2..pos + 4].try_into()?);
        let time = u64::from_be_bytes(data[pos + 4..pos + 12].try_into()?);
        let time = time & 0x7fff_ffff_ffff_ffff;
        let folded = (time ^ (time >> 16) ^ (time >> 32) ^ (time >> 48)) as u16;
        ensure!(check ^ folded == 0xaaaa, "LSP connection checksum mismatch");
        connection_time = Some(time);
        pos += 14;
        for i in 0..64 {
            let id = var(data, &mut pos)?;
            let n = var(data, &mut pos)? as usize;
            pos = pos.checked_add(n).context("LSP extension overflow")?;
            ensure!(pos <= data.len(), "truncated LSP extension");
            if id == 0 {
                break;
            }
            ensure!(i < 63, "too many LSP extensions");
        }
    }
    let flags = *data.get(pos).context("LSP packet flags missing")?;
    pos += 1;
    if flags & 2 != 0 {
        pos += 2;
    }
    ensure!(pos <= data.len(), "LSP sequence missing");
    let mut fragments = Vec::new();
    while pos < data.len() && data[pos] != 0 {
        ensure!(fragments.len() < 256, "too many LSP fragments");
        let mut bits = Bits::new(data, pos);
        let stream = bits.uint()?;
        ensure!(stream < 32, "unknown LSP stream");
        let final_part = bits.read(1)? != 0;
        let (kind, delta) = if final_part {
            (Some(bits.uint()?), Some(bits.delta()?))
        } else {
            (None, None)
        };
        let size = bits.read(13)? as usize + 1;
        pos = bits.byte();
        let end = pos.checked_add(size).context("LSP fragment overflow")?;
        let payload = data.get(pos..end).context("truncated LSP fragment")?;
        fragments.push(Fragment {
            stream,
            final_part,
            kind,
            delta,
            data: payload,
        });
        pos = end;
    }
    Ok(Packet {
        sequence,
        connection_time,
        fragments,
    })
}
#[derive(Debug)]
pub struct MediaUnit {
    pub stream: u32,
    pub kind: u32,
    pub sequence: u64,
    pub timestamps: Option<(u64, u64)>,
    pub payload: Vec<u8>,
}
#[derive(Default)]
pub struct Reassembler {
    streams: BTreeMap<u32, Vec<u8>>,
    last: Option<u32>,
    pub gaps: u64,
    pub incomplete: u64,
    pub duplicates: u64,
    pub invalid: u64,
    pub malformed: u64,
}
impl Reassembler {
    /// Live UDP corruption is a lost datagram, not a fatal session error.
    /// Discard all partial records so later fragments cannot splice across it.
    pub fn push_resilient(&mut self, data: &[u8]) -> Vec<MediaUnit> {
        match self.push(data) {
            Ok(units) => units,
            Err(_) => {
                self.malformed += 1;
                self.incomplete += self.streams.values().filter(|s| !s.is_empty()).count() as u64;
                self.streams.clear();
                Vec::new()
            }
        }
    }
    pub fn push(&mut self, data: &[u8]) -> Result<Vec<MediaUnit>> {
        let packet = decode(data)?;
        if let Some(last) = self.last {
            let diff = packet.sequence.wrapping_sub(last) & 0xffffff;
            if diff == 0 || diff > 0x800000 {
                self.duplicates += 1;
                return Ok(vec![]);
            }
            if diff != 1 {
                self.gaps += u64::from(diff - 1);
                self.incomplete += self.streams.values().filter(|s| !s.is_empty()).count() as u64;
                self.streams.clear();
            }
        }
        self.last = Some(packet.sequence);
        let mut out = Vec::new();
        for f in packet.fragments {
            let b = self.streams.entry(f.stream).or_default();
            ensure!(
                b.len() + f.data.len() <= MAX_RECORD,
                "LSP record exceeds limit"
            );
            b.extend_from_slice(f.data);
            if f.final_part {
                match tlv(f.stream, f.kind.unwrap_or(0), b) {
                    Ok(unit) => out.push(unit),
                    Err(_) => self.invalid += 1,
                }
                b.clear();
            }
        }
        Ok(out)
    }
}
fn tlv(stream: u32, kind: u32, b: &[u8]) -> Result<MediaUnit> {
    ensure!(
        b.len() >= 8
            && b[..4] == *b"Tlv0"
            && u32::from_be_bytes(b[4..8].try_into()?) as usize == b.len(),
        "incomplete MoLive record"
    );
    let mut pos = 8;
    let mut payload = None;
    let mut timestamps = None;
    let mut sequence = 0;
    while pos < b.len() {
        ensure!(b.len() - pos >= 8, "truncated MoLive tag");
        let n = u32::from_be_bytes(b[pos + 4..pos + 8].try_into()?) as usize;
        let end = pos.checked_add(8 + n).context("MoLive size overflow")?;
        let value = b.get(pos + 8..end).context("truncated MoLive value")?;
        match &b[pos..pos + 4] {
            b"Sch0" => {
                ensure!(value.len() == 24, "invalid schedule");
                sequence = u64::from_be_bytes(value[..8].try_into()?);
                timestamps = Some((
                    u64::from_be_bytes(value[8..16].try_into()?),
                    u64::from_be_bytes(value[16..24].try_into()?),
                ));
            }
            b"Pac0" => {
                ensure!(
                    payload.is_none() && end == b.len(),
                    "invalid media payload position"
                );
                payload = Some(value.to_vec());
            }
            _ => {}
        }
        pos = end;
    }
    Ok(MediaUnit {
        stream,
        kind,
        sequence,
        timestamps,
        payload: payload.context("MoLive payload missing")?,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn live_corruption_discards_partials_and_allows_the_next_datagram() {
        let mut reassembler = Reassembler::default();
        let mut valid = vec![0; 24];
        valid[..2].copy_from_slice(&[0xec, 0xc2]);
        valid[9] = 8;
        valid[4] = 1;
        assert!(reassembler.push_resilient(&valid).is_empty());
        reassembler.streams.insert(0, vec![1, 2, 3]);
        let mut broken = vec![0; 17];
        broken[..2].copy_from_slice(&[0xec, 0xc2]);
        broken[4] = 2;
        broken[12] = 0xf;
        broken[15..17].copy_from_slice(&[0x85, 0x96]);
        assert!(reassembler.push_resilient(&broken).is_empty());
        assert_eq!(reassembler.malformed, 1);
        assert_eq!(reassembler.incomplete, 1);
        assert!(reassembler.streams.is_empty());
        valid[4] = 3;
        assert!(reassembler.push_resilient(&valid).is_empty());
        assert_eq!(reassembler.last, Some(3));
        assert_eq!(reassembler.gaps, 1);
        assert_eq!(reassembler.malformed, 1);
    }
    #[test]
    fn known_fragment_descriptors() {
        let mut b = Bits::new(&[0x80, 0x2f], 0);
        assert_eq!(b.uint().unwrap(), 0);
        assert_eq!(b.read(1).unwrap(), 0);
        assert_eq!(b.read(13).unwrap() + 1, 48);
        let mut b = Bits::new(&[0x85, 0x96], 0);
        assert_eq!(b.uint().unwrap(), 0);
        assert_eq!(b.read(1).unwrap(), 0);
        assert_eq!(b.read(13).unwrap() + 1, 1431);
        let mut b = Bits::new(&[0xc0, 0x2f], 0);
        assert_eq!(b.uint().unwrap(), 1);
    }
    #[test]
    fn rejects_truncated_fragment() {
        let mut b = vec![0; 17];
        b[..2].copy_from_slice(&[0xec, 0xc2]);
        b[12] = 0xf;
        b[15] = 0x85;
        b[16] = 0x96;
        assert!(decode(&b).is_err());
    }
    #[test]
    fn zero_is_a_packet_terminator() {
        let mut b = vec![0; 24];
        b[..2].copy_from_slice(&[0xec, 0xc2]);
        b[9] = 8;
        b[12] = 0xf;
        assert!(decode(&b).unwrap().fragments.is_empty());
    }
    #[test]
    fn malformed_tlv_is_not_media() {
        assert!(tlv(0, 0, b"Tlv0\0\0\0\x08").is_err());
    }
    #[test]
    fn full_packet_without_terminator_excludes_transport_timestamp() {
        let mut b = vec![0x55; 1456];
        b[..12].fill(0);
        b[..2].copy_from_slice(&[0xec, 0xc2]);
        b[8..10].copy_from_slice(&1440u16.to_be_bytes());
        b[12..17].copy_from_slice(&[0x0f, 0, 28, 0x85, 0x96]);
        let packet = decode(&b).unwrap();
        assert_eq!(packet.fragments.len(), 1);
        assert_eq!(packet.fragments[0].data, &[0x55; 1431]);
        assert!(!packet.fragments[0].final_part);
        assert!(decode(&b[..1455]).is_err());
    }
    #[test]
    fn fragment_cannot_borrow_bytes_from_transport_timestamp() {
        let mut b = vec![0; 24];
        b[..2].copy_from_slice(&[0xec, 0xc2]);
        b[9] = 8;
        // A two-byte fragment has only one byte left before the timestamp.
        b[13..15].copy_from_slice(&[0x80, 1]);
        assert!(decode(&b).is_err());
    }
}
