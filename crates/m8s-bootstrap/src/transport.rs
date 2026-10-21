//! Real iroh QUIC/TLS adapter. No provider calls or plaintext fallback.
//! Tests requiring endpoint bind are separately runtime-qualified: BSD AF_ROUTE
//! netmon capability is denied in the current managed sandbox.
use crate::{wire, *};
use iroh::{
    Endpoint, EndpointAddr,
    endpoint::{Connection, Incoming, IncomingAddr, RecvStream, SendStream},
};
use std::{future::Future, time::Duration};

const IO_TIMEOUT: Duration = Duration::from_secs(5);
async fn bounded<T>(future: impl Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(IO_TIMEOUT, future)
        .await
        .map_err(|_| Error::Transport)?
}
/// Allocate once under zeroizing ownership BEFORE reading plaintext. No growing
/// read_to_end Vec can free old plaintext allocations without zeroizing them.
async fn read_secret_frame(recv: &mut RecvStream, limit: usize) -> Result<Zeroizing<Vec<u8>>> {
    let mut bytes = Zeroizing::new(vec![0; limit.checked_add(1).ok_or(Error::Refused)?]);
    bounded(async {
        let mut used = 0;
        loop {
            match recv
                .read(&mut bytes[used..])
                .await
                .map_err(|_| Error::Transport)?
            {
                Some(0) => return Err(Error::Transport),
                Some(n) => {
                    used += n;
                    if used > limit {
                        return Err(Error::Refused);
                    }
                }
                None => {
                    bytes.truncate(used);
                    return Ok(bytes);
                }
            }
        }
    })
    .await
}
fn direct_ip(connection: &Connection) -> Result<IpAddr> {
    // Require the selected authenticated path, not a relay or a claimed request IP.
    let paths = connection.paths();
    let mut selected = paths.iter().filter(|p| p.is_selected());
    let path = selected.next().ok_or(Error::Refused)?;
    if selected.next().is_some() {
        return Err(Error::Refused);
    }
    match path.remote_addr() {
        iroh::TransportAddr::Ip(address) => Ok(address.ip()),
        _ => Err(Error::Refused),
    }
}
async fn handshake(incoming: Incoming, alpn: &[u8]) -> Result<Connection> {
    // Await full authentication. Never call into_0rtt or accept early-data streams.
    let connection = bounded(async { incoming.await.map_err(|_| Error::Transport) }).await?;
    if connection.alpn() != alpn {
        connection.close(1u32.into(), b"refused");
        return Err(Error::Refused);
    }
    Ok(connection)
}
async fn write_frame(send: &mut SendStream, frame: &[u8]) -> Result<()> {
    bounded(async {
        send.write_all(frame).await.map_err(|_| Error::Transport)?;
        send.finish().map_err(|_| Error::Transport)?;
        Ok(())
    })
    .await
}

