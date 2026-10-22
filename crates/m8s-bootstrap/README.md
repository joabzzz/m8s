# m8s-bootstrap

Owner-side ADR-0006 policy plus optional **real iroh QUIC/TLS adapter**.
Implementation compiles; this crate's transport and composed bootstrap runtime
remain unqualified. Separate external MP-001 endpoint PASS at cc08962a does not
qualify this integration. This is not a working provider/admission integration.

## Policy core

- `LaunchBinding` is owner-persisted instance/container ID, provider-observed IP
  and verified signed-image digest, not untrusted request metadata. Caller checks
  exact launch ownership and Booting state before issuing/redeeming.
- `BootNonce` owns zeroizing 32 caller-generated CSPRNG bytes, no Debug/Serialize.
  SQLite stores SHA-256 and public launch metadata only. Inject seconds.
- `Ledger::redeem` checks launch + authenticated endpoint + provider IP, validity
  `[issued, issued+900)`, atomically burns, then invokes `NodeSigner` and independent
  `CertificateVerifier`. Failures/unwind/cancellation/dropped delivery never restore
  the nonce. Certificate expiry is owner policy, not agent input. Optional
  default-OFF canonical verification is supplied by `mp001-certificates`;
  owner/machine signing and the default daemon route remain unwired.
  Tests with synthetic authority are not admission.
- `ControlAcl` is explicit endpoint allowlist, default `[owner]`, not "all admitted
  mesh members". `HeartbeatGuard` checks authenticated node identity, owner, ALPN,
  sequence/time/TTL/cert expiry. Persist node replay state before acknowledging.

The standalone `Ledger` owns a separate `bootstrap_nonce` table. m8sd's
`Store::redeem_bootstrap` already uses its authoritative nonce/gpu_node path
through `NonceConsumer`, committing burn + cleanup intent before signing and
certifying afterward. It never instantiates `Ledger`: do not combine them or run
a second authorization ledger. Trusted launch observations remain adapter
assertions, not provider identity proof; no provider/TLS listener is wired.
Ledger rejects an already-open outer transaction so rollback cannot restore a
nonce after signing. It accepts an already-open connection, never opens/chmods
files or relaxes policy: owner retains private-dir/no-symlink/0600 DB/WAL/SHM and
durable SQLite settings. SQLite errors are uncertainty and fail closed.

## Optional `iroh-transport` feature

`redeem_verified_over_iroh` returns an opaque, single-use `VerifiedBootstrap`
receipt after authenticated delivery and verification. Its private constructor
prevents a decoded or synthetic `ReceivedBundle` from being used as transport
provenance. Consuming `into_bundle` relinquishes that provenance. The agent's
default-off `secure-runtime` feature consumes this receipt, rechecks the concrete
certificate and native node identity, and supervises the native child with live
authenticated heartbeats and a certificate-bounded monotonic deadline. This is
a library integration; the secure executable entrypoint remains unfinished.

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

The portable, workspace-excluded `vendor/mesh-llm-identity` package and optional
default-OFF `mp001-certificates` supply bounded canonical public verification.
The vendor's `UPSTREAM.md` and `SHA256SUMS` are the authoritative snapshot
inventory; no temporary Cargo path or duplicate encoder is used. The immutable
trusted pin explicitly maps the owner iroh endpoint to a mesh signing public key
and canonical-derived signing owner ID; certificate milliseconds bind exactly to
expected times. Do not equate endpoint bytes to signing-owner ID or trust a public
key supplied inside an untrusted certificate. Owner/machine signer integration
remains unwired behind the existing injection boundary on the owner host; only
signatures/certs may leave it. The legacy `iroh_loopback` test authority is real
Ed25519 but not MP-001. In contrast, `mp001_public` and separate public adversarial
fixtures exercise canonical MP-001 signatures, not runtime admission readiness.

## Default-OFF canonical `mp001-certificates`

