# Code evidence and documentation drift for the original ADRs

Initially audited against `main` at `4837d4d` (2026-10-23); updated for the
crate consolidation and Keychain documentation correction by reading source
layout, public items and crate READMEs. It supports, and does not replace,
[`../ORIGINAL_PLAN_STATUS.md`](../ORIGINAL_PLAN_STATUS.md), which is the status
authority for each ADR. This file adds (a) a crate-to-ADR map with pointers to
where the evidence lives and (b) specific divergences between an ADR's *sketch*
and the code. It never changes an ADR's recorded status; run
[`scripts/check`](../../scripts/check) for test evidence. Live results are
established only by the receipts under
[`deploy/container`](../../deploy/container).

## Crate to ADR map

| Crate / area | ADRs | Evidence to read |
| --- | --- | --- |
| [`m8s-core`](../../crates/m8s-core) | 0005, 0008, 0009 | `src/policy.rs` (location, bandwidth, launch-rate and budget admission), `src/queue.rs` |
| [`core::provider`](../../crates/m8s-core/src/provider.rs), [`-local`](../../crates/m8s-provider-local), [`-vast`](../../crates/m8s-provider-vast) | 0003, 0004, 0007 | `Provider` trait and daemon `mock_provider`; Vast adapter, exact-resource destroy and watchdog |
| [`m8s-bootstrap`](../../crates/m8s-bootstrap) | 0006 | `BOOT_ALPN`/`CONTROL_ALPN`, `NONCE_TTL_SECONDS`, `ControlAcl`, `src/mp001` |
| [`m8s-agent`](../../crates/m8s-agent) | 0006, 0007 | `DeadManSwitch`, control bridge, `vast_node.rs` (per-instance container key) |
| [`m8sd`](../../crates/m8sd) | 0008 | SQLite store, budget ledger, nonce ledger, operation claims |
| [`m8s-config`](../../crates/m8s-config) | 0009, 0005 | Draft schema 1 and schedule projection |
| [`core::models`](../../crates/m8s-core/src/models.rs) | 0016, 0017 | `ModelCatalog`, `ModelProfile`, `plan()`, `estimate_download()`, `models/examples.toml` |
| [`m8s-secrets`](../../crates/m8s-secrets) | 0012 | TOML file backend, ACL checks, experimental binary codec, Keychain driver behind a lock domain |
| [`m8s-cli`](../../crates/m8s-cli), [`m8s-api`](../../crates/m8s-api), [dashboard](../../crates/m8s-cli/src/dashboard) | 0012, 0015, 0017 | CLI grammar (secrets requires explicit `--file`), local protocol, dashboard |
| [`deploy/container`](../../deploy/container) | 0002, 0006 | Image recipes, `native-source.json`, receipts |

## Where the sketch and the code differ

These are factual divergences, not defects: the ADR text is preserved as
recorded, and the code is the current contract.

| ADR | Sketch says | Code does |
| --- | --- | --- |
| 0004 | Async `Provider` with `capabilities()` (ZDR ceiling, billing model, `self_destruct`) and `self_destruct_credential()` | Synchronous `&mut self` trait with `id/search/launch/describe/list_owned/destroy`; launch idempotency keyed by `node_uid`; no capabilities. `Harness` trait absent |
| 0005 | "Occupied regions of Ukraine"; bandwidth floor configurable | Country denylist `CN CU IR KP SY RU BY`; unknown or unassigned codes rejected; Ukraine by conservative subdivision allowlist; download floor can only be raised above 1000 Mbps |
| 0005, 0009 | `$`/hour, Gbps | Micro-USD, Mbps, byte capacities, injected seconds (repo `AGENTS.md`) |
| 0006 | Agent dials the owner on `m8s-boot/1` | Live prototype provisions through restricted SSH framing, with the owner initiating pinned direct iroh TLS; see [P001](../decisions/P001-secure-admission.md). Nonce is 900 s as accepted |
| 0007 | Three layers incl. owner-side sweep and provider limits | Agent layer and exact-resource cleanup exist; the composed daemon sweep is on the roadmap |
| 0008 | Re-rent limit of 3 launches/tier/hour, `Paused` tier | `DEFAULT_LAUNCHES_PER_HOUR = 3` in core policy, which takes launch history as an input to admission; operator-visible `Paused` intent exists in queue/top code. A reconciling daemon that applies them to live launches is not composed |
| 0009 | `m8s.toml` sketch | Versioned draft schema 1; the config README states the ADR "was a sketch, not a finished public contract" |
| 0011 | `ZdrLevel`, `m8s.audit/v1`, cosign/SBOM | None in code; a pinned image digest and verified node certificate are not an audit or ZDR level |
| 0012 | Keychain default; `security-framework` | Keychain not the working default (CLI fails closed without `--file`); driver uses the `keyring` crate behind a lock domain |
| 0017 | `file_gb`, `kv_mb_per_1k_tokens` | Decimal-byte fields (`weights_bytes`, `kv_bytes_per_1024_tokens`); example catalog is explicitly "NOT recommendations" |

## Documentation drift found

- Corrected [`crates/m8s-secrets/README.md`](../../crates/m8s-secrets/README.md)
  to match ADR-0012 v9: the synthetic OS attempt failed and qualification remains
  incomplete. Source tests do not establish successful Keychain qualification.
- Several crate READMEs cite "ADR-00NN". Those mean the originals in
  [`decisions/`](decisions/index.md), not the prototype records `P00N` in
  [`../decisions`](../decisions/README.md).

Keep this file honest by updating it when code moves; amend an ADR only by adding
a dated amendment in Knowledge and re-recovering it.
