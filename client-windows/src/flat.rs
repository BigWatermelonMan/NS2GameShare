//! Bounded FlatBuffers reader for the observed LRP envelope. No schema guesses
//! are used for pointer arithmetic outside an explicitly checked buffer.
use anyhow::{Result, bail, ensure};

#[derive(Clone, Copy)]
pub struct Table<'a> {
    data: &'a [u8],
    pos: usize,
    vtable: usize,
    vlen: usize,
    object_len: usize,
}

fn bytes<const N: usize>(data: &[u8], offset: usize) -> Result<[u8; N]> {
    let end = offset
        .checked_add(N)
        .ok_or_else(|| anyhow::anyhow!("offset overflow"))?;
    Ok(data
        .get(offset..end)
        .ok_or_else(|| anyhow::anyhow!("truncated FlatBuffer"))?
        .try_into()?)
}

impl<'a> Table<'a> {
    pub fn root(data: &'a [u8]) -> Result<Self> {
        let pos = u32::from_le_bytes(bytes(data, 0)?) as usize;
        Self::at(data, pos)
    }
    pub fn at(data: &'a [u8], pos: usize) -> Result<Self> {
        let relative = i32::from_le_bytes(bytes(data, pos)?) as i64;
        let vt = (pos as i64)
            .checked_sub(relative)
            .ok_or_else(|| anyhow::anyhow!("vtable overflow"))?;
        ensure!(vt >= 0, "negative vtable");
        let vtable = vt as usize;
        let vlen = u16::from_le_bytes(bytes(data, vtable)?) as usize;
        let object_len = u16::from_le_bytes(bytes(data, vtable + 2)?) as usize;
        ensure!(
            vlen >= 4 && vlen.is_multiple_of(2) && object_len >= 4,
            "invalid FlatBuffer table size"
        );
        ensure!(
            vtable.checked_add(vlen).is_some_and(|n| n <= data.len()),
            "vtable outside buffer"
        );
        ensure!(
            pos.checked_add(object_len).is_some_and(|n| n <= data.len()),
            "object outside buffer"
        );
        Ok(Self {
            data,
            pos,
            vtable,
            vlen,
            object_len,
        })
    }
    fn field(&self, index: usize, size: usize) -> Result<Option<usize>> {
        let slot = index
            .checked_mul(2)
            .and_then(|x| x.checked_add(4))
            .ok_or_else(|| anyhow::anyhow!("field overflow"))?;
        if slot + 2 > self.vlen {
            return Ok(None);
        }
        let offset = u16::from_le_bytes(bytes(self.data, self.vtable + slot)?) as usize;
        if offset == 0 {
            return Ok(None);
        }
        ensure!(
            offset >= 4
                && offset
                    .checked_add(size)
                    .is_some_and(|n| n <= self.object_len),
            "field outside object"
        );
        Ok(Some(self.pos + offset))
    }
    pub fn u8(&self, index: usize, default: u8) -> Result<u8> {
        Ok(self
            .field(index, 1)?
            .map(|p| self.data[p])
            .unwrap_or(default))
    }
    pub fn u16(&self, index: usize, default: u16) -> Result<u16> {
        self.field(index, 2)?
            .map(|p| Ok(u16::from_le_bytes(bytes(self.data, p)?)))
            .unwrap_or(Ok(default))
    }
    pub fn u32(&self, index: usize, default: u32) -> Result<u32> {
        self.field(index, 4)?
            .map(|p| Ok(u32::from_le_bytes(bytes(self.data, p)?)))
            .unwrap_or(Ok(default))
    }
    pub fn inline<const N: usize>(&self, index: usize) -> Result<Option<[u8; N]>> {
        self.field(index, N)?
            .map(|p| bytes(self.data, p))
            .transpose()
    }
    pub fn tables(&self, index: usize) -> Result<Vec<Self>> {
        let Some(pos) = self.field(index, 4)? else {
            return Ok(vec![]);
        };
        let off = u32::from_le_bytes(bytes(self.data, pos)?) as usize;
        let start = pos
            .checked_add(off)
            .ok_or_else(|| anyhow::anyhow!("vector offset overflow"))?;
        let n = u32::from_le_bytes(bytes(self.data, start)?) as usize;
        ensure!(n <= 1024, "too many tables in vector");
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let p = start + 4 + i * 4;
            let off = u32::from_le_bytes(bytes(self.data, p)?) as usize;
            let t = p
                .checked_add(off)
                .ok_or_else(|| anyhow::anyhow!("table offset overflow"))?;
            out.push(Self::at(self.data, t)?);
        }
        Ok(out)
    }
    pub fn table(&self, index: usize) -> Result<Option<Self>> {
        let Some(pos) = self.field(index, 4)? else {
            return Ok(None);
        };
        let delta = u32::from_le_bytes(bytes(self.data, pos)?) as usize;
        ensure!(delta >= 4, "invalid table offset");
        let target = pos
            .checked_add(delta)
            .ok_or_else(|| anyhow::anyhow!("table offset overflow"))?;
        Ok(Some(Self::at(self.data, target)?))
    }
    pub fn vector(&self, index: usize) -> Result<Option<&'a [u8]>> {
        let Some(pos) = self.field(index, 4)? else {
            return Ok(None);
        };
        let delta = u32::from_le_bytes(bytes(self.data, pos)?) as usize;
        ensure!(delta >= 4, "invalid vector offset");
        let target = pos
            .checked_add(delta)
            .ok_or_else(|| anyhow::anyhow!("vector offset overflow"))?;
        let len = u32::from_le_bytes(bytes(self.data, target)?) as usize;
        let start = target
            .checked_add(4)
            .ok_or_else(|| anyhow::anyhow!("vector overflow"))?;
        let end = start
            .checked_add(len)
            .ok_or_else(|| anyhow::anyhow!("vector length overflow"))?;
        Ok(Some(
            self.data
                .get(start..end)
                .ok_or_else(|| anyhow::anyhow!("vector outside buffer"))?,
        ))
    }
}