The separately default-off `mp001-owner-signing` feature exposes
`mp001::signing::Mp001NodeSigner`. It borrows an in-memory canonical owner key,
checks its public key against the configured endpoint/signing-key pin, and calls
the canonical exact-time signer with explicit lifetime and issuance-age limits.
It does not load keys, enable host-io, or authorize an invoker. The authoritative
Store must burn the nonce before invoking it. Canonical signing rechecks real
time; adapter tests use deterministic synthetic keys and exercise refusal and
conversion boundaries without minting current certificates. Source is pinned
at `f2cbf8fd8159a16401040a3536550d49a3da8ce2`; see vendor provenance and hashes.

This feature exposes `mp001::{Mp001TrustPin, Mp001CertificateVerifier}`.
Construct a pin from a trusted owner iroh EndpointId and separately provisioned
mesh signing public key [u8;32]; it validates the key and derives the owner ID
using the canonical library. Certificate fields cannot establish trust.
Construct a verifier with that pin and an owned canonical TrustStore. Fields
are private, immutable and non-Deserialize. Existing CertificateVerifier returns
only Result<()>, never admission/session/origin/Ready evidence.

The trust store is a bounded snapshot, **not live revocation authority or a
currentness proof**. Trusted orchestration must construct a fresh verifier from
current policy for EVERY redemption/startup attempt. No propagation or daemon
wiring is implemented. Unsupported trust version is refused. Each list <=256;
owner IDs are exact lowercase64hex, node IDs64hex, cert IDs1..128UTF8 bytes,
all retained display labels/reasons <=256UTF8 bytes. The pin is allowlisted
(within the same256 bound); canonical Allowlist verification always preserves
owner/cert/node revocations, regardless of the supplied policy enum.

JSON is nonempty and <=128KiB before parsing, with private strict DTOs rejecting
duplicates, unknown top/nested fields, wrong types and trailing data. Certificate
IDs1..128UTF8 bytes; owner ID exact canonical lowercase64hex; public keys/node IDs
64hex and signature128hex, decoded byte semantics (ASCII hex case permitted);
optional label/hostname <=256UTF8 bytes. This is a bounded canonical subset,
not a new encoder. All failures are opaque Error::Certificate, without input or
parse-error chains. Canonical signature verification binds all signed fields.

Expected owner endpoint/key/derived owner ID/node must match exactly. Expected
seconds must be nonnegative, checked u64*1000, expires>issued, with exact
millisecond equality (no rounding/rebase/saturation). The trait reads real
SystemTime and enforces issued<=now<expires. No public clock override exists.
Accepted expired static canonical vectors prove real crypto only through private
unit-test timing; the real trait correctly rejects them. No host signer or
fixture encoder is invoked.

The portable vendor snapshot is the full identity package described by its
`UPSTREAM.md` and `SHA256SUMS`, NOT a verification-only crate. Existing broader
crypto/signing source is included unchanged; no authority/key access is wired. See vendor/mesh-llm-identity/UPSTREAM.md
and SHA256SUMS. It is explicitly workspace-excluded, with default-features=false
and no host-io forwarding. Root lock integration/full checks remain primary-owned.
Exact-time machine signer, TLS/Store integration, current-policy provenance,
served model/canary and Ready gates remain separate and incomplete. Existing
Keychain failed qualification/defaultOFF and endpoint PASS at cc08962a are not
changed or rerun by this source milestone.

### Primary-reviewed current-time public fixtures (synthetic TEST DATA)