/// Original peer-bound boot connection. Only a real completed handshake creates it.
/// Not Clone/Debug/Deserialize; drop closes the connection, without restoring a nonce.
pub struct BootConnection {
    connection: Connection,
    session: IrohSession,
    send: Option<SendStream>,
    received: bool,
}
impl Drop for BootConnection {
    fn drop(&mut self) {
        self.connection.close(0u32.into(), b"boot complete");
    }
}
pub async fn accept_boot(incoming: Incoming, owner: EndpointId) -> Result<BootConnection> {
    // Relay origin cannot establish the ADR-0006 provider IP binding.
    let origin = match incoming.remote_addr() {
        IncomingAddr::Ip(address) => address.ip(),
        _ => return Err(Error::Refused),
    };
    let connection = handshake(incoming, BOOT_ALPN).await?;
    if direct_ip(&connection)? != origin {
        return Err(Error::Refused);
    }
    let session = IrohSession {
        owner,
        peer: EndpointId(*connection.remote_id().as_bytes()),
        source_ip: origin,
        alpn: BOOT_ALPN.to_vec(),
    };
    Ok(BootConnection {
        connection,
        session,
        send: None,
        received: false,
    })
}
impl BootConnection {
    pub fn session(&self) -> &IrohSession {
        &self.session
    }
    pub async fn receive(&mut self) -> Result<Redemption> {
        if self.received
            || self.connection.close_reason().is_some()
            || direct_ip(&self.connection)? != self.session.source_ip
        {
            return Err(Error::Refused);
        }
        // Mark before awaiting; cancellation cannot restart processing a partial stream.
        self.received = true;
        let (send, mut recv) = bounded(async {
            self.connection
                .accept_bi()
                .await
                .map_err(|_| Error::Transport)
        })
        .await?;
        self.send = Some(send);
        let bytes = read_secret_frame(&mut recv, wire::REQUEST_LIMIT).await?;
        if recv.is_0rtt() {
            return Err(Error::Refused);
        }
        let request = wire::decode_request(&bytes)?;
        if request.node != self.session.peer
            || direct_ip(&self.connection)? != self.session.source_ip
        {
            return Err(Error::Refused);
        }
        Ok(request)
    }
    /// Delivers only on the original authenticated stream, never a caller-selected peer.
    pub async fn deliver(mut self, pending: PendingDelivery, now: i64) -> Result<()> {
        if !self.received
            || self.connection.close_reason().is_some()
            || pending.claims.owner != self.session.owner
            || pending.claims.node != self.session.peer
            || now < pending.claims.issued_at
            || now >= pending.claims.expires_at
            || direct_ip(&self.connection)? != self.session.source_ip
        {
            return Err(Error::Refused);
        }
        let frame = wire::encode_bundle(&pending)?;
        let send = self.send.as_mut().ok_or(Error::Refused)?;
        write_frame(send, &frame).await?;
        // Keep connection alive until peer acknowledges all stream data. Delivery is
        // uncertain on cancellation/error, but the ledger has already consumed nonce.
        bounded(async {
            send.stopped().await.map_err(|_| Error::Transport)?;
            Ok(())
        })
        .await
    }
}

/// Secret-bearing client result. A real CertificateVerifier must validate MP-001
/// bytes before this is returned; no production verifier is supplied by this crate.
pub struct ReceivedBundle {
    pub claims: CertificateClaims,
    pub certificate: NodeCertificate,
    pub join_token: Zeroizing<Vec<u8>>,
    pub acl: ControlAcl,
    pub settings: BundleSettings,
}
pub async fn redeem_over_iroh(
    endpoint: &Endpoint,
    owner: EndpointAddr,
    request: Redemption,
    verifier: &impl CertificateVerifier,
    clock: impl Fn() -> i64,
) -> Result<ReceivedBundle> {
    let expected_owner = EndpointId(*owner.id.as_bytes());
    if EndpointId(*endpoint.id().as_bytes()) != request.node {
        return Err(Error::Refused);
    }
    // EndpointAddr pins owner key; iroh rejects a different remote TLS identity.
    let connection = bounded(async {
        endpoint
            .connect(owner, BOOT_ALPN)
            .await
            .map_err(|_| Error::Transport)
    })
    .await?;
    let result = async {
        if connection.alpn() != BOOT_ALPN
            || EndpointId(*connection.remote_id().as_bytes()) != expected_owner
        {
            return Err(Error::Refused);
        }
        let (mut send, mut recv) =
            bounded(async { connection.open_bi().await.map_err(|_| Error::Transport) }).await?;
        let frame = wire::encode_request(&request)?;
        write_frame(&mut send, &frame).await?;
        let bytes = read_secret_frame(&mut recv, wire::BUNDLE_LIMIT).await?;
        if recv.is_0rtt() {
            return Err(Error::Refused);
        }
        let bundle = wire::decode_bundle(&bytes)?;
        let now = clock();
        if bundle.claims.owner != expected_owner
            || bundle.claims.node != request.node
            || now < bundle.claims.issued_at
            || now >= bundle.claims.expires_at
        {
            return Err(Error::Refused);
        }
        verifier
            .verify(&bundle.certificate, &bundle.claims)
            .map_err(|_| Error::Certificate)?;
        Ok(ReceivedBundle {
            claims: bundle.claims,
            certificate: bundle.certificate,
            join_token: bundle.join_token,
            acl: bundle.acl,
            settings: bundle.settings,
        })
    }
    .await;
    if result.is_ok() {
        bounded(async {
            connection.closed().await;
            Ok(())
        })
        .await?;
    }
    connection.close(0u32.into(), b"boot complete");
    result
}

