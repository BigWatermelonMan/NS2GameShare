use crate::flat;
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::io::Read;

pub const MAX_CONTROL: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct MatchingInfo {
    pub control_port: u16,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate: u32,
    pub audio_rate: u32,
    pub matching_id_bytes: usize,
    #[serde(skip)]
    pub host_user: [u8; 16],
    pub envelope_type: u8,
}
impl MatchingInfo {
    pub fn decode(app: &[u8]) -> Result<Self> {
        ensure!(
            app.len() > 68 && app[64..68] == [0x28, 0xb5, 0x2f, 0xfd],
            "not a signed Zstandard LRP advertisement"
        );
        let mut d = zstd::stream::read::Decoder::new(&app[64..])?;
        d.window_log_max(20)?;
        let mut body = Vec::new();
        d.take(65537).read_to_end(&mut body)?;
        ensure!(
            body.len() <= 65536,
            "LRP advertisement expands beyond limit"
        );
        let (kind, t) = flat::envelope(&body)?;
        ensure!(kind == 1, "not LRP matching information");
        let port = t.u16(2, 0)?;
        ensure!(port != 0, "missing control port");
        Ok(Self {
            control_port: port,
            width: t.u32(15, 0)?,
            height: t.u32(16, 0)?,
            fps: t.u32(17, 0)?,
            bitrate: t.u32(18, 0)?,
            audio_rate: t.u32(21, 0)?,
            matching_id_bytes: t.vector(0)?.map_or(0, <[u8]>::len),
            host_user: t
                .inline(7)?
                .ok_or_else(|| anyhow::anyhow!("host user identifier missing"))?,
            envelope_type: kind,
        })
    }
}

pub struct ControlRecord {
    pub signature: [u8; 64],
    pub body: Vec<u8>,
}
#[derive(Default)]
pub struct ControlDecoder {
    buffer: Vec<u8>,
}
impl ControlDecoder {
    pub fn push(&mut self, data: &[u8]) -> Result<Vec<ControlRecord>> {
        ensure!(
            self.buffer.len() + data.len() <= MAX_CONTROL + 68,
            "control receive buffer exceeded"
        );
        self.buffer.extend_from_slice(data);
        let mut offset = 0;
        let mut out = Vec::new();
        while self.buffer.len() - offset >= 68 {
            let len = u32::from_be_bytes(self.buffer[offset..offset + 4].try_into()?) as usize;
            ensure!(
                len > 64 && len <= MAX_CONTROL + 64,
                "invalid control message length"
            );
            if self.buffer.len() - offset < 4 + len {
                break;
            }
            let signature = self.buffer[offset + 4..offset + 68].try_into()?;
            out.push(ControlRecord {
                signature,
                body: self.buffer[offset + 68..offset + 4 + len].to_vec(),
            });
            offset += 4 + len;
        }
        self.buffer.drain(..offset);
        Ok(out)
    }
}

pub fn frame_unsigned(body: &[u8]) -> Result<Vec<u8>> {
    ensure!(
        !body.is_empty() && body.len() <= MAX_CONTROL,
        "invalid control body"
    );
    let mut p = vec![0; 68];
    p[..4].copy_from_slice(&((body.len() + 64) as u32).to_be_bytes());
    p.extend_from_slice(body);
    Ok(p)
}

