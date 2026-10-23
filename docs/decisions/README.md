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
