//! Complete bounded legacy encoding, not bounded snapshot collection/atomic delivery.
use m8s_api::{MAX_FRAME, Response};
use std::io::{self, Write};
struct Buffer(Vec<u8>);
impl Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_FRAME.saturating_sub(self.0.len()) {
            return Err(io::Error::other("control response unavailable"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn render(response: &Response) -> io::Result<Vec<u8>> {
    // Fixed capacity plus checked-before-extend writes prevents growth past cap.
    let mut out = Buffer(Vec::with_capacity(MAX_FRAME));
    serde_json::to_writer(&mut out, response)?;
    out.write_all(b"\n")?;
    Ok(out.0)
}
pub(super) fn frame(response: &Response) -> Vec<u8> {
    if let Ok(frame) = render(response) {
        return frame;
    }
    // The fixed refusal is much smaller than MAX_FRAME. Never expose any bytes
    // from the failed encoding, nor claim a preceding mutation was rolled back.
    render(&Response {
        ok: false,
        mode: "mock-only".into(),
        nodes: vec![],
        tiers: vec![],
        events: vec![],
        error: Some(
            "control response unavailable; outcome uncertain; empty fields are not absence".into(),
        ),
        secure_bootstrap: "unavailable; nodes cannot become Ready".into(),
    })
    .expect("fixed legacy control refusal fits frame")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn response(ok: bool) -> Response {
        Response {
            ok,
            mode: "mock-only".into(),
            nodes: vec![m8s_core::GpuNode {
                uid: "synthetic-node".into(),
                tier: "small".into(),
                owner: "synthetic-owner".into(),
                instance_id: None,
                phase: m8s_core::Phase::Booting,
                created_at: 42,
            }],
            tiers: vec![("small".into(), 1)],
            events: vec!["synthetic event".into()],
            error: (!ok).then(|| "control request refused".into()),
            secure_bootstrap: "unavailable; nodes cannot become Ready".into(),
        }
    }
    #[test]
    fn fitting_success_and_failure_are_exact_old_bytes_without_response_mutation() {
        for ok in [true, false] {
            let r = response(ok);
            let original = serde_json::to_vec(&r).unwrap();
            let mut expected = original.clone();
            expected.push(b'\n');
            assert_eq!(frame(&r), expected);
            assert_eq!(serde_json::to_vec(&r).unwrap(), original);
        }
    }
    #[test]
    fn newline_inclusive_exact_cap_fits_and_one_more_falls_back() {
        let mut r = response(true);
        r.events = vec![String::new()];
        let base = render(&r).unwrap().len();
        r.events[0] = "x".repeat(MAX_FRAME - base);
        assert_eq!(render(&r).unwrap().len(), MAX_FRAME);
        assert_eq!(frame(&r).len(), MAX_FRAME);
        r.events[0].push('x');
        assert!(render(&r).is_err());
        assert_refusal(&frame(&r));
    }
    fn assert_refusal(bytes: &[u8]) {
        assert!(bytes.len() < 1024);
        assert!(bytes.ends_with(b"\n"));
        assert!(!bytes[..bytes.len() - 1].contains(&b'\n'));
        let r: Response = serde_json::from_slice(bytes).unwrap();
        assert!(!r.ok);
        assert_eq!(r.mode, "mock-only");
        assert_eq!(r.secure_bootstrap, "unavailable; nodes cannot become Ready");
        assert!(r.nodes.is_empty() && r.tiers.is_empty() && r.events.is_empty());
        assert_eq!(
            r.error.as_deref(),
            Some("control response unavailable; outcome uncertain; empty fields are not absence")
        );
    }
    #[test]
    fn escaping_and_large_unicode_emit_only_fixed_refusal() {
        let mut r = response(true);
        r.events = vec!["\u{1b}".repeat(12000)];
        assert!(r.events[0].len() < MAX_FRAME);
        assert!(render(&r).is_err());
        let fixed = frame(&r);
        assert_refusal(&fixed);
        r.events = vec!["sensitive\u{202e}é".repeat(6000)];
        assert!(render(&r).is_err());
        assert_eq!(frame(&r), fixed);
        assert!(!String::from_utf8(fixed).unwrap().contains("sensitive"));
    }
    #[test]
    fn buffer_checks_before_extending_and_never_exceeds_fixed_capacity() {
        let mut b = Buffer(Vec::with_capacity(MAX_FRAME));
        b.write_all(&vec![b'x'; MAX_FRAME - 1]).unwrap();
        let before = b.0.clone();
        assert!(b.write_all(b"xx").is_err());
        assert_eq!(b.0, before);
        b.write_all(b"\n").unwrap();
        assert_eq!(b.0.len(), MAX_FRAME);
        assert_eq!(b.0.capacity(), MAX_FRAME);
        assert!(b.write_all(b"x").is_err());
        assert_eq!(b.0.len(), MAX_FRAME);
    }
}
