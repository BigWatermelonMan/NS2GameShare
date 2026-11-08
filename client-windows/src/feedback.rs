//! Observed GameShare input datagram format: BE header + Zstandard records.
use anyhow::{Result, ensure};
#[derive(Default, Clone, Copy)]
pub struct Controls {
    pub motion: Option<crate::motion::Pose>,
    pub buttons: u64,
    pub axes: [i32; 4],
}
pub fn npad(state: Controls, sample: u64) -> Vec<u8> {
    let mut b = vec![0; 96];
    b[0] = 1;
    b[2..4].copy_from_slice(&96u16.to_be_bytes());
    b[8] = 1;
    b[16..24].copy_from_slice(&4u64.to_be_bytes());
    b[32..40].copy_from_slice(&sample.to_be_bytes());
    b[40..48].copy_from_slice(&state.buttons.to_be_bytes());
    for (i, a) in state.axes.into_iter().enumerate() {
        b[48 + i * 4..52 + i * 4].copy_from_slice(&a.clamp(-32767, 32767).to_be_bytes());
    }
    // NpadAttribute is a 32-bit field followed by padding, not a BE u64.
    b[64..68].copy_from_slice(&0x15u32.to_be_bytes());
    b
}
pub fn packet(state: Controls, sample: u64, sequence: u32, host_time: u64) -> Result<Vec<u8>> {
    let raw = npad(state, sample);
    packet_records(&raw, sequence, host_time)
}
fn packet_records(raw: &[u8], sequence: u32, host_time: u64) -> Result<Vec<u8>> {
    let compressed = zstd::bulk::compress(raw, 1)?;
    ensure!(
        compressed.len() <= u16::MAX as usize,
        "feedback exceeds UDP size"
    );
    let mut b = vec![0; 56];
    b[..4].copy_from_slice(&0x70000u32.to_be_bytes());
    b[4..8].copy_from_slice(&1u32.to_be_bytes());
    b[16..20].copy_from_slice(&sequence.to_be_bytes());
    b[20..22].copy_from_slice(&(compressed.len() as u16).to_be_bytes());
    b[22..24].copy_from_slice(&(raw.len() as u16).to_be_bytes());
    b[24..32].copy_from_slice(&host_time.to_be_bytes());
    b.extend_from_slice(&compressed);
    Ok(b)
}
/// Type 3 / 144-byte record. Field offsets and JoyDual handles match own peer captures.
pub fn sixaxis(sensor: crate::motion::Sensor, sample: u64, handle: u32) -> Vec<u8> {
    let mut b = vec![0; 144];
    b[0] = 3;
    b[2..4].copy_from_slice(&144u16.to_be_bytes());
    b[8] = 1;
    b[12..16].copy_from_slice(&handle.to_be_bytes());
    b[24..32].copy_from_slice(&sensor.time_ns.to_be_bytes());
    b[32..40].copy_from_slice(&sample.to_be_bytes());
    for (offset, values) in [
        (44, sensor.acceleration.as_slice()),
        (56, sensor.velocity.as_slice()),
        (68, sensor.angle.as_slice()),
        (80, sensor.direction.as_slice()),
    ] {
        for (i, &value) in values.iter().enumerate() {
            let value = if value == 0.0 { 0.0 } else { value };
            b[offset + i * 4..offset + i * 4 + 4].copy_from_slice(&value.to_be_bytes());
        }
    }
    b[116..120].copy_from_slice(&1u32.to_be_bytes());
    b
}
#[derive(Default, Clone)]
pub struct Encoder {
    motion: crate::motion::Encoder,
}
impl Encoder {
    pub fn packet(
        &mut self,
        state: Controls,
        sample: u64,
        sequence: u32,
        host_time: u64,
        local_ns: u64,
    ) -> Result<Vec<u8>> {
        let sensor = self.motion.sample(state.motion, local_ns);
        let mut raw = npad(state, sample);
        if let Some(sensor) = sensor {
            raw.extend(sixaxis(sensor, sample, 5));
            raw.extend(sixaxis(sensor, sample, 0x10005));
        }
        packet_records(&raw, sequence, host_time)
    }
}
pub fn ntp_request(local_ns: u64) -> [u8; 24] {
    let mut b = [0; 24];
    b[16..].copy_from_slice(&local_ns.to_be_bytes());
    b
}
pub fn ntp_offset(reply: &[u8], received_ns: u64) -> Result<(i128, u64)> {
    ensure!(reply.len() == 24, "invalid NTP response");
    let t0 = u64::from_be_bytes(reply[..8].try_into()?);
    let t1 = u64::from_be_bytes(reply[8..16].try_into()?);
    let t2 = u64::from_be_bytes(reply[16..24].try_into()?);
    ensure!(t0 <= received_ns && t2 >= t1, "invalid NTP times");
    let offset = ((t1 as i128 - t0 as i128) + (t2 as i128 - received_ns as i128)) / 2;
    let rtt = (received_ns - t0).saturating_sub(t2 - t1);
    Ok((offset, rtt))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn neutral_sixaxis_matches_complete_own_peer_record() {
        let mut sensor = crate::motion::Encoder::default()
            .sample(Some(crate::motion::Pose::default()), 0)
            .unwrap();
        sensor.time_ns = 5_000_000; // reference peer's 5 ms sampling period
        let captured = crate::ldn::unhex(concat!(
            "03000090000000000100000000000005000000000000000000000000004c4b40",
            "00000000000017aa000000000000000000000000bf8000000000000000000000",
            "000000000000000000000000000000003f800000000000000000000000000000",
            "3f8000000000000000000000000000003f800000000000010000000000000000",
            "00000000000000000000000000000000"
        ))
        .unwrap();
        assert_eq!(sixaxis(sensor, 6058, 5), captured);
    }
    #[test]
    fn motion_is_opt_in_and_both_joydual_handles_are_framed() {
        let mut encoder = Encoder::default();
        let c = Controls::default();
        assert_eq!(
            encoder.packet(c, 1, 1, 1, 0).unwrap(),
            packet(c, 1, 1, 1).unwrap()
        );
        let c = Controls {
            motion: Some(crate::motion::Pose::default()),
            ..Default::default()
        };
        let packet = encoder.packet(c, 2, 2, 2, 8_000_000).unwrap();
        let raw = zstd::bulk::decompress(&packet[56..], 1024).unwrap();
        assert_eq!(raw.len(), 384);
        assert_eq!(raw[96], 3);
        assert_eq!(raw[240], 3);
        assert_eq!(u32::from_be_bytes(raw[108..112].try_into().unwrap()), 5);
        assert_eq!(
            u32::from_be_bytes(raw[252..256].try_into().unwrap()),
            0x10005
        );
        assert_eq!(u16::from_be_bytes(packet[22..24].try_into().unwrap()), 384);
    }
    #[test]
    fn state_layout_and_compression() {
        let s = Controls {
            buttons: (1u64 << 40) | 3,
            axes: [32767, -32767, 22, -33],
            ..Default::default()
        };
        let p = packet(s, 17, 9, 1234).unwrap();
        let raw = zstd::bulk::decompress(&p[56..], 65536).unwrap();
        assert_eq!(raw.len(), 96);
        assert_eq!(&raw[40..48], &[0, 0, 1, 0, 0, 0, 0, 3]);
        assert_eq!(i32::from_be_bytes(raw[52..56].try_into().unwrap()), -32767);
        assert_eq!(u32::from_be_bytes(p[16..20].try_into().unwrap()), 9);
    }
    #[test]
    fn captured_b_and_left_stick_up_button_order() {
        // Own guest capture: B + stick-up is the BE u64 value 0x20002.
        let raw = npad(
            Controls {
                buttons: 0x20002,
                axes: [0, 32767, 0, 0],
                ..Default::default()
            },
            2853,
        );
        assert_eq!(&raw[40..48], &[0, 0, 0, 0, 0, 2, 0, 2]);
        assert_eq!(&raw[64..72], &[0, 0, 0, 0x15, 0, 0, 0, 0]);
    }
    #[test]
    fn matches_complete_captured_npad_record() {
        let raw = npad(
            Controls {
                buttons: 0x20000,
                axes: [300, 32765, 0, 0],
                ..Default::default()
            },
            2853,
        );
        let captured = crate::ldn::unhex(concat!(
            "01000060000000000100000000000000",
            "00000000000000040000000000000000",
            "0000000000000b250000000000020000",
            "0000012c00007ffd0000000000000000",
            "00000015000000000000000000000000",
            "00000000000000000000000000000000"
        ))
        .unwrap();
        assert_eq!(raw, captured);
    }
    #[test]
    fn clock_offset() {
        let mut b = [0; 24];
        b[..8].copy_from_slice(&100u64.to_be_bytes());
        b[8..16].copy_from_slice(&1110u64.to_be_bytes());
        b[16..24].copy_from_slice(&1112u64.to_be_bytes());
        assert_eq!(ntp_offset(&b, 122).unwrap(), (1000, 20));
    }
}
