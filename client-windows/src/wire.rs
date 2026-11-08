use anyhow::{Result, ensure};

pub const PKT: u8 = 1;
pub const TEXT: u8 = 2;
pub const H2D: u8 = 3;
pub const D2H: u8 = 4;
pub const EVENT: u8 = 5;
pub const MAX_PAYLOAD: usize = 4200;

pub fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0xffffu16;
    for &byte in data {
        crc ^= (byte as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}
pub fn encode(kind: u8, payload: &[u8]) -> Result<Vec<u8>> {
    ensure!(payload.len() <= MAX_PAYLOAD, "serial payload too large");
    let mut out = Vec::with_capacity(payload.len() + 7);
    out.extend_from_slice(&[0xa5, 0x5a, kind]);
    out.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    out.extend_from_slice(payload);
    let crc = crc16(&out[2..]);
    out.extend_from_slice(&crc.to_le_bytes());
    Ok(out)
}
pub fn ethernet_tx(frame: &[u8], seq: u32) -> Result<Vec<u8>> {
    ensure!(
        (14..=4078).contains(&frame.len()),
        "invalid Ethernet length"
    );
    let mut p = vec![0; 18];
    p[8..10].copy_from_slice(&(frame.len() as u16).to_le_bytes());
    p[14..18].copy_from_slice(&seq.to_le_bytes());
    p.extend_from_slice(frame);
    encode(H2D, &p)
}
pub fn ethernet_rx(payload: &[u8]) -> Result<(u32, &[u8])> {
    ensure!((32..=4096).contains(&payload.len()), "invalid D2H size");
    let size = u16::from_le_bytes(payload[8..10].try_into()?) as usize;
    ensure!(size == payload.len() - 18, "D2H length mismatch");
    Ok((
        u32::from_le_bytes(payload[14..18].try_into()?),
        &payload[18..],
    ))
}

#[derive(Default)]
pub struct Decoder {
    buffer: Vec<u8>,
    pub bad_crc: u64,
    pub malformed: u64,
}
impl Decoder {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<(u8, Vec<u8>)> {
        self.buffer.extend_from_slice(bytes);
        let mut frames = Vec::new();
        let mut start = 0;
        while start + 2 <= self.buffer.len() {
            if self.buffer[start..start + 2] != [0xa5, 0x5a] {
                start += 1;
                continue;
            }
            if self.buffer.len() - start < 5 {
                break;
            }
            let kind = self.buffer[start + 2];
            let len = u16::from_le_bytes([self.buffer[start + 3], self.buffer[start + 4]]) as usize;
            if !(1..=5).contains(&kind) || len > MAX_PAYLOAD {
                self.malformed += 1;
                start += 2;
                continue;
            }
            let end = start + len + 7;
            if end > self.buffer.len() {
                break;
            }
            let crc = u16::from_le_bytes([self.buffer[end - 2], self.buffer[end - 1]]);
            if crc16(&self.buffer[start + 2..end - 2]) != crc {
                self.bad_crc += 1;
                start += 2;
                continue;
            }
            frames.push((kind, self.buffer[start + 5..end - 2].to_vec()));
            start = end;
        }
        self.buffer.drain(..start);
        frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fx2_packet_padding_preserves_frames_across_usb_boundaries() {
        let payload: Vec<_> = (0..1524).map(|i| i as u8).collect();
        let frame = encode(PKT, &payload).unwrap();
        let mut stream = Vec::new();
        for _ in 0..3 {
            stream.extend_from_slice(&frame);
            stream.resize(stream.len().div_ceil(512) * 512, 0);
        }
        let mut decoder = Decoder::default();
        let mut frames = Vec::new();
        for chunk in stream.chunks(512) {
            frames.extend(decoder.push(chunk));
        }
        assert_eq!(frames, vec![(PKT, payload); 3]);
        assert_eq!(decoder.bad_crc, 0);
        assert_eq!(decoder.malformed, 0);
    }
    #[test]
    fn standard_crc() {
        assert_eq!(crc16(b"123456789"), 0x29b1)
    }
    #[test]
    fn fragmented_and_corrupt_stream() {
        let good = encode(TEXT, b"ok hello").unwrap();
        let mut bad = good.clone();
        bad[5] ^= 1;
        let stream = [b"noise".as_slice(), &bad, &good].concat();
        let mut d = Decoder::default();
        let mut out = Vec::new();
        for byte in stream {
            out.extend(d.push(&[byte]));
        }
        assert_eq!(out, vec![(TEXT, b"ok hello".to_vec())]);
        assert_eq!(d.bad_crc, 1);
    }
    #[test]
    fn d2h_length_validation() {
        let mut p = vec![0; 32];
        p[8] = 14;
        assert_eq!(ethernet_rx(&p).unwrap().1.len(), 14);
        p[8] = 15;
        assert!(ethernet_rx(&p).is_err())
    }
    #[test]
    fn oversized_rejected() {
        assert!(encode(H2D, &vec![0; MAX_PAYLOAD + 1]).is_err())
    }
}