/// Observed read-only host-information request (envelope 2, request union 3).
/// The challenge is fresh; requested profile ID comes from the host's advertisement.
pub fn host_information_request(host_user: &[u8; 16]) -> Result<Vec<u8>> {
    let mut b = Builder(vec![0; 4]);
    let root = b.table(16, &[8, 7, 12]);
    b.0[root + 7] = 2;
    let version = b.table(8, &[4]);
    b.0[version + 4..version + 8].copy_from_slice(&0x70000u32.to_le_bytes());
    b.pointer(root + 8, version);
    let req = b.table(16, &[8, 7, 12]);
    b.0[req + 7] = 3;
    b.pointer(root + 12, req);
    let tx = b.table(12, &[6, 8, 10]);
    for (o, v) in [(6, 1u16), (8, 4), (10, 1)] {
        b.0[tx + o..tx + o + 2].copy_from_slice(&v.to_le_bytes());
    }
    b.pointer(req + 8, tx);
    let payload = b.table(12, &[4, 8]);
    b.pointer(req + 12, payload);
    let mut nonce = [0; 32];
    getrandom::fill(&mut nonce).map_err(|_| anyhow::anyhow!("random source failed"))?;
    let p = b.vector(32, &nonce);
    b.pointer(payload + 4, p);
    let p = b.vector(1, host_user);
    b.pointer(payload + 8, p);
    b.0[..4].copy_from_slice(&(root as u32).to_le_bytes());
    frame_unsigned(&b.0)
}
/// Initial session request. Profile/capability defaults are being validated against hardware.
pub fn session_request(
    name: &str,
    feedback_port: u16,
    media_port: u16,
    width: u32,
    height: u32,
) -> Result<Vec<u8>> {
    crate::profile::validate_name(name)?;
    ensure!(
        feedback_port != 0 && media_port != 0 && feedback_port != media_port,
        "invalid session ports"
    );
    let mut b = Builder(vec![0; 4]);
    let root = b.table(16, &[8, 7, 12]);
    b.0[root + 7] = 2;
    let ver = b.table(8, &[4]);
    b.0[ver + 4..ver + 8].copy_from_slice(&0x70000u32.to_le_bytes());
    b.pointer(root + 8, ver);
    let req = b.table(16, &[8, 7, 12]);
    b.0[req + 7] = 1;
    b.pointer(root + 12, req);
    let header = b.table(12, &[6, 8, 10]);
    for (o, v) in [(6, 1u16), (8, 4), (10, 4)] {
        b.0[header + o..header + o + 2].copy_from_slice(&v.to_le_bytes());
    }
    b.pointer(req + 8, header);
    let payload = b.table(32, &[12, 6, 16, 8, 10, 20, 24, 28]);
    b.pointer(req + 12, payload);
    b.0[payload + 6..payload + 8].copy_from_slice(&1u16.to_le_bytes());
    b.0[payload + 8..payload + 10].copy_from_slice(&feedback_port.to_le_bytes());
    b.0[payload + 10..payload + 12].copy_from_slice(&media_port.to_le_bytes());
    b.0[payload + 16..payload + 20].copy_from_slice(&2u32.to_le_bytes());
    b.0[payload + 24..payload + 28].copy_from_slice(&0.01f32.to_le_bytes());
    let mut challenge = [0; 32];
    getrandom::fill(&mut challenge).map_err(|_| anyhow::anyhow!("random source failed"))?;
    let v = b.vector(32, &challenge);
    b.pointer(payload + 12, v);
    let profile = b.table(100, &[8, 28, 32, 4, 6, 96]);
    b.pointer(payload + 20, profile);
    let mut uid = [0; 16];
    getrandom::fill(&mut uid).map_err(|_| anyhow::anyhow!("random source failed"))?;
    b.0[profile + 8..profile + 24].copy_from_slice(&uid);
    b.0[profile + 32..profile + 37].fill(1);
    let mut display_name = name.as_bytes().to_vec();
    display_name.push(0);
    let v = b.vector(name.len() as u32, &display_name);
    b.pointer(profile + 28, v);
    let v = b.vector(0, &[]);
    b.pointer(profile + 96, v);
    let v = b.vector(2, &[0; 8]);
    b.pointer(payload + 28, v);
    for (i, (w, h)) in [(width, height), (width, height)].into_iter().enumerate() {
        let t = b.table(12, &[4, 8]);
        b.0[t + 4..t + 8].copy_from_slice(&w.to_le_bytes());
        b.0[t + 8..t + 12].copy_from_slice(&h.to_le_bytes());
        b.pointer(v + 4 + i * 4, t);
    }
    b.0[..4].copy_from_slice(&(root as u32).to_le_bytes());
    frame_unsigned(&b.0)
}
pub struct SessionInfo {
    pub feedback_port: u16,
    pub ntp_port: u16,
}
impl SessionInfo {
    pub fn decode(t: flat::Table<'_>) -> Result<Self> {
        ensure!(!t.tables(1)?.is_empty(), "session node missing");
        Ok(Self {
            feedback_port: t.u16(2, 0)?,
            ntp_port: t.u16(3, 0)?,
        })
    }
}
pub struct VideoStream {
    pub id: u32,
    pub width: u32,
    pub height: u32,
}
impl VideoStream {
    /// Use the host's format-notification ID for all streaming feedback.
    pub fn notice(message: flat::Table<'_>) -> Result<Option<Self>> {
        if message.u8(1, 0)? != 3 {
            return Ok(None);
        }
        let notice = message.table(2)?.context("stream notice missing")?;
        if notice.u8(0, 0)? != 1 {
            return Ok(None);
        }
        let id = u32::from_le_bytes(notice.inline::<4>(1)?.context("stream ID missing")?);
        let format = notice.table(2)?.context("video format missing")?;
        let (width, height) = (format.u32(0, 0)?, format.u32(1, 0)?);
        ensure!(
            (1..=8192).contains(&width) && (1..=8192).contains(&height),
            "invalid video dimensions"
        );
        Ok(Some(Self { id, width, height }))
    }
}
pub fn request_keyframe(session: u32, serial: u16) -> Result<Vec<u8>> {
    stream_command(session, 3, serial, |b| {
        let inner = b.table(8, &[4]);
        b.0[inner + 4..inner + 8].copy_from_slice(&session.to_le_bytes());
        inner
    })
}
/// QoS extended fields use unsigned 15-bit-shifted bitrate/value-error pairs.
/// Derived from the owner's SDK serializer (QOSX), not an official schema.
pub fn qos(
    session: u32,
    serial: u16,
    flags: u8,
    capacity_bps: u32,
    error_bps: u32,
) -> Result<Vec<u8>> {
    stream_command(session, 6, serial, |b| {
        let inner = b.table(16, &[4, 8, 10, 12]);
        b.0[inner + 4..inner + 8].copy_from_slice(&session.to_le_bytes());
        b.0[inner + 8] = flags;
        for (offset, value) in [(10, capacity_bps), (12, error_bps)] {
            b.0[inner + offset..inner + offset + 2]
                .copy_from_slice(&((value >> 15).min(0x7fff) as u16).to_le_bytes());
        }
        inner
    })
}
/// Invalidate missing frame references through the observed INVL control message.
pub fn invalidate(session: u32, serial: u16, first: u64, last: u64) -> Result<Vec<u8>> {
    ensure!(first <= last, "invalid missing-frame interval");
    stream_command(session, 3, serial, |b| {
        let inner = b.table_aligned(32, &[4, 8, 16, 24], 8);
        b.0[inner + 4..inner + 8].copy_from_slice(&session.to_le_bytes());
        b.0[inner + 8..inner + 16].copy_from_slice(&first.to_le_bytes());
        b.0[inner + 16..inner + 24].copy_from_slice(&last.to_le_bytes());
        inner
    })
}
fn stream_command(
    session: u32,
    kind: u8,
    serial: u16,
    payload: impl FnOnce(&mut Builder) -> usize,
) -> Result<Vec<u8>> {
    let mut b = Builder(vec![0; 4]);
    let root = b.table(16, &[8, 7, 12]);
    b.0[root + 7] = 4;
    let ver = b.table(8, &[4]);
    b.0[ver + 4..ver + 8].copy_from_slice(&0x70000u32.to_le_bytes());
    b.pointer(root + 8, ver);
    let message = b.table(16, &[8, 7, 12]);
    b.0[message + 7] = 4;
    b.pointer(root + 12, message);
    let header = b.table(12, &[6, 8, 10]);
    for (o, v) in [(6, 2u16), (8, 2), (10, serial)] {
        b.0[header + o..header + o + 2].copy_from_slice(&v.to_le_bytes());
    }
    b.pointer(message + 8, header);
    let command = b.table(16, &[8, 7, 12]);
    b.0[command + 7] = kind;
    b.0[command + 8..command + 12].copy_from_slice(&session.to_le_bytes());
    b.pointer(message + 12, command);
    let inner = payload(&mut b);
    b.pointer(command + 12, inner);
    b.0[..4].copy_from_slice(&(root as u32).to_le_bytes());
    frame_unsigned(&b.0)
}
pub fn controller_info() -> Result<Vec<u8>> {
    let mut b = Builder(vec![0; 4]);
    let root = b.table(16, &[8, 7, 12]);
    b.0[root + 7] = 4;
    let ver = b.table(8, &[4]);
    b.0[ver + 4..ver + 8].copy_from_slice(&0x70000u32.to_le_bytes());
    b.pointer(root + 8, ver);
    let message = b.table(16, &[8, 7, 12]);
    b.0[message + 7] = 6;
    b.pointer(root + 12, message);
    let header = b.table(12, &[6, 8, 10]);
    for (o, v) in [(6, 1u16), (8, 4), (10, 2)] {
        b.0[header + o..header + o + 2].copy_from_slice(&v.to_le_bytes());
    }
    b.pointer(message + 8, header);
    let body = b.table(20, &[4, 8, 12, 16]);
    b.0[body + 8..body + 12].copy_from_slice(&4u32.to_le_bytes());
    b.pointer(message + 12, body);
    let colors = b.vector(
        4,
        &[
            0x404040ffu32.to_le_bytes(),
            0x202020ffu32.to_le_bytes(),
            0x404040ffu32.to_le_bytes(),
            0x202020ffu32.to_le_bytes(),
        ]
        .concat(),
    );
    b.pointer(body + 16, colors);
    b.0[..4].copy_from_slice(&(root as u32).to_le_bytes());
    frame_unsigned(&b.0)
}
struct Builder(Vec<u8>);
impl Builder {
    fn align(&mut self) {
        while !self.0.len().is_multiple_of(4) {
            self.0.push(0)
        }
    }
    fn table(&mut self, len: u16, fields: &[u16]) -> usize {
        self.table_aligned(len, fields, 4)
    }
    fn table_aligned(&mut self, len: u16, fields: &[u16], alignment: usize) -> usize {
        self.align();
        let vt = self.0.len();
        self.0
            .extend_from_slice(&((4 + fields.len() * 2) as u16).to_le_bytes());
        self.0.extend_from_slice(&len.to_le_bytes());
        for f in fields {
            self.0.extend_from_slice(&f.to_le_bytes());
        }
        self.align();
        while !self.0.len().is_multiple_of(alignment) {
            self.0.push(0);
        }
        let p = self.0.len();
        self.0.resize(p + len as usize, 0);
        self.0[p..p + 4].copy_from_slice(&((p - vt) as i32).to_le_bytes());
        p
    }
    fn pointer(&mut self, at: usize, target: usize) {
        self.0[at..at + 4].copy_from_slice(&((target - at) as u32).to_le_bytes());
    }
    fn vector(&mut self, count: u32, data: &[u8]) -> usize {
        self.align();
        let p = self.0.len();
        self.0.extend_from_slice(&count.to_le_bytes());
        self.0.extend_from_slice(data);
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn custom_name_is_a_utf8_flatbuffer_string() {
        let name = "みずの PC";
        let bytes = session_request(name, 51000, 51001, 960, 540).unwrap();
        let mut decoder = ControlDecoder::default();
        let records = decoder.push(&bytes).unwrap();
        let (_, request) = flat::envelope(&records[0].body).unwrap();
        let profile = request
            .table(2)
            .unwrap()
            .unwrap()
            .table(5)
            .unwrap()
            .unwrap();
        assert_eq!(profile.vector(1).unwrap().unwrap(), name.as_bytes());
        assert!(session_request("a\0b", 51000, 51001, 960, 540).is_err());
    }
    #[test]
    fn captured_notice_supplies_host_stream_id() {
        // Own capture, with the ephemeral stream ID replaced by 0x12345678.
        let bytes = crate::ldn::unhex(concat!(
            "04000000deffffff000000041000000020000000000006000a0004000600000000000700",
            "00000a001200080007000c000a00000000000003440000001000000000000a001000070008",
            "000c000a00000000000001785634120c00000008000e000400080008000000660400007602",
            "000000000a000c00060008000a000a0000000000010001000200"
        ))
        .unwrap();
        let (kind, message) = flat::envelope(&bytes).unwrap();
        assert_eq!(kind, 4);
        let stream = VideoStream::notice(message).unwrap().unwrap();
        assert_eq!(
            (stream.id, stream.width, stream.height),
            (0x12345678, 1126, 630)
        );
    }
    #[test]
    fn qos_extended_and_invalidation_schema() {
        let p = qos(0x12345678, 3, 4, 1_200_000, 100_000).unwrap();
        let mut d = ControlDecoder::default();
        let r = d.push(&p).unwrap();
        let (kind, message) = flat::envelope(&r[0].body).unwrap();
        assert_eq!(kind, 4);
        assert_eq!(message.u8(1, 0).unwrap(), 4);
        let command = message.table(2).unwrap().unwrap();
        assert_eq!(command.u8(1, 0).unwrap(), 6);
        let q = command.table(2).unwrap().unwrap();
        assert_eq!(q.u32(0, 0).unwrap(), 0x12345678);
        assert_eq!(q.u8(1, 0).unwrap(), 4);
        assert_eq!(q.u16(2, 0).unwrap(), 36);
        assert_eq!(q.u16(3, 0).unwrap(), 3);
        let p = invalidate(4, 5, 11, 13).unwrap();
        let r = d.push(&p).unwrap();
        let (_, m) = flat::envelope(&r[0].body).unwrap();
        let q = m.table(2).unwrap().unwrap().table(2).unwrap().unwrap();
        assert_eq!(u64::from_le_bytes(q.inline::<8>(1).unwrap().unwrap()), 11);
        assert_eq!(u64::from_le_bytes(q.inline::<8>(2).unwrap().unwrap()), 13);
        assert!(invalidate(4, 5, 13, 11).is_err());
    }
    #[test]
    fn fragmented_records() {
        let p = frame_unsigned(b"test").unwrap();
        let mut d = ControlDecoder::default();
        assert!(d.push(&p[..69]).unwrap().is_empty());
        let records = d.push(&p[69..]).unwrap();
        assert_eq!(records[0].body, b"test");
        assert_eq!(records[0].signature, [0; 64]);
    }
    #[test]
    fn rejects_huge_control() {
        let mut d = ControlDecoder::default();
        let mut p = vec![0; 68];
        p[..4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(d.push(&p).is_err());
    }
    #[test]
    fn length_includes_signature() {
        let p = frame_unsigned(b"abcd").unwrap();
        assert_eq!(u32::from_be_bytes(p[..4].try_into().unwrap()), 68);
        let mut d = ControlDecoder::default();
        let r = d.push(&[p.clone(), p].concat()).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[1].body, b"abcd");
    }
    #[test]
    fn information_request_schema() {
        let p = host_information_request(&[9; 16]).unwrap();
        let mut d = ControlDecoder::default();
        let r = d.push(&p).unwrap();
        let (kind, t) = flat::envelope(&r[0].body).unwrap();
        assert_eq!(kind, 2);
        assert_eq!(t.u8(1, 0).unwrap(), 3);
        assert_eq!(
            t.table(2)
                .unwrap()
                .unwrap()
                .vector(0)
                .unwrap()
                .unwrap()
                .len(),
            32
        );
    }
}
