# Durable bootstrap persistence (not a live admission listener)

The daemon now exposes one authoritative Store-backed launch/nonce adapter.
It still runs mock-only: main does not bind an iroh listener, contact a real
provider, wire MP-001 signer/verifier, or advertise Ready.

The default-off `secure-bootstrap` feature provides
`Store::deliver_bootstrap` in `bootstrap_delivery`: one authenticated
`BootConnection` is received, redeemed through this Store, signed and verified
with the concrete MP-001 verifier, then delivered on its original TLS stream.
Only an acknowledged delivery followed by successful Store confirmation returns
`DeliveredBootstrap`. The clock is sampled again after acknowledgement, so
expiry or cancellation cannot produce successful confirmation. A failed or
cancelled attempt after nonce burn retains cleanup intent and must not be
retried with a new nonce. The caller supplies current trusted launch observation,
owner-side signer custody, bundle settings, listener and surviving cleanup.
This library composition does not yet wire the CLI, launch a native node,
implement its DMS loop, or prove readiness/inference. Its new tests exercise
record selection and persistence with synthetic fixtures; no endpoint or provider
qualification was rerun.

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

## Durable all-in budget intent (not live spend integration)

`m8sd::budget` adds owner-scoped `budget_account`, immutable `budget_period`, and
retained `budget_reservation` rows on **the same private Store connection/database**.
No separate journal, file, process, background executor or budget authority is
created. `Store::open` initializes only these schemas. Existing reconcile remains
explicitly mock-only and does **not** call these APIs. This is not permission to
rent, a verified Admin approval, a billing proof, or a wired paid-spend guard.

Public trusted-operation-manager API:

```text
reserve_budget(&AdminBudget, &CalendarPeriod, &BudgetRequest, now)
    -> Result<BudgetReservation>
budget_reservations() -> Result<Vec<BudgetReservation>>
mark_budget_uncertain(request_id, now) -> Result<()>
bind_budget_resources(request_id, &ResourceBinding, now) -> Result<()>
settle_budget(request_id, &CalendarPeriod, &SettlementEvidence, now)
    -> Result<BudgetReservation>
```

All times are trusted injected local seconds; costs use core `MicroUsd = u64`.
Every input is a **trusted adapter assertion, not independently verified evidence**.
Keep all IDs/provenance non-sensitive; no credentials, tokens or secret quote
payloads belong in these serializable/debuggable metadata types. There is no
approval default. The caller supplies explicit Admin owner/approval ID, validity
window, lifetime all-in trial cap (the initial authorized trial may be2,000,000
microUSD; that value is NOT a universal product limit), monthly/total-hourly/
per-instance-hourly/concurrent-instance/runtime caps, minimum cleanup grace and
contingency. The first accepted envelope is immutable for that owner: changing
approval ID, caps or run ID cannot replenish/reset the account. No amendment API
exists in this slice.

Reservation is BEGIN IMMEDIATE, pool-owner checked, and commits before any future
hypothetical create. Complete successful cost observation must be nonfuture,
nonnegative and at most30s old. `CostQuote.valid_until` is explicitly the normalized
rate/all-components cost bound THROUGH the reserved cleanup deadline, inclusive;
it is not merely offer acceptance validity. Checked runtime+cleanup deadline must
be <= this bound (equality accepted); short acceptance quotes cannot qualify. Runtime+cleanup must fit the explicit
approval window. It reserves separately rounded-up compute and storage over
runtime+cleanup, plus explicit full transfer and contingency totals. Checked
u128 intermediates/checked u64 conversion prevent truncation and overflow.
The **all-in hourly** cap is conservatively defined as
`ceil(full_reserved_microUSD * 3600 / serving_runtime_seconds)`; this includes
cleanup/storage/transfer/contingency rather than merely the provider compute
rate. Lifetime actual costs plus every unresolved full reservation count against
the immutable trial cap. Current settlement-period actual costs plus unresolved
reservations from **all** periods count against the monthly cap; unresolved
all-in hourly and instance counts aggregate across runs/restarts.

`CalendarPeriod` is an explicit trusted UTC calendar-month interval, not derived
from an untrusted clock. Adapter must supply real month boundaries; Store checks
28–31day bounds, now in `[start,end)`, immutable ID+boundaries, monotone local clock
and only nonoverlapping forward transitions. There is no automatic epoch reset.
A known final bill is conservatively charged in full to its explicit settlement
period, including bills resolved after rollover. Lifetime spend never resets.
An unresolved earlier period carries its full liability into later months.
An ambiguous operation, unknown final billing, or missed reserved cleanup deadline
freezes **all fresh spend**, even if numeric caps have headroom. Clock rollback,
future observations/windows, incomplete/unknown/error quote evidence refuse.

Request ID and node UID are owner-unique forever. Exact request+quote+period retry
returns retained metadata without debit, state/clock mutation or renewed quote;
changed request fingerprint refuses. Recovery listing remains available when old
quotes/windows no longer qualify. A returned reservation is **not an effect or
repeat-create authorization**. Future operation manager must persist effect/claim
intent and deduplicate provider effects; this API alone does not implement
exactly-once create. Mark uncertainty before ambiguous effects; never rerent just
because a process died or an API returned an error. Down/up and provider-confirmed
node cleanup never delete/reset these accounting tombstones.

Resource binding is one immutable complete fresh owner/run/nodeUID/instance and
explicit volume set (or explicit `NoneAllocated`), with provenance. Missing or
empty lists are not implicitly no-volume. Settlement must exactly match this
retained binding and supply a fresh complete trusted observation of owned-instance
absence AND every bound volume's absence, plus explicit final billing. Destroy
acknowledgement, empty list, error, partial/unknown, stopped and process-dead
enums do not qualify. `FinalBilling::Unknown` retains full reservation and blocks
fresh spend, even with complete absence. Known final actual cost alone permits
unused release; actual overruns are recorded in full even above all caps (never
clamped/refused to hide debt). Exact repeated settlement changes no accounting;
changed settled evidence refuses. No asynchronous/provider call runs inside a
transaction; no effect executor exists.

Remaining gates: trusted real Admin/clock/cost-unit/complete resource/billing
adapters, historical external debt reconciliation before enabling spending,
durable operation-manager effect claiming/deduplication, independent remote
watcher/deadline/destroy/volume cleanup, secure runtime prerequisites and actual
provider qualification. Missing or ambiguous evidence must keep paid admission
disabled. The earlier Python fake watchdog journal is unchanged and is NOT
production budget authority. Tests here are actual SQLite races/restarts and
synthetic trusted observations/mocks, not a paid/provider/watcher qualification.

