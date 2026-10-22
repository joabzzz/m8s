//! Stage A compiles/lists this source ONLY. Endpoint execution needs separate approval.
//! Synthetic launch SQL is not provider/budget/approval/effect evidence.
use iroh::{
    Endpoint, EndpointAddr, SecretKey,
    endpoint::{BindError, presets},
};
use m8s_bootstrap::{
    mp001::{Mp001CertificateVerifier, Mp001TrustPin},
    transport::*,
    *,
};
use m8sd::{
    Store,
    nonce::{BootstrapStage, LaunchIdentity, ObservationStatus},
};
use mesh_llm_identity::{
    OwnerKeypair, TrustStore,
    ownership::exact_request::{
        ExactNodeSigningRequest, ExactSigningPolicy, sign_node_ownership_exact,
    },
};
use rusqlite::{Connection, params};
use std::{
    net::Ipv4Addr,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use zeroize::Zeroizing;

const OWNER: &str = "synthetic-composition-owner";
const UID: &str = "synthetic-launch-uid";
const TIER: &str = "synthetic-tier";
// Fixed synthetic inputs ONLY; never load a key, nonce, join or config from disk.
const NONCE: [u8; 32] = [42; 32];
const JOIN: &[u8] = b"synthetic-join";
const CONFIG: &[u8] = b"synthetic-config";

// Deliberately discard every underlying error/value, including secret-bearing Debug.
fn need<T, E>(result: std::result::Result<T, E>, phase: &'static str) -> T {
    match result {
        Ok(value) => value,
        Err(_) => panic!("{phase}"),
    }
}
// Only typed categories and integers may escape this classifier. Never format an
// underlying error, its metadata, or a source (even for an unknown future variant).
struct BindDiagnostic {
    stage: &'static str,
    io_kind: &'static str,
    raw_os_error: Option<i32>,
}
impl std::fmt::Display for BindDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "endpoint unavailable; REDACTED stage={} io_kind={} raw_os_error=",
            self.stage, self.io_kind
        )?;
        match self.raw_os_error {
            Some(code) => write!(f, "{code}"),
            None => f.write_str("none"),
        }?;
        f.write_str("; no retry or fallback")
    }
}
// At most four nodes, including the supplied root; cycles also terminate.
fn bind_io_source<'a>(
    mut error: &'a (dyn std::error::Error + 'static),
) -> Option<&'a std::io::Error> {
    for _ in 0..4 {
        if let Some(io) = error.downcast_ref::<std::io::Error>() {
            return Some(io);
        }
        error = error.source()?;
    }
    None
}
fn bind_diagnostic(error: &BindError) -> BindDiagnostic {
    let (stage, io) = match error {
        BindError::Sockets { source, .. } => ("sockets", Some(source)),
        BindError::CreateQuicEndpoint { source, .. } => ("quic_endpoint", Some(source)),
        // AnyError's std source skips its wrapped root; check its typed
        // downcast too, so a directly wrapped IO error is not lost.
        BindError::CreateNetmonMonitor { source, .. } => (
            "netmon_monitor",
            source
                .downcast_ref::<std::io::Error>()
                .or_else(|| bind_io_source(source)),
        ),
        BindError::InvalidTransportConfig { .. } => ("transport_config", None),
        BindError::InvalidCaRootConfig { source, .. } => ("ca_root_config", Some(source)),
        BindError::AddressLookup { source, .. } => ("address_lookup", bind_io_source(source)),
        BindError::TlsConfigError { source, .. } => ("tls_config", bind_io_source(source)),
        BindError::InvalidCryptoProvider { .. } => ("crypto_provider", None),
        _ => ("unknown", None),
    };
    let io_kind = match io.map(std::io::Error::kind) {
        None => "unavailable",
        Some(std::io::ErrorKind::PermissionDenied) => "permission_denied",
        Some(std::io::ErrorKind::AddrInUse) => "address_in_use",
        Some(std::io::ErrorKind::AddrNotAvailable) => "address_not_available",
        Some(std::io::ErrorKind::InvalidInput) => "invalid_input",
        Some(std::io::ErrorKind::Unsupported) => "unsupported",
        Some(std::io::ErrorKind::Other) => "other",
        Some(_) => "other_kind",
    };
    BindDiagnostic {
        stage,
        io_kind,
        raw_os_error: io.and_then(std::io::Error::raw_os_error),
    }
}
fn need_endpoint(result: std::result::Result<Endpoint, BindError>) -> Endpoint {
    match result {
        Ok(endpoint) => endpoint,
        Err(error) => panic!("{}", bind_diagnostic(&error)),
    }
}

