# Durable bootstrap persistence (not a live admission listener)

The daemon now exposes one authoritative Store-backed launch/nonce adapter.
It still runs mock-only: main does not bind an iroh listener, contact a real
provider, wire MP-001 signer/verifier, or advertise Ready.

## One authority and public contracts

`Store::issue_bootstrap_nonce(nonce: &[u8;32], LaunchIdentity, now)` registers
trusted owner/provider/image observations for an existing exact node UID +
instance in Booting, desired>0, and not deleting. `LaunchIdentity` contains the
owner label (must equal Store/pool owner), node UID and `m8s_bootstrap::LaunchBinding`
(pinned owner transport endpoint, provider instance/container/IP, verified signed
image digest). These are **trusted adapter inputs**, not proof supplied by agent
request fields. Real provider identity/scoped self-destroy proof is not implemented.

`Store::redeem_bootstrap(session, request, expected, observation, now, cert_expires_at,
join_token, signer, verifier) -> m8s_bootstrap::Result<PendingDelivery>` uses the
bootstrap crate's opaque `ValidatedRedemption` and `NonceConsumer` boundary.
Only the real transport/request validation helper can create the opaque validated
record; no public session/proof constructor was added. The private observation-bound Store adapter implements consume
and certified hooks using its existing `nonce` table. It NEVER instantiates the
bootstrap crate's standalone `Ledger`/`bootstrap_nonce` table. Do not configure
that standalone ledger as a second daemon authority.

Legacy `issue_nonce` / `redeem_nonce` raw-string APIs return
`BootstrapUnavailable` unconditionally. They cannot authenticate owner/container/
UID/endpoint identity and are not alternative successful redemption paths.

## Transactions and effects

1. Issue uses BEGIN IMMEDIATE and INSERT..SELECT, atomically requiring exact pool
   owner, UID+instance, Booting, desired>0, deleting=0. One modern nonce per launch
   UID; no reissue after failure/consumption. Persisted nonce hash, never nonce.
2. `NonceConsumer::consume` atomically checks the same authoritative gates plus
   instance/container/IP/image/owner-endpoint binding, issued<=now<issued+900,
   unused and uncancelled. It commits burn + redeemer endpoint/cert expiry +
   cleanup-required safety intent BEFORE returning to the external signer.
3. No transaction/await spans signing or verifier. Error, unwind, dropped delivery
   or crash leaves consumed intent and endpoint revocation identity durable.
4. After verification, certified hook stores public certificate SHA256 and
   rechecks active launch state. Down during signing refuses the bundle and keeps
   cleanup intent. A cert minted before down still requires revocation/cleanup;
   a database transaction cannot atomically retract an external signature.
5. `bootstrap_attempts()` returns owner-scoped (plus explicitly unbound quarantined legacy) recovery metadata, not secrets or
   admission proof. `confirm_bootstrap_delivery(&attempt, now)` is a trusted owner
   persistence hook, called ONLY after actual encrypted stream acknowledgement.
   It clears cleanup-required only for an unexpired certified attempt whose
   exact launch is still active. It is NOT a cryptographic delivery receipt.
   Current main never calls this hook; wiring it correctly is a later integration.
6. Down, scale0, Destroying and observed owner/instance mismatch atomically mark
   persistent deletion/cancellation and cleanup intent before provider mutation.
   Later up cannot revive the old nonce or node. Reconcile retains the deletion
   intent even if desired becomes positive again; foreign-owner mismatch never
   authorizes provider destruction.
7. The existing explicit per-instance provider-confirmed absence path is still the
   only node-row deletion site. Nonce tombstones/revocation metadata survive it.
   No Ready/Bootstrapped phase promotion is introduced.

`cleanup_required` starts true at burn as a safety obligation, not an assertion
that provider destroy/revocation already ran. A future owner operation manager
must distinguish an owned in-flight operation from abandoned/restarted/deadline/
cancelled work, drain/revoke/destroy appropriately and confirm provider absence.
No cleanup/revocation worker is claimed wired by these persistence hooks.

## Migration, file and process safety

Old incomplete nonce rows migrate once to legacy-quarantined/cancelled; matching node rows
are marked deleting so a fresh token cannot silently reauthorize the same old
launch. Rows are not deleted by migration. Owner-scoped reconciliation determines
cleanup and only explicit absence releases finalizers. One-time quarantine never re-targets a later UID if a provider recycles an ID.
Unbound old endpoint text
is not upgraded into cryptographic identity.