### Explicit initial reconciliation and normalized units

The Admin envelope additionally requires `AccountBaseline` with prior
`prior_lifetime_spent`, `prior_current_period_spent` bound to explicit
`initial_period: CalendarPeriod`, and explicit `ExternalLiability::{NoneConfirmed,KnownReserved,Unknown}`,
local observed time, completeness and provenance. **No missing baseline defaults
to zero**. `initialize_budget(&AdminBudget, &CalendarPeriod, now)` persists this
trusted fresh baseline once even if it is unknown or already exceeds approved
caps; `budget_account()` exposes it for recovery. Reserve first performs this
idempotent initialization in its own short transaction, so an accepted initial
debt baseline remains recorded even when the proposed reservation is refused.
The reservation/accounting decision itself still commits atomically in a single
BEGIN IMMEDIATE. Missing account with existing reservation records is corruption,
not a new epoch. Prior lifetime spend always counts against the lifetime cap; prior current-period
spend counts against its exact immutable initial period. It must not exceed prior
lifetime spend. Actual late/cross-period new bills count in their settlement
period, while all unresolved liabilities carry into every period. Nonzero known external liability and unknown
external liability freeze fresh spending across all periods until a future
explicit Admin-approved reconciliation/migration exists; they cannot be silently
cleared by changing envelope, run, month, down/up or process restart. Initial
zero spend + `NoneConfirmed` must be an **explicit trusted reconciliation**, not
inferred from an empty local ledger. This covers the tracked approval and its
baseline, NOT universal provider account balance/credit/debt proof.

Compute and storage each require explicit `RateUnit::MicroUsdPerHour`; transfer
and contingency require `TotalUnit::MicroUsd`. Unknown/unsupported/missing units
are refused, not coerced. Provenance must identify the trusted normalized quote;
this module does not resolve provider pricing-unit conflicts. Reservation math
is intent only: it does NOT prove watchdog runtime/cleanup deadlines, storage or
egress limits are actually enforced. The immutable envelope limitation is for
this bounded implementation, not a permanent product restriction: future explicit
Admin-approved increases or migration need their own reviewed operation, retaining
all spend/liability history. Late/cross-period actual bills, including overruns,
are charged in full to the explicit settlement month and remain lifetime charges.

## Durable operation/create claim (no executor)

`m8sd::operation` adds `create_operation` in the same private Store database;
no separate file, connection, provider authority, dependency or background worker.
Only `Store::open` schema initialization is wired. Existing main/reconcile/budget/
nonce/provider behavior and transport proof boundaries are unchanged.

```text
prepare_create(&CreateIntent, now) -> Result<CreateOperation>
claim_create(operation_id, now) -> Result<CreateClaim>
cancel_create(operation_id, now) -> Result<CreateOperation>
record_create_result(operation_id, &CreateObservation, now)
    -> Result<CreateOperation>
create_operations() -> Result<Vec<CreateOperation>>
```

`CreateIntent` immutably binds owner/run/request/nodeUID/operationID and supplied
canonical nonsecret launch fingerprint. The fingerprint is a **trusted adapter
assertion**, not signed-image/specification/admission proof. Recovery metadata
`CreateOperation` is cloneable/serializable, never effect permission. Exact
prepare retry returns retained metadata; changing identity/fingerprint refuses.
Unique owner/request and owner/nodeUID constraints prevent operation-ID rotation
from bypassing a used/cancelled budget reservation.

Every mutation is owner-checked BEGIN IMMEDIATE. Prepare requires exact existing
Reserved budget metadata with no prior resource/settlement, valid normalized cost
arithmetic, current trusted clock/approval/quote bounds and an exact persisted
Launching node with no instance, deleting=0 and desired tier>0. Missing/corrupt
account/reservation/period refuses, never means zero cost or absence. Claim
rechecks the immutable original reservation snapshot AND current aggregate
budget charges/caps/other unknown liabilities, so a late overrun or uncertainty
cannot be bypassed with an old prepared record. It rechecks live node deletion
and desire in that same transaction: existing down/scale0 permanently set deleting,
so down/up, missing node or changed phase cannot silently restore dispatch.

Exactly one Prepared->DispatchConsumed transition may return `CreateClaim`.
Dispatch timestamp + cleanup-required intent AND linked budget Uncertain state
commit **before** returning it. The claim has private fields, no public constructor,
Clone/Debug/Serialize/Deserialize; only read-only exact intent and anchored serving/
cleanup deadline accessors. No recovery API remints it. Drop, unwind, restart,
ambiguous response, timeout or caller PID change cannot restore authority. No
lease/takeover/reset/delete API exists. A compile-fail ownership test rejects
moving the claim twice. A future trusted executor must take the claim **by value**,
use its exact specification and enforce its original reservation-anchored deadline;
it must not restart a full runtime when dispatch is delayed. The type itself does
not prove deadline/egress/watcher enforcement. There is no executor in this slice.

Explicit cancellation changes Prepared or DispatchConsumed irreversibly to
Cancelled, preserves consumed-dispatch history, and marks budget uncertainty;
no refund or retry is inferred. Preclaim cancellation requires no external cleanup
because no create authority was issued, but reservation liability is retained.
After claim, cancellation keeps cleanup-required: an outstanding issued claim
cannot be atomically recalled or prove an effect was not sent. No DB transaction
spans any future external effect; tests keep a claim alive while another actual
SQLite connection cancels. Cancellation on corrupt/missing accounting refuses
rather than performing an implicit repair/refund; dispatch remains fail-closed
until explicit trusted recovery. Existing down reads are a claim gate, not a new
operation cleanup executor or automatic operation-state integration.

`CreateObservation::Ambiguous` covers timeout/error/ACK/lost or incomplete result;
it retains uncertainty and cleanup obligation, never absence/zero cost. A Bound
result requires complete <=30s fresh local trusted owner/run/nodeUID observation,
not predating durable dispatch, exact instance and explicit volume IDs (or explicit
NoneAllocated). It is stored immutably on the exact operation and budget together.
Wrong owner/node, incomplete/future/stale observation, changed instance/volume set
or a conflicting existing binding refuses. Bound after ambiguous/cancelled recovery
may learn identity but never resets authority/uncertainty, changes node phase,
settles billing or advertises Ready. Existing BillingUnknown detail is preserved,
and Settled budget cannot be overwritten. Empty lists/ACK/errors are not absence.

