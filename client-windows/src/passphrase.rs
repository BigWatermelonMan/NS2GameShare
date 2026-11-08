//! Read-only, bounded splay extraction. No console keys or passphrase constants.
//! Only a verified module image profile is accepted; unknown builds fail closed.
use aes::{
    Aes128,
    cipher::{BlockDecrypt, BlockEncrypt, KeyInit},
};
use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

const LIMIT: usize = 128 * 1024 * 1024;
const HEADER: usize = 0xc00;
const TITLE: u64 = 0x0100000000001048;
// Metadata only: SHA-256 of the complete decompressed image, not of its secret.
const IMAGE_SHA: &str = "fda129244ada2e908a05d9cc971d66a185dfd3be119b6f5e35c5087499f49a76";
const PASSPHRASE_OFFSET: usize = 0x6c1520;

struct Keyset(BTreeMap<String, Vec<u8>>); // Deliberately no Debug implementation.
impl Keyset {
    fn load(path: &Path) -> Result<Self> {
        let mut text = String::new();
        File::open(path)
            .context("Could not open the selected key file")?
            .take(4 * 1024 * 1024 + 1)
            .read_to_string(&mut text)
            .context("Could not read the selected key file")?;
        ensure!(text.len() < 4 * 1024 * 1024, "Key file is too large");
        let mut values = BTreeMap::new();
        for line in text.lines() {
            if let Some((name, value)) = line.split('#').next().unwrap_or("").split_once('=')
                && let Ok(value) = crate::ldn::unhex(value.trim())
            {
                values.insert(name.trim().to_owned(), value);
            }
        }
        Ok(Self(values))
    }
    fn get<const N: usize>(&self, name: &str) -> Result<[u8; N]> {
        self.0
            .get(name)
            .and_then(|v| v.as_slice().try_into().ok())
            .with_context(|| format!("Key file lacks a valid {name}"))
    }
}
fn part(data: &[u8], start: usize, len: usize) -> Result<&[u8]> {
    data.get(start..start.checked_add(len).context("Integer overflow")?)
        .context("Truncated or invalid input")
}
fn u32le(data: &[u8], offset: usize) -> Result<usize> {
    Ok(u32::from_le_bytes(part(data, offset, 4)?.try_into()?) as usize)
}
fn u64le(data: &[u8], offset: usize) -> Result<u64> {
    Ok(u64::from_le_bytes(part(data, offset, 8)?.try_into()?))
}
fn bounded(value: u64) -> Result<usize> {
    ensure!(
        value <= LIMIT as u64,
        "Input exceeds the 128 MiB extraction limit"
    );
    Ok(value as usize)
}
fn hash_matches(data: &[u8], expected: &[u8]) -> bool {
    Sha256::digest(data).as_slice() == expected
}

fn xts_header(data: &[u8], key: &[u8; 32]) -> Result<Vec<u8>> {
    ensure!(data.len() == HEADER, "Truncated NCA header");
    let cipher = Aes128::new_from_slice(&key[..16])?;
    let tweak_cipher = Aes128::new_from_slice(&key[16..])?;
    let mut out = data.to_vec();
    for (index, sector) in out.as_chunks_mut::<512>().0.iter_mut().enumerate() {
        let mut tweak = (index as u128).to_be_bytes().into();
        tweak_cipher.encrypt_block(&mut tweak);
        for block in sector.as_chunks_mut::<16>().0 {
            let mut value = [0; 16];
            for i in 0..16 {
                value[i] = block[i] ^ tweak[i];
            }
            let mut value = value.into();
            cipher.decrypt_block(&mut value);
            for i in 0..16 {
                block[i] = value[i] ^ tweak[i];
            }
            let mut carry = 0;
            for byte in tweak.iter_mut() {
                let next = *byte >> 7;
                *byte = (*byte << 1) | carry;
                carry = next;
            }
            if carry != 0 {
                tweak[0] ^= 0x87;
            }
        }
    }
    Ok(out)
}
fn ctr(data: &mut [u8], key: &[u8; 16], upper: &[u8], absolute: u64) -> Result<()> {
    ensure!(
        upper.len() == 8 && absolute.is_multiple_of(16),
        "Invalid NCA counter"
    );
    let cipher = Aes128::new(key.into());
    for (i, block) in data.chunks_mut(16).enumerate() {
        let mut counter = [0; 16];
        for j in 0..8 {
            counter[j] = upper[7 - j];
        }
        counter[8..].copy_from_slice(&(absolute / 16 + i as u64).to_be_bytes());
        let mut stream = counter.into();
        cipher.encrypt_block(&mut stream);
        for (byte, mask) in block.iter_mut().zip(stream) {
            *byte ^= mask;
        }
    }
    Ok(())
}