#[test]
fn bind_diagnostic_known_variants() {
    use std::io::{Error as IoError, ErrorKind};
    let errors = [
        (
            BindError::Sockets {
                source: IoError::new(ErrorKind::AddrInUse, "SECRET_MARKER"),
                meta: Default::default(),
            },
            "sockets",
        ),
        (
            BindError::CreateQuicEndpoint {
                source: IoError::new(ErrorKind::AddrInUse, "SECRET_MARKER"),
                meta: Default::default(),
            },
            "quic_endpoint",
        ),
        (
            BindError::InvalidCaRootConfig {
                source: IoError::new(ErrorKind::AddrInUse, "SECRET_MARKER"),
                meta: Default::default(),
            },
            "ca_root_config",
        ),
    ];
    for (error, stage) in errors {
        let diagnostic = bind_diagnostic(&error);
        assert_eq!(diagnostic.stage, stage);
        assert_eq!(diagnostic.io_kind, "address_in_use");
        assert_eq!(diagnostic.raw_os_error, None);
        assert!(!diagnostic.to_string().contains("SECRET_MARKER"));
    }
    // Pure construction: no syscall, no interpretation of this synthetic code.
    let io = IoError::from_raw_os_error(12345);
    let diagnostic = bind_diagnostic(&BindError::Sockets {
        source: io,
        meta: Default::default(),
    });
    assert_eq!(diagnostic.raw_os_error, Some(12345));
}

#[test]
fn bind_diagnostic_absent_io() {
    for (error, stage) in [
        (
            BindError::InvalidTransportConfig {
                meta: Default::default(),
            },
            "transport_config",
        ),
        (
            BindError::InvalidCryptoProvider {
                meta: Default::default(),
            },
            "crypto_provider",
        ),
        (
            BindError::CreateNetmonMonitor {
                source: "SECRET_MARKER".into(),
                meta: Default::default(),
            },
            "netmon_monitor",
        ),
    ] {
        let diagnostic = bind_diagnostic(&error);
        assert_eq!(diagnostic.stage, stage);
        assert_eq!(diagnostic.io_kind, "unavailable");
        assert_eq!(diagnostic.raw_os_error, None);
        assert!(!diagnostic.to_string().contains("SECRET_MARKER"));
    }
    // BindError is non_exhaustive: future variants cannot be constructed here.
    // The classifier's wildcard deliberately reports unknown with no IO claim.
}

#[test]
fn bind_diagnostic_nested_io_and_depth() {
    #[derive(Debug)]
    struct Layer(Box<dyn std::error::Error + Send + Sync>);
    impl std::fmt::Display for Layer {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("SECRET_MARKER")
        }
    }
    impl std::error::Error for Layer {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(self.0.as_ref())
        }
    }
    for (layers, expected) in [
        (0, "permission_denied"),
        (3, "permission_denied"),
        (4, "unavailable"),
    ] {
        let mut source: Box<dyn std::error::Error + Send + Sync> = Box::new(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "SECRET_MARKER",
        ));
        for _ in 0..layers {
            source = Box::new(Layer(source));
        }
        let error = BindError::CreateNetmonMonitor {
            source: source.into(),
            meta: Default::default(),
        };
        let diagnostic = bind_diagnostic(&error);
        assert_eq!(diagnostic.stage, "netmon_monitor");
        assert_eq!(diagnostic.io_kind, expected);
        assert_eq!(diagnostic.raw_os_error, None);
        assert!(!diagnostic.to_string().contains("SECRET_MARKER"));
    }
}

#[test]
fn bind_diagnostic_descriptor_format() {
    let diagnostic = BindDiagnostic {
        stage: "sockets",
        io_kind: "other_kind",
        raw_os_error: Some(-12345),
    };
    assert_eq!(
        diagnostic.to_string(),
        "endpoint unavailable; REDACTED stage=sockets io_kind=other_kind raw_os_error=-12345; no retry or fallback"
    );
    let diagnostic = BindDiagnostic {
        stage: "unknown",
        io_kind: "unavailable",
        raw_os_error: None,
    };
    assert_eq!(
        diagnostic.to_string(),
        "endpoint unavailable; REDACTED stage=unknown io_kind=unavailable raw_os_error=none; no retry or fallback"
    );
}

