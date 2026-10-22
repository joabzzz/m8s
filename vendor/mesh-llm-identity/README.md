# mesh-llm-identity

Shared owner identity and message-envelope crypto for Mesh LLM crates.

This crate owns dependency-light identity primitives that are needed by both the
host runtime and embedded clients:

- owner keypair generation and owner ID derivation
- signed-and-encrypted control-message envelopes
- key provider traits for client/runtime integration
- shared crypto error types

The default feature set stays pure and does not depend on host filesystem or
OS keychain crates. This keeps embedded/client dependency graphs free of local
machine policy.

Public node ownership certificate types and verification are pure and available
without `host-io`. Enable `host-io` for host-facing binaries and runtime crates
that need OS keychain access, encrypted keystore files, node key files,
certificate signing, or trust-store persistence.

## Exact-time signing mechanics (pure, no `host-io`)

`exact_node_signing` (also importable at the legacy `ownership::exact_request`
path under `host-io`) exposes `ExactNodeSigningRequest::new(node, issued_ms,
expires_ms)`, `ExactSigningPolicy::new(max_lifetime_ms, max_issuance_age_ms)`,
`preflight_exact_node_ownership`, and `sign_node_ownership_exact(owner, request,
policy)`. Request and policy fields are private, with no deserialization or
defaults. Limits are mandatory trusted configuration, **not remote authorization,
Admin approval, or nonce-consumption proof**. Production lifetime/age limits and
automated-invoker authorization remain unresolved; no CLI, service, policy store,
default machine route, or m8s wiring is enabled here. Legacy signing is unchanged.

Requests contain only a public 32-byte node and exact nonzero, ordered,
whole-1000ms issuance/expiry bounds. The signer reuses the existing canonical
encoder and mesh-owned in-memory `OwnerKeypair`; it never loads keys, paths or
owner labels. Optional certificate metadata is absent and all output fields are
bounded (the synthetic test checks serialized output below 8KiB). No IO is done.
The library cannot prove key-load ordering when a caller already holds the key.
A future owner invocation must real-clock preflight **before key loading**, then
call the signer, which always rechecks rather than accepting a cached capability.

Real `SystemTime` is sampled at entry, immediately before signing and immediately
after signing. Observed backward time, future issuance, excessive age or expiry
refuses the request; post-sign failure discards the response. Requested times
are never rounded, rebased, extended or saturated. Clock checks retain sub-ms
precision; an explicitly zero-age policy is strict and never automatically widened.
There is no hard real-time preemption or guarantee of detecting jumps between
observations, and a discarded response cannot undo a signature already computed.

For a future m8s adapter, original redemption seconds captured **before SQL**
must be converted with checked multiplication by 1000. Age includes up to 999ms
of fractional offset plus SQL/dispatch/key-access delay. SQL delay is potentially
unbounded: the owner must choose tolerance; this source does not pick one. Stale
work rejects without rewriting issuance or retrying a consumed nonce. There is
no nonce ledger, remint recovery or deduplication here: invoking twice generates
fresh independent certificate IDs. Independent caller verification remains needed.
Tests use explicit test-only limits and fixed synthetic in-memory keys, never
host signing wrappers, keystores or default paths.

## Required pre-signed certificate verification (pure)

`required_node_certificate::verify_required_node_ownership(cert, actual_node,
trust_store, now_ms)` is the fail-closed counterpart of `verify_node_ownership`.
It returns `Err` for absent, malformed, badly signed, tampered, wrong-node,
expired, not-yet-issued, inverted-window, revoked, unsupported-version and
non-allowlisted-owner certificates, and `Ok(())` only for a fully verified
certificate whose owner appears in the caller's explicit trusted-owner list. The
certificate never approves its own owner. It reuses the single canonical claim
encoder and performs no IO, key loading or clock reads. A future-issued
certificate is rejected with no skew tolerance. Hosts opt in with
`mesh-llm --node-ownership <cert> --node-ownership-required --trust-policy
allowlist --trust-owner <id>`; the mode is off by default and unrelated to
`--owner-required`, which demands an owner keystore.