/// Live owner-controlled ACL/revocation policy. Update this same shared object;
/// every frame rechecks it after IO and before DMS authorization.
pub struct ControlPolicy {
    owner: EndpointId,
    state: std::sync::Arc<std::sync::RwLock<ControlPolicyState>>,
}
struct ControlPolicyState {
    acl: ControlAcl,
    revoked: BTreeSet<EndpointId>,
    not_after: i64,
}
impl ControlPolicy {
    pub fn new(
        owner: EndpointId,
        delegates: impl IntoIterator<Item = EndpointId>,
        revoked: impl IntoIterator<Item = EndpointId>,
        not_after: i64,
    ) -> Result<Self> {
        if not_after <= 0 {
            return Err(Error::Refused);
        }
        Ok(Self {
            owner,
            state: std::sync::Arc::new(std::sync::RwLock::new(ControlPolicyState {
                acl: ControlAcl::with_delegates(owner, delegates),
                revoked: revoked.into_iter().collect(),
                not_after,
            })),
        })
    }
    /// Atomically installs current owner policy. A shared reference can be updated
    /// while receive_heartbeat awaits IO; post-read checks consult this live state.
    pub fn update(
        &self,
        delegates: impl IntoIterator<Item = EndpointId>,
        revoked: impl IntoIterator<Item = EndpointId>,
        not_after: i64,
    ) -> Result<()> {
        if not_after <= 0 {
            return Err(Error::Refused);
        }
        let next = ControlPolicyState {
            acl: ControlAcl::with_delegates(self.owner, delegates),
            revoked: revoked.into_iter().collect(),
            not_after,
        };
        *self.state.write().map_err(|_| Error::Refused)? = next;
        Ok(())
    }
    fn shared(&self) -> Self {
        Self {
            owner: self.owner,
            state: self.state.clone(),
        }
    }
    fn lease_deadline(&self) -> Result<i64> {
        Ok(self.state.read().map_err(|_| Error::Refused)?.not_after)
    }
    fn authorize(&self, owner: EndpointId, peer: EndpointId, now: i64) -> Result<()> {
        self.with_authorized(owner, peer, now, || ())
    }
    // Hold the authorization read guard until the short synchronous bridge
    // returns. Revocation update cannot complete before an authorized renewal.
    fn with_authorized<T>(
        &self,
        owner: EndpointId,
        peer: EndpointId,
        now: i64,
        apply: impl FnOnce() -> T,
    ) -> Result<T> {
        let state = self.state.read().map_err(|_| Error::Refused)?;
        if self.owner != owner
            || now < 0
            || now >= state.not_after
            || !state.acl.endpoints.contains(&peer)
            || state.revoked.contains(&peer)
        {
            Err(Error::Refused)
        } else {
            let result = apply();
            drop(state);
            Ok(result)
        }
    }
}