Tests are actual SQLite claim/cancel/reservation visibility races and restart,
drop/unwind, immutable fingerprint/result, corruption/missing accounting,
late-bill/unknown-cap refusal, existing deletion/down gates and move-only compile
checks with synthetic trusted data. No provider action or fake transport proof.
Remaining gates include real trusted operation/result/cost adapters, exact driver
spec dispatch, independent deadline/volume/remote watcher, secure admission,
resource/billing reconciliation and explicit effect integration. The accepted
budget ledger and this claim seam are NOT live-paid qualification or permission.

### Shared budget-owned dispatch validation

The only additional budget visibility is crate-private
`budget::validate_create_dispatch(conn, reservation, account, now)`. It requires
an already-held transaction and reuses budget's existing private `costs`, `rows`,
`clock`, `period_valid` and `policy_valid`; none of those became public. The
private `aggregate_available` is extracted from existing reservation admission:
new reserve seeds one proposed amount/count, while dispatch seeds none and counts
its already-persisted reservation exactly once. The existing reserve API,
settlement behavior, serialized types and schema are unchanged; budget regression
tests are unchanged. Operation no longer copies cost math or aggregate policy.

Original cost reconstruction uses **reserved_at**, not claim time. Current quote
freshness/account/period/caps/other liabilities are checked separately in the
same transaction before marking this operation's own Uncertain state. Positive
remaining serving time is mandatory and the full original cleanup grace stays
protected. New operation tests exercise two-operation concurrent claims, concurrent
new reservation versus dispatch, late other bills/unknown liabilities, and a
claim with exactly one second of serving time left and untouched cleanup grace.

## Explicit sealed development executor (synthetic only)

`operation::development` adds a deliberately bounded fake effect path, not a
Provider adapter or daemon route. No callback, trait object, executable/argv,
network endpoint, credential or arbitrary provider is accepted. Construction is
explicit `SyntheticPlan::new(namespace, target, tier)` and
`DevelopmentExecutor::new_synthetic(plan)`; provider kind is fixed SyntheticV1.
Labels are immutable/private and bounded to64 bytes. Canonical SHA256 input has a
fixed domain and version plus length-prefixed fixed kind/namespace/target/tier
and every intent identity field (owner/run/request/nodeUID/operationID). The
launch fingerprint is the hash output slot, not recursively part of its input.
There is no serde/Debug/map-based canonicalization or target mutation API.

```text
plan.fingerprint(&CreateIntent) -> Result<[u8;32]>
executor.prepare(&Store, &CreateIntent, now) -> Result<CreateOperation>
executor.claim(&Store, operation_id, now) -> Result<CreateClaim>
executor.execute(&mut self, &Store, CreateClaim, now)
    -> Result<DevelopmentOutcome::{Bound(Box<metadata>), Ambiguous, NotInvoked}>
executor.invocations() / resources()        # synthetic diagnostics only
executor.advance(now)                     # explicit virtual clock only
```

Each open Store has a private Arc allocation token; each executor has a separate
private Arc token. Only executor claim issuance can bind that actual executor
token during the SAME existing durable claim transition. There is no later
attachment/rebind API. Execute consumes the claim on every outcome, compares
both allocations with Arc::ptr_eq, recomputes the pinned canonical fingerprint,
and checks tier. Equal owner/IDs/plans, another connection/path, reopened DB,
replacement executor and ordinary unbound `Store::claim_create` are not substitutes.
Prepared metadata may be claimed once through a new correct executor after reopen;
already consumed history never yields new authority.

Execution re-reads live operation/immutable dispatch identity, current node/desire/
deleting, original anchored deadlines, shared clock and current budget under
owner-checked IMMEDIATE. Budget-owned consumed validation permits ONLY this exact
Uncertain hold, fully charged once; all other uncertain/unknown/expired liabilities
and late caps still block. It does not temporarily restore Reserved or copy cost
math. A private exact-consumed exception cannot cover two duplicate uncertain
payloads. Optional `invocation_started_at` defaults toNone for old JSON rows;
metadata without allocation authority does not become executable. Inconsistent or
already-started invocation markers refuse.

The invocation marker and clocks commit BEFORE the sealed backend increments or
creates any fake resource. No transaction spans an effect. Cancel before that
transaction wins means zero invocations; cancel after marker commit may race one
invocation, even in the commit-to-backend gap, and cleanup remains owed. It is
not retroactive cancellation. A bound late response may update cleanup metadata
without resurrecting cancellation or clearing ambiguity. Fault/lost-response/
result-write failure retains full budget liability and any in-memory fake resource;
no outcome returns the claim or offers retry. Test-only private scripted faults
and bounded synchronization hooks are cfg(test), not public executable callbacks.

Fake service eligibility uses injected time strictly before the original serving
deadline; explicit virtual advancement removes fake resources at the original
cleanup deadline. This is SYNTHETIC state/deadline enforcement, not an autonomous
wall-clock/remote watchdog, process preemption, provider exactly-once, owned absence,
refund or Ready. Dropping Store/executor/claim is not proof of absence; recovery
cleanup intent remains durable. No schema table/dependency/provider/main/reconcile/
agent/CLI/transport wiring was added. Real child/provider execution requires a
separate design, continuing supervisor clock, independent watcher and all existing
Admin/OS/admission/billing gates. The sealed backend cannot spend money.

## Opt-in real LOCAL development executor (not cloud or daemon wiring)

The `local-development` Cargo feature is **off by default**. On Unix it exposes
`operation::local_development::{LocalDevelopmentPlan, LocalDevelopmentExecutor}`.
The only backend is the actual timed `m8s-provider-local::Local`, constructed
inside this wrapper. There is no arbitrary Provider, closure, executable runner
trait, URL, environment-secret or credential input. `DevelopmentCommand` is
trusted nonsecret local configuration: the fixed env-clear/null-stdio/new-group
contract and its bounded raw Unix path/argv digest are pinned, not executable
file contents or a sandbox. Use only a trusted harmless developer executable
that does not daemonize/escape its group; exclusive retained-child reaping and
trusted path ancestors are required. No model/network workload is authorized.

