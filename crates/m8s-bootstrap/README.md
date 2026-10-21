# m8s-bootstrap

Owner-side ADR-0006 policy plus optional **real iroh QUIC/TLS adapter**.
Implementation compiles; real network qualification is blocked in the current
managed macOS sandbox. This is not a working provider/admission integration.

## Policy core

- `LaunchBinding` is owner-persisted instance/container ID, provider-observed IP
  and verified signed-image digest, not untrusted request metadata. Caller checks
  exact launch ownership and Booting state before issuing/redeeming.
- `BootNonce` owns zeroizing 32 caller-generated CSPRNG bytes, no Debug/Serialize.
  SQLite stores SHA-256 and public launch metadata only. Inject seconds.
- `Ledger::redeem` checks launch + authenticated endpoint + provider IP, validity
  `[issued, issued+900)`, atomically burns, then invokes `NodeSigner` and independent
  `CertificateVerifier`. Failures/unwind/cancellation/dropped delivery never restore
  the nonce. Certificate expiry is owner policy, not agent input. No production
  signer/verifier is supplied. Tests with synthetic authority are not admission.
- `ControlAcl` is explicit endpoint allowlist, default `[owner]`, not "all admitted
  mesh members". `HeartbeatGuard` checks authenticated node identity, owner, ALPN,
  sequence/time/TTL/cert expiry. Persist node replay state before acknowledging.

The separate `bootstrap_nonce` table is **not wired** to m8sd's nonce/gpu_node
state. Do not run independent authorization ledgers. Root must design one
transactional authoritative path for persisted launch state/ownership and nonce.
Ledger rejects an already-open outer transaction so rollback cannot restore a
nonce after signing. It accepts an already-open connection, never opens/chmods
files or relaxes policy: owner retains private-dir/no-symlink/0600 DB/WAL/SHM and
durable SQLite settings. SQLite errors are uncertainty and fail closed.

## Optional `iroh-transport` feature

Dependencies: exact iroh 1.3.0, defaults disabled, `tls-aws-lc-rs`; tokio rt/macros/net/
time/io-util. This matches mesh-llm transport and avoids competing TLS backends. Integration
owner owns final root Cargo.lock resolution and must
review transitive versions (iroh 1.3 permits newer compatible subcrates).

- `transport::accept_boot(Incoming, owner)` awaits full real handshake, requires
  boot ALPN, direct origin + selected path, creates private `IrohSession` evidence.
  No public string/key constructor exists. `BootConnection::receive` reads one
  bounded versioned frame; peer must equal request node key. Cancellation prevents
  re-reading/restarting that stream on the same wrapper.
- Call ledger redemption using `boot.session()`; add explicit model/config/
  heartbeat settings with `pending.with_settings(BundleSettings {..})`.
  `boot.deliver(pending, now)` sends on the **original peer-bound encrypted stream**
  and waits for stream acknowledgement. No independent plaintext server-side
  extraction/destination API. Failure means delivery uncertain, nonce still burned.
- Client `redeem_over_iroh(endpoint, owner_addr, request, verifier, clock)` pins
  owner EndpointAddr identity and validates owner/node/time + certificate through
  mandatory verifier before returning a non-Debug/non-Serialize `ReceivedBundle`.
  Clock is sampled after reading. This codec does not implement MP-001 crypto.
- `AuthenticatedControlSession` (also re-exported at crate root) is opaque,
  non-Clone/non-Debug/non-Deserialize. Only `transport::accept_control(&Endpoint, owner, policy, now)` after a real
  fully authenticated control handshake constructs it. `peer_endpoint(&self)->&str`
  is canonical `connection.remote_id().to_string()`, not request bytes. Pin agent
  ACL strings using that exact canonical representation.
- `ControlPolicy` is live shared RwLock-backed explicit owner/delegate/revocation/
  expiry policy. `update(&self, delegates, revoked, not_after)` atomically replaces
  it even while IO awaits; each frame rechecks after reading. Poisoned lock fails
  closed. Owner chooses deadline bounded by relevant node cert/lease expiry;
  the owner endpoint itself does not require inventing a rented-node cert.
- `receive_heartbeat(&mut self, clock)` checks full ALPN, live connection, bound
  current ACL/revocation/lease, sequence and monotonic time around bounded IO,
  then returns opaque `VerifiedHeartbeat<'_>` borrowing that session.
  Session stores a private shared handle to the original live policy; callers
  cannot substitute another stale/permissive policy snapshot after acceptance.