/// Opaque live encrypted control session. No public/synthetic constructor,
/// Deserialize or Clone. Only accept_control's real handshake can construct it.
pub struct AuthenticatedControlSession {
    connection: Connection,
    owner: EndpointId,
    peer: EndpointId,
    peer_endpoint: String,
    local_node: EndpointId,
    not_after: i64,
    last_seen: i64,
    sequence: Option<u64>,
    policy: ControlPolicy,
}
impl Drop for AuthenticatedControlSession {
    fn drop(&mut self) {
        self.connection.close(0u32.into(), b"control complete");
    }
}
pub async fn accept_control(
    endpoint: &Endpoint,
    owner: EndpointId,
    policy: &ControlPolicy,
    now: i64,
) -> Result<AuthenticatedControlSession> {
    // Capture the actual local TLS endpoint, never a claimed target node in a frame.
    let incoming = bounded(async { endpoint.accept().await.ok_or(Error::Unavailable) }).await?;
    let connection = handshake(incoming, CONTROL_ALPN).await?;
    let peer = EndpointId(*connection.remote_id().as_bytes());
    policy.authorize(owner, peer, now)?;
    Ok(AuthenticatedControlSession {
        peer_endpoint: connection.remote_id().to_string(),
        local_node: EndpointId(*endpoint.id().as_bytes()),
        connection,
        owner,
        peer,
        not_after: policy.lease_deadline()?,
        last_seen: now,
        sequence: None,
        policy: policy.shared(),
    })
}
impl AuthenticatedControlSession {
    pub fn peer_endpoint(&self) -> &str {
        &self.peer_endpoint
    }
    pub fn node_endpoint(&self) -> EndpointId {
        self.local_node
    }
    pub fn bound_owner(&self) -> EndpointId {
        self.owner
    }
    pub fn alpn(&self) -> &[u8] {
        self.connection.alpn()
    }
    /// Fixed session lease ceiling, not current authorization. Current policy can
    /// revoke or shorten it; evidence consumption rechecks policy for every event.
    pub fn expires_at(&self) -> i64 {
        self.not_after
    }

    fn check(&self, now: i64) -> Result<()> {
        self.check_transport(now)?;
        self.policy.authorize(self.owner, self.peer, now)
    }
    fn check_transport(&self, now: i64) -> Result<()> {
        if self.connection.close_reason().is_some()
            || self.connection.alpn() != CONTROL_ALPN
            || now < self.last_seen
            || now >= self.not_after
        {
            return Err(Error::Refused);
        }
        Ok(())
    }
    /// Receives one actual authenticated frame and returns an opaque one-use
    /// evidence borrow. No cached session accessor or method can mint renewal.
    /// Clock is sampled AFTER read so an expired in-flight lease cannot refresh DMS.
    pub async fn receive_heartbeat(
        &mut self,
        clock: impl Fn() -> i64,
    ) -> Result<VerifiedHeartbeat<'_>> {
        self.check(clock())?;
        let (mut send, mut recv) = bounded(async {
            self.connection
                .accept_bi()
                .await
                .map_err(|_| Error::Transport)
        })
        .await?;
        let bytes = read_secret_frame(&mut recv, 13).await?;
        if recv.is_0rtt() {
            return Err(Error::Refused);
        }
        let sequence = wire::decode_heartbeat(&bytes)?;
        let now = clock();
        self.check(now)?;
        if self.sequence.is_some_and(|last| sequence <= last) {
            return Err(Error::Refused);
        }
        // Commit replay state before acknowledgement; cancellation cannot replay it.
        self.sequence = Some(sequence);
        self.last_seen = now;
        write_frame(&mut send, &[1]).await?;
        self.check(clock())?;
        Ok(VerifiedHeartbeat {
            session: self,
            sequence,
            observed_at: now,
        })
    }
}