fn now() -> i64 {
    need(
        i64::try_from(need(SystemTime::now().duration_since(UNIX_EPOCH), "clock").as_secs()),
        "clock range",
    )
}
fn milliseconds(seconds: i64) -> Result<u64> {
    u64::try_from(seconds)
        .ok()
        .and_then(|s| s.checked_mul(1000))
        .ok_or(Error::Certificate)
}
fn mesh_key() -> OwnerKeypair {
    need(
        OwnerKeypair::from_bytes(&[71; 32], &[72; 32]),
        "synthetic key",
    )
}
fn verifier(owner: EndpointId) -> Mp001CertificateVerifier {
    need(
        Mp001CertificateVerifier::new(
            need(
                Mp001TrustPin::new(owner, mesh_key().verifying_key().to_bytes()),
                "pin",
            ),
            TrustStore::default(),
        ),
        "verifier",
    )
}
fn id(endpoint: &Endpoint) -> EndpointId {
    EndpointId(*endpoint.id().as_bytes())
}
fn address(endpoint: &Endpoint) -> EndpointAddr {
    let sockets = endpoint.bound_sockets();
    assert!(sockets.len() == 1, "one explicit loopback socket required");
    assert!(
        sockets[0].ip() == Ipv4Addr::LOCALHOST,
        "loopback socket required"
    );
    EndpointAddr::new(endpoint.id()).with_ip_addr(sockets[0])
}
async fn endpoint(seed: u8) -> Endpoint {
    need_endpoint(
        need(
            Endpoint::builder(presets::Minimal)
                .secret_key(SecretKey::from_bytes(&[seed; 32]))
                .clear_ip_transports()
                .bind_addr("127.0.0.1:0"),
            "bind configuration",
        )
        .clear_relay_transports()
        .clear_address_lookup()
        .alpns(vec![BOOT_ALPN.to_vec(), CONTROL_ALPN.to_vec()])
        .bind()
        .await,
    )
}
struct Pair {
    owner: Endpoint,
    node: Endpoint,
}
impl Pair {
    async fn new() -> Self {
        let pair = Self {
            owner: endpoint(11).await,
            node: endpoint(22).await,
        };
        assert!(id(&pair.owner) != id(&pair.node), "distinct TLS identities");
        let public = mesh_key().verifying_key().to_bytes();
        assert!(
            public != id(&pair.owner).0 && public != id(&pair.node).0,
            "distinct mesh signing identity"
        );
        pair
    }
    async fn close(self) {
        self.node.close().await;
        self.owner.close().await;
    }
    async fn receive(&self) -> (BootConnection, Redemption) {
        let incoming = self.owner.accept().await.expect("incoming");
        let mut boot = need(accept_boot(incoming, id(&self.owner)).await, "TLS accept");
        let request = need(boot.receive().await, "TLS request");
        (boot, request)
    }
}
struct Fixture {
    path: PathBuf,
    store: Store,
    binding: LaunchBinding,
}
impl Fixture {
    fn new(root: &Path, name: &str, pair: &Pair) -> Self {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let directory = root.join(name);
        need(
            std::fs::DirBuilder::new().mode(0o700).create(&directory),
            "private fixture directory",
        );
        let directory = need(directory.canonicalize(), "canonical fixture");
        assert!(
            need(directory.metadata(), "private directory metadata")
                .permissions()
                .mode()
                & 0o7777
                == 0o700,
            "private fixture mode"
        );
        let path = directory.join("state.db");
        let store = need(Store::open(&path, OWNER), "Store schema");
        need(store.scale(TIER, 1), "Store desired");
        let binding = LaunchBinding {
            owner: id(&pair.owner),
            instance_id: "synthetic-instance".into(),
            container_id: "synthetic-container".into(),
            // LOOPBACK FIXTURE OBSERVATION, NOT CLOUD OWNERSHIP.
            provider_ip: Ipv4Addr::LOCALHOST.into(),
            image_sha256: [3; 32],
        };
        {
            let mut connection = need(Connection::open(&path), "fixture connection");
            let standalone: bool = need(connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='bootstrap_nonce')", [], |r| r.get(0)), "schema assertion");
            assert!(!standalone, "Store must be sole nonce authority");
            let tx = need(connection.transaction(), "fixture transaction");
            need(tx.execute("INSERT INTO gpu_node(uid,tier,instance_id,phase,created_at,orphan,deleting) VALUES (?1,?2,?3,'\"Booting\"',?4,0,0)", params![UID,TIER,binding.instance_id,now()]), "synthetic launch metadata");
            need(
                tx.execute(
                    "INSERT INTO launch_attempt(uid,tier,at) VALUES (?1,?2,?3)",
                    params![UID, TIER, now()],
                ),
                "synthetic attempt metadata",
            );
            need(tx.commit(), "fixture commit");
            assert!(connection.is_autocommit(), "fixture transaction closed");
        } // Fixture connection closed before protocol. No nonce/authority SQL writes.
        let fixture = Self {
            path,
            store,
            binding,
        };
        let issued = now();
        need(
            fixture.store.issue_bootstrap_nonce(
                &NONCE,
                fixture.observation(issued, &fixture.binding),
                issued,
            ),
            "Store issue",
        );
        fixture
    }
    fn observation<'a>(&self, observed_at: i64, binding: &'a LaunchBinding) -> LaunchIdentity<'a> {
        LaunchIdentity {
            owner: OWNER,
            node_uid: UID,
            binding,
            observed_at,
            status: ObservationStatus::Complete,
        }
    }
    fn request(&self, node: EndpointId) -> Redemption {
        Redemption {
            nonce: BootNonce::new(NONCE),
            instance_id: self.binding.instance_id.clone(),
            container_id: self.binding.container_id.clone(),
            node,
            image_sha256: self.binding.image_sha256,
            self_destruct_present: true,
        }
    }
    fn attempt(&self) -> m8sd::nonce::BootstrapAttempt {
        let mut attempts = need(self.store.bootstrap_attempts(), "Store attempts");
        assert!(attempts.len() == 1, "one authoritative attempt");
        attempts.remove(0)
    }
    fn assert_state(&self, stage: BootstrapStage, cancelled: bool, cleanup: bool) {
        let a = self.attempt();
        assert!(
            a.stage == stage && a.cancelled == cancelled && a.cleanup_required == cleanup,
            "persistent stage"
        );
        assert!(
            a.owner == OWNER
                && a.node_uid == UID
                && a.instance_id == self.binding.instance_id
                && a.container_id == self.binding.container_id,
            "persistent identity"
        );
    }
    fn redeem(
        &self,
        boot: &BootConnection,
        request: Redemption,
        binding: &LaunchBinding,
        signer: &mut Signing,
    ) -> Result<PendingDelivery> {
        // ORIGINAL pre-SQL seconds; never rebase, renew or retry stale work.
        let issued = now();
        let expires = issued.checked_add(60).ok_or(Error::Certificate)?;
        self.store
            .redeem_bootstrap(
                boot.session(),
                request,
                binding,
                self.observation(issued, binding),
                issued,
                expires,
                Zeroizing::new(JOIN.to_vec()),
                signer,
                &verifier(self.binding.owner),
            )?
            .with_settings(BundleSettings {
                model: "synthetic-model".into(),
                config: Zeroizing::new(CONFIG.to_vec()),
                heartbeat_ttl_seconds: 60,
            })
    }
    fn into_reopened(self) -> Self {
        let Self {
            path,
            store,
            binding,
        } = self;
        drop(store);
        let store = need(Store::open(&path, OWNER), "Store reopen");
        Self {
            path,
            store,
            binding,
        }
    }
}
#[derive(Clone, Copy)]
enum SignMode {
    Exact,
    Fail,
    CancelDuring,
}
struct Signing {
    path: PathBuf,
    binding: LaunchBinding,
    node: EndpointId,
    mode: SignMode,
    calls: usize,
    crypto: usize,
}
impl Signing {
    fn new(f: &Fixture, pair: &Pair, mode: SignMode) -> Self {
        Self {
            path: f.path.clone(),
            binding: f.binding.clone(),
            node: id(&pair.node),
            mode,
            calls: 0,
            crypto: 0,
        }
    }
    fn committed(&self, claims: &CertificateClaims) -> Result<()> {
        let connection = Connection::open(&self.path)?;
        connection.busy_timeout(Duration::ZERO)?;
        // Independent writer reservation proves Store committed before this call.
        // Busy=0: do not wait/retry. No transaction survives into down/crypto/TLS.
        connection.execute_batch("BEGIN IMMEDIATE")?;
        let exact: bool = connection.query_row(
            "SELECT count(*)=1 FROM nonce WHERE owner=?1 AND node_uid=?2 AND instance_id=?3 AND container_id=?4 AND owner_endpoint=?5 AND node_endpoint=?6 AND redeemed_at=?7 AND cert_expires_at=?8 AND stage='consumed' AND cancelled=0 AND cleanup_required=1 AND certificate_hash IS NULL AND length(hash)=32 AND issued_at<=?7 AND ip='127.0.0.1' AND digest=?9",
            params![OWNER, UID, self.binding.instance_id, self.binding.container_id,
                claims.owner.0.as_slice(), claims.node.0.as_slice(), claims.issued_at, claims.expires_at,
                format!("sha256:{}", "03".repeat(32))], |r| r.get(0))?;
        connection.execute_batch("ROLLBACK")?;
        if !exact
            || !connection.is_autocommit()
            || claims.owner != self.binding.owner
            || claims.node != self.node
        {
            return Err(Error::Refused);
        }
        drop(connection);
        Ok(())
    }
}
impl NodeSigner for Signing {
    fn sign(&mut self, claims: &CertificateClaims) -> Result<NodeCertificate> {
        self.calls += 1;
        self.committed(claims)?;
        match self.mode {
            SignMode::Fail => return Err(Error::Certificate), // fail-only; never a fake positive
            SignMode::CancelDuring => {
                let other = Store::open(&self.path, OWNER).map_err(|_| Error::Storage)?;
                other.down().map_err(|_| Error::Storage)?;
                drop(other);
            }
            SignMode::Exact => {}
        }
        let request = ExactNodeSigningRequest::new(
            claims.node.0,
            milliseconds(claims.issued_at)?,
            milliseconds(claims.expires_at)?,
        )
        .map_err(|_| Error::Certificate)?;
        // Explicit TEST VALUES, NOT production defaults or invoker authorization.
        let policy = ExactSigningPolicy::new(60_000, 30_000).map_err(|_| Error::Certificate)?;
        self.crypto += 1;
        let certificate = sign_node_ownership_exact(&mesh_key(), &request, &policy)
            .map_err(|_| Error::Certificate)?;
        Ok(NodeCertificate(
            serde_json::to_vec(&certificate).map_err(|_| Error::Certificate)?,
        ))
    }
}

