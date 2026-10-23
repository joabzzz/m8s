# m8s — mesh-llm patch queue


## Current MP-001 prerequisite snapshot — 2026-10-21

Canonical original `m8s` remains untouched. Admin-executed endpoint admission PASS at `cc08962a` is now backed by the independently inspected existing artifact; see [qualification v7](agor://kb/document/01a15e6d-b40a-7624-99ab-bbb9979b37b8). Historical pending endpoint wording below is superseded, not rerun.

Pure canonical public certificate extraction accepted in isolated external primary commit `c2190fdd6bc4f9100ac5029621622341c3e8c8e6` (base cc08962a): exactly four files, unchanged canonical encoding/verify/trust/host signing semantics, no dependency or policy changes. [Source evidence](agor://kb/document/01a160d7-bac2-728b-93be-f570ee98e3da), [independent clean review](agor://kb/document/01a160d9-5690-72cc-ba0d-898894ed6957). Required shipped-binary check + strict all-target Clippy and focused pure/host-public crypto tests passed. Source bundle SHA256 `916d39cfb505e2423436f8c56818bae6ade8961db79ef99c370d50eac11948c0`; no new endpoint/host signer execution or full runtime qualification. Canonical import, portable m8s dependency/trust adapter and exact-timestamp machine signer remain separate work; no PR/push/publication.

Owner: Metal-0 · Approver: Admin · Rules: [SOP §5a](agor://kb/document/01a1575b-167a-721a-bb44-4562f6f36351) · Origin: [ADR-0006](agor://kb/document/01a15b4d-b4ae-765a-ba27-4491ae240bf9), amending [ADR-0002](agor://kb/document/01a15b4c-e13e-72a9-8cef-7554c25c0a12)

This is the list of mesh-llm changes m8s needs for a secure prototype. Each one is a small commit on the **`m8s` branch** of our local mesh-llm checkout (`~/Development/m8s/mesh-llm`), stacked on a pinned upstream commit. The m8s-node image builds mesh-llm from that branch.

We don't have write access to mesh-llm yet. So every patch is written as a future upstream PR: small, tested, and generic (no m8s-specific names). m8s may eventually become a crate inside mesh-llm, so this queue is also the path for getting there.

```
 upstream mesh-llm  main ──●──────────────●  (pinned base, bumped deliberately)
                                            \
 local branch       m8s                      ●─ MP-001 ─●─ MP-00N …   ← image builds this
                                               │            │
                                               └── each commit = one future upstream PR,
                                                   linked to the ADR that needs it
```

## Branch state

| Field | Value |
|---|---|
| Repo | `~/Development/m8s/mesh-llm` (local only, [ADR-0001](agor://kb/document/01a1576c-8eee-73e9-806c-d99a0e071e05)) |
| Patch branch | `m8s` (created 2026-10-21, local only; name confirmed by Admin) |
| Upstream base commit | `9d97fbfa58c428fb95263a0de719bed959f2510a` (tag `m8s-base-2026-10-21`; upstream main 2026-10-20, #2449) |
| Upstream PRs filed | none (no write access; filing needs Admin) |

## Queue

Status values: `candidate` (identified, not yet approved) → `approved` → `in progress` → `on m8s` (commit stacked) → `upstream PR open` → `merged upstream` (commit dropped from `m8s`) · or `dropped`.

| ID | Patch | Why m8s needs it | ADR | Status | Commit on `m8s` | Upstream PR |
|---|---|---|---|---|---|---|
| **MP-001** | **Key-less node attestation.** Add `--node-ownership <file>` (and auto-load the default path) so a node with no owner keystore can load a pre-signed `auth sign-node` cert, check it against its own endpoint id, report itself as owner-Verified, and pass `require-owned` / `allowlist` admission. Add a helper to print a node key's endpoint id so the owner can pre-sign. Tests: admitted with a valid cert; rejected when expired, revoked, or the endpoint id doesn't match. | Lets rented boxes join under `require-owned`/`allowlist` **without any owner private key on the box**. Closes the S2 admission gap (BLOCKER 1). The Mac signs a short-lived cert for each node in the sealed bootstrap. | [0006](agor://kb/document/01a15b4d-b4ae-765a-ba27-4491ae240bf9) | **source reviewed; Admin endpoint PASS artifact verified at cc08962a; canonical import pending (pure API follow-on c2190fdd source accepted)**, [brief](agor://kb/m8s/plans/briefs/mp-001-keyless-node-attestation.md) | NOT on canonical `m8s`; reviewed source `6b4dd72fd2445ca459caf495ae18b2fe3df9dd5a` + policy tests `cc08962a38d48aaa4fbefa4f00e2459c497dee33` | — |
| MP-002 | **Token mint API.** Loopback `POST /api/token` / SDK method to mint signed join tokens with a configurable TTL, an audience, and a single-use nonce; a token revocation list. | Today tokens are fixed at 24h with no scope or single use. That's mitigated by join-file rotation plus admission, but weak. | [0006](agor://kb/document/01a15b4d-b4ae-765a-ba27-4491ae240bf9) | candidate | — | — |
| MP-003 | **Revocation propagation.** Gossip, or a CRL, for revoked node/cert ids instead of local-only trust-store lists. | Lets a node revoked on `m8s down` or theft be cut off mesh-wide, not just on the Mac mini. | [0006](agor://kb/document/01a15b4d-b4ae-765a-ba27-4491ae240bf9) | candidate | — | — |
| MP-004 | **Signed node tags + route filter.** Tags/capabilities in the owner cert claim; route by tag (e.g. `min_zdr`). | Lets clients refuse nodes below a ZDR level. | [0011](agor://kb/document/01a15b4e-5c46-729d-bb8f-7b1aaf99793a) | candidate | — | — |
| MP-005 | **GPU Dockerfile + entrypoint** (CUDA, `runtime install`, non-root, HEALTHCHECK). | Image hygiene; useful upstream. Not a security blocker. | [0002](agor://kb/document/01a15b4c-e13e-72a9-8cef-7554c25c0a12) | candidate | — | — |

## MP-001 Sprint 1 qualification gate (2026-10-21)

[Sol review evidence](agor://kb/document/01a15cbc-6b00-7793-aae1-8cc71daa9ed1) · [Morning handoff](agor://kb/document/01a1575c-3c1f-7036-84be-7c5f7b6deb12) · [Frozen source worktree](http://localhost:3030/ui/w/01a15c9ce74c70f4b5bcb47c/).

Original protected mesh repo and canonical `m8s` remain at pinned base. Root created an independent local temporary clone, copied ONLY the reviewed39-file allowlist, excluded uncommitted `MP-001-PR.md`, committed `[MP-001] feat(auth): attest nodes without owner private keys` as `6b4dd72fd2445ca459caf495ae18b2fe3df9dd5a` on `mp-001-reviewed`, and exported complete-history bundle. Durable copy: integration worktree `bundles/mesh-mp001-reviewed.bundle`, SHA256 `5238ebc57271256cb9cec79ee5ab97f4809551d08a1a8c96fdf813601fe87cba`. Coordinator independently verified bundle and two pure ownership startup tests.

Worker checks: six touched crates check/clippy(all targets,-Dwarnings), fmt/no-console-print pass; config266 and CLI116 pass. Full suites have permission-dependent failures. **New enforcing-policy gossip admission test cannot create iroh netmon (`Operation not permitted`), so runtime admission is NOT qualified.** OS keychain/filesystem tests also denied. No mesh inference/live P0 test claimed. Next: authorized canonical import, then rerun enforcing admission/network tests in an appropriate runtime before treating patch ready for fleet admission. Never mark `on m8s` until canonical import actually occurs.


## MP-001 focused policy qualification follow-up

Status **reviewed-policytests**, not live/endpoint admission or canonical import. Primary reviewed frozen +120 test-only lines and independently ran current built ownership suite4PASS. Worker identity host-io ownership5/config266/fmt/check/Clippy -Dwarnings/no-console-print pass. Exact original endpoint-backed test reproduced netmon EPERM before admission; it remains outstanding. [Exact evidence, commands and runtime requirement](agor://kb/m8s/status/mp-001-admission-qualification.md).

Primary ordinary Git commit with original hook intact: `cc08962a38d48aaa4fbefa4f00e2459c497dee33`, `test(auth): qualify keyless peer admission policy [MP-001]`. Complete-history verified bundle preserved in qualification worktree ignored `bundles/mesh-mp001-qualified-policy.bundle`, SHA256 `805e2b5789f27cfb15eaebdd8857888d0cb96f8eae8f53690240b6a7361cd7ff`; primary copy `/private/tmp/mesh-mp001-qualified-policy.bundle`. Canonical `m8s` import pending; no fleet/provider/security-readiness claim.

## Not needed (avoided by design)

The m8s-agent control channel ([ADR-0006](agor://kb/document/01a15b4d-b4ae-765a-ba27-4491ae240bf9)) means we don't use mesh-llm's `mesh-llm-control/1`. So these S2 gaps don't block us:

- owner-control under `--trust-policy allowlist` (S2 BLOCKER 2);
- owner-control over relay / without inbound UDP (S2 BLOCKER 3).

They are still worth reporting upstream as issues, but m8s won't carry patches for them unless that changes.

Source of gaps: [S2 §Required upstream gaps](agor://kb/document/01a15b51-c388-76f8-b6ed-8f10d51fdb83) · [Architecture v0 §7](agor://kb/document/01a15b4f-6f37-76b2-8ac8-e3c1be618392)
