//! Offline analysis uses the same LSP reassembler as live play, without opening USB.
use crate::{lsp, pcap};
use anyhow::{Result, ensure};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufWriter, Write},
    path::Path,
};

#[derive(Default, Serialize)]
pub struct Report {
    pub ethernet_packets: u64,
    pub media_packets: u64,
    pub malformed_packets: u64,
    pub malformed_reasons: BTreeMap<String, u64>,
    pub video_units: u64,
    pub audio_units: u64,
    pub video_bytes: u64,
    pub nal_types: BTreeMap<u8, u64>,
    pub packet_gaps: u64,
    pub incomplete_records: u64,
    pub invalid_records: u64,
    pub duplicates: u64,
    pub media_flows: usize,
}
pub fn run(path: &Path, video_write: Option<&Path>) -> Result<Report> {
    let mut reader = pcap::Reader::open(path)?;
    ensure!(reader.linktype == 1, "diagnose requires Ethernet pcap");
    let mut video = video_write
        .map(File::create)
        .transpose()?
        .map(BufWriter::new);
    let mut flows = BTreeMap::new();
    let mut report = Report::default();
    while let Some(frame) = reader.next_packet()? {
        report.ethernet_packets += 1;
        let Some((flow, payload)) = udp(&frame) else {
            continue;
        };
        if !payload.starts_with(&[0xec, 0xc2]) {
            continue;
        }
        report.media_packets += 1;
        let assembler = flows.entry(flow).or_insert_with(lsp::Reassembler::default);
        if let Err(error) = lsp::decode(payload) {
            *report
                .malformed_reasons
                .entry(error.to_string())
                .or_default() += 1;
        }
        // Match live playback's partial-record reset on malformed packets.
        let units = assembler.push_resilient(payload);
        for unit in units {
            if unit.stream == 0 {
                report.video_units += 1;
                report.video_bytes += unit.payload.len() as u64;
                for nal in nal_types(&unit.payload) {
                    *report.nal_types.entry(nal).or_default() += 1;
                }
                if let Some(video) = &mut video {
                    video.write_all(&unit.payload)?;
                }
            } else if unit.stream == 1 {
                report.audio_units += 1;
            }
        }
    }
    report.media_flows = flows.len();
    for assembler in flows.values() {
        report.malformed_packets += assembler.malformed;
        report.packet_gaps += assembler.gaps;
        report.incomplete_records += assembler.incomplete;
        report.invalid_records += assembler.invalid;
        report.duplicates += assembler.duplicates;
    }
    if let Some(video) = &mut video {
        video.flush()?;
    }
    Ok(report)
}
// src IP, dst IP, src port, dst port; keep independent LSP sequence spaces apart.
type Flow = [u8; 12];
fn udp(frame: &[u8]) -> Option<(Flow, &[u8])> {
    if frame.get(12..14)? != [8, 0] {
        return None;
    }
    let ip = frame.get(14..)?;
    if ip.len() < 20 || ip[0] >> 4 != 4 || ip[9] != 17 {
        return None;
    }
    let ihl = usize::from(ip[0] & 15) * 4;
    let total = u16::from_be_bytes(ip[2..4].try_into().ok()?) as usize;
    // IPv4 fragments require separate reassembly; never decode a partial UDP datagram.
    if ihl < 20 || u16::from_be_bytes(ip[6..8].try_into().ok()?) & 0x3fff != 0 {
        return None;
    }
    let udp = ip.get(ihl..total)?;
    if udp.len() < 8 {
        return None;
    }
    let len = u16::from_be_bytes(udp[4..6].try_into().ok()?) as usize;
    if len < 8 {
        return None;
    }
    let mut flow = [0; 12];
    flow[..8].copy_from_slice(&ip[12..20]);
    flow[8..].copy_from_slice(&udp[..4]);
    Some((flow, udp.get(8..len)?))
}
pub fn nal_types(bytes: &[u8]) -> impl Iterator<Item = u8> + '_ {
    bytes
        .windows(4)
        .filter_map(|b| (b[..3] == [0, 0, 1]).then_some(b[3] & 31))
}
pub fn udp_payload(frame: &[u8]) -> Option<&[u8]> {
    udp(frame).map(|(_, payload)| payload)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn annex_b_three_and_four_byte_start_codes() {
        assert_eq!(
            nal_types(&[0, 0, 0, 1, 0x67, 4, 0, 0, 1, 0x65]).collect::<Vec<_>>(),
            [7, 5]
        );
    }
    #[test]
    fn rejects_fragmented_and_truncated_udp() {
        let mut f = vec![0; 43];
        f[12..14].copy_from_slice(&[8, 0]);
        f[14] = 0x45;
        f[16..18].copy_from_slice(&29u16.to_be_bytes());
        f[23] = 17;
        f[38..40].copy_from_slice(&9u16.to_be_bytes());
        f[42] = 42;
        assert_eq!(udp(&f).unwrap().1, &[42]);
        assert!(udp(&f[..42]).is_none());
        f[20] = 0x20;
        assert!(udp(&f).is_none());
    }
}