fn header(file: &mut File, keys: &Keyset) -> Result<(Vec<u8>, bool)> {
    let mut bytes = vec![0; HEADER];
    file.read_exact(&mut bytes)
        .context("Could not read the NCA header")?;
    let plaintext = matches!(part(&bytes, 0x200, 4)?, b"NCA2" | b"NCA3");
    if !plaintext {
        bytes = xts_header(&bytes, &keys.get("header_key")?)?;
    }
    ensure!(
        matches!(part(&bytes, 0x200, 4)?, b"NCA2" | b"NCA3"),
        "Invalid NCA header or incorrect header_key"
    );
    Ok((bytes, plaintext))
}

fn main_from_nca(file: &mut File, h: &[u8], plaintext: bool, keys: &Keyset) -> Result<Vec<u8>> {
    ensure!(
        u64le(h, 0x210)? == TITLE && h[0x205] == 0,
        "Not a Switch splay program NCA"
    );
    ensure!(
        part(h, 0x230, 16)?.iter().all(|b| *b == 0),
        "Rights-managed NCA is unsupported"
    );
    let file_len = file.metadata()?.len();
    ensure!(
        u64le(h, 0x208)? == file_len,
        "Split or truncated NCA is unsupported"
    );
    let mut found = None;
    for i in 0..4 {
        let start = u32le(h, 0x240 + i * 16)? as u64 * 512;
        let end = u32le(h, 0x244 + i * 16)? as u64 * 512;
        if start == 0 {
            continue;
        }
        ensure!(
            start >= HEADER as u64 && end > start && end <= file_len,
            "Invalid NCA section extent"
        );
        let fs = part(h, 0x400 + i * 512, 512)?;
        ensure!(
            hash_matches(fs, part(h, 0x280 + i * 32, 32)?),
            "NCA filesystem header hash mismatch"
        );
        if fs[2..4] != [1, 2] {
            continue;
        }
        ensure!(found.is_none(), "Ambiguous ExeFS sections");
        let mut section = vec![0; bounded(end - start)?];
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut section)?;
        ensure!(
            matches!(fs[4], 1 | 3),
            "Unsupported NCA encryption (requires standard AES-CTR)"
        );
        if !plaintext && fs[4] == 3 {
            let generation = h[0x206].max(h[0x220]).saturating_sub(1);
            let kind = ["application", "ocean", "system"]
                .get(h[0x207] as usize)
                .context("Unknown NCA key area")?;
            let area: [u8; 16] = keys.get(&format!("key_area_key_{kind}_{generation:02x}"))?;
            let mut content_key = part(h, 0x320, 16)?.to_vec();
            Aes128::new((&area).into()).decrypt_block((&mut content_key[..]).into());
            ctr(
                &mut section,
                content_key.as_slice().try_into()?,
                part(fs, 0x140, 8)?,
                start,
            )?;
        }
        let table = part(
            &section,
            bounded(u64le(fs, 0x30)?)?,
            bounded(u64le(fs, 0x38)?)?,
        )?;
        ensure!(
            hash_matches(table, part(fs, 8, 32)?),
            "ExeFS hash table mismatch (wrong keys or damaged input)"
        );
        let pfs = part(
            &section,
            bounded(u64le(fs, 0x40)?)?,
            bounded(u64le(fs, 0x48)?)?,
        )?;
        let block = u32le(fs, 0x28)?;
        ensure!(
            (512..=4 * 1024 * 1024).contains(&block) && block.is_power_of_two(),
            "Invalid ExeFS hash block size"
        );
        for (index, data) in pfs.chunks(block).enumerate() {
            ensure!(
                hash_matches(data, part(table, index * 32, 32)?),
                "ExeFS content hash mismatch"
            );
        }
        found = Some(pfs_main(pfs)?);
    }
    found.context("No supported ExeFS section found")
}
fn pfs_main(pfs: &[u8]) -> Result<Vec<u8>> {
    ensure!(part(pfs, 0, 4)? == b"PFS0", "Invalid ExeFS magic");
    let count = u32le(pfs, 4)?;
    ensure!(count <= 256, "Too many ExeFS entries");
    let strings = 16 + count * 24;
    let names = part(pfs, strings, u32le(pfs, 8)?)?;
    let base = strings + names.len();
    let mut main = None;
    for i in 0..count {
        let row = 16 + i * 24;
        let name = names
            .get(u32le(pfs, row + 16)?..)
            .context("Invalid ExeFS name")?;
        let end = name
            .iter()
            .position(|b| *b == 0)
            .context("Unterminated ExeFS name")?;
        if &name[..end] == b"main" {
            ensure!(main.is_none(), "Duplicate main module");
            main = Some(
                part(
                    pfs,
                    base.checked_add(bounded(u64le(pfs, row)?)?)
                        .context("Offset overflow")?,
                    bounded(u64le(pfs, row + 8)?)?,
                )?
                .to_vec(),
            );
        }
    }
    main.context("ExeFS has no main module")
}

