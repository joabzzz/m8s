//! Bounded ASCII escaping of already JSON-encoded output (caller owns framing).
pub(super) fn ascii(encoded: &str) -> Result<Vec<u8>, ()> {
    let mut safe = String::new();
    for c in encoded.chars() {
        if c.is_ascii() && c != '\u{7f}' {
            safe.push(c);
        } else {
            use std::fmt::Write;
            for unit in c.encode_utf16(&mut [0; 2]) {
                write!(safe, "\\u{unit:04x}").map_err(|_| ())?;
            }
        }
        if safe.len() >= m8s_api::MAX_FRAME {
            return Err(());
        }
    }
    Ok(safe.into_bytes())
}
