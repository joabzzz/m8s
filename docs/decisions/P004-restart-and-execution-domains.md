# P004: Restart metadata and execution domains

Status: source integration with deterministic qualification; no new live run.
Implements part of original ADR-0008 and ADR-0015. Does not complete P1.

## Context

The successful prototype kept its coordinator in one process. A restart must
retain the original identity, consumed claim, cleanup deadline and cost hold.
The development daemon uses mock observations; it must never interpret managed
resource records as mock state, including when another process opens the same
database during startup.

## Decision

Persist the canonical public launch specification atomically with the launch
intent. Reconstruct only immutable recovery inputs after validating its original
fingerprint, original creator account ID and budget binding. A reconstructed object contains no create
request, credential or signing key. Its original deadline is not rebased.
Missing legacy metadata refuses recovery; an audit reports that absence rather
than fabricating a replacement intent.

Provider-confirmed exact absence, node-row deletion, grant cancellation, a
retained removal receipt and unsettled billing commit together. Failed writes
roll back together. Consumed claims and nonce/revocation tombstones survive.
Only an opaque fresh observation, after matching the original creator account,
can enter this finalizer.
Stored JSON, destroy acknowledgement and stopped/exited status cannot do so.

Operator audit reads one bounded SQLite snapshot. Resource stage, cancellation,
billing state and recovery-input availability are separate fields. An empty or
failed query never means provider absence. An old binding is historical metadata,
not current provider presence or model readiness.

Mock/unmanaged reconciliation and managed operation history require mutually
exclusive database execution domains. The domain must be established durably
before provider effects or budget initialization, so a check-then-act race cannot
mix them. No reset or implicit conversion between domains is permitted. This
does not replace the existing single-reconciler process lock or authorize
concurrent provider effects within a domain.

## Restart reconciler (issue #12, slice 1)

`Store::reconcile_vast` walks persisted managed operations through an injected
`RestartPort` (lookup, destroy, confirm-absent; no create). Prepared and
consumed-not-invoked rows take no effect. An invoked create with no recorded
result is looked up by exact owner label and image; `None` or an error leaves it
unresolved, never absent. A match commits the binding and node row together.
Cleanup commits Destroying and grant cancellation before the provider destroy,
confirms exact-ID absence with a fresh creator-account receipt, then uses the
existing atomic finalizer. Triggers are the original serving deadline, an
operator down, or `Immediate` for leftovers from a dead process. Deadlines come
from the persisted reservation and are never rebased. Deterministic faults cover
every persisted write at the unresolved and bound stages.

## Recovery daemon mode (issue #12, slice 2)

`m8sd --vast-daemon --state-dir ABS --owner LABEL --credential-file ABS
[--cleanup-now]` (feature `prototype-vast`) resumes an **existing** managed Store
and runs the reconciler on a 5 s tick with the production `HttpsRestartPort`
(exact label lookup, account-authenticated exact GET/DELETE). It refuses an
empty, mock or legacy database, a missing/relative/unsafe path, and a credential
file that is not a private single-key regular file; it never creates a domain.
The control socket reports `vast-prototype` mode, serves status/logs/down and
scale-to-zero, and refuses `up`, positive scale, batch, reload and shutdown
without effect. Nothing is advertised Ready. Default cleanup waits for the
original serving deadline or an operator down; `--cleanup-now` is an explicit
choice because a live process elsewhere may still be serving that resource.
The default mock `m8sd` is unchanged and still refuses managed state.

## Remaining integration

- The original creator account ID is now persisted and fingerprinted. A restarted
  bound-resource transport now authenticates that expected account before every
  exact GET or DELETE, retaining one credential across each pair. The Store
  entrypoint requires a recorded invocation and canonical resource binding.
  Construction has no effects and preserves original deadlines; it is not an
  armed worker. Cleanup remains available after the deadline. Only explicit
  exact-ID absence can produce a fresh opaque finalization receipt.
- Compose ambiguous-create lookup and surviving cleanup with that transport.
  Do not turn stored removal metadata into a fresh observation.
- Ask the native owner to sign exact claims without transferring its permanent
  private key into m8s. The existing CLI `auth sign-node` uses relative hours and
  file output; it does not yet implement the required exact-time machine protocol.
- Integrate live daemon control/status and replacement admission only after
  resource uncertainty and cumulative billing obligations can be reconciled.

The small-model demo remains valid historical evidence. It does not qualify
these new restart paths, larger models, total owner loss, or production fleet
operation. Existing E/endpoint qualification is reused without rerunning it.
