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

The cleanup consolidates 13 workspace crates to 10 without changing these wire
or lifecycle contracts. `m8s-core::models` owns model fit arithmetic;
`m8s-core::provider` owns the provider interface; `m8sd::mock_provider` owns the
development mock. `m8s-cli::dashboard` supplies both `m8s top` and `m8s-top`.
Rust imports and `cargo -p` selections for the three removed packages must change;
operator command names, serialized records and native image pins stay the same.

The retained boundaries isolate TOML/time-zone parsing, control socket types,
secret backends, native identity/TLS, local process execution, Vast HTTP, daemon
persistence, and worker supervision. Combining the local and Vast adapters would
also create an agent dependency cycle: local depends on agent, and the Vast node
agent depends on Vast's scoped cleanup transport. Reducing the count further
requires changing those responsibilities, not only moving files.

A new Flint performance run must provide its own runtime/model pins, measured
inference and cleanup evidence. Historical Qwen receipts and source tests cannot
establish Flint compatibility or performance.