/// One actual received frame, borrowed from the original live session. No
/// constructor/Clone/Deserialize, and dropping it discards authorization.
///
/// Evidence cannot be consumed twice:
/// ```compile_fail
/// use m8s_bootstrap::VerifiedHeartbeat;
/// fn cannot_reuse(evidence: VerifiedHeartbeat<'_>) {
///     let _ = evidence.consume(m8s_bootstrap::EndpointId([0;32]), 101, |_, _| true);
///     let _ = evidence.consume(m8s_bootstrap::EndpointId([0;32]), 101, |_, _| true);
/// }
/// ```
pub struct VerifiedHeartbeat<'a> {
    session: &'a mut AuthenticatedControlSession,
    sequence: u64,
    observed_at: i64,
}
impl VerifiedHeartbeat<'_> {
    pub fn peer_endpoint(&self) -> &str {
        self.session.peer_endpoint()
    }
    pub fn node_endpoint(&self) -> EndpointId {
        self.session.node_endpoint()
    }
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    pub fn observed_at(&self) -> i64 {
        self.observed_at
    }
    /// Consumes by value and invokes the agent bridge exactly once only after
    /// rechecking bound live policy/connection/lease and five-second freshness.
    /// Callback receives the authenticated receive time, never delayed caller now.
    /// It is a trusted short nonblocking bridge: authorization read guard remains
    /// held through apply. Do not block, await, or update policy from that callback.
    /// A completed policy update therefore cannot precede a stale authorized apply.
    pub fn consume<T>(
        self,
        expected_node: EndpointId,
        now: i64,
        apply: impl FnOnce(&str, i64) -> T,
    ) -> Result<T> {
        self.session
            .policy
            .with_authorized(self.session.owner, self.session.peer, now, || {
                self.session.check_transport(now)?;
                if self.session.local_node != expected_node {
                    return Err(Error::Refused);
                }
                if now.saturating_sub(self.observed_at) > IO_TIMEOUT.as_secs() as i64 {
                    return Err(Error::Refused);
                }
                Ok(apply(self.session.peer_endpoint(), self.observed_at))
            })?
    }
}
/// Owner sends a heartbeat on a real pinned connection. Endpoint possession is
/// authenticated by iroh; receiving agent still applies current ACL/revocation.
pub async fn send_heartbeat(connection: &Connection, sequence: u64) -> Result<()> {
    if connection.alpn() != CONTROL_ALPN {
        return Err(Error::Refused);
    }
    let (mut send, mut recv) =
        bounded(async { connection.open_bi().await.map_err(|_| Error::Transport) }).await?;
    let frame = wire::encode_heartbeat(sequence);
    write_frame(&mut send, &frame).await?;
    let ack = bounded(async { recv.read_to_end(1).await.map_err(|_| Error::Transport) }).await?;
    if ack != [1] {
        return Err(Error::Refused);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn current_policy_rejects_revocation_expiry_and_owner_mismatch() {
        let owner = EndpointId([1; 32]);
        let delegate = EndpointId([2; 32]);
        let p = ControlPolicy::new(owner, [delegate], [], 100).unwrap();
        assert!(p.authorize(owner, delegate, 99).is_ok());
        let bound = p.shared();
        p.update([], [], 100).unwrap();
        assert!(p.authorize(owner, delegate, 99).is_err());
        assert!(bound.authorize(owner, delegate, 99).is_err());
        p.update([delegate], [], 100).unwrap();
        assert!(p.authorize(owner, delegate, 100).is_err());
        assert!(p.authorize(owner, delegate, -1).is_err());
        assert!(p.authorize(delegate, owner, 99).is_err());
        let p = ControlPolicy::new(owner, [delegate], [delegate], 100).unwrap();
        assert!(p.authorize(owner, delegate, 99).is_err());
        assert!(p.authorize(owner, owner, 99).is_ok());
    }
    #[test]
    fn revocation_update_finishes_only_after_authorized_bridge() {
        use std::sync::{
            Arc, Barrier,
            atomic::{AtomicBool, Ordering},
        };
        let owner = EndpointId([1; 32]);
        let policy = Arc::new(ControlPolicy::new(owner, [], [], 100).unwrap());
        let barrier = Arc::new(Barrier::new(2));
        let applied = Arc::new(AtomicBool::new(false));
        let p = policy.clone();
        let b = barrier.clone();
        let a = applied.clone();
        let writer = policy
            .with_authorized(owner, owner, 1, || {
                assert!(policy.state.try_write().is_err());
                let writer = std::thread::spawn(move || {
                    b.wait();
                    p.update([], [owner], 100).unwrap();
                    a.load(Ordering::SeqCst)
                });
                barrier.wait();
                applied.store(true, Ordering::SeqCst);
                writer
            })
            .unwrap();
        assert!(writer.join().unwrap());
        assert!(policy.authorize(owner, owner, 1).is_err());
    }
}
