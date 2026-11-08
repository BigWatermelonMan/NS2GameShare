//! Protocol-3/GameShare GCM authentication. Secret material is never Debug/logged.
use aes::{
    Aes128,
    cipher::{BlockDecrypt, KeyInit as _},
};
use aes_gcm::{Aes128Gcm, Nonce, Tag, aead::AeadInPlace};
use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};

const AD: [u8; 16] = [
    0x19, 0x18, 0x84, 0x74, 0x3e, 0x24, 0xc7, 0x7d, 0x87, 0xc6, 0x9e, 0x42, 0x07, 0xd0, 0xc4, 0x38,
];
const DATA: [u8; 16] = [
    0xf1, 0xe7, 0x01, 0x84, 0x19, 0xa8, 0x4f, 0x71, 0x1d, 0xa7, 0x14, 0xc2, 0xcf, 0x91, 0x9c, 0x9c,
];
const PREFIX: [u8; 8] = [0x88, 0xb7, 0, 0x22, 0xaa, 1, 2, 0];
pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
pub fn unhex(s: &str) -> Result<Vec<u8>> {
    ensure!(s.len().is_multiple_of(2), "invalid hex length");
    s.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| Ok(u8::from_str_radix(std::str::from_utf8(b)?, 16)?))
        .collect()
}
pub fn mac(b: &[u8; 6]) -> String {
    b.iter()
        .map(|x| format!("{x:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}
fn decrypt(key: &[u8; 16], block: &[u8; 16]) -> [u8; 16] {
    let mut b = (*block).into();
    Aes128::new(key.into()).decrypt_block(&mut b);
    b.into()
}
pub struct Keys {
    keys: BTreeMap<String, [u8; 16]>,
}
impl Keys {
    pub fn load(path: &Path) -> Result<Self> {
        let mut keys = BTreeMap::new();
        for line in std::fs::read_to_string(path)?.lines() {
            if let Some((n, v)) = line.split('#').next().unwrap_or("").split_once('=')
                && let Ok(b) = unhex(v.trim())
                && let Ok(b) = b.try_into()
            {
                keys.insert(n.trim().to_owned(), b);
            }
        }
        if let Some(source) = keys.get("master_key_source").copied() {
            for (n, k) in keys.clone() {
                if let Some(rev) = n.strip_prefix("master_kek_") {
                    keys.entry(format!("master_key_{rev}"))
                        .or_insert_with(|| decrypt(&k, &source));
                }
            }
        }
        for n in ["aes_kek_generation_source", "aes_key_generation_source"] {
            ensure!(keys.contains_key(n), "keys file lacks {n}");
        }
        Ok(Self { keys })
    }
    fn derive(&self, master: &[u8; 16], source: &[u8; 16], input: &[u8]) -> [u8; 16] {
        let k = decrypt(master, &self.keys["aes_kek_generation_source"]);
        let k = decrypt(&k, source);
        let k = decrypt(&k, &self.keys["aes_key_generation_source"]);
        decrypt(&k, &Sha256::digest(input)[..16].try_into().unwrap())
    }
    fn auth_key(&self, input: &[u8]) -> Result<[u8; 16]> {
        Ok(self.derive(
            self.keys
                .get("master_key_12")
                .context("keys file lacks master_key_12")?,
            &DATA,
            input,
        ))
    }
}
pub struct Host {
    pub bssid: [u8; 6],
    pub channel: u8,
    pub version: u8,
    pub session: [u8; 32],
    pub app_version: u16,
    pub nodes: Vec<([u8; 6], [u8; 4])>,
    pub app: Vec<u8>,
    net_key: [u8; 16],
    master: [u8; 16],
}
pub fn is_advertisement(f: &[u8]) -> bool {
    f.len() >= 76
        && f[0] == 0xd0
        && f[24..29] == [127, 0, 0x22, 0xaa, 4]
        && f[30..32] == [1, 1]
        && f[36..44] == 0x4du64.to_be_bytes()
}
impl Host {
    pub fn decode(f: &[u8], channel: u8, keys: &Keys) -> Result<Self> {
        ensure!(is_advertisement(f), "not GameShare advertisement");
        let p = &f[36..];
        ensure!(p[33] == 3, "only AES-GCM advertisements supported");
        let size = u16::from_be_bytes(p[34..36].try_into()?) as usize;
        ensure!(p.len() >= 56 + size, "truncated advertisement");
        let session: [u8; 32] = p[..32].try_into()?;
        let mut found = None;
        for (name, master) in keys
            .keys
            .iter()
            .rev()
            .filter(|(n, _)| n.starts_with("master_key_"))
        {
            let _ = name;
            let key = keys.derive(master, &AD, &session);
            let cipher = Aes128Gcm::new((&key).into());
            // Verified GameShare layout: counter BE32 followed by eight zero bytes; core header AAD.
            let mut iv = [0; 12];
            iv[..4].copy_from_slice(&p[36..40]);
            let mut plain = p[56..56 + size].to_vec();
            if cipher
                .decrypt_in_place_detached(
                    Nonce::from_slice(&iv),
                    &p[..40],
                    &mut plain,
                    Tag::from_slice(&p[40..56]),
                )
                .is_ok()
            {
                found = Some((*master, plain));
                break;
            }
        }
        let (master, d) = found.context("advertisement GCM verification failed")?;
        ensure!(d.len() >= 42, "short advertisement plaintext");
        let n = d[39] as usize;
        ensure!(n <= 8 && d.len() >= 42 + 48 * n, "invalid participant list");
        let mut nodes = Vec::new();
        for rec in d[40..40 + n * 48].as_chunks::<48>().0 {
            nodes.push((rec[4..10].try_into()?, rec[..4].try_into()?));
        }
        let off = 40 + n * 48;
        let len = u16::from_be_bytes(d[off..off + 2].try_into()?) as usize;
        ensure!(off + 2 + len == d.len(), "application data length mismatch");
        Ok(Self {
            bssid: f[10..16].try_into()?,
            channel,
            version: p[32],
            session,
            app_version: u16::from_be_bytes(d[26..28].try_into()?),
            nodes,
            app: d[off + 2..].to_vec(),
            net_key: d[..16].try_into()?,
            master,
        })
    }
    pub fn ssid(&self) -> String {
        hex(&self.session[16..32])
    }
    pub fn tk(&self, keys: &Keys, passphrase: &[u8]) -> Result<[u8; 16]> {
        ensure!(
            (16..=64).contains(&passphrase.len()),
            "passphrase must contain 16..64 raw bytes"
        );
        Ok(keys.derive(
            &self.master,
            &DATA,
            &[self.net_key.as_slice(), passphrase].concat(),
        ))
    }
    pub fn ip(&self, mac: &[u8; 6]) -> Option<[u8; 4]> {
        self.nodes
            .iter()
            .find(|(m, ip)| m == mac && *ip != [0; 4])
            .map(|(_, ip)| *ip)
    }
    pub fn auth(&self, keys: &Keys, own: [u8; 6], username: &str) -> Result<Auth> {
        crate::profile::validate_name(username)?;
        let mut h = vec![0; 72];
        h[0] = self.version;
        h[1] = 100;
        h[5] = 1;
        h[8..40].copy_from_slice(&self.session);
        h[8..16].reverse();
        h[18..20].reverse();
        h[40..56].copy_from_slice(&self.net_key);
        getrandom::fill(&mut h[56..72]).map_err(|_| anyhow::anyhow!("random source failed"))?;
        let key = keys.auth_key(&h[56..72])?;
        let mut payload = vec![0; 100];
        let name = username.as_bytes();
        ensure!(name.len() <= 32, "username exceeds 32 UTF-8 bytes");
        payload[..name.len()].copy_from_slice(name);
        payload[32..34].copy_from_slice(&self.app_version.to_be_bytes());
        let tag = Aes128Gcm::new((&key).into())
            .encrypt_in_place_detached(Nonce::from_slice(&h[..12]), &h, &mut payload)
            .map_err(|_| anyhow::anyhow!("auth encryption failed"))?;
        let request = [
            self.bssid.as_slice(),
            own.as_slice(),
            &PREFIX,
            &h,
            &tag,
            &payload,
        ]
        .concat();
        Ok(Auth { request, key })
    }
}
pub struct Auth {
    pub request: Vec<u8>,
    key: [u8; 16],
}
impl Auth {
    pub fn verify(&self, eth: &[u8]) -> Result<Option<u8>> {
        if eth.len() < 20
            || eth[12..20] != PREFIX
            || eth[..6] != self.request[6..12]
            || eth[6..12] != self.request[..6]
        {
            return Ok(None);
        }
        let d = &eth[20..];
        let h = &self.request[20..92];
        ensure!(d.len() >= 88, "short auth response");
        ensure!(
            d[0] == h[0] && d[3] == 1 && d[5] == 1,
            "auth direction/version/encryption mismatch"
        );
        let n = d[1] as usize + ((d[4] as usize) << 8);
        ensure!(
            d.len() == 88 + n && d[8..72] == h[8..72],
            "auth context/length mismatch"
        );
        let mut plain = d[88..].to_vec();
        if Aes128Gcm::new((&self.key).into())
            .decrypt_in_place_detached(
                Nonce::from_slice(&d[..12]),
                &d[..72],
                &mut plain,
                Tag::from_slice(&d[72..88]),
            )
            .is_err()
        {
            bail!("auth response GCM verification failed")
        };
        Ok(Some(d[2]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn keys() -> Keys {
        Keys {
            keys: BTreeMap::from([
                ("master_key_12".into(), [1; 16]),
                ("aes_kek_generation_source".into(), [2; 16]),
                ("aes_key_generation_source".into(), [3; 16]),
            ]),
        }
    }
    fn host() -> Host {
        Host {
            bssid: [1, 2, 3, 4, 5, 6],
            channel: 11,
            version: 4,
            session: [7; 32],
            app_version: 2,
            nodes: vec![],
            app: vec![],
            net_key: [8; 16],
            master: [1; 16],
        }
    }
    fn response(a: &Auth) -> Vec<u8> {
        let req = &a.request;
        let mut h = req[20..92].to_vec();
        h[1] = 132;
        h[3] = 1;
        let mut p = vec![0; 132];
        let tag = Aes128Gcm::new((&a.key).into())
            .encrypt_in_place_detached(Nonce::from_slice(&h[..12]), &h, &mut p)
            .unwrap();
        [&req[6..12], &req[..6], &PREFIX, &h, &tag, &p].concat()
    }
    #[test]
    fn verifies_response_and_rejects_tampering() {
        let a = host().auth(&keys(), [10; 6], "native").unwrap();
        let mut r = response(&a);
        assert_eq!(a.verify(&r).unwrap(), Some(0));
        let last = r.len() - 1;
        r[last] ^= 1;
        assert!(a.verify(&r).is_err());
    }
    #[test]
    fn rejects_context_substitution() {
        let a = host().auth(&keys(), [10; 6], "native").unwrap();
        let mut r = response(&a);
        r[20 + 40] ^= 1;
        assert!(a.verify(&r).is_err());
    }
    #[test]
    fn unrelated_frame_cannot_authenticate() {
        let a = host().auth(&keys(), [10; 6], "native").unwrap();
        let mut r = response(&a);
        r[0] ^= 1;
        assert_eq!(a.verify(&r).unwrap(), None);
    }
    #[test]
    fn credentials_are_required_and_name_is_byte_bounded() {
        assert!(host().tk(&keys(), &[]).is_err());
        assert!(host().auth(&keys(), [10; 6], &"あ".repeat(11)).is_err());
    }
    #[test]
    fn custom_utf8_name_is_in_authenticated_payload() {
        let name = "テスト PC";
        let a = host().auth(&keys(), [10; 6], name).unwrap();
        let header = &a.request[20..92];
        let mut payload = a.request[108..].to_vec();
        Aes128Gcm::new((&a.key).into())
            .decrypt_in_place_detached(
                Nonce::from_slice(&header[..12]),
                header,
                &mut payload,
                aes_gcm::Tag::from_slice(&a.request[92..108]),
            )
            .unwrap();
        assert_eq!(&payload[..name.len()], name.as_bytes());
        assert!(payload[name.len()..32].iter().all(|b| *b == 0));
    }
    #[test]
    fn session_key_includes_passphrase() {
        let h = host();
        assert_ne!(
            h.tk(&keys(), &[0; 16]).unwrap(),
            h.tk(&keys(), &[1; 16]).unwrap()
        );
    }
}
