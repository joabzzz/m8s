//! Deliberately restricted TOML v1 schema, not a general-purpose TOML parser.
//! All value-bearing allocations are zeroizing; errors never retain input.
use crate::{
    Error, SecretName, SecretValue,
    codec::{Entries, MAX_BYTES, MAX_ENTRIES, MAX_VALUE},
};
use zeroize::Zeroizing;

pub(crate) fn decode(bytes: &[u8]) -> Result<Entries, Error> {
    if bytes.len() > MAX_BYTES {
        return Err(Error::InvalidData);
    }
    if bytes.iter().enumerate().any(|(i, b)| {
        !matches!(*b, b'\t' | b'\n' | b'\r' | 0x20..=0x7e)
            || (*b == b'\r' && bytes.get(i + 1) != Some(&b'\n'))
    }) {
        return Err(Error::InvalidData);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| Error::InvalidData)?;
    let mut lines = text
        .lines()
        .map(|line| line.trim_matches([' ', '\t']))
        .filter(|l| !l.is_empty() && !l.starts_with('#'));
    if lines.next() != Some("version = 1") {
        return Err(Error::InvalidData);
    }
    let mut entries = Entries::new();
    let mut record = false;
    let (mut name, mut value, mut expiry) = (None, None, None);
    let mut expiry_seen = false;
    fn finish(
        entries: &mut Entries,
        name: &mut Option<SecretName>,
        value: &mut Option<SecretValue>,
        expiry: Option<i64>,
    ) -> Result<(), Error> {
        if entries.len() >= MAX_ENTRIES {
            return Err(Error::InvalidData);
        }
        let n = name.take().ok_or(Error::InvalidData)?;
        let v = value.take().ok_or(Error::InvalidData)?;
        if entries.insert(n, (v, expiry)).is_some() {
            return Err(Error::InvalidData);
        }
        Ok(())
    }
    for line in lines {
        if line == "[[secrets]]" {
            if record {
                finish(&mut entries, &mut name, &mut value, expiry)?;
            }
            record = true;
            expiry = None;
            expiry_seen = false;
            continue;
        }
        if !record {
            return Err(Error::InvalidData);
        }
        let (key, raw) = line.split_once('=').ok_or(Error::InvalidData)?;
        let raw = raw.trim();
        match key.trim() {
            "name" if name.is_none() => {
                let s = quoted(raw)?;
                name = Some(SecretName::new(s).map_err(|_| Error::InvalidData)?);
            }
            "value_hex" if value.is_none() => {
                let s = quoted(raw)?.as_bytes();
                if s.len() > MAX_VALUE * 2 || s.len() % 2 != 0 {
                    return Err(Error::InvalidData);
                }
                // Exact capacity avoids leaving secret bytes in reallocated buffers.
                let mut out = Zeroizing::new(Vec::with_capacity(s.len() / 2));
                for pair in s.chunks_exact(2) {
                    fn nibble(b: u8) -> Result<u8, Error> {
                        match b {
                            b'0'..=b'9' => Ok(b - b'0'),
                            b'a'..=b'f' => Ok(b - b'a' + 10),
                            _ => Err(Error::InvalidData),
                        }
                    }
                    out.push(nibble(pair[0])? * 16 + nibble(pair[1])?);
                }
                value = Some(SecretValue::new(std::mem::take(&mut *out)));
            }
            "expires_at" if !expiry_seen => {
                // Canonical decimal integers only; no alternate representations.
                let e: i64 = raw.parse().map_err(|_| Error::InvalidData)?;
                if e.to_string() != raw {
                    return Err(Error::InvalidData);
                }
                expiry = Some(e);
                expiry_seen = true;
            }
            _ => return Err(Error::InvalidData),
        }
    }
    if record {
        finish(&mut entries, &mut name, &mut value, expiry)?;
    }
    Ok(entries)
}
fn quoted(raw: &str) -> Result<&str, Error> {
    raw.strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .ok_or(Error::InvalidData)
}

