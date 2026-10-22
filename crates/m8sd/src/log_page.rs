//! Bounded separate response, fully rendered before any socket write.
use m8s_api::{LogPageResponse, MAX_FRAME};
use m8sd::Store;
use std::io::{self, Write};
struct Buffer(Vec<u8>);
impl Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_FRAME.saturating_sub(self.0.len()) {
            return Err(io::Error::other("log page output unavailable"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn render(response: &LogPageResponse) -> io::Result<Vec<u8>> {
    let mut out = Buffer(Vec::with_capacity(MAX_FRAME));
    serde_json::to_writer(&mut out, response)?;
    out.write_all(b"\n")?;
    Ok(out.0)
}
pub(super) fn frame(store: &Store, after: u64, limit: u32) -> Vec<u8> {
    let page = store
        .log_page(after, limit)
        .ok()
        .map(|(records, has_more)| LogPageResponse {
            next_cursor: records.last().map_or(after, |r| r.seq as u64),
            ok: true,
            mode: "mock-only".into(),
            inference: "not-carried".into(),
            error: None,
            records,
            has_more,
        });
    encode(page, after)
}
fn encode(page: Option<LogPageResponse>, after: u64) -> Vec<u8> {
    if let Some(frame) = page.as_ref().and_then(|p| render(p).ok()) {
        return frame;
    }
    // A fixed, small refusal, including for an invalid cursor. Empty records on
    // ok=false say nothing about history. No partially rendered success escapes.
    render(&LogPageResponse {
        ok: false,
        mode: "mock-only".into(),
        inference: "not-carried".into(),
        error: Some("log-page-unavailable".into()),
        records: vec![],
        next_cursor: after,
        has_more: false,
    })
    .expect("fixed log-page refusal fits frame")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_frame_cap_including_newline_and_escaped_messages() {
        let mut response = LogPageResponse {
            ok: true,
            mode: "mock-only".into(),
            inference: "not-carried".into(),
            error: None,
            records: vec![],
            next_cursor: 0,
            has_more: false,
        };
        assert!(render(&response).unwrap().ends_with(b"\n"));
        response.records = (1..=200)
            .map(|seq| m8s_api::LogEvent {
                seq,
                at: 0,
                message: "\u{1b}".repeat(4096),
            })
            .collect();
        assert!(render(&response).is_err());
        let refused: LogPageResponse = serde_json::from_slice(&encode(Some(response), 42)).unwrap();
        assert!(!refused.ok);
        assert_eq!(refused.error.as_deref(), Some("log-page-unavailable"));
        assert!(refused.records.is_empty());
        assert_eq!(refused.next_cursor, 42);
        assert!(!refused.has_more);
        assert!(
            !serde_json::from_slice::<LogPageResponse>(&encode(None, 42))
                .unwrap()
                .ok
        );
        // Exercise exact serialized newline boundary, independently of the
        // Store's much smaller per-message admission cap.
        let mut boundary = refused;
        boundary.records.push(m8s_api::LogEvent {
            seq: 43,
            at: 0,
            message: String::new(),
        });
        let base = render(&boundary).unwrap().len();
        boundary.records[0].message = "x".repeat(MAX_FRAME - base);
        assert_eq!(render(&boundary).unwrap().len(), MAX_FRAME);
        boundary.records[0].message.push('x');
        assert!(render(&boundary).is_err());
        let mut b = Buffer(vec![0; MAX_FRAME]);
        assert!(b.write_all(b"\n").is_err());
        assert_eq!(b.0.len(), MAX_FRAME);
    }
}
