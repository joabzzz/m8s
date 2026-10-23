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
