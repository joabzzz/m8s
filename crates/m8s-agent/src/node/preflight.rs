//! Native public-ID observation only; never an admission permit or key decoder.
use super::*;
use std::time::{Duration, Instant};
const CAP: usize = 256;
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicId {
    node_id: String,
}
fn decode(bytes: &[u8], expected: EndpointId) -> Result<()> {
    let id: PublicId = serde_json::from_slice(bytes).map_err(|_| Error::NativeId)?;
    if id.node_id.len() != 64 {
        return Err(Error::NativeId);
    }
    for (pair, expected) in id
        .node_id
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .zip(expected.0)
    {
        let nibble = |c| match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            _ => None,
        };
        let high = nibble(pair[0]).ok_or(Error::NativeId)?;
        let low = nibble(pair[1]).ok_or(Error::NativeId)?;
        if high * 16 + low != expected {
            return Err(Error::NativeId);
        }
    }
    Ok(())
}
impl NodeMaterial {
    /// Run the selected native binary's *offline* public node-ID command once.
    /// No key parsing/generation here; the native command reads the explicit node
    /// key. This observation is not TLS receipt, admission, native fail-closed
    /// certificate enforcement, or proof a later serve process uses the same key.
    /// The coordinator must still compose BootGate and current launch policy.
    pub fn check_native_node_id(
        &self,
        verifier: &Mp001CertificateVerifier,
        now: i64,
        timeout: Duration,
    ) -> Result<()> {
        let deadline = Instant::now().checked_add(timeout).ok_or(Error::NativeId)?;
        if timeout.is_zero() || timeout > Duration::from_secs(60) {
            return Err(Error::NativeId);
        }
        self.recheck(verifier, now)?;
        self.native_id(deadline)?;
        self.recheck(verifier, now)?;
        if Instant::now() >= deadline {
            return Err(Error::NativeId);
        }
        Ok(())
    }
    pub(super) fn native_id(&self, deadline: Instant) -> Result<()> {
        check(&self.anchors)?;
        if Instant::now() >= deadline {
            return Err(Error::NativeId);
        }
        let mut command = Command::new(&self.launch.program);
        command
            .args(["auth", "node-id", "--key-path"])
            .arg(&self.launch.node_key)
            .arg("--json")
            .env_clear()
            .env("HOME", &self.root)
            .env("MESH_LLM_ANALYTICS", "0")
            .env("DO_NOT_TRACK", "1");
        let bytes = super::process::capture(command, deadline, CAP)?;
        check(&self.anchors)?;
        decode(&bytes, self.launch.node)?;
        if Instant::now() >= deadline {
            return Err(Error::NativeId);
        }
        Ok(())
    }
}