fn lz4_block(input: &[u8], size: usize) -> Result<Vec<u8>> {
    ensure!(size <= LIMIT, "NSO segment is too large");
    let mut out = Vec::with_capacity(size);
    let mut pos = 0;
    fn length(input: &[u8], pos: &mut usize, nibble: usize) -> Result<usize> {
        let mut len = nibble;
        if nibble == 15 {
            loop {
                let byte = *input.get(*pos).context("Truncated LZ4 length")?;
                *pos += 1;
                len = len
                    .checked_add(byte as usize)
                    .context("LZ4 length overflow")?;
                if byte != 255 {
                    break;
                }
            }
        }
        Ok(len)
    }
    while pos < input.len() {
        let token = input[pos];
        pos += 1;
        let literals = length(input, &mut pos, (token >> 4) as usize)?;
        ensure!(
            literals <= size - out.len(),
            "LZ4 output exceeds declared size"
        );
        out.extend_from_slice(part(input, pos, literals)?);
        pos += literals;
        if pos == input.len() {
            break;
        }
        let distance = u16::from_le_bytes(part(input, pos, 2)?.try_into()?) as usize;
        pos += 2;
        ensure!(
            distance > 0 && distance <= out.len(),
            "Invalid LZ4 back-reference"
        );
        let count = length(input, &mut pos, (token & 15) as usize)?
            .checked_add(4)
            .context("LZ4 length overflow")?;
        ensure!(
            count <= size - out.len(),
            "LZ4 output exceeds declared size"
        );
        for _ in 0..count {
            out.push(out[out.len() - distance]);
        }
    }
    ensure!(out.len() == size, "NSO decompressed size mismatch");
    Ok(out)
}
fn nso_image(nso: &[u8]) -> Result<Vec<u8>> {
    ensure!(
        nso.len() >= 256 && part(nso, 0, 4)? == b"NSO0",
        "Select an NSO main module, not a raw memory image"
    );
    let flags = u32le(nso, 12)?;
    ensure!(
        flags & !0x7f == 0,
        "Unsupported NSO compression (ZBIC is not supported)"
    );
    let mut image = Vec::new();
    let mut ranges = Vec::new();
    for i in 0..3 {
        let row = 0x10 + i * 16;
        let offset = u32le(nso, row)?;
        let address = u32le(nso, row + 4)?;
        let size = u32le(nso, row + 8)?;
        let end = address.checked_add(size).context("NSO extent overflow")?;
        ensure!(end <= LIMIT && offset >= 256, "Invalid NSO segment extent");
        ensure!(
            ranges.iter().all(|(a, b)| end <= *a || address >= *b),
            "Overlapping NSO segments"
        );
        ranges.push((address, end));
        let compressed = flags & (1 << i) != 0;
        let data = part(
            nso,
            offset,
            if compressed {
                u32le(nso, 0x60 + i * 4)?
            } else {
                size
            },
        )?;
        let plain = if compressed {
            lz4_block(data, size)?
        } else {
            data.to_vec()
        };
        if flags & (1 << (i + 3)) != 0 {
            ensure!(
                hash_matches(&plain, part(nso, 0xa0 + i * 32, 32)?),
                "NSO segment hash mismatch"
            );
        }
        if end > image.len() {
            image.resize(end, 0);
        }
        image[address..end].copy_from_slice(&plain);
    }
    Ok(image)
}
fn secret_from_nso(nso: &[u8]) -> Result<[u8; 16]> {
    let image = nso_image(nso)?;
    ensure!(
        crate::ldn::hex(&Sha256::digest(&image)) == IMAGE_SHA,
        "Unsupported splay build. No passphrase was written. See docs/PASSPHRASE.md; do not use offsets from another firmware version."
    );
    Ok(part(&image, PASSPHRASE_OFFSET, 16)?.try_into()?)
}
fn linked(path: &Path) -> Result<bool> {
    let metadata = fs::symlink_metadata(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        Ok(metadata.file_attributes() & 0x400 != 0)
    }
    #[cfg(not(windows))]
    {
        Ok(metadata.file_type().is_symlink())
    }
}
fn source_main(source: &Path, keys_path: &Path, cancel: &AtomicBool) -> Result<Vec<u8>> {
    ensure!(
        !linked(source)?,
        "Links and reparse points are not accepted as extraction sources"
    );
    if source.is_file() {
        ensure!(
            source.metadata()?.len() <= LIMIT as u64,
            "Input is too large"
        );
        let mut file = File::open(source)?;
        let mut magic = [0; 4];
        file.read_exact(&mut magic)?;
        file.seek(SeekFrom::Start(0))?;
        if magic == *b"NSO0" {
            let mut bytes = Vec::new();
            file.take((LIMIT + 1) as u64).read_to_end(&mut bytes)?;
            return Ok(bytes);
        }
        let keys = Keyset::load(keys_path)?;
        let (h, plain) = header(&mut file, &keys)?;
        return main_from_nca(&mut file, &h, plain, &keys);
    }
    let registered = if source.join("Contents/registered").is_dir() {
        source.join("Contents/registered")
    } else if source
        .file_name()
        .is_some_and(|n| n.eq_ignore_ascii_case("registered"))
    {
        source.to_owned()
    } else {
        bail!(
            "Choose the mounted SYSTEM root, Contents/registered, a splay Program NCA, or its ExeFS main NSO"
        );
    };
    ensure!(
        !linked(&registered)?,
        "Linked registered directory is not accepted"
    );
    let keys = Keyset::load(keys_path)?;
    let mut pending = vec![(registered, 0)];
    let mut visited = 0;
    let mut found = None;
    while let Some((directory, depth)) = pending.pop() {
        ensure!(!cancel.load(Ordering::Relaxed), "Extraction cancelled");
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            visited += 1;
            ensure!(visited <= 20_000, "Directory scan limit exceeded");
            let path = entry.path();
            ensure!(
                !linked(&path)?,
                "Linked files are not accepted in SYSTEM content"
            );
            if path.is_dir() {
                if depth < 2 {
                    pending.push((path, depth + 1));
                }
                continue;
            }
            let candidate = path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("nca"))
                || path.file_name().is_some_and(|n| n == "00");
            if !candidate {
                continue;
            }
            let mut file = File::open(&path)?;
            if file.metadata()?.len() < HEADER as u64 {
                continue;
            }
            let (h, plain) = header(&mut file, &keys)?;
            if u64le(&h, 0x210)? != TITLE || h[0x205] != 0 {
                continue;
            }
            ensure!(
                found.is_none(),
                "Multiple splay Program NCAs found; select the intended NCA file directly"
            );
            found = Some(main_from_nca(&mut file, &h, plain, &keys)?);
        }
    }
    found.context(
        "No Switch splay Program NCA found. A Switch 2 SYSTEM dump is not currently supported",
    )
}

