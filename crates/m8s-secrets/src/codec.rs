// Internal keyring envelope only; FileStore never accepts this binary format.
// No value-bearing type implements formatting/serde.
use crate::{Error, SecretInfo, SecretName, SecretValue};
use std::collections::BTreeMap;
use zeroize::Zeroizing;

pub(crate) const MAX_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_VALUE: usize = 64 * 1024;
pub(crate) const MAX_ENTRIES: usize = 1024;
pub(crate) type Entries = BTreeMap<SecretName, (SecretValue, Option<i64>)>;

pub(crate) fn decode(bytes: &[u8]) -> Result<Entries, Error> {
    if bytes.len() > MAX_BYTES {
        return Err(Error::InvalidData);
    }
    let mut rest = bytes;
    fn take<'a>(r: &mut &'a [u8], n: usize) -> Result<&'a [u8], Error> {
        if r.len() < n {
            return Err(Error::InvalidData);
        }
        let (v, tail) = r.split_at(n);
        *r = tail;
        Ok(v)
    }
    if take(&mut rest, 8)? != b"M8SSEC01" {
        return Err(Error::InvalidData);
    }
    let count = u32::from_le_bytes(take(&mut rest, 4)?.try_into().unwrap()) as usize;
    if count > MAX_ENTRIES {
        return Err(Error::InvalidData);
    }
    let mut entries = Entries::new();
    for _ in 0..count {
        let len = u16::from_le_bytes(take(&mut rest, 2)?.try_into().unwrap()) as usize;
        if len > 128 {
            return Err(Error::InvalidData);
        }
        let name = SecretName::new(
            std::str::from_utf8(take(&mut rest, len)?).map_err(|_| Error::InvalidData)?,
        )
        .map_err(|_| Error::InvalidData)?;
        let expiry = match take(&mut rest, 1)?[0] {
            0 => None,
            1 => Some(i64::from_le_bytes(take(&mut rest, 8)?.try_into().unwrap())),
            _ => return Err(Error::InvalidData),
        };
        let len = u32::from_le_bytes(take(&mut rest, 4)?.try_into().unwrap()) as usize;
        if len > MAX_VALUE {
            return Err(Error::InvalidData);
        }
        let value = SecretValue::new(take(&mut rest, len)?.to_vec());
        if entries.insert(name, (value, expiry)).is_some() {
            return Err(Error::InvalidData);
        }
    }
    if !rest.is_empty() {
        return Err(Error::InvalidData);
    }
    Ok(entries)
}

pub(crate) fn encode(entries: &Entries) -> Result<Zeroizing<Vec<u8>>, Error> {
    let size = entries.iter().try_fold(12usize, |size, (n, (v, e))| {
        if v.expose().len() > MAX_VALUE {
            return Err(Error::InvalidData);
        }
        Ok(
            size + 2
                + n.as_str().len()
                + 1
                + if e.is_some() { 8 } else { 0 }
                + 4
                + v.expose().len(),
        )
    })?;
    if entries.len() > MAX_ENTRIES || size > MAX_BYTES {
        return Err(Error::InvalidData);
    }
    let mut out = Zeroizing::new(Vec::with_capacity(size));
    out.extend_from_slice(b"M8SSEC01");
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for (n, (v, e)) in entries {
        out.extend_from_slice(&(n.as_str().len() as u16).to_le_bytes());
        out.extend_from_slice(n.as_str().as_bytes());
        out.push(u8::from(e.is_some()));
        if let Some(e) = e {
            out.extend_from_slice(&e.to_le_bytes());
        }
        out.extend_from_slice(&(v.expose().len() as u32).to_le_bytes());
        out.extend_from_slice(v.expose());
    }
    Ok(out)
}
pub(crate) fn get(entries: &Entries, name: &SecretName, now: i64) -> Option<SecretValue> {
    entries
        .get(name)
        .filter(|(_, e)| e.is_none_or(|e| now < e))
        .map(|(v, _)| SecretValue::new(v.expose().to_vec()))
}
pub(crate) fn list(entries: &Entries, now: i64) -> Vec<SecretInfo> {
    entries
        .iter()
        .filter(|(_, (_, e))| e.is_none_or(|e| now < e))
        .map(|(n, (_, e))| SecretInfo {
            name: n.clone(),
            expires_at: *e,
        })
        .collect()
}