async fn positive_and_replay(root: &Path) {
    let pair = Pair::new().await;
    let f = Fixture::new(root, "positive-replay", &pair);
    let mut signer = Signing::new(&f, &pair, SignMode::Exact);
    let server = async {
        let (boot, request) = pair.receive().await;
        let pending = need(
            f.redeem(&boot, request, &f.binding, &mut signer),
            "real redemption",
        );
        let expected = pending.claims().clone();
        f.assert_state(BootstrapStage::Certified, false, true);
        need(boot.deliver(pending, now()).await, "original delivery");
        expected
    };
    let v = verifier(f.binding.owner);
    let client = redeem_over_iroh(
        &pair.node,
        address(&pair.owner),
        f.request(id(&pair.node)),
        &v,
        now,
    );
    let (expected, received) = tokio::join!(server, client);
    let received = need(received, "real client verification");
    assert!(
        received.claims == expected && received.claims.node == id(&pair.node),
        "exact received claims"
    );
    need(
        v.verify(&received.certificate, &expected),
        "independent canonical verification",
    );
    assert!(
        received.join_token.as_slice() == JOIN && received.settings.config.as_slice() == CONFIG,
        "exact synthetic bundle"
    );
    assert!(
        received.settings.model == "synthetic-model"
            && received.settings.heartbeat_ttl_seconds == 60,
        "synthetic settings"
    );
    assert!(
        signer.calls == 1 && signer.crypto == 1,
        "one real signature"
    );
    // Both actual successes above, never just a boolean substituted for either.
    assert!(
        need(
            f.store.confirm_bootstrap_delivery(&f.attempt(), now()),
            "Store confirmation"
        ),
        "confirmation changed row"
    );
    f.assert_state(BootstrapStage::Delivered, false, false);
    let old = f.attempt();
    let f = f.into_reopened();
    let server = async {
        let (boot, request) = pair.receive().await;
        assert!(
            matches!(
                f.redeem(&boot, request, &f.binding, &mut signer),
                Err(Error::Refused)
            ),
            "replay refused"
        );
        drop(boot);
    };
    let v = verifier(f.binding.owner);
    let client = redeem_over_iroh(
        &pair.node,
        address(&pair.owner),
        f.request(id(&pair.node)),
        &v,
        now,
    );
    let (_, result) = tokio::join!(server, client);
    assert!(
        result.is_err() && signer.calls == 1 && signer.crypto == 1,
        "replay before signer"
    );
    f.assert_state(BootstrapStage::Delivered, false, false);
    let after = f.attempt();
    assert!(
        old.redeemed_at == after.redeemed_at
            && old.cert_expires_at == after.cert_expires_at
            && old.node_endpoint == after.node_endpoint,
        "replay preserves history"
    );
    pair.close().await;
}