`LocalDevelopmentPlan::new(command, journal, namespace, target, tier)` owns the
actual command and immutable bounded labels, requiring a bounded absolute,
lexically canonical raw Unix journal path (no dot/parent/alias spelling). Local
validates its real private descriptor/journal/lock policy when opened; the
wrapper does not repair/replace that anchor. A versioned domain-separated,
length-prefixed SHA256 binds fixed LocalDevelopmentV1, command digest, raw journal
path, namespace/target/tier and owner/run/request/nodeUID/operationID. Namespace
is a pinned label, not transport identity; actual Local journal identity is also
bound through that exact path and private allocation. Fingerprint is the output
slot, never recursively self-hashed. Different kinds/targets/commands refuse.

`LocalDevelopmentExecutor::open(plan, owner)` opens no child. `prepare/claim`
reuse the existing Store transaction and attach the actual issuing executor Arc
and exact Store per-open Arc only during issuance. Ordinary/unbound, SyntheticV1,
other-executor, second-connection and reopened-Store claims refuse; recovery
metadata never reconstructs authority. Existing plain Store API is unchanged.
The only synthetic-module change delegates its invocation transaction to a
private shared helper, preserving its explicit-clock and rollback-test behavior.

`execute(&mut self, &Store, CreateClaim)` consumes the claim on **every** outcome.
It rechecks exact intent/dispatch, live node/tier/deletion/desired, no earlier
invocation/result/ambiguity, current budget/clock/period/caps/other liabilities,
and the original serving/cleanup bounds. The existing budget-owned validator
allows only this exact own Uncertain hold, still charged once, never a Reserved
reset. It samples real UTC Unix seconds **after** acquiring BEGIN IMMEDIATE,
commits the invocation marker before submission, and releases SQL before any
Local journal/spawn/channel wait. Absolute original Unix-second deadlines are
checked into SystemTime, then LocalDeadlines samples actual high-resolution UTC
and monotonic time; no new runtime is added and expired bounds refuse. The
executor makes at most one timed Local submission over its entire lifetime.

A successful exact Local return binds owner/run/UID/instance plus explicit
`NoneAllocated` volumes because this local adapter creates no volumes. This is
LOCAL cleanup metadata, not provider/MP-001/bootstrap/admission, Ready, serving
model, canary, cloud absence, billing or settlement evidence. Cancellation and
ambiguity remain sticky; stopped/reaped cannot refund or delete Store state.
Launch/result errors, lost result, journal or SQL write failure keep full budget
liability and possible-effect intent; no create retry or capability is returned.
The executor retains LocalLaunch before writing results; failed Local launch
drops the coordinator to request stop while its timed worker retains any owned
child and journal lock until direct reap. Drop requests stop, never claims absence.

**Cancellation is paired, not automatically observed:** take the cloneable
`executor.stop_handle()` before calling execute. Pair durable
`Store::cancel_create` with `handle.request_stop()`. This stop-only atomic/OnceLock
latch uses one AtomicU8 Requested|Published state plus OnceLock, and no SQL, I/O,
mutex or wait; no API resets it or renews a deadline.
Request before handle publication is retained and applied when publication
occurs. Both request/publication use AcqRel fetch_or on the SAME atomic: the
second RMW necessarily sees the other's bit. OnceLock initialization precedes
the Published release, so an acquiring requester also sees the real handle;
when request wins, publication itself signals. This is a modification-order
argument, not a two-variable visibility assumption or stress-test proof.
A duplicate private attachment refuses and stops both handles while retaining
new launch ownership; it cannot silently replace the original handle.
**Local publishes the actual stop handle only after startup and active
journal persistence return: an early wrapper request is ONLY latched during
that gap, not a prompt signal.** The independent timed worker still progresses
after spawn without Store/journal locks; a kernel-stalled spawn is excluded.
DB-only down/cancel records cleanup intent and does NOT automatically wake the
handle. Cancellation before invocation commit wins with zero submission; after
commit there may be one child, including the commit/submission gap. Once the
actual handle is published, stop progresses independently of result Store writes.

Parent-alive timely OS scheduling/nonblocking signal/wait assumptions apply:
anchored deadline means a stop request plus polling/scheduling latency, not
proof every descendant exited by that instant. Parent SIGKILL/abort/host loss,
process-wide suspension, scheduler starvation, escaping executables and stalled
kernel primitives are not contained. No paid/cloud capability, autonomous
watcher, cancellation observer, restart PID adoption, absence/refund automation,
main/reconcile integration or production security qualification is introduced.
Tests use private synthetic accounting/paths and harmless `/bin/sleep` children,
not real provider billing or remote admission.

### Explicit retained-owned local journal cleanup (diagnostics only)

`request_local_cleanup(&mut self)` first requests the existing race-correct stop
latch, then records irreversible local cleanup intent; no Store, SQL, journal
I/O, wait or caller IDs. The early-handle-publication gap above is unchanged.
`poll_local_cleanup(&mut self) -> LocalCleanupStatus` operates ONLY on the actual
retained Local coordinator and LocalLaunch ID. Before submission the executor
retains its exact consumed CreateIntent; after success it pins the returned
immutable instance snapshot. Owner, UID, tier, canonical plan fingerprint and
that actual snapshot must match before any mutation. No caller binding, latest
Store row, list result, PID scan, restart adoption or decoy target is accepted.
The launch is privately Arc-retained; public `launch()` still returns only a
borrow. Test-only observers can watch worker progress without sharing a mutable
coordinator or cloning any create authority.

A poll performs at most ONE concrete Local destroy and ONE fresh describe, with
no internal retry loop/sleep. It may block on filesystem/journal work: **number
of calls is bounded, I/O latency is not**. Stop is requested before journal work;
the independent timed worker does not wait for this coordinator or SQLite.
Failures preserve ownership/liability and return Uncertain; there is no repair,
anchor reset, adoption or automatic Store/accounting change.

Diagnostic distinctions:
- `NoLocalSubmission`: THIS executor has never called the Local launch API. It
  says nothing about old/unrelated journal resources or Store effects. A newly
  opened executor does not adopt unresolved old records; such records still
  block new UIDs in the concrete journal. A failed submitted attempt with no
  retained launch/coordinator returns Uncertain, never NoLocalSubmission.
- `NotRequested`: no local journal finalization requested, even if an independent
  deadline, natural exit, stop-only handle or dropped launch stopped/reaped the
  child. Direct reap alone is not journal absence.
- `Pending { direct_child }`: successful destroy acknowledgment followed by a
  fresh exact Present observation; typed direct-child state is separate evidence,
  not a durable absence claim. Process uncertainty/errors remain Uncertain.
