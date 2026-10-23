//! Destructive in-memory classification. Never retain or forward native lines:
//! native output may contain private bootstrap material. Only closed labels exit.
use std::io::Read;

fn category(line: &[u8]) -> Option<&'static str> {
    let text = std::str::from_utf8(line).ok()?.to_ascii_lowercase();
    let error = text.contains("error")
        || text.contains("failed")
        || text.contains("not found")
        || text.contains("no such file")
        || text.contains("cannot")
        || text.contains("unable");
    if text.contains("unexpected argument") || text.contains("unrecognized option") {
        Some("native-argument-error")
    } else if error && (text.contains("certificate") || text.contains("ownership")) {
        Some("native-certificate-error")
    } else if error && text.contains("cuda") {
        Some("native-cuda-error")
    } else if text.contains("runtime")
        && (error || text.contains("no compatible") || text.contains("incompatible"))
    {
        Some("native-runtime-error")
    } else if error && (text.contains("revision") || text.contains("model ref")) {
        Some("native-model-reference-error")
    } else if error && (text.contains("download") || text.contains("hugging")) {
        Some("native-download-error")
    } else if error {
        Some("native-other-error")
    } else {
        None
    }
}

pub(crate) fn drain(mut input: impl Read, mut emit: impl FnMut(&'static str)) {
    let mut block = [0u8; 512];
    let mut line = Vec::with_capacity(8192);
    let mut overflow = false;
    let mut seen = std::collections::BTreeSet::new();
    loop {
        let n = match input.read(&mut block) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        for byte in &block[..n] {
            if *byte == b'\n' {
                if !overflow
                    && let Some(label) = category(&line)
                    && seen.insert(label)
                {
                    emit(label);
                }
                line.fill(0);
                line.clear();
                overflow = false;
            } else if line.len() < 8192 && !overflow {
                line.push(*byte);
            } else {
                line.fill(0);
                line.clear();
                overflow = true;
            }
        }
        block.fill(0);
    }
    if !overflow
        && let Some(label) = category(&line)
        && seen.insert(label)
    {
        emit(label);
    }
    line.fill(0);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_values_and_oversized_lines_never_escape_classifier() {
        let mut output = Vec::new();
        let input = b"invite=synthetic-private\nerror downloading synthetic-private\nerror downloading other-private\nCUDA failed private-value\n";
        drain(&input[..], |v| output.push(v));
        assert_eq!(output, ["native-download-error", "native-cuda-error"]);
        assert_eq!(
            category(b"libcuda.so.1: cannot open shared object file"),
            Some("native-cuda-error")
        );
        assert_eq!(
            category(b"no compatible native runtime found"),
            Some("native-runtime-error")
        );
        let mut oversized = vec![b'x'; 9000];
        oversized.extend_from_slice(b" error private\nunknown\n");
        output.clear();
        drain(&oversized[..], |v| output.push(v));
        assert!(output.is_empty());
    }
}