#[derive(Clone, Copy)]
enum Failure {
    Signer,
    LostDelivery,
    CancelBefore,
    CancelDuring,
    DispatchExpiry,
    Origin,
    Owner,
}
async fn failure(root: &Path, name: &str, case: Failure) {
    let pair = Pair::new().await;
    let f = Fixture::new(root, name, &pair);
    let mode = match case {
        Failure::Signer => SignMode::Fail,
        Failure::CancelDuring => SignMode::CancelDuring,
        _ => SignMode::Exact,
    };
    let mut signer = Signing::new(&f, &pair, mode);
    if matches!(case, Failure::CancelBefore) {
        need(f.store.down(), "cancel before consume");
    }
    let server = async {
        let (boot, request) = pair.receive().await;
        let mut binding = f.binding.clone();
        match case {
            Failure::Origin => binding.provider_ip = Ipv4Addr::new(127, 0, 0, 2).into(),
            Failure::Owner => binding.owner = id(&pair.node),
            _ => {}
        }
        let result = f.redeem(&boot, request, &binding, &mut signer);
        match case {
            Failure::LostDelivery => {
                drop(need(result, "canonical sign before lost delivery"));
                drop(boot);
            }
            Failure::DispatchExpiry => {
                let pending = need(result, "canonical sign before dispatch boundary");
                let expires = pending.claims().expires_at;
                // Explicit dispatch-time injection, NOT observed wall-clock expiry.
                assert!(
                    matches!(boot.deliver(pending, expires).await, Err(Error::Refused)),
                    "dispatch expiry boundary"
                );
            }
            Failure::Signer => {
                assert!(matches!(result, Err(Error::Certificate)), "signer failure");
                drop(boot);
            }
            _ => {
                assert!(matches!(result, Err(Error::Refused)), "refused");
                drop(boot);
            }
        }
    };
    let v = verifier(f.binding.owner);
    let client = redeem_over_iroh(
        &pair.node,
        address(&pair.owner),
        f.request(id(&pair.node)),
        &v,
        now,
    );
    let (_, result) = tokio::join!(server, client);
    assert!(result.is_err(), "client must not receive verified bundle");
    let (stage, cancelled, cleanup, calls, crypto) = match case {
        Failure::Signer => (BootstrapStage::Consumed, false, true, 1, 0),
        Failure::LostDelivery | Failure::DispatchExpiry => {
            (BootstrapStage::Certified, false, true, 1, 1)
        }
        Failure::CancelBefore => (BootstrapStage::Cancelled, true, false, 0, 0),
        Failure::CancelDuring => (BootstrapStage::Consumed, true, true, 1, 1),
        Failure::Origin | Failure::Owner => (BootstrapStage::Issued, false, false, 0, 0),
    };
    assert!(
        signer.calls == calls && signer.crypto == crypto,
        "signer/crypto counts"
    );
    f.assert_state(stage, cancelled, cleanup);
    let f = f.into_reopened();
    f.assert_state(stage, cancelled, cleanup);
    // No confirm hook on any failure, no reset/reissue/retry; burned remains burned.
    pair.close().await;
}

