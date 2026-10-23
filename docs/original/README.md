# Original m8s design records

Recovered on 2026-10-23 from the local Agor Knowledge database using a read-only
connection. These are verbatim snapshots of the original roadmap, charter,
architecture, cost model, patch queue and ADRs 0001–0017, plus their index.
`manifest.json` records each source URI, version ID, version number and SHA-256.
The source database was not modified. Agor links remain original links.

Start with [Roadmap v1](plans/roadmap.md), [Architecture v0](designs/architecture-v0.md),
and the [ADR index](decisions/index.md). The [current integration roadmap](../ROADMAP.md)
tracks implementation against them. These snapshots are historical design inputs,
not automatically current provider facts, permission grants or completed tests.

## Reconciliation with later user direction and evidence

- ADR-0001's no-remote restriction was superseded by explicit user authorization
  for private joabzzz source publication. Both repositories are private.
- The original P0 27B/70B, Goose, multi-GPU and owner-network-loss criteria remain
  unqualified. The later user-requested small Qwen CPU demo passed its narrower
  launch → secure bootstrap → owner-routed inference → verified removal scope.
- The original USD20/month design and estimated stage costs do not grant a new
  experiment budget. The completed demo used separately authorized cumulative
  limits, retained reservations and no agent top-ups. No more spend is needed
  for source integration.
- ADR-0006 includes early relay/bootstrap sketches. Later handoff restrictions
  and current implementation remain direct-only and fail closed; these snapshots
  do not authorize a relay fallback or security change.
- ADR-0012's intended Keychain default remains unavailable pending its unfinished
  OS qualification. Its historical attendance wording was explicitly corrected
  in that ADR; no synthetic failure is recast as a current permission denial.
- The proven demo uses a fresh ephemeral owner, never the user's owner keystore.
  Integrating an existing owner must preserve ADR-0012: m8s requests signatures
  from mesh-llm and never reads or stores the owner's private key.
- A pinned image digest and verified node certificate do not establish image
  signature verification, an audit pass, or a ZDR level. P3 remains unfinished.

Do not rewrite these snapshots to make completed work appear to satisfy broader
original acceptance criteria. Record implementation progress and later decisions
in current documents, retaining the original source and status.
