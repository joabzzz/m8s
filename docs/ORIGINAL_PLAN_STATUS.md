# Original plan versus implemented evidence

Reviewed 2026-10-23 against the [recovered originals](original/README.md), current
source, and saved qualification. This is an implementation map, not a new ADR or
an assertion that the full original product is complete.

## Milestones

| Original milestone | Implemented / demonstrated | Remaining exit criteria |
| --- | --- | --- |
| P0: prove it by hand | Actual required-certificate native node; prepared image; Vast TCP/UDP path; owner-routed 0.5B inference; exact instance removal | 27B and 70B profiles, Goose tasks, multi-GPU layouts, isolated node self-destroy test and owner-network-loss test; CPU demo does not qualify these |
| P1: control plane | Core policy, model/config schema, SQLite intent/nonce/budget journals, CLI/TUI mock control, shared live preparation/dispatch, actual one-shot launch | Real provider daemon composition, persistent owner signing integration, restart observation/cleanup and safe replacement policy, live status; Keychain default remains unavailable |
| P2: scale/evals/cost model | Local model fit/planning, manual/schedule/queue planning foundations | Real queue observation, sustained fleet scaling, Goose/pi harness integration, N-concurrency evals and measured cost data |
| P3: privacy/security | Required node certificates, pinned TLS, strict control ACL, scoped node credential handling, digest-pinned image | Signed image/release attestation enforcement, SBOM verification, complete hardening/audit suite, signed audit reports and effective ZDR assessments |
| P4 | Deferred as designed | Frontier, additional providers, payments, confidential compute and Kubernetes mode |

No new rental is authorized merely by a milestone's old estimated cost. Current
experiment authority, prior liabilities and separately bounded qualification
plans remain required. Implementation can proceed with deterministic fixtures.

## ADR implementation map

| ADR | Current implementation and next boundary |
| --- | --- |
| 0001: local repository | Superseded only for publication by later explicit private-GitHub authorization; source/history retained in m8s and its native companion |
| 0002: Rust/native dependency | Rust workspace and pinned native patch stack; vendor provenance and separate server/client revisions recorded |
| 0003: one owner, one mesh | Exact owner labels/control pins; prototype uses a fresh ephemeral owner; permanent owner integration remains |
| 0004: adapters | Provider trait, Mock/local and bounded Vast transports; full live adapter lifecycle and capabilities still need composition |
| 0005: placement/budget | Core policy, normalized units, conservative budget journal; prototype admission is narrower than full signed-image policy and shared account planning |
| 0006: admission/control | Canonical required certificate, one-use Store nonce, direct pinned iroh TLS, explicit ACL, authenticated heartbeat; no relay fallback. Permanent owner signing and revocation distribution remain |
| 0007: DMS | Agent TTL/lifetime supervision and scoped cleanup plus independent owner cleanup worker; total-owner-loss and scoped self-destroy live qualification still pending |
| 0008: daemon/SQLite | Durable create intent/claim, retained uncertainty, exact-absence finalization; durable execution domains prevent generic mock reconciliation from interpreting managed journals. Live daemon restart/reconciliation remains |
| 0009: owner configuration | Versioned config and manual/schedule/queue planning; real observations must drive any production trigger |
| 0010: inference over mesh | Demo traversed the owner client and actual mesh; no production inference gateway. The bounded demo/canary captures its own result only |
| 0011: privacy/audit | Strong transport is not provider confidentiality. No effective L1/L2/L3 claim without the required audit/attestation evidence |
| 0012: secrets | File backend and safe CLI; Keychain library backend passed the synthetic OS roundtrip/concurrency run (2026-10-23) but is not wired into the CLI and not default; see ADR 0012 for blockers. Prototype dotenv is explicitly selected. m8s must not read a permanent mesh owner private key |
| 0013: payments | Still out of scope; no wallet or top-up implementation |
| 0014: evals | Real single completion is acceptance evidence, not throughput/quality evaluation; reuse the native mesh-routed tooling when adding evals |
| 0015: interfaces | CLI/TUI/control API foundations; live managed recovery/status, metrics and alert sinks remain incomplete |
| 0016: model per host | Original ADR remains proposed; small single-host CPU test supplies no multi-GPU/layout benchmark |
| 0017: model profiles | Catalog and placement calculations exist; fixed demo model is a fixture, not a production model-selection policy |

## P1 integration order

1. Keep the proven one-shot path working while moving lifecycle transactions into
   the daemon library. Shared preparation/dispatch is complete; exact-removal
   finalization is now atomic with retained billing liability (synthetic
   fault tests; the new finalizer has not been live-qualified).
2. Persist enough immutable launch metadata to reconstruct **observation and
   cleanup only** after restart. A reconstructed coordinator must not mint a new
   create claim, renew its lifetime, or treat an empty lookup as absence.
   Canonical metadata and the fingerprinted creator-account pin are implemented;
   account-bound absence confirmation and authenticated restart cleanup for an
   already bound invocation are source-tested. Ambiguous-create observation and
   surviving worker composition remain.
3. Expose a bounded recovery/status view that distinguishes reservation-only,
   consumed-but-not-invoked, ambiguous create, positively bound resource,
   provider-confirmed removal and unsettled billing. The bounded CLI journal
   snapshot is implemented; live provider status and TUI integration remain.
   Stored data is historical.
4. Connect a real daemon mode to those boundaries, an explicit credential
   supplier, and the native owner's signing interface. Keep the default mock
   mode isolated and fail closed until these pieces are actually composed.
5. Add replacement admission only once outstanding resource uncertainty and
   billing are reconciled, retaining the per-tier launch rate and cumulative
   budget. Do not remove one-shot guards as a shortcut to fleet mode.