- `LocalOnlyAbsent`: successful exact retained destroy AND fresh describe Absent
  under Local's existing validated private journal rules, with retained direct
  child reaped. Repeated polls revalidate destroy/describe/anchor every time;
  terminal success is never cached across anchor replacement or storage errors.
- `Uncertain`: missing retained ownership, identity conflict, failed submission,
  process/journal error or invalid observation. It is not permission to retry a
  create, infer zero cost, reset the namespace or guess absence.

Valid local finalization leaves a permanent old-UID journal tombstone. Dropping
and reopening the same valid journal may allow a NEW UID at the journal layer,
but the executor remains one-shot and Store uncertainty/history/caps are NEVER
reset. NoLocalSubmission/LocalOnlyAbsent are ordinary diagnostic enum variants,
not verified provider absence or SettlementEvidence; there is no conversion,
automatic cleanup of Store rows, billing settlement or refund. Local-only
no-volume metadata is not a cloud volume-absence assertion. Corrupt/missing Store
accounting cannot block retained stop/cleanup because cleanup takes no Store and
performs no SQL. Drop alone requests stop, never durable journal finalization;
restart without retained ownership remains deliberately unrecoverable here.
All default-OFF, parent-alive, publication-gap, DB-cancel-intent-only, no main or
cloud/spend/admission/Ready guarantees remain unchanged. Tests use harmless
sleep children; the natural-exit case uses the already-reviewed controlled
sleep-group fixture because this sandbox may deny signals to an already-empty
group. No process/security primitive is weakened to obtain a green test.

## Runnable harmless LOCAL lifecycle demo

The explicit example composes synthetic accounting, durable one-use intent, one
fixed `/bin/sleep 15` child, timed stop/paired cancellation and retained journal
cleanup. It is NOT the live product/mesh/model demo, Admin approval, provider
balance, real pricing, credential workflow or paid integration. Feature defaults
remain OFF; no daemon/main/config route changed.

From the integrated checkout (with the actual Local crate), choose a **NEW,
nonexistent** path under an already-existing canonical trusted parent:

```sh
cargo run --locked -p m8sd --features local-development \
  --example local-lifecycle-demo -- \
  --synthetic-accounting --state-dir /private/tmp/m8s-local-demo-cancel-NEW \
  --synthetic-cap-micro-usd 10000 --scenario cancel
```

Use a DIFFERENT new path and `--scenario deadline` for the original three-second
serving deadline plus two-second cleanup grace. No runtime renewal is offered.
The cap must be explicitly supplied, positive, integer and <=1,000,000 simulated
micro-USD; there is no default approval. Synthetic nonzero compute/storage,
transfer and contingency require a full held reservation before any child. A
too-small cap refuses before Local submission, preserving the fixture state.
The 28-day interval, baseline, NoneConfirmed external-liability field, rates and
all owner/approval/provenance labels are fictional SYNTHETIC-NOT-ADMIN-APPROVAL
inputs, NOT claims of real provider reconciliation or a real calendar period.

The example now uses pure planned_intent metadata, explicit synthetic budget
reservation, then Store-owned prepare_managed to atomically create the derived
LOCAL UID, Launching node, attempt and Prepared operation in the SAME database.
There is NO example node/attempt INSERT/setup SQL or caller-supplied UID. The
constructor may open an EMPTY Local journal before reservation so it can preview
its pinned identity; a too-small cap creates ZERO manager nodes/attempts/operations
or children, with no submission. Empty journal creation is neither absence nor
create authority. Only accounting inputs remain synthetic-not-approval; no
arbitrary command/argv/URL/cloud/config/secret input is supported.

State directory0700 and exclusive no-follow0600 demo lock are held throughout
Store/effects/cleanup. Store/Local retain their reviewed private UID/ACL/journal
policies. Trusted ancestors are required; symlinked/noncanonical parents, existing
paths/final symlinks, duplicate/unknown flags and missing explicit accounting
refuse. No --force/reset/recovery/rerun/settle/zero-bill or directory erasure exists.
Keep the resulting DB/journal for inspection; do not delete ledger rows to rerun.
A new path is a NEW SYNTHETIC fixture, never a live-account budget reset.

Output is bounded NDJSON with fixed phase names, no raw argv/env/journals. Exit0
means LOCAL journal cleanup observed LocalOnlyAbsent and read-only checks retained
unknown billing, the full Uncertain hold, consumed dispatch/invocation history and
cleanup-required intent. The managed LOCAL node remains Launching, NOT reconciled
fleet/Ready/model/admission status. No spent=0, refund or absence-proof claim is
emitted. Errors exit nonzero, preserve state, request stop and attempt exact-owned
cleanup without repeating create. A failed/incomplete cleanup is uncertainty,
not safe deletion or permission to retry.

Caller polling has bounded attempts/elapsed checks, NOT a hard latency bound
around filesystem/kernel calls or stdout. The independent worker progresses
only under its existing parent-alive/OS scheduling assumptions. Early stop is
latched until actual Local handle publication; parent exit/SIGKILL/abort/host
loss, stalled spawn/signal/wait, suspended scheduling and escaping executables
remain uncontained. On uncertain error/exit, the fixed harmless sleep may remain;
no child-survival/complete-teardown guarantee is made. Drop is stop-only. No
signals/watchdog architecture, automatic Store cleanup/billing settlement,
provider/TLS/MP-001/Keychain/model/network workload or spending is added.

## Store-owned managed LOCAL preparation (explicit existing budget only)

`LocalLaunchRequest { run_id, request_id, operation_id }` contains bounded metadata,
not owner/UID/tier overrides or a permit. `executor.planned_intent(&store, &request)`
is pure: no DB/journal I/O, effects, approval or claim. Its domain/version/length-
prefixed SHA256 derives local-dev- UID from exact owner/run/request only. Rotating
operationID, target, command or tier cannot manufacture a different UID under an
old request; the full canonical plan fingerprint separately binds all intent and
plan fields. Owner comes from exact Store+executor and tier from constructor.

Caller explicitly scales persisted desired and reserves an exact matching budget
using preview UID BEFORE `prepare_managed`. Managed preparation starts owner-
checked IMMEDIATE, samples real UTC AFTER writer acquisition, recomputes identity
inside the transaction, and requires the existing Reserved owner/run/request/UID
budget through the existing current-policy/caps/liability/original-deadline
validator. It never reserves/rebinds budget or treats config/metadata as approval.