async fn wrong_original(root: &Path) {
    // Fixed architecture: distinct endpoint actors/sockets, identical TLS keys.
    // Each client actor has only its own owner's socket; no connection cache can
    // reuse the other actor's session. Two actual accept/receive events, no retry.
    let a = Pair::new().await;
    let b = Pair::new().await;
    assert!(
        id(&a.node) == id(&b.node) && id(&a.owner) == id(&b.owner),
        "same TLS peers"
    );
    assert!(
        address(&a.owner) != address(&b.owner),
        "distinct accept sockets"
    );
    let f = Fixture::new(root, "wrong-original", &a);
    let mut signer = Signing::new(&f, &a, SignMode::Exact);
    let server = async {
        let ((boot_a, request_a), (boot_b, request_b)) = tokio::join!(a.receive(), b.receive());
        // Same claims and request; private original-stream capability differs.
        assert!(
            request_a.node == request_b.node
                && request_a.instance_id == request_b.instance_id
                && request_a.container_id == request_b.container_id
                && request_a.image_sha256 == request_b.image_sha256,
            "same peer/request fields"
        );
        let pending = need(
            f.redeem(&boot_a, request_a, &f.binding, &mut signer),
            "A canonical redemption",
        );
        assert!(
            matches!(boot_b.deliver(pending, now()).await, Err(Error::Refused)),
            "wrong original stream"
        );
        drop(boot_a);
    };
    let va = verifier(f.binding.owner);
    let vb = verifier(f.binding.owner);
    let ca = redeem_over_iroh(&a.node, address(&a.owner), f.request(id(&a.node)), &va, now);
    let cb = redeem_over_iroh(&b.node, address(&b.owner), f.request(id(&b.node)), &vb, now);
    let (_, ra, rb) = tokio::join!(server, ca, cb);
    assert!(ra.is_err() && rb.is_err(), "neither stream delivered");
    assert!(signer.calls == 1 && signer.crypto == 1, "only A signed");
    f.assert_state(BootstrapStage::Certified, false, true);
    a.close().await;
    b.close().await;
}

