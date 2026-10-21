# m8s-bootstrap

Owner-side **protocol-policy slice**, not a working bootstrap server, encrypted
bundle codec, MP-001 admission implementation, or provider integration.

## API boundary

- `LaunchBinding`: owner-persisted instance/container ID, provider-confirmed public
  IP and verified signed-image digest. Request values are compared to this record;
  an agent's claim is not provider verification. The caller must check launch
  ownership and Booting state before issuing and redeeming.
- `BootNonce`: 32 caller-generated CSPRNG bytes, zeroizing, no Debug/Serialize.
  SQLite persists only SHA-256 and public launch metadata. Supply injected seconds.
- `Ledger::redeem`: exact launch + authenticated endpoint binding, `[issued,
  issued+900)` validity, atomic burn, then `NodeSigner` and independent
  `CertificateVerifier`. Sign/verify/delivery failures never restore the nonce.
  The supplied expiry is owner policy, not agent input. Signer adapters must verify
  actual MP-001 owner/node/validity claims; test authorities are not cryptography.
- `ControlAcl`: explicit owner/delegate endpoint list and control ALPN.
  `HeartbeatGuard`: authenticated node endpoint + owner binding, increasing
  sequence, injected monotonic observation time, certificate expiry and TTL.
  A late heartbeat cannot revive an expired guard. Persist state before ack;
  restarts must not silently reset replay protection or DMS deadlines.

`IrohSession` has **no public constructor**. `accept_iroh` and
`PendingDelivery::deliver` always return `Unavailable`. No unauthenticated input
can manufacture accepted transport evidence using this public API. Internal unit
fixtures alone construct synthetic sessions. There is no plaintext fallback and
no escape hatch returning join material. No production signer/verifier exists.

## Remaining integration gates

An iroh adapter must negotiate `m8s-boot/1` / `m8s-control/1`, pin the owner,
authenticate the peer endpoint with possession of its private key, and deliver
only on the original peer-bound encrypted stream. In this policy the mesh node
endpoint and authenticated boot peer are the same key; separate sidecar keys need
an explicit possession-proof protocol before changing that constraint.

**Relay address is not source IP.** ADR-0006 IP binding cannot use the relay's IP
or agent-reported IP. Relay-only boot must fail closed until a separately verified
origin-evidence design is approved. This is an unresolved deployment blocker,
not a reason to relax encryption or IP checks.

Wire framing/size limits/timeouts, sealed bundle model/config/heartbeat fields,
real certificate signing/verification and revocation, provider identity fetching,
and DMS destruction are not implemented here. `self_destruct_present` is merely
an agent assertion, not proof a container-scoped destroy key works. Consumed nonce
or verification/delivery failure requires owner-side alert/cleanup/revocation;
the crate performs no provider effects. Never infer Ready from a successful policy
redemption: verified cert + served model + owner-routed canary remain separate.

The separate `bootstrap_nonce` table is not wired to m8sd's `nonce` or `gpu_node`
tables. Integration must atomically gate launch state/ownership and nonce issuance;
do not run both ledgers as independent authorization authorities. The ledger rejects connections already inside an outer transaction, so a later
rollback cannot restore a consumed nonce after signing. Configure the owner DB
for durable storage; SQLite errors fail closed.

Workspace glob already includes the crate. Integration owner must update
`Cargo.lock` (new package depends on rusqlite, sha2, thiserror, zeroize); consumers
may add `m8s-bootstrap = { path = "crates/m8s-bootstrap" }` under workspace deps.

Tests use synthetic identities, internal channel fixtures and a mock authority.
They cover expiry, binding mismatches, replay, two-connection contention, actual
SQLite close/reopen, signer/verifier failure burns, ACL, heartbeat and unavailable
transport. They are not real admission, iroh networking, or provider tests.