For new work, ALL retained tier nodes (including deleting, orphan and unknown
unconfirmed rows) count toward persisted desired. The exact shared mock launch
rate gate refuses future timestamps and requires fewer than three attempts at
at>now-3600; equality at the cutoff is unchanged. One atomic commit inserts NEW
node+attempt+Prepared operation and shared clock, or rolls everything back. There
is no upsert, adoption, repair, random UID, nested public transaction, claim,
Local filesystem/spawn or other effect inside that SQL transaction.

Exact identity retries return retained metadata only, even cancelled/dispatched/
stopped or reopened: no node/attempt recreation, clock/deadline renewal, budget
debit or recovered claim. Changed identity/request target refuses. Down/up cannot
revive a deleting old node; claim/execute still revalidate current state/budget
and retain single-use uncertainty. These methods do not wire the default daemon,
generic/cloud operation manager, paid provider or approval route. Public lower-
level prepare and mock reserve retain their previous behavior via private shared
transaction/rate helpers. Local cleanup still cannot erase Store history, settle
unknown bills, refund or authorize another create.

## Opt-in configuration-backed MOCK control

```sh
m8sd /private/tmp/my-private-mock-state synthetic-mock-owner --config /absolute/workspace/m8s/crates/m8s-config/examples/m8s.toml --catalog /absolute/workspace/m8s/crates/m8s-models/models/examples.toml
# In another terminal, using only the local mock control socket:
m8s --socket /private/tmp/my-private-mock-state/control.sock up small
m8s --socket /private/tmp/my-private-mock-state/control.sock scale small 2
m8s --socket /private/tmp/my-private-mock-state/control.sock status --json
m8s --socket /private/tmp/my-private-mock-state/control.sock down
```

Choose your own private state path and replace the absolute checkout paths.
`m8sd <private-state-dir> [nonsecret-owner-label]` without flags retains legacy
mock behavior/default owner. To opt in, supply `--config ABS --catalog ABS` once
**together**, after the state directory (owner/flags may otherwise be reordered).
No defaults, file/env discovery, provider selection or automatic reload. Unknown,
duplicate, partial, relative-input-path and non-UTF8 arguments fail opaquely.
The whole config and catalog use existing bounded regular-file, final-component
no-follow/nonblocking 65536-byte loaders and actual default ValidationLimits.
Whole-input validation and configured-ID/bound representability checks finish
before any state directory, lock, socket, Store or Mock-provider effects.

Configured tier membership and replica limits are immutable until explicit validated
`reload-config` (file edits alone do not change the active snapshot):
- `up TIER` selects `max(1, configured_min)` and refuses if above configured max.
- Positive `scale TIER N` requires a known tier and `min <= N <= max`, within
  Store's existing cap16. Unknown tiers cannot create durable desired rows.
- Explicit `scale TIER 0` is always allowed for a **known** tier, even with a
  positive minimum. `down` always remains the emergency stop for every tier.
- A max0/min0 tier is disabled: up/positive scale refuse, zero/down are allowed.
  Empty configurations are invalid. Config IDs containing dots (valid in the
  general config schema but unsupported by Store), or limits beyond Store's cap,
  refuse the whole startup before filesystem effects; nothing is rewritten.
- No configured tiers are automatically inserted or scaled at startup. Existing
  desired values persist and still drive the ordinary hardwired Mock reconcile.

Restart with the same state/owner and compatible configuration to retain desired
values. Before opening Mock, reconciling or replacing a stale control socket,
configured startup reads persisted `Store::tiers`: any unknown retained tier
(including desired0), or a positive desired count outside current limits, refuses
startup and retains state. No clamp, down, reset, repair, tier/node/history rewrite
or config-driven migration is performed. Supply a compatible configuration; do
not erase state to evade a refusal. Normal Store initialization/existing migrations
still apply: this is not immutable database inspection or a migration bypass.
The configuration handling itself only reads startup desired state; node/attempt/
history transitions remain the existing Store/reconcile behavior.

This is **MOCK-ONLY control intent**, not a model deployment or provider policy/
budget approval mechanism. Response.mode stays `mock-only`, secure bootstrap
unavailable, and nodes cannot become Ready. Even configured Vast preferences,
models, schedules, DMS and budgets do not select a provider, execute a model,
start scheduling or establish live all-in accounting/security qualification.
Mock stays hardwired; no Local/paid provider, iroh listener, credentials, download,
new socket transport or automatic startup scaling/reload is introduced. Existing
filesystem/lock/trusted-ancestor assumptions are unchanged, not strengthened into
protection against uncooperative same-UID processes by this feature.

`m8sd --help` prints a fixed concise guide to legacy/configured Mock mode,
replica/stop/restart semantics and non-authorization boundaries. Only standalone
`--help` bypasses parsing and all filesystem/config/state effects; combinations
with state/config arguments refuse. Broken help output returns a static error
and nonzero exit. Existing Store initialization/migration behavior is unchanged.

### Explicit Mock shutdown

`m8s --socket /absolute/private/state/control.sock shutdown --json` first commits
`down` (all desired replicas zero), then uses at most two ordinary reconcile
passes. Success requires fresh exact-owner Mock inventory empty, no retained node
rows and all desired counts zero; destroy acknowledgement alone is insufficient.
On refusal the daemon stays running; any committed stop intent remains durable
for ordinary reconciliation. A failed database write cannot establish that intent. `down` alone does **not** exit the daemon.

After success the response is best-effort and the daemon exits normally; a lost
response cannot undo committed stop intent. Restart with compatible configuration
retains zero, with no automatic scaling. The stale socket path may remain; normal
startup handles it. No state/history reset or filesystem cleanup is performed.
This confirms only hardwired Mock absence, not real-provider absence, Ready,
security qualification, budget settlement or deployment authorization.

### One-shot durable history pages

```sh
m8s --socket /absolute/private/state/control.sock logs --after 0 --limit 50 --json
```

Both flags are required once: cursor is ASCII decimal 0..i64::MAX; limit is
1..200. The separate `logs_page` response has `records` (`seq`, injected `at`,
`message`), `next_cursor` (last returned sequence, or original cursor if empty),
and `has_more` from one ascending SQLite SELECT with limit+1. Request the next
page explicitly; this is not follow/poll. Legacy `logs [--json]` tail and its
Response shape remain unchanged. Paging issues only SELECT, never mutating event
or control rows. This is not immutable filesystem inspection: ordinary WAL
reader bookkeeping may update shared memory, and the daemon reconciliation timer
continues independently.