/// Writes exactly 16 bytes using create_new; never overwrites any existing file.
pub fn extract_to_file(
    source: &Path,
    keys: &Path,
    output: &Path,
    cancel: &AtomicBool,
) -> Result<()> {
    let nso = source_main(source, keys, cancel)?;
    ensure!(!cancel.load(Ordering::Relaxed), "Extraction cancelled");
    let secret = secret_from_nso(&nso)?;
    ensure!(!cancel.load(Ordering::Relaxed), "Extraction cancelled");
    save_secret(output, &secret)
}
fn save_secret(output: &Path, secret: &[u8; 16]) -> Result<()> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .context("Output already exists or cannot be created. Choose a new private output file")?;
    file.write_all(secret)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn xts_matches_an_independent_synthetic_vector() {
        let key: [u8; 32] = std::array::from_fn(|i| i as u8);
        let vector =
            crate::ldn::unhex("b3a5829f75001e33a3e33d96293537271839b0f42b2193fe6b6639f25fce3dc9")
                .unwrap();
        let mut header = vec![0; HEADER];
        header[512..544].copy_from_slice(&vector);
        let plain = xts_header(&header, &key).unwrap();
        assert_eq!(&plain[512..544], &(0u8..32).collect::<Vec<_>>());
    }
    #[test]
    fn private_output_is_exact_and_never_overwritten() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "NS2GameShare-extraction-test-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap();
        let output = directory.join("passphrase.bin");
        save_secret(&output, &[3; 16]).unwrap();
        assert_eq!(fs::read(&output).unwrap(), vec![3; 16]);
        assert!(save_secret(&output, &[4; 16]).is_err());
        assert_eq!(fs::read(&output).unwrap(), vec![3; 16]);
        fs::remove_file(output).unwrap();
        fs::remove_dir(directory).unwrap();
    }
    #[test]
    fn checked_lz4_overlap_and_reject_corruption() {
        assert_eq!(
            lz4_block(&[0x32, b'a', b'b', b'c', 3, 0], 9).unwrap(),
            b"abcabcabc"
        );
        for data in [&[0x00, 0, 0][..], &[0xf0, 255][..], &[0x10, b'a', 2, 0][..]] {
            assert!(lz4_block(data, 16).is_err());
        }
        assert!(lz4_block(&[0x10, b'a'], 0).is_err());
    }
    #[test]
    fn unknown_image_never_yields_a_secret() {
        let mut nso = vec![0; 256];
        nso[..4].copy_from_slice(b"NSO0");
        for i in 0..3 {
            nso[0x10 + i * 16..0x14 + i * 16].copy_from_slice(&256u32.to_le_bytes());
        }
        assert!(secret_from_nso(&nso).is_err());
        nso[12] = 128;
        assert!(nso_image(&nso).is_err());
    }
    #[test]
    fn malformed_pfs_and_segment_sizes_are_bounded() {
        assert!(pfs_main(b"PFS0").is_err());
        let mut pfs = vec![0; 16];
        pfs[..4].copy_from_slice(b"PFS0");
        pfs[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(pfs_main(&pfs).is_err());
        assert!(part(&pfs, usize::MAX, 4).is_err());
        assert!(bounded(u64::MAX).is_err());
    }
}