- `VerifiedHeartbeat::consume(self, expected_node, now, apply)` consumes by value, rechecks
  bound current policy/connection/lease, exact local node and five-second freshness, then invokes
  `apply(canonical_peer, verified_receive_timestamp)` exactly once. Agent bridge
  passes that timestamp to lowlevel heartbeat, not delayed caller time. Evidence
  has no public/synthetic constructor, Clone or Deserialize. Target node is derived
  internally from the actual accepting Endpoint.id(), never a frame claim. Dropping it loses
  permission; its mutable session borrow prevents receiving another frame while
  holding it. There is **no session.authorize_heartbeat or cached-session mint
  method**. Compile-fail doctest proves evidence cannot be consumed twice.
  Authorization read guard remains held through the synchronous bridge, so a
  revocation update cannot return before a stale authorized application. Callback
  must be trusted, short and nonblocking: never await, block or update policy
  inside it (that would deadlock). No global write lock is held around callbacks.
  BootGate remains failclosed until actual agent/admission integration is done.


Manual private codec uses `M8B`, version 1 and message kind. Limits: request16KiB,
bundle1MiB, IDs/model256 bytes, join/cert128KiB each, config512KiB, ACL32 entries,
heartbeat13 bytes. Rejects truncation/trailing bytes/version/kind/boolean errors,
invalid UTF-8/empty/NUL IDs, duplicate ACL IDs and missing owner. Sender frames reserve bounded capacity before secrets are copied; receiver frames
allocate fixed bounded buffers under zeroizing ownership before reading, not a
growing unprotected read_to_end Vec. Secret frames and
join/config buffers are zeroizing and never Debug/Serialize/logged. No 0-RTT.
Each handshake/read/write has a five-second timeout. Caller bounds total accepted
connections/streams and persists heartbeat replay state; reconnect policy and
cross-session command replay remain integration work.

## Qualification and deployment gates

**No public discovery/relay/provider calls in local tests.** Real loopback test uses
presets::Minimal, explicit127.0.0.1 bind, no address lookup/relay, ephemeral keys.
It is deliberately ignored by ordinary test runs: it must not appear green as a
network qualification. Root/MP independently observed EPERM before admission:
iroh1.3 socket.rs unconditionally starts netmon; netwatch0.19.3 BSD netmon opens
AF_ROUTE/SOCK_RAW. Minimal/loopback/disabled relays do not avoid that OS capability.
No netmon/security disabling, sandbox workaround or repeated path retries.

On an approved runner permitting UDP loopback and BSD AF_ROUTE/SOCK_RAW read
monitoring, or a separate supported runner:

```sh
cargo check -p m8s-bootstrap --features iroh-transport --locked -j 1
cargo test -p m8s-bootstrap --features iroh-transport --test iroh_loopback --locked -j 1 -- --ignored --test-threads=1
```

The real test compiles. This worker explicitly invoked the ignored target once:
**FAILED/BLOCKED at initial bind**, `Failed to create netmon monitor` → actor/IO →
`Operation not permitted (os error 1)`; 0 passed,1 failed,0 ignored. No sandbox or
netmon changes/retries. It has **not run successfully in this worker**. Its
certificate encoding is test-only but Ed25519 signatures/verifications are real: even a pass would prove protocol
exchange over actual TLS, not MP-001 cryptographic admission/provider identity.
Unit policy/codec tests do run without sockets, including actual Ed25519 signature
tampering, owner/node/time claim confusion and wrong-signer burn/replay. Compiled
loopback fixtures cover wrong ALPN, client key confusion and reconnect replay in
addition to exchange/control single-use, but remain unexecuted after the bind gate. Actual MP-001 fixture verification,
wrong-pin/race/timeouts and malicious-control runtime tests
must qualify on the supported runner before claiming transport ready.

**Relay address is not provider IP.** Relay-only boot is refused until an approved
origin-evidence design exists; any change to the accepted IP-binding security
contract needs ADR/board review. Control can use relays after authentication/ACL
checks because it does not establish initial provider-IP identity. The boot peer
and mesh node currently use the same key; separate sidecar keys need possession
proof before relaxing that binding.

No provider identity fetch, scoped destroy-key proof, MP-001 signer/revocation
propagation, m8sd/agent integration, Ready promotion, filesystem materialization or
DMS destruction is implemented. `self_destruct_present` is only an agent assertion.
Successful protocol redemption does not imply Ready: verified cert + model served
+ owner-routed canary remain separate. Denials, burn/sign/delivery uncertainty must
trigger owner alert/cleanup/revocation policy, not nonce retry or instance-row
removal. Root owns coherent commits, manifest/lock integration and bundle export.

## Certificate identity adapter gate

Existing reviewed mesh identity is a separate checkout/workspace, not a portable
registered dependency here. No absolute external path or duplicate MP-001 format
is added. Its signing owner ID (`sha256(signing public key)`), transport endpoint
ID, certificate millisecond fields and wall-clock signing API need an explicit
trusted mapping/time adapter. Do not equate endpoint bytes to signing-owner ID or
trust a public key supplied inside an untrusted certificate. Production signer
implementation belongs behind the existing injection boundary on the owner host;
only signatures/certs leave it. Current test-only Ed25519 authority is real crypto
but deliberately NOT an MP-001 certificate or admission-readiness claim.