Only this state's retained history scopes a cursor: no cross-database identity,
global continuity, current-clock, real-time or no-event-loss guarantee. Successful
empty pages mean no later retained events at that query snapshot, not no history;
`ok=false` empty records establish nothing about history. Later changes are not
included in that snapshot. There is no inference/secret transport or readiness/
authorization claim: mode is `mock-only`, inference `not-carried`.

Messages over 4096 UTF-8 bytes or malformed persisted values refuse the page;
nothing is silently truncated/omitted while advancing the cursor. The entire
newline response must fit 65536 bytes before any success bytes are written;
otherwise a fixed failed-page response is sent. Client text uses an escaped JSON
projection, not raw event/control text. Broken output may still partially write
and returns nonzero; no delivery/exactly-once claim is made.

### Explicit configured Mock reload

```sh
m8s --socket /absolute/private/state/control.sock reload-config --json
```

Only a daemon started with paired `--config ABS --catalog ABS` supports reload.
The request carries **no paths or overrides**: it rereads exactly those selected
startup paths using the same bounded loaders, whole-config/catalog validation,
default ValidationLimits and Store-compatible tier IDs/bounds. Legacy no-config
mode refuses without discovering files. No watcher or automatic reload exists.

Before publication, every retained tier (even desired0) must still be configured;
positive desired must fit new min/max. Removing a retained tier, raising min above
positive desired, or lowering max below it refuses. Old policy stays active and
no desired/node/history is reset, clamped or stopped. Use explicit `down` or known
tier `scale TIER 0` before retrying changed bounds; zero does not permit removal
of a retained tier. A failed reload never publishes a partially validated policy.

Successful publication is one snapshot assignment in the serial control loop;
subsequent up/scale use new bounds, including disabled/minimum rules. Reload itself
only reads files/current desired state: no Store writes, reconcile, provider call,
model activation or startup scaling. The ordinary reconciliation timer continues
independently, so this is not a global filesystem/state freeze. Compatible restart
uses files normally. Mock stays hardwired, bootstrap unavailable/no Ready; reload
is not provider choice, scheduling/DMS, budget/security or execution approval.

### Inspect the actual active admission snapshot

```sh
m8s --socket /absolute/private/state/control.sock config-status --json
```

This separate response reports only the active **in-memory** Mock tier admission
snapshot: `configured`, sorted tier `id`/`min`/`max`, fixed `mode: mock-only` and
`scope: mock-control-admission-only`, plus `ok`/`error`. It rereads neither files
nor Store: deleting/editing files does not alter this view; successful explicit
reload changes it, refused reload leaves it unchanged. Unlike `config validate`,
this shows active bounds, not current file contents. No paths, models, owner,
catalog text, credentials, fingerprint or generation/version identifier appear.

Legacy no-config mode explicitly returns `configured=false` and empty tiers:
this does **not** mean desired state is empty or verify a default cap. An
`ok=false` empty response is not evidence of no policy. One serial-loop snapshot
can change after query/restart; no authentication/provenance, globally unique
version, live policy or runtime/Ready/approval guarantee is implied. The daemon
timer continues independently. Frames fully serialize within 65536 bytes before
write; fixed failure response on error, no partial successful frame rendering.

### Reload preflight

`m8s --socket /absolute/state/control.sock reload-config --check [--json]`
validates the exact startup-selected config/catalog and current retained tiers
using the same candidate checks as explicit reload, but does **not** replace the
active policy. Legacy unconfigured daemons refuse. Success reports actual active
and prospective candidate admission bounds; `bounds_changed` compares ONLY tier
membership/min/max, not whole configuration equality. `check_only=true`,
`applied=false`, `later_reload_guaranteed=false`: files and desired may change
before a later reload. No paths/overrides/force are accepted or returned.
The handler performs no Store writes, reconciliation or provider calls; ordinary
SQLite reads may perform bookkeeping and independent timer reconciliation remains
independent. This is not immutable filesystem inspection, authorization or Ready.

### Manual atomic batch desired intent

`scale-batch TIER=N ...` applies1..32 unique Store-compatible targets atomically:
all admission checks first, then ONE IMMEDIATE transaction for selected desired
rows and existing nonce cancellation on each explicit zero. Unmentioned tiers
stay unchanged. Configured membership/min/max applies to every target; unknown
zero tiers refuse too. Any SQL/transaction failure rolls back that transaction,
not earlier/later independent commands. Provider effects/reconciliation are NOT
atomic with desired changes; no convergence/Ready claim. A failed response can
follow commit, so clients must not infer no effects or automatically retry.

`explain-tier TIER` exposes one read-only DEFERRED stored snapshot using internal
now() and the UNCHANGED existing launch_rate_available helper. Only tier/attempt/
node metadata is read, never provider/nonce/budget/keys. All stored rows/phases
count, not confirmed runtime inventory. The3/hour strict cutoff includes future
attempts and any future attempt blocks this rate gate. Gate availability is NOT
complete eligibility, permission or will-launch prediction. Null desired is not
zero; active configured context is informational only. A failed explanation has
no data/context, never an absence claim. SQLite bookkeeping and independent
reconcile timing remain; no mutation/provider/reconcile call inside this handler.

### Store source components

Private `manual` holds the existing atomic batch method/tests; private
`diagnostics` holds the existing stored launch-gate snapshot method/tests. Both
remain associated methods on `m8sd::Store`. Root Store initialization, legacy
scale/down, reconcile and shared launch-rate gate remain unchanged. This split
adds no public modules, field visibility, constructors or execution authority.

Each accepted control connection has one absolute two-second I/O budget shared
across request reads and response writes. Private nonblocking/poll transport does
not renew it per byte or use socket read/write timeout options. Buffered complete
first frames still work after peer close; partial EOF/oversized frames refuse.
This limits transport waiting, not SQL, reconciliation, serialization or OS
scheduling. A failed/timed-out reply can follow committed intent: no rollback or
safe retry is implied. Confirmed shutdown still exits even if its reply fails.
No peer authentication or filesystem trust policy is added by this deadline.

Legacy responses are fully encoded into a bounded buffer before the socket write:
MAX_FRAME (64 KiB) includes the final newline. Fitting responses keep their exact
JSON field order/bytes; encoding/cap failures instead send a fixed refusal whose
empty fields are explicitly not absence and whose outcome is uncertain. This
encoder alone bounds encoding/output, not snapshot collection (separately bounded
for legacy control below).
It is not atomic socket delivery; failed/partial output may follow committed
intent. No retry/rollback is performed, and confirmed shutdown still exits after
the write attempt, including when only a fallback refusal can be encoded.