async fn preburn_confusion(root: &Path) {
    let pair = Pair::new().await;
    let f = Fixture::new(root, "alpn-node-confusion", &pair);
    let signer = Signing::new(&f, &pair, SignMode::Exact);
    let server = async {
        let incoming = pair.owner.accept().await.expect("wrong ALPN incoming");
        assert!(
            matches!(
                accept_boot(incoming, id(&pair.owner)).await,
                Err(Error::Refused)
            ),
            "wrong ALPN"
        );
    };
    let client = async {
        let connection = need(
            pair.node.connect(address(&pair.owner), CONTROL_ALPN).await,
            "real wrong ALPN handshake",
        );
        connection.closed().await;
    };
    tokio::join!(server, client);
    let v = verifier(f.binding.owner);
    assert!(
        matches!(
            redeem_over_iroh(
                &pair.node,
                address(&pair.owner),
                f.request(id(&pair.owner)),
                &v,
                now
            )
            .await,
            Err(Error::Refused)
        ),
        "wrong node before connect"
    );
    assert!(signer.calls == 0 && signer.crypto == 0, "no signer");
    f.assert_state(BootstrapStage::Issued, false, false);
    pair.close().await;
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "Stage B only: separate explicit primary instruction after frozen source acceptance; real endpoint/OS runtime, no automatic capability probe"]
async fn composed_bootstrap() {
    // Keep immediately: failure/timeout preserves private uncertainty artifacts,
    // even on panic; never automatic deletion/reset/recovery or secret dumps.
    let directory = need(
        tempfile::Builder::new()
            .prefix("m8s-composed-bootstrap-")
            .tempdir(),
        "fresh private root",
    )
    .keep();
    let root = need(directory.canonicalize(), "canonical private root");
    need(
        tokio::time::timeout(Duration::from_secs(60), async {
            positive_and_replay(&root).await;
            failure(&root, "signer-failure", Failure::Signer).await;
            failure(&root, "lost-delivery", Failure::LostDelivery).await;
            failure(&root, "cancel-before", Failure::CancelBefore).await;
            failure(&root, "cancel-during", Failure::CancelDuring).await;
            wrong_original(&root).await;
            failure(&root, "dispatch-expiry", Failure::DispatchExpiry).await;
            failure(&root, "origin-confusion", Failure::Origin).await;
            failure(&root, "owner-confusion", Failure::Owner).await;
            preburn_confusion(&root).await;
        })
        .await,
        "overall async timeout; synchronous SQL/crypto/kernel not preemptible",
    );
}
