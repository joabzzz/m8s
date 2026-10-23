# Knowledge catalog: recovered, not yet recovered, and why

Inventory of the Agor Knowledge space `m8s` (118 documents, archived included,
enumerated on 2026-10-23) against what this repository holds. It complements
the [recovery README](README.md) and the
[implementation map](../ORIGINAL_PLAN_STATUS.md), and answers one question: *what
design history exists in Knowledge that is not in the repo, and why?*

**Method and limits.** Disposition of documents that are not in the repository is
from the Knowledge *tree metadata* (path, title, lifecycle status) only. Their
bodies were **not** reviewed for this catalog, so "not added" reflects scope
rules (working records, possible local detail), not a finding that a document is
sensitive or unimportant. Lifecycle statuses (`published`, `draft`) are Knowledge
document states, not decision statuses.

## 1. Already in the repository (23 documents)

Recovered by the earlier read-only snapshot and listed in
[`manifest.json`](manifest.json) with source URI, version and SHA-256.

| Knowledge document | In repository as |
| --- | --- |
| `decisions/0001` … `0017` (the complete numbered ADR series) | [`decisions/NNNN-*.md`](decisions/index.md) |
| `decisions/index.md` | [`decisions/index.md`](decisions/index.md) |
| `designs/architecture-v0.md` | [`designs/architecture-v0.md`](designs/architecture-v0.md) |
| `plans/charter.md`, `plans/roadmap.md`, `plans/cost-model.md`, `plans/mesh-llm-patch-queue.md` | [`plans/`](plans/charter.md) |

Spot check for this catalog (2026-10-23): for the ADRs, architecture, charter,
roadmap and patch queue (22 documents), the manifest's SHA-256 and version equal
the current Knowledge version read directly from the Knowledge store, so no
snapshot is stale. The cost model was not re-read. The numbered ADR series is
complete: 0001–0017 are all present, with the recorded statuses (0016 remains
*proposed*).

**Content note.** Snapshots are verbatim, as the recovery README states. Two
retain source text that points at the original author's environment: ADR-0012
(local log/metadata file paths, a log checksum, a toolchain/OS build identifier,
a synthetic test account name and internal session URLs) and the patch queue
(local bundle paths and checksums, internal session URLs). They carry no
credentials, and the session URLs are not resolvable from the repository. The
inventory below is *not* a request to edit them: doing so would break the
snapshot-equals-source property recorded in the manifest.

## 2. Not in the repository — design rationale and research worth adding next

These are technical and likely belong in `docs/original` after a per-document
privacy review (several may cite local artifacts). They are the highest-value gap:
the ADRs on `main` cite S1–S4, the two primers, the MP-001 brief and the kickoff
Q&A by name, but those documents are not in the repository.

| Path (under `agor://kb/m8s/`) | Title | Knowledge status |
| --- | --- | --- |
| `research/spike-s1-vast-provider-and-costs.md` | Spike S1: Vast.ai provider, security model, and cost model | published |
| `research/spike-s2-mesh-llm-integration.md` | Spike S2: mesh-llm integration research | published |
| `research/spike-s3-confidential-compute-and-privacy.md` | Spike S3: Confidential Compute, ZDR and Privacy Audit | published |
| `research/spike-s4-harnesses-evals-orchestration.md` | Spike S4: Agent harnesses, evals, orchestration | published |
| `discussions/control-channel.md` | Discussion primer: handing a rented box its invite + owner auth | published |
| `discussions/operator-vs-virtual-kubelet.md` | Discussion primer: kube-rs operator vs virtual-kubelet (vs no Kubernetes) | published |
| `plans/briefs/mp-001-keyless-node-attestation.md` | Brief MP-001 — Key-less node attestation (mesh-llm patch) | published |
| `notes/2026-10-20-architecture-kickoff-qa.md` | Architecture kickoff — owner answers (2026-10-20) | published |
| `plans/demo-p3.md` | Self-recorded demo plan (after P3) | published |

## 3. Not in the repository — design notes for the local-development and MP-001 slices

