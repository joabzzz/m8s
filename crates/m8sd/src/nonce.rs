//! Durable single-use nonce ledger, not a bootstrap transport.
//! Caller supplies cryptographically random nonce bytes; this module never mints credentials.
use crate::{Error, Result, Store};
use rusqlite::params;
use sha2::{Digest, Sha256};

/// Deliberately not Debug/Serialize. All public fields are non-secret instance binding metadata.
pub struct Binding<'a> {
    pub instance_id: &'a str,
    pub source_ip: &'a str,
    pub node_endpoint: &'a str,
    pub image_digest: &'a str,
    pub self_destruct_present: bool,
}
impl Store {
    fn nonce_schema(&self) -> Result<()> {
        self.conn.execute_batch("CREATE TABLE IF NOT EXISTS nonce(hash BLOB PRIMARY KEY, instance_id TEXT NOT NULL, ip TEXT NOT NULL, digest TEXT NOT NULL, issued_at INTEGER NOT NULL, redeemed_at INTEGER, node_endpoint TEXT);")?;
        Ok(())
    }
    /// Only bindings for an already persisted instance id can be issued.
    pub fn issue_nonce(
        &self,
        nonce: &[u8],
        instance_id: &str,
        ip: &str,
        digest: &str,
        now: i64,
    ) -> Result<()> {
        self.nonce_schema()?;
        if nonce.len() < 32
            || ip.parse::<std::net::IpAddr>().is_err()
            || !digest.starts_with("sha256:")
            || digest.len() != 71
            || !digest[7..].bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(Error::Invalid);
        }
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM gpu_node WHERE instance_id=? AND phase='\"Booting\"')",
            [instance_id],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(Error::Invalid);
        }
        self.conn.execute(
            "INSERT INTO nonce(hash,instance_id,ip,digest,issued_at) VALUES (?,?,?,?,?)",
            params![
                Sha256::digest(nonce).as_slice(),
                instance_id,
                ip,
                digest,
                now
            ],
        )?;
        Ok(())
    }
    /// One atomic UPDATE checks expiry, instance, IP, digest and consumption state.
    /// The endpoint must be transport-authenticated by the future pinned iroh implementation.
    pub fn redeem_nonce(&self, nonce: &[u8], binding: Binding<'_>, now: i64) -> Result<bool> {
        self.nonce_schema()?;
        if !binding.self_destruct_present || binding.node_endpoint.is_empty() || nonce.len() < 32 {
            return Ok(false);
        }
        let n=self.conn.execute("UPDATE nonce SET redeemed_at=?,node_endpoint=? WHERE hash=? AND instance_id=? AND ip=? AND digest=? AND redeemed_at IS NULL AND issued_at<=? AND issued_at>? AND EXISTS(SELECT 1 FROM gpu_node WHERE instance_id=nonce.instance_id AND phase='\"Booting\"')",
            params![now,binding.node_endpoint,Sha256::digest(nonce).as_slice(),binding.instance_id,binding.source_ip,binding.image_digest,now,now.saturating_sub(900)])?;
        Ok(n == 1)
    }
}