pub(crate) fn encode(entries: &Entries) -> Result<Zeroizing<Vec<u8>>, Error> {
    // Exact size checked before allocation; no value-bearing reallocation.
    let size = entries.iter().try_fold(12usize, |size, (n, (v, e))| {
        if v.expose().len() > MAX_VALUE {
            return Err(Error::InvalidData);
        }
        size.checked_add(
            b"\n[[secrets]]\nname = \"".len()
                + n.as_str().len()
                + b"\"\nvalue_hex = \"".len()
                + v.expose().len() * 2
                + b"\"\n".len()
                + e.map_or(0, |expiry| {
                    b"expires_at = ".len() + expiry.to_string().len() + 1
                }),
        )
        .ok_or(Error::InvalidData)
    })?;
    if entries.len() > MAX_ENTRIES || size > MAX_BYTES {
        return Err(Error::InvalidData);
    }
    let mut out = Zeroizing::new(Vec::with_capacity(size));
    out.extend_from_slice(b"version = 1\n");
    for (n, (v, e)) in entries {
        out.extend_from_slice(b"\n[[secrets]]\nname = \"");
        out.extend_from_slice(n.as_str().as_bytes());
        out.extend_from_slice(b"\"\nvalue_hex = \"");
        for b in v.expose() {
            out.push(b"0123456789abcdef"[(b >> 4) as usize]);
            out.push(b"0123456789abcdef"[(b & 15) as usize]);
        }
        out.extend_from_slice(b"\"\n");
        if let Some(e) = e {
            out.extend_from_slice(b"expires_at = ");
            out.extend_from_slice(e.to_string().as_bytes());
            out.push(b'\n');
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn record() -> &'static str {
        "[[secrets]]\nname = \"test/session\"\nvalue_hex = \"007fff\"\nexpires_at = 10\n"
    }
    #[test]
    fn roundtrip_binary_values_expiry_and_canonical_toml() {
        let text = format!("version = 1\n{}", record());
        let entries = decode(text.as_bytes()).unwrap();
        assert_eq!(entries.values().next().unwrap().0.expose(), &[0, 127, 255]);
        assert!(
            crate::codec::get(&entries, &SecretName::new("test/session").unwrap(), 10).is_none()
        );
        let encoded = encode(&entries).unwrap();
        assert!(decode(&encoded).is_ok());
        assert!(
            std::str::from_utf8(&encoded)
                .unwrap()
                .starts_with("version = 1\n")
        );
        assert!(decode(b"version = 1\n# empty store\n").unwrap().is_empty());
        assert!(decode(b"version = 1\r\n").unwrap().is_empty());
    }
    #[test]
    fn rejects_duplicates_versions_unknown_fields_and_malformed_data_opaquely() {
        let base = format!("version = 1\n{}", record());
        let cases = [
            base.replace("version = 1", "version = 2"),
            base.replace("version = 1", "version = 1\nversion = 1"),
            format!("{base}{}", record()),
            format!("{base}name = \"test/other\"\n"),
            format!("{base}value_hex = \"ff\"\n"),
            format!("{base}expires_at = 11\n"),
            format!("{base}unknown = \"synthetic-sensitive-marker\"\n"),
            base.replace("007fff", "abc"),
            base.replace("007fff", "FF"),
            base.replace("007fff", "gg"),
            base.replace("expires_at = 10", "expires_at = 9223372036854775808"),
            base.replace("expires_at = 10", "expires_at = 01"),
            base.replace("name = \"test/session\"", "name = \"bad_name\""),
            base.replace("value_hex = \"007fff\"\n", ""),
            format!("{base}[[secrets]]\n"),
            format!("{base}trailing garbage"),
            "M8SSEC01".into(),
            "version = 1\n\u{a0}".into(),
            "version = 1\r".into(),
        ];
        for bytes in cases {
            let err = decode(bytes.as_bytes()).err().expect("must fail closed");
            assert_eq!(err, Error::InvalidData);
            assert!(!format!("{err:?} {err}").contains("synthetic-sensitive-marker"));
        }
        assert!(decode(&[0xff]).is_err());
    }
    #[test]
    fn bounds_and_no_binary_migration_or_parser_panics() {
        assert!(decode(&vec![b' '; MAX_BYTES + 1]).is_err());
        let value = "ff".repeat(MAX_VALUE);
        let text =
            format!("version = 1\n[[secrets]]\nname = \"test/max\"\nvalue_hex = \"{value}\"\n");
        assert!(decode(text.as_bytes()).is_ok());
        assert!(decode(text.replace(&value, &(value.clone() + "ff")).as_bytes()).is_err());
        let mut entries = Entries::new();
        for i in 0..=MAX_ENTRIES {
            entries.insert(
                SecretName::new(&format!("test/{i}")).unwrap(),
                (SecretValue::new(vec![]), None),
            );
        }
        assert!(encode(&entries).is_err());
        let many = (0..=MAX_ENTRIES)
            .map(|i| format!("[[secrets]]\nname = \"test/{i}\"\nvalue_hex = \"\"\n"))
            .collect::<String>();
        assert!(decode(format!("version = 1\n{many}").as_bytes()).is_err());
        let mut too_big = Entries::new();
        for i in 0..8 {
            too_big.insert(
                SecretName::new(&format!("test/{i}")).unwrap(),
                (SecretValue::new(vec![0; MAX_VALUE]), None),
            );
        }
        assert!(encode(&too_big).is_err());
        assert!(decode(&crate::codec::encode(&Entries::new()).unwrap()).is_err());
        for size in 0..4096 {
            let bytes: Vec<u8> = (0..size).map(|i| ((i * 31 + size) % 256) as u8).collect();
            assert!(decode(&bytes).is_err());
        }
    }
}