Design documents for the local/development executor, MP-001 integration and
config/CLI milestones. Much of their substance is now in code and crate READMEs.

| Path (under `agor://kb/m8s/`) | Title | Knowledge status |
| --- | --- | --- |
| `designs/composed-mp001-bootstrap-qualification.md` | Next P0 qualification source: real TLS + Store burn + exact canonical crypto | published |
| `designs/config-cli-public-adversarial-qualification.md` | Config → CLI intent plan — independent public adversarial test design | published |
| `designs/config-local-dry-run-cli-milestone.md` | Explicit config-to-CLI local dry-run milestone | published |
| `designs/config-schema-v1-draft.md` | m8s-config schema1 draft | draft |
| `designs/development-executor-seam-review.md` | Next minimal development executor seam | published |
| `designs/exact-time-mesh-owner-signer.md` | Exact-time mesh-owned signer: bounded request API | published |
| `designs/local-operator-demo-milestone.md` | Explicit local lifecycle demonstration milestone | published |
| `designs/managed-local-launch-preparation.md` | Authoritative LOCAL launch-intent preparation | published |
| `designs/managed-local-public-adversarial-qualification.md` | Managed LOCAL preparation — adversarial qualification plan | published |
| `designs/mp001-adapter-public-adversarial-qualification.md` | Canonical MP-001 adapter — public acceptance design | published |
| `designs/mp001-public-certificate-contract-integration.md` | Expose the canonical MP-001 public certificate contract | published |
| `designs/owned-local-cleanup-status-seam.md` | Owned local cleanup/status seam | published |
| `designs/portable-mp001-import-and-verifier.md` | Portable canonical MP-001 import and bounded verifier | published |
| `designs/real-local-development-executor-seam.md` | Real local-development runtime seam | published |

## 4. Not in the repository — plans, handoffs and process

Sprint plans, accepted-slice records, handoffs and working procedure. They mix
useful rationale with local operational detail (paths, session references, task
routing); not added pending review.

| Path (under `agor://kb/m8s/`) | Title | Knowledge status |
| --- | --- | --- |
| `plans/afternoon-inference-prototype.md` | Afternoon prototype — CLI to real inference and teardown | published |
| `plans/bootstrap-core-contracts-local-iroh-slice.md` | Bootstrap core and real iroh transport checkpoint | published |
| `plans/composed-bootstrap-stage-b-one-shot.md` | Composed bootstrap Stage B — exact next step, not an execution grant | published |
| `plans/contract-driven-execution-pilot.md` | Contract-driven execution — research assessment and proposed pilot | draft |
| `plans/daemon-authoritative-bootstrap-ledger.md` | Authoritative daemon bootstrap ledger review corrections | published |
| `plans/handoff-build-run-1.md` | Handoff: build run 1 (no live provider) | published |
| `plans/lanes.md` | Lane system | published |
| `plans/local-fake-operation-qualification.md` | Local fake-provider operation qualification test plan | published |
| `plans/local-lifecycle-demo.md` | Runnable LOCAL lifecycle example — accepted | published |
| `plans/managed-local-preparation.md` | Authoritative managed LOCAL preparation — accepted v2 | published |
| `plans/retained-local-cleanup.md` | Retained-owned LOCAL cleanup — accepted | published |
| `plans/sprint-1-execution.md` | Sprint 1 execution | published |
| `plans/store-all-in-budget-ledger.md` | Store all-in budget reservation ledger — review snapshot | published |
| `plans/store-development-executor.md` | Sealed development executor — accepted | published |
| `plans/store-local-development-executor.md` | Store-bound real LOCAL development wrapper — accepted v2 | published |
| `plans/store-operation-create-claim.md` | Durable operation/create claim — review snapshot | published |
| `plans/synthetic-executor-adversarial-qualification.md` | Synthetic executor adversarial qualification test design | published |
| `handoffs/codex-astra-prototype.md` | Handoff to a prototype-execution agent | published |
| `implementation/local-demo-cli-01a161da.md` | Feature-gated local-demo implementation checkpoint | published |
| `sop/standard-operating-procedure.md` | Standard Operating Procedure | published |
| `decisions/lean-async-execution.md` | Lean asynchronous execution — approved operating procedure | published |