Store requires an existing real private parent directory (0700), private regular
DB/WAL/SHM files (exact0600), no symlink/hardlink files and SQLite NOFOLLOW.
DB is precreated0600, WAL uses synchronous FULL. Canonicalizing a validated private
parent normalizes macOS /var system aliases before NOFOLLOW; direct symlink state
parent/DB/sidecars still fail. Non-Unix library opens fail closed until equivalent ACL policy exists.
Existing overly broad files fail closed instead of
silently chmodding them. Daemon process fs2 lock and private Unix API are unchanged;
callers must still hold the process lock across a whole reconcile/provider loop.
Concurrent SQLite tests qualify transactions, not a multi-reconciler deployment.

## Test evidence boundaries

Tests use synthetic trusted identity/image observations and deterministic mocks,
actual SQLite two-connection races, restart, down-before/during/after consumption,
late delivery, expiry/replay/wrong binding, signer error/unwind windows, legacy
migration, private paths and provider-confirmed absence. SQL fixtures do NOT mint
production transport proof types and are NOT actual TLS/MP-001/provider tests.
Bootstrap tests separately cover real Ed25519 test signatures and consumer hook
ordering. Real transport qualification is still AF_ROUTE/RAW-blocked in this
sandbox; no realnet retry or bypass was made for this persistence slice.

## Review corrections: observation, quarantine, filesystem and delivery

`LaunchIdentity` now includes `observed_at: i64` (local injected owner clock)
and `status: ObservationStatus::{Complete, Partial, Unknown, Error}`. The owner
adapter must independently obtain a complete successful snapshot binding the
exact owner, persisted node UID, instance, container, direct IP and image digest.
Only `Complete` with `0 <= now - observed_at <= 30` is accepted; future/negative
local observations are refused. Issuance checks it before opening a transaction.
Redemption now requires the observation as an additional argument:

```text
Store::redeem_bootstrap(session, request, expected, observation,
                       now, cert_expires_at, join_token, signer, verifier)
```

A private observation-bound consumer rechecks freshness/status/owner/exact
LaunchBinding at consumption and binds the observed UID in the same atomic burn
SQL. Store no longer implements a publicly reusable observation-free consumer.
Both local times must come from the trusted adapter clock, not agent/provider
wire timestamps. These inputs remain **trusted assertions, not verified provider
proof**; main has no production provider observation adapter and admission stays
unavailable. Unit tests exercising private SQL records do not qualify transport.

Recovery listing explicitly represents `BootstrapStage::LegacyQuarantined`.
Unbound legacy rows are included under the already-verified single-owner database
boundary, retaining original owner metadata (possibly empty), instance,
`redeemed_at`, cancellation/cleanup intent and `legacy_endpoint` text. Text is
never interpreted as a verified endpoint, even if it happens to look like a key.
Quarantined rows cannot authorize delivery or redemption. Redeemed legacy rows
retain cleanup intent. Modern rows still require canonical 32-byte endpoint blobs.

Filesystem policy is now exact: effective-UID ownership, directory0700,
regular single-link DB/WAL/SHM0600, no special permission bits, and **no extended
ACL** (even a restrictive one). Descriptor-based no-follow/nonblocking opens,
fstat policy, ACL checks, and device/inode/path rechecks surround SQLite opening
and schema setup; original directory/DB descriptors remain held during startup.
Darwin descriptor ACL absence is the live-fd `acl_get_fd_np` ENOENT result; any
returned ACL or other error is refused. Linux uses descriptor POSIX access/default
ACL xattr absence. Other platforms fail closed. This is a startup boundary, not
a continuing filesystem monitor or a defense against hostile same-effective-UID
processes. Ancestors above the canonical private directory must be trusted not
to rename/replace it. Trusted system ancestor symlink aliases are normalized;
the actual private directory and final files cannot be symlinks. No automatic
chmod/chown or ACL removal occurs. Tests inject foreign UID metadata without
chown, exercise real ACL fixtures on Darwin, and detect descriptor/path replacement.

The transport correction binds `PendingDelivery` to an opaque allocation identity
created **only after** successful complete request decoding/peer/path validation
on the original boot connection/stream. The capability also fingerprints every
accepted request field. A different same-owner/node connection cannot deliver
that pending bundle; an early/cancelled/malformed receive has no capability even
though its attempt latch stays terminal. Changing a request after receive fails
before burn. No caller-forgeable ID or public proof constructor was introduced.
These are pure internal negative-policy tests; the actual iroh loopback remains
runtime-unqualified and is not retried in this review slice.