The existing test file embeds three EXACT fixed public certificate JSON constants,
including each original trailing newline. No extra fixture file, signer, encoder
or capture source is imported. Primary approved consumption after reading the
capture diff/metadata and independently checking18/18 public hashes:
[reviewed capture](agor://kb/document/01a160ea-273a-7685-8f4b-5c11b428bde1).

Source accepted commit: c2190fdd6bc4f9100ac5029621622341c3e8c8e6.
Capture was one temporary inline cfg(test) function beside the canonical tests in
an isolated external TESTCOPY, calling the existing PRIVATE canonical_claim_bytes
and canonical owner_id_from_verifying_key; no encoder duplication/public signing
hook/host signer/vendor edit. Fixed PUBLIC synthetic in-memory seed [7;32],
node[0x42;32], version1, label Some("synthetic-only"), hostname None for all.
Public key ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c;
derived owner fe812c12f3ab4ce6ac5db69ac352f906cb1b11ef43fb33e252ef7ff552263889.
IDs are m8s-synthetic-public-{long-window|future|millisecond-remainder}-v1.

|Constant|Issued / expires (exact milliseconds)|Exact JSON SHA256|
|---|---|---|
|LONG_WINDOW_JSON|1577836800000 / 4102444800000|5594d0ca6b4c6af64ff3f89414ca9dc0c710d45e666f95c20cc0f19a4a2d4f88|
|FUTURE_JSON|4102444800000 / 4102531200000|a3979316e53a105f5cecfbecf7924e777bb9323a52e42a8c88113c5713c3ea45|
|REMAINDER_JSON|1577836800001 / 4102444800000|cc81f9d23007237496ef3b4b91ad7cc39c865b7f20f819045eb5af1c70ef3dc5|

**2020..2100 is an ABSURD SYNTHETIC TEST lifetime, NOT production issuance policy,
authorization, backdating permission or a lifetime-gate relaxation.**
Tests assert real UTC2020-01-01<=now<2100-01-01, with clear failure outside that
support interval, never skip/clock override. The long-window cert verifies via
actual canonical Allowlist AND real CertificateVerifier::verify; uppercase
key/signature hex also succeeds (node42hex has no case-varying letters).
Future and +1ms remainder are separately signed canonical-base positives at
real now but adapter negatives, isolating strict future/exact-time binding.
Existing expired-vector/private-clock boundary tests remain unchanged.

Capture provenance files under /private/tmp/m8s-mp001-fixture-capture are review
evidence only, NEVER build/runtime paths:
- capture.patch SHA256 f13f76d98d510e66c75f097789944c0eee2fd1634ab181c72831dd363703f620.
- RECIPE.md SHA256 07c1460a8474972dde7b8cb9744f02adeed6491e7a7b97ee207a6f82946925f3.
- public/SHA256SUMS SHA256 de6e23d1c0bed660fc454e958fc2c1323d585740b362f3b8c16aca14781cb2bb
  covers18 public files (canonical binary/hex, JSON/signature, claims/control
  metadata, public key/ownerID and index). Rechecked18/18 before consumption.
- baseline-source.sha256 SHA256 4d2ff52354238cd81a6da161c187cb6f696cc929de0770c3a7ec0210cf8be833.
- Accepted original node_certificate.rs SHA256 0c872bbb0294837b428a6bee924a2d548cca9c95b8ebad052844ec0a3945e1eb;
  temporary TESTCOPY SHA256 e7017e7082bd5010d8e73308f2f3919fa54181c8af51a525422a2a0527cfc0c9.
- Accepted capture Cargo.lock SHA256 45ef597032fa72257da8142c832d043ec878ee1531130da6467826a80043cecf.
- compiled-packages.txt SHA256 6b29dfda4729a332a4d14dbe9cfcaebd102e1fe00137171827eeaaeef7131c86;
  includes ed25519-dalek3.0.0,sha2=0.11.0. Capture rustc1.98.0
  (88d9e12ae178fab0fb5cc050a94da85685d449ea),cargo1.98.0,aarch64-apple-darwin.
- capture.log SHA256 d1dc8165656671e2e0af6928acd7296d6977563b345752d63843c4d82f0079a5:
  first/only capture1PASS,17filtered; each cert canonical Allowlist Verified
  at issued+1000ms and3000ms before issuance, Expired at exact expiry.
- Reviewed public-only archive SHA256 9af276dc014fb1d3775335d3ede541be9b03c4243b374f2173b884e4b6268d08.
  Archive/provenance are not production authority or source acceptance.

Capture recipe (already executed by authorized fixture owner, NOT by this adapter):
fresh no-hardlinks clone detached at accepted commit, only temporary capture.patch
in testcopy; own registry-only cache, empty HOME, inherited environment cleared;
existing preinstalled1.98 toolchain, empty RUSTC_WRAPPER and system cc linker.
From the testcopy, cargo test -p mesh-llm-identity --lib --no-default-features
--locked --offline -j1 node_certificate::tests::m8s_synthetic_public_fixture_capture
-- --exact --nocapture. Do NOT rerun capture without separate permission.
The adapter tests consume only approved public outputs, never seed/signing APIs.
No production API/source/dependency/vendor changes accompany these fixture tests.
