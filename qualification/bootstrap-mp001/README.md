# Composed MP-001 bootstrap qualification — Stage A

**SOURCE/COMPILE ONLY. Runtime cases: NOT EXECUTED.** This standalone package is
excluded from the 13-member root workspace and owns its lockfile. There is one
ignored current-thread Tokio test, `composed_bootstrap`. Normal root checks do
not include it. Do not invoke its ignored body or a capability probe in Stage A.
Stage B requires a **separate explicit primary instruction after frozen source
acceptance**, in a permitted usable runtime context. No fallback or automatic
follow-on. Previous Gate 2 Admin PASS covers only cc08962a, not this source.

Design: [bounded composition](agor://kb/document/01a16108-e37d-75cf-8223-ea95551f59bc).
Base: 14ff55348bf5b5559e6214f09c2d26a34736c58b.
Vendor authority: accepted cecee5d23e33d2626a0bf4624bfa71dc1615d167 Git objects;
committed relative path dependencies only. No external checkout build authority.

## Actual lifecycle in the ignored source

- Fresh canonical private SQLite fixtures: Store creates schema/owner, scale
  sets desired, then a closed fixture transaction inserts ONLY synthetic
  `gpu_node` Booting/instance binding and `launch_attempt` rows. No standalone
  Ledger, provider calls/fake provider, nonce SQL writes/resets, budget or
  approval fixtures. A schema assertion refuses a `bootstrap_nonce` table.
- Explicit 127.0.0.1 sockets, Minimal iroh, no relay or address discovery. TLS
  owner/node identities and mesh signing key are distinct. Source contains only
  fixed inline synthetic key seeds; no HOME/default path/keystore/CLI/Keychain
  or disk key source. Compiled host-io is isolated to this excluded package.
- Actual `accept_boot` / `BootConnection::receive` create private session and
  request-bound delivery proof. Actual `Store::redeem_bootstrap` burns durably.
  The NodeSigner uses an independent SQLite connection with busy timeout zero,
  `BEGIN IMMEDIATE`, exact consumed/cleanup/identity/time assertions, `ROLLBACK`,
  and connection close BEFORE crypto. No SQL transaction spans effects/awaits.
- Actual accepted `sign_node_ownership_exact`, in-memory `OwnerKeypair`, explicit
  TEST policy (60,000ms lifetime / 30,000ms issuance age), actual pinned canonical
  server/client verifiers. These are TEST VALUES, not production defaults or
  automated-invoker authorization. Original pre-SQL seconds use checked *1000;
  stalls may fail stale, never rebase/renew/retry. Verifiers use real SystemTime.
- Actual original-stream delivery, real client verification and exact claims,
  certificate, node, join/config/settings checks. Confirmation occurs only after
  BOTH actual server delivery and client success, then asserts persisted Delivered.

## Sequential source cases — all NOT EXECUTED

1. Genuine positive TLS + SQL + canonical crypto; replay after actual Store close
   and reopen refuses before another signer invocation and preserves history.
2. Fail-only signer after burn: persisted Consumed + cleanup, no positive fake.
3. Canonical sign then lost delivery: persisted Certified + cleanup after reopen.
4. Down before consume: Cancelled, zero signer invocations.
5. Down inside signer after committed-burn probe: actual sign, certification
   refuses; Consumed + cancelled + cleanup after reopen, no false delivery.
6. Same TLS peers, wrong original stream: fixed two-pair architecture with
   **distinct endpoint actors/sockets but identical owner/node TLS key seeds**.
   Each client actor connects only its paired accept socket. This forces two
   actual TLS accept/receive sessions without relying on a connection-cache
   retry/fallback or weakening identity. Only A signs; A pending into B refuses.
7. Exact dispatch-time boundary: canonical current-time signing, then public
   deliver(now == stored expiry) refuses. **Injected dispatch boundary, NOT
   observed wall-clock timer expiry qualification.** No fake-clock positive.
8. Actual received TLS request with wrong expected origin refuses before burn.
   127.0.0.1 is a LOOPBACK FIXTURE observation, NOT cloud ownership evidence.
9. Actual received TLS request with wrong expected owner refuses before burn.
10. Actual wrong-ALPN handshake refuses; mismatched local node claim refuses in
    real client API before connecting. Both leave the attempt Issued, signer zero.

A positive is never represented by a boolean signer/provider substitute. No
public proof/session constructor, copied encoder or production API was added.
No confirmation occurs in any failure case. Persisted synthetic launch metadata
is NOT proof of provider ownership, launch authorization, accounting, budget,
approval or effects. Delivered is NOT Ready/served model/canary/billing/absence
or refund proof. Production numerical owner policy/invoker route remain unresolved.

## Bounds and artifacts

Adapter IO bounds plus one 60s TEST-only async timeout bound sequential work.
Tokio cannot preempt synchronous SQLite/crypto/kernel stalls: NOT a hard deadline.
No child/PID/group/watchdog, capability probe, netmon bypass or permission change.
Success explicitly closes endpoints. Panic/timeout drops futures/endpoints and
preserves uncertainty; there is no automatic reset/delete/retry. Private fixture
root (`m8s-composed-bootstrap-*` in OS temp) is kept immediately, on success too,
for later explicitly authorized inspection. No fixture has been created in Stage A.
No raw nonce/join/config/key/request/certificate dumps or Debug logging; failures
use fixed nonsecret phases. Host-io compilation does not execute a host signer,
key loader, Keychain or any legacy host tests. Keychain remains default OFF after
its single failed qualification; no gate is lifted here.

## Stage A commands / isolation

Use already-installed Rust 1.98.0 aarch64-apple-darwin, jobs1, serial builds,
private registry-only copied cache and own target. No config/credentials/git
cache/hardlinks copied. Ordinary public crate downloads are permitted; no
provider/network-service/runtime calls. Dependency build scripts are compilation,
not execution of the ignored body. Root Cargo.lock must remain byte-identical.

```sh
cargo test --manifest-path qualification/bootstrap-mp001/Cargo.toml --locked -j1 --test composed_bootstrap --no-run
cargo test --manifest-path qualification/bootstrap-mp001/Cargo.toml --locked -j1 --test composed_bootstrap -- --list
cargo test --manifest-path qualification/bootstrap-mp001/Cargo.toml --locked -j1 --test composed_bootstrap -- --list --ignored
cargo clippy --manifest-path qualification/bootstrap-mp001/Cargo.toml --locked -j1 --all-targets -- -D warnings
cargo fmt --manifest-path qualification/bootstrap-mp001/Cargo.toml --package m8s-bootstrap-mp001-qualification -- --check
```

Both listing commands use `--list`; the latter filters listing ONLY, never calls
an ignored body. Do not omit `--list`. Root default/all-feature current-host
metadata and inverse graphs must show 13 members, harness exclusion and no
identity host-io. Harness metadata intentionally shows host-io. The standalone
lock includes optional/platform host packages; locked does NOT mean compiled.
Primary owns full root `scripts/check` AFTER freeze; worker does not run/compete
with it. Source is paused UNCOMMITTED for independent review and primary checks.
Exact command outcomes, lock/graph review, frozen ten-file SHA inventory and
known failures are recorded in the accompanying Knowledge evidence, not runtime
qualification claims.