Legacy control now collects `Store::control_snapshot()` in one DEFERRED read
transaction: at most 256 nodes and 256 tiers (overflow refuses, never truncates),
plus the existing latest-200 event window in ascending sequence order. Borrowed
SQLite values are type/UTF-8/byte-bound checked before cloning: tier IDs <=64
Store-compatible ASCII, desired <=16, node UID 1..256, owner/optional instance ID
<=256, encoded phase <=32 with a known stored enum, event messages <=4096.
All known phases remain stored metadata, not Ready/runtime evidence. Existing
`nodes`/`tiers`/`logs` and reconciliation semantics remain unchanged/unbounded.
This bounds Rust control collections and encoding, not SQLite engine work/memory,
SQL scheduling or global currentness. Any snapshot failure returns a fixed
uncertainty refusal and keeps the daemon running; empty fields are not absence.
A request may already have committed before observation fails. Shutdown snapshot
failure still refuses exit; no automatic reset, repair, retry or provider action.

Managed Store connections share inode-scoped validation-anchor leases within the
process. Extra database/WAL/SHM validation descriptors remain owned until the last
associated SQLite connection closes; last-anchor close and the next acquire use
the same registry mutex. SQLite initialization/busy waits never hold that mutex.
Rechecks borrow retained anchors; canonical parent aliases share the same lease.
Hardlinks, live replacement and unsafe metadata refuse rather than closing and
reopening SQLite inodes. Failed opens and
unwinds retain the same lifetime ordering; an unidentified descriptor or poisoned
registry fails closed. No ACL/mode/nofollow/identity checks are relaxed or repaired.
This coordinates **managed Store** connections only, not arbitrary raw SQLite
connections in the same process or hostile same-UID replacement. It adds no
cross-process authentication or new execution/permission authority.

### Owner bootstrap/control session composition

With `secure-bootstrap`, `Store::with_bootstrapped_node` accepts the expected
TLS-authenticated node, performs the existing original-stream Store burn and
signed delivery, then establishes pinned control and awaits the first heartbeat
acknowledgement before invoking the caller's work. At most eight unrelated peers
are considered within a maximum 60-second bootstrap window. Wrong peers cannot
consume the nonce through this entrypoint.

Heartbeats run concurrently with the caller's work and stop on cancellation,
control failure, completion or certificate expiry. A monotonic deadline prevents
wall-clock rollback from extending the session; current wall time is also checked
before each heartbeat and before returning work output. A completed bootstrap or
heartbeat does not prove model readiness. The caller must verify the served model
and owner-routed canary, and retain output until exact owned cleanup is confirmed.

This library composition creates no endpoint, reserves no budget and performs no
provider mutation. The owner-side CLI LAUNCH command and independently surviving
resource watchdog remain required; the caller retains cleanup custody on every
return and cancellation. Existing completed E qualification is unchanged and is
not rerun by the standard checks.

## Shared live launch preparation and dispatch

Under `prototype-vast`, `operation::vast::VastExecutor::prepare_launch` now
composes the live CLI's accounting and launch preparation: validate executor
identity, reserve budget, persist the managed launch intent, and read the
original serving deadline from the same Store. It returns metadata rather than
an effect permit. Failure after reservation keeps the accounting debt. Reopening
or repeating preparation does not renew the deadline or remint consumed claims.

`dispatch_prepared` checks the exact independently armed cleanup worker before
claim consumption, then uses the existing one-shot executor. The executor still
rechecks cleanup after claim and after durable invocation. The live CLI calls
these shared methods; a future daemon coordinator can reuse the same ordering.
No provider call occurs during preparation, and neither method grants Ready.
Desired-state admission, fresh evidence, signer custody, bootstrap/inference,
recovery and cleanup settlement still require their explicit coordinators. The
normal daemon binary remains mock-only.

### Exact resource removal and retained billing

`VastExecutor::finalize_removal` consumes a fresh, opaque creator-account absence
receipt from the armed cleanup handle. It matches the exact immutable launch
intent and persisted instance binding. One SQLite transaction records the
`vast_removal` receipt, cancels the node's grants, removes its exact `gpu_node`
row through the shared finalizer, and retains `BillingUnknown` accounting.
Failure rolls all of these writes back together. The consumed create claim,
nonce tombstones, revocation metadata and held cost survive. Repeating
finalization cannot remint a claim or downgrade a settled final bill.

`Store::vast_removals` and `m8s prototype audit` expose the retained observation,
not a new provider read or settled-invoice claim. Older successful demo journals
may have settlement evidence without a `vast_removal` record; they are not
silently backfilled or reset. The historical live receipt remains authoritative
for that historical run.

The legacy `Store::reconcile` path and default mock daemon refuse a Store with
managed budget/create history before touching a provider. A mock observation
must never delete real managed node rows or launch replacement mocks into a
managed account. Full live daemon recovery requires the original coordinator
and authenticated provider evidence; it remains an explicit integration task.

### Immutable restart metadata

New Vast preparations atomically persist the exact canonical public launch
specification alongside the managed intent. `Store::recover_vast` validates the
original fingerprint, owner, node, tier and budget binding in one read snapshot.
It returns original cleanup times anchored to reservation, even if overdue.
It returns no create request or claim, cannot renew a deadline, and refuses
missing legacy metadata or tampering without resetting or backfilling history.
The stored SSH key is public; no credential, bootstrap nonce or owner key is saved.

This is a recovery input boundary, not a running recovery worker. An authenticated
creator-account transport and independently surviving cleanup still need to be
composed for process restart. The capsule alone cannot destroy a resource or
prove absence; removal still requires the opaque fresh observation receipt.

### Bounded managed journal snapshot

`Store::vast_audit` projects operations, budgets, atomic removal receipts and
recovery-metadata availability in one DEFERRED transaction. Its per-attempt stage
separates reservation, preparation, consumed claim, unresolved invocation,
recorded binding and removal. Billing and cancellation are independent fields.
A stored binding is not current resource presence; a missing binding is not
absence. Missing older recovery metadata is reported explicitly; malformed
present metadata refuses the whole snapshot.

Each journal is capped at 128 owner rows and each JSON record at 64 KiB before
Rust decoding; capsules remain capped at 4 KiB. Orphans, cross-linked budget
records and a removal receipt with a retained node row refuse. Limits bound
application collection size, not SQLite engine work. The query performs no
mutation or provider calls and never consumes a claim or releases a hold.
