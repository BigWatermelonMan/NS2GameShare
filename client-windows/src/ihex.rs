//! Strict RAM-only Intel HEX loading for FX2LP; EEPROM is never addressed.
use anyhow::{Result, ensure};
pub fn parse(text: &str) -> Result<Vec<(u16, Vec<u8>)>> {
    let mut records = Vec::new();
    let mut eof = false;
    let mut used = [false; 0x4000];
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        ensure!(!eof, "data after HEX EOF");
        let line = line
            .trim()
            .strip_prefix(':')
            .ok_or_else(|| anyhow::anyhow!("missing HEX colon"))?;
        ensure!(
            line.is_ascii() && line.len().is_multiple_of(2) && line.len() >= 10,
            "invalid HEX length"
        );
        let data = (0..line.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&line[i..i + 2], 16))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        ensure!(
            data.len() == usize::from(data[0]) + 5
                && data.iter().fold(0u8, |a, b| a.wrapping_add(*b)) == 0,
            "HEX checksum/length mismatch"
        );
        let address = u16::from_be_bytes([data[1], data[2]]);
        match data[3] {
            0 => {
                let end = usize::from(address) + usize::from(data[0]);
                ensure!(end <= 0x4000, "HEX is outside FX2 internal RAM");
                for flag in &mut used[usize::from(address)..end] {
                    ensure!(!*flag, "overlapping HEX data");
                    *flag = true;
                }
                records.push((address, data[4..data.len() - 1].to_vec()));
            }
            1 => {
                ensure!(data[0] == 0 && address == 0, "invalid HEX EOF");
                eof = true;
            }
            4 => {
                ensure!(
                    data[0] == 2 && address == 0 && data[4..6] == [0, 0],
                    "extended HEX bank unsupported"
                );
            }
            _ => anyhow::bail!("unsupported HEX record type"),
        }
    }
    ensure!(eof && used[0], "HEX requires EOF and reset vector");
    Ok(records)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn valid_ram_image_and_bad_images() {
        assert_eq!(
            parse(":03000000020010EB\n:00000001FF").unwrap(),
            vec![(0, vec![2, 0, 16])]
        );
        for s in [
            ":03000000020010EA\n:00000001FF",
            ":0140000000BF\n:00000001FF",
            ":03000000020010EB",
            ":020000040001F9\n:00000001FF",
            ":03000000020010EB\n:03000000020010EB\n:00000001FF",
        ] {
            assert!(parse(s).is_err());
        }
    }
}