## 5. Not in the repository — status, review and reports

| Path (under `agor://kb/m8s/`) | Title | Knowledge status |
| --- | --- | --- |
| `reviews/metal-0-16-exact-certificate-mechanics-freeze.md` | Exact certificate mechanics — uncommitted review freeze | published |
| `status/current.md` | Current status | published |
| `status/development-watch.md` | Development watch | published |
| `status/history/2026-10-21-integration-v60.md` | Integration progression history (status v60) | published |
| `status/mp-001-admission-qualification.md` | MP-001 admission qualification | published |
| `status/sprint-1-sol-review.md` | Sprint 1 — review evidence | published |

## 6. Not in the repository — evidence and independent-review reports (41)

These are **historical test and review reports** about individual source slices
at specific revisions. They are point-in-time artifacts: they do not show current
state and are not runtime proof. Omitted as per-task working records; the
resulting behaviour is described by the code, crate READMEs and the alignment map.

| Path (under `agor://kb/m8s/`) | Title | Knowledge status |
| --- | --- | --- |
| `evidence/agent-authenticated-control-bridge.md` | Typed authenticated agent control bridge | published |
| `evidence/budget-ledger-independent-review.md` | Independent Store budget ledger review | published |
| `evidence/composed-bootstrap-stage-a-freeze.md` | Composed bootstrap Stage A uncommitted freeze | published |
| `evidence/composed-bootstrap-stage-a-independent-review.md` | Composed bootstrap Stage A — independent source review | published |
| `evidence/config-cli-plan-adversarial-integration.md` | CLI plan adversarial public handoff | published |
| `evidence/config-intent-cli-independent-review.md` | Config intent CLI independent frozen source review | published |
| `evidence/config-intent-plan-metal-0-12.md` | Config intent plan accepted handoff | published |
| `evidence/config-parser-independent-review.md` | Independent m8s-config parser review | published |
| `evidence/development-executor-independent-review.md` | Sealed synthetic executor independent source review | published |
| `evidence/development-executor-public-integration.md` | Independent public sealed-executor tests | published |
| `evidence/exact-certificate-mechanics-independent-review.md` | Exact certificate mechanics — independent review | published |
| `evidence/inprocess-top-cli-dispatch.md` | In-process `m8s top` CLI dispatch | published |
| `evidence/keychain-qualification-independent-review.md` | Synthetic Keychain qualification harness independent review | published |
| `evidence/keychain-qualification-phase-a-metal-0-13.md` | Phase A frozen synthetic Keychain harness and source audit | draft |
| `evidence/local-cleanup-public-integration.md` | Public retained-local cleanup handoff | published |
| `evidence/local-development-phase-b-independent-review.md` | Local development Phase B v2 independent review | published |
| `evidence/local-development-public-integration.md` | Phase B local-development public tests | published |
| `evidence/local-fixture-lifetimes-hardening.md` | Local provider fixture lifetime hardening | published |
| `evidence/local-lifecycle-demo-independent-review.md` | Runnable local lifecycle demo independent source review | published |
| `evidence/managed-local-preparation-independent-review.md` | Managed LOCAL preparation independent source review | published |
| `evidence/managed-local-public-integration.md` | Managed LOCAL public integration handoff | published |
| `evidence/metal-0-15-bootstrap-readme-alignment.md` | Bootstrap README alignment — one-file freeze | published |
| `evidence/metal-0-15-canonical-mp001-verifier.md` | Default-OFF canonical MP-001 verifier candidate | published |
| `evidence/metal-0-8-bootstrap-independent-review.md` | Independent bootstrap/control review | published |
| `evidence/metal-0-8-local-provider-contention-regression.md` | Local-provider transient flock regression | published |
| `evidence/metal-0-8-local-provider-handoff.md` | Local-provider handoff | published |
| `evidence/mp001-adapter-v2-independent-review.md` | Canonical MP-001 adapter v2 — independent review | published |
| `evidence/mp001-public-adversarial-integration.md` | MP-001 public adversarial handoff, source closed | published |
| `evidence/mp001-public-certificate-extraction.md` | MP-001 public certificate extraction — checkpoint and history handoff | published |
| `evidence/mp001-public-certificate-independent-review.md` | MP-001 public certificate extraction — independent review | published |
| `evidence/mp001-synthetic-public-fixture-capture.md` | Synthetic canonical MP-001 public fixture capture | published |
| `evidence/operation-create-claim-design-review.md` | Operation/create-claim security design review | published |
| `evidence/operation-create-claim-independent-review.md` | Operation/create-claim independent review | published |
| `evidence/operation-fake-integration.md` | Test-only fake operation integration | published |
| `evidence/provider-local-restart-open-diagnosis.md` | Provider-local restart negative-open diagnosis | published |
| `evidence/redacted-endpoint-diagnostics-freeze.md` | Redacted bind diagnostic two-file freeze | published |
| `evidence/retained-local-cleanup-independent-review.md` | Retained-owned LOCAL cleanup independent review | published |
| `evidence/secrets-cli-top-mock-integration.md` | Standalone top / mock daemon / CLI IPC qualification | published |
| `evidence/timed-local-development-phase-a-metal-0-10.md` | Timed local development Phase A handoff and evidence | draft |
| `evidence/timed-local-phase-a-independent-review.md` | Timed local Phase A independent review | published |
| `evidence/top-status-intermittent-diagnosis.md` | `m8s-top` intermittent status fixture diagnosis | published |

