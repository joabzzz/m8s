# Prototype decisions

These records distinguish the live-demonstrated prototype from subsequent
source-only integration. Each record states its qualification scope.
They do not replace the [original numbered ADRs](../original/decisions/index.md),
now recovered with source versions and hashes in `docs/original/`.
Their scope is the opt-in prototype, not an approved production fleet design.

| Record | Status | Decision |
| --- | --- | --- |
| [P001](P001-secure-admission.md) | Implemented; live demonstrated | Canonical native identity and authenticated owner-initiated admission |
| [P002](P002-resource-accounting.md) | Implemented; live demonstrated | Durable intent, retained liability, independent exact-resource cleanup |
| [P003](P003-runtime-and-readiness.md) | Implemented; live demonstrated | Immutable prepared runtime and owner-routed model acceptance |
| [P004](P004-restart-and-execution-domains.md) | Source integration; deterministic qualification | Immutable restart inputs, atomic removal, isolated execution domains |

See the [roadmap](../ROADMAP.md) for remaining work and acceptance conditions.

## Source alignment review (2026-10-23)

P001–P003 describe the saved Qwen CPU prototype. P004 describes source-tested
restart primitives, not a demonstrated daemon restart. The original ADRs are
historical design decisions and proposals: an accepted decision is not evidence
that its full design shipped. The [implementation map](../ORIGINAL_PLAN_STATUS.md)
covers all 17 originals, and [code evidence](../original/CODE_EVIDENCE.md) records
specific divergences including the synchronous provider interface, owner-initiated
bootstrap, explicit file secrets, and absent privacy-attestation levels.

The provider consolidation reduces the workspace to nine crates. `m8s-core::models`
owns model fit arithmetic; `m8s_provider` owns provider contracts and the `mock`,
`vast`, and optional `local` modules. `m8sd::mock_provider` remains a re-export for
existing callers. `m8s-cli::dashboard` supplies both `m8s top` and `m8s-top`.

The provider crate assembles the `m8s-agent` executable, including Vast node
composition, while the agent library keeps supervision and authenticated runtime
primitives. This preserves the dependency direction `provider -> agent -> core`:
core has no provider contracts or I/O, and the agent library has no provider
dependency. Heavy local and HTTPS dependencies are optional provider features.
Operator command names, serialized records and native image pins stay the same.

A new Flint performance run must provide its own runtime/model pins, measured
inference and cleanup evidence. Historical Qwen receipts and source tests cannot
establish Flint compatibility or performance.