pub fn envelope(data: &[u8]) -> Result<(u8, Table<'_>)> {
    let root = Table::root(data)?;
    let kind = root.u8(1, 0)?;
    let Some(payload) = root.table(2)? else {
        bail!("LRP envelope has no payload")
    };
    Ok((kind, payload))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> Vec<u8> {
        [
            12u32.to_le_bytes().as_slice(),
            &[6, 0, 8, 0, 4, 0, 0, 0],
            &8i32.to_le_bytes(),
            &42u32.to_le_bytes(),
        ]
        .concat()
    }
    #[test]
    fn reads_table_and_defaults() {
        let b = sample();
        let t = Table::root(&b).unwrap();
        assert_eq!(t.u32(0, 0).unwrap(), 42);
        assert_eq!(t.u8(4, 9).unwrap(), 9);
    }
    #[test]
    fn rejects_truncated_object() {
        let b = sample();
        assert!(Table::root(&b[..b.len() - 1]).is_err());
    }
    #[test]
    fn rejects_negative_or_outside_vtable() {
        let mut b = sample();
        b[12..16].copy_from_slice(&i32::MAX.to_le_bytes());
        assert!(Table::root(&b).is_err());
    }
    #[test]
    fn permits_forward_vtable() {
        let b = [
            4u32.to_le_bytes().as_slice(),
            &(-8i32).to_le_bytes(),
            &12u32.to_le_bytes(),
            &[6, 0, 8, 0, 4, 0],
        ]
        .concat();
        assert_eq!(Table::root(&b).unwrap().u32(0, 0).unwrap(), 12);
    }
    #[test]
    fn rejects_field_overrun() {
        let mut b = sample();
        b[8] = 7;
        assert!(Table::root(&b).unwrap().u32(0, 0).is_err());
    }
}
