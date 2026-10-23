# Composed MP-001 bootstrap qualification — Stage A

**Harness-only diagnostic revision; ignored runtime NOT EXECUTED again.** The
[first Stage B attempt](agor://kb/document/01a1617e-de61-73d7-8027-68a8fb120565)
failed at endpoint bind with the old opaque phase; actual cause remains **UNKNOWN**.
Its original log/hash/evidence is preserved. The one-shot authority is consumed:
no automatic retry, fallback, probe or permission/security expansion is authorized.

The bind failure now fails with a REDACTED fixed stage label from locked iroh
1.3's typed `BindError`, a static IO-kind category and optional typed raw OS-error
integer. Source inspection is bounded to four nodes; absent/deeper/unknown IO is
reported unavailable, never inferred. No underlying Display/Debug, message,
metadata, address, path, key or config is emitted. Future narrowly authorized
output is diagnostic information, **not proof of the past cause or a runtime fix**.
Other `need()` redaction and all runtime lifecycle/case bodies are unchanged.

This standalone package remains excluded from the 10-member root workspace and
owns its lockfile. Four normal pure `bind_diagnostic_` tests exercise typed errors,
redaction, source depth and descriptor formatting without endpoint, mesh key,
Store or platform operations. One current-thread Tokio test, `composed_bootstrap`,
remains ignored. Root checks do not include this package. Previous Gate 2 Admin
PASS covers only cc08962a, not this composition.

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

## Sequential source cases — protocol groups NOT REACHED in the first attempt

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
for later explicitly authorized inspection. Stage A created no fixture; the first
Stage B attempt kept an artifact root before bind failed. Its contents/state
remain uninspected.
No raw nonce/join/config/key/request/certificate dumps or Debug logging; failures
use fixed nonsecret phases or the typed REDACTED bind descriptor. Host-io
compilation does not execute a host signer,
key loader, Keychain or any legacy host tests. Keychain remains default OFF after
its single failed qualification; no gate is lifted here.

## Stage A commands / isolation

Use already-installed Rust 1.98.0 aarch64-apple-darwin, jobs1, serial builds,
private registry-only copied cache and own target. No config/credentials/git
cache/hardlinks copied. Ordinary public crate downloads are permitted; no
provider/network-service/runtime calls. Dependency build scripts are compilation,
not execution of the ignored body. Root Cargo.lock must remain byte-identical.

```sh
cargo test --manifest-path qualification/bootstrap-mp001/Cargo.toml --locked -j1 --test composed_bootstrap -- bind_diagnostic_ --test-threads=1
cargo test --manifest-path qualification/bootstrap-mp001/Cargo.toml --locked -j1 --test composed_bootstrap --no-run
cargo test --manifest-path qualification/bootstrap-mp001/Cargo.toml --locked -j1 --test composed_bootstrap -- --list
cargo test --manifest-path qualification/bootstrap-mp001/Cargo.toml --locked -j1 --test composed_bootstrap -- --list --ignored
cargo clippy --manifest-path qualification/bootstrap-mp001/Cargo.toml --locked -j1 --all-targets -- -D warnings
cargo fmt --manifest-path qualification/bootstrap-mp001/Cargo.toml --package m8s-bootstrap-mp001-qualification -- --check
```

The execution command explicitly filters ONLY the normal pure diagnostic tests.
Neither it nor the compile/list commands invoke the ignored runtime body.
Both listing commands use `--list`; the latter filters listing ONLY, never calls
an ignored body. Do not omit `--list`. Root default/all-feature current-host
metadata and inverse graphs must show 13 members, harness exclusion and no
identity host-io. Harness metadata intentionally shows host-io. The standalone
lock includes optional/platform host packages; locked does NOT mean compiled.
Primary owns full root `scripts/check` AFTER freeze; worker does not run/compete
with it. Source is paused UNCOMMITTED for independent review and primary checks.
Exact two-file freeze hashes, focused command outcomes and limitations are
recorded in the accompanying diagnostic Knowledge evidence, not runtime claims.


## Prototype operator qualification (ordinary permitted Terminal only)

The managed runtime already denied netmon initialization. **Do not execute this
packet there or change permissions/disable monitoring.** In a known-permitted
normal macOS Terminal on this machine, the exact one-shot command is:

```sh
/private/tmp/m8s-root-integration.VSK0VT/qualification/bootstrap-mp001/run-permitted-once --run-once-in-permitted-terminal
```

The wrapper verifies the accepted dependency-source base, qualification
manifest/lock/test and already-compiled binary hashes before any endpoint call.
It creates a private evidence root and exclusively claims one invocation in
ignored `bundles/prototype-secure-operator-once.attempt`; it never clears that
marker, retries, rebuilds, resets state, reads real keys or changes policy.
Return the printed evidence path and exit code, not environment or credential
values. Failure preserves all artifacts and may follow synthetic durable effects.
A pass qualifies this synthetic composed transport packet only—not an actual
container/model inference, real owner issuance profile, provider cleanup or
paid launch. New prototype dependency-source changes require a refreshed packet.

Preparation compiled only (`--no-run`); no ignored body was executed here.
The updated standalone lock preserves every original registry package
version/checksum and adds only the dependencies required by accepted local
control/config changes. Source/compile status is separate from runtime status.