## 7. Not in the repository — draft research notes

Knowledge-status `draft` working notes from early Vast read-only transport and
watchdog spikes. Superseded in practice by the shipped crates.

| Path (under `agor://kb/m8s/`) | Title | Knowledge status |
| --- | --- | --- |
| `research/metal-0-6-local-watchdog-supervisor.md` | Durable fake-effect watchdog and local read-only supervisor | draft |
| `research/metal-0-6-readonly-transport.md` | Bounded read-only HTTPS transport slice | draft |
| `research/metal-0-6-rust-readonly-transport.md` | Optional Rust Vast read-only HTTPS transport | draft |
| `research/metal-0-6-vast-preflight.md` | Vast preflight evidence and cleanup watcher design | draft |

## 8. Resolving ids cited by the recovered documents

The recovered ADRs, charter, roadmap and patch queue link to some of these
documents by Agor id (the links remain original, per the [README](README.md)).
All are listed above by path; this table resolves the ids that occur in them.

| Document id (prefix) | Path under `agor://kb/m8s/` |
| --- | --- |
| `01a1575b-167a` | `sop/standard-operating-procedure.md` (referenced as "SOP §5/§5a/§7") |
| `01a1575c-3c1f` | `status/current.md` ("status"/"morning handoff" at the time) |
| `01a15b4b-a774` | `notes/2026-10-20-architecture-kickoff-qa.md` |
| `01a15b4c-9887` | `research/spike-s3-confidential-compute-and-privacy.md` (S3) |
| `01a15b4e-7d4e` | `research/spike-s1-vast-provider-and-costs.md` (S1) |
| `01a15b51-c388` | `research/spike-s2-mesh-llm-integration.md` (S2) |
| `01a15b4e-43ee` | `research/spike-s4-harnesses-evals-orchestration.md` (S4) |
| `01a15b4f-b222` | `plans/cost-model.md` |
| `01a15b50-d7b4` | `discussions/operator-vs-virtual-kubelet.md` |
| `01a15b50-d7e4` | `discussions/control-channel.md` |
| `01a15c1c-ac26` | `plans/briefs/mp-001-keyless-node-attestation.md` |
| `01a15cbc-6b00` | `status/sprint-1-sol-review.md` |
| `01a15e6d-b40a` | `status/mp-001-admission-qualification.md` |
| `01a160d7-bac2` | `evidence/mp001-public-certificate-extraction.md` |
| `01a160d9-5690` | `evidence/mp001-public-certificate-independent-review.md` |

Document ids and paths are stable inside the Agor Knowledge store; they are not
resolvable from this repository.
