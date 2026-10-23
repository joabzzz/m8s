# From proven prototype to production

Design authority: [original roadmap](original/plans/roadmap.md),
[architecture](original/designs/architecture-v0.md), and
[ADRs](original/decisions/index.md). These are recovered original records;
[reconciliation notes](original/README.md) distinguish later user direction and
the narrower proven demo from original P0–P3 acceptance.

## Completed acceptance

The opt-in CLI deployed Vast instance 57330173, securely bootstrapped the actual
mesh node, routed real Qwen inference through its owner client, destroyed the
resource, and independently verified absence. See the
[receipt](../deploy/container/demo-proof.json). The default daemon remains a
development controller; this live result does not promote it to a fleet manager.

## Next implementation slices

| Priority | Work | Acceptance before claiming completion |
| --- | --- | --- |
| 1 | Compose the proven launch path into daemon reconciliation | Preserve one authoritative Store, one-use effects and exact-owner cleanup across restart; mock faults cover each persisted transition |
| 2 | Recovery and operator status | Resume observation/cleanup without repeating CREATE; distinguish uncertain, stopped, destroyed, Ready, and unresolved billing in CLI/TUI |
| 3 | Cross-attempt budget ledger | Persist cumulative authorization and retained liabilities in one owner-scoped ledger; enforce cap atomically across attempts/processes; reconcile final invoices without reset |
| 4 | Provider network admission | Classify unusable advertised paths without weakening TLS; bound diagnostics, retries and cost; qualify more host networks only under explicit live-test budgets |
| 5 | Runtime release pipeline | Reproduce pinned Linux images and Mac artifacts with checksums/provenance; separate lightweight agent rebuilds from native/model layers; keep secrets out of contexts |
| 6 | Model and GPU profiles | Pin model/license/runtime artifacts; validate memory estimates and actual owner-routed output on each supported profile; measure startup/throughput before advertising capacity |
| 7 | Failure and operational qualification | Exercise owner loss, node loss, expired leases, cleanup transport failure and stale billing; prove bounded behavior and actionable recovery without false absence |

## Qualification boundaries

The latest selected Rust profile passed 772 tests, fmt and strict Clippy. Four
synthetic PTY and three ACL-mutation tests were excluded by the documented
profile. Existing endpoint/E PASS evidence remains valid and was not repeated.
No additional rentals are needed for documentation or deterministic source work.
Future live qualification must have a fresh bounded plan and preserve all prior
claims and reservations. Final reported charges are not yet a settled invoice.

## Source integration

The main workspace and its pinned native companion are private repositories:

- https://github.com/joabzzz/m8s
- https://github.com/joabzzz/m8s-mesh-llm

The native companion retains its history and the exact owner identity fix.
[Source pins](../deploy/container/native-source.json) distinguish the client,
server image, and vendored identity contract. Existing local origins, branches,
ignored bundles, and original worktrees are preserved.

## Integration progress after the live proof

The CLI now uses the shared daemon library's `prepare_launch` and
`dispatch_prepared` boundaries for budget reservation, durable intent, original
cleanup deadline, cleanup guard, and one-use dispatch. Synthetic regression
coverage checks idempotent metadata recovery, unchanged deadlines, rejection of
wrong-node requests before reservation, and retained liability after preparation
failure or claim consumption. This is the first extraction toward priority 1;
it does not yet connect real Vast reconciliation to the daemon's control socket.
The demo workflow is available to agents in `.agents/skills/m8s-demo/SKILL.md`.

The [original-plan implementation map](ORIGINAL_PLAN_STATUS.md) records P0–P4
exit criteria, all 17 ADR boundaries, and the next P1 integration order.

Subsequent local integration adds an atomic absence/node/accounting finalizer,
immutable recovery inputs including the original creator-account pin, a bounded
consistent audit snapshot, and durable mock/managed execution-domain isolation.
These are source-tested boundaries. Authenticated restart transport, permanent
native-owner signing, the live daemon loop and safe fleet replacement remain
unfinished; see [P004](decisions/P004-restart-and-execution-domains.md).
