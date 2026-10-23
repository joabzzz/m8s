# m8s integration handoff — 2026-10-23

## Start here

Worktree: `/private/tmp/m8s-root-integration.VSK0VT`
Branch: `metal-0-rolling-integration`.
Read `AGENTS.md`, `docs/ORIGINAL_PLAN_STATUS.md`, and `docs/ROADMAP.md` first.
Preserve every committed, staged, unstaged and untracked change. No reset/clean.

The latest user-provided instructions for this continuation are **local only**:
no real provider calls/searches, credentials, spend, pushes, publication or upstream
filing. Never read/request/set Vast credentials. Earlier permission to publish
private repositories describes the completed prototype session and does not
override the latest continuation instructions. Do not reinterpret old failed
qualifications as current tool denials or silently bypass them.

## Original design recovered

`docs/original/` contains 23 verbatim Agor Knowledge snapshots: original roadmap,
charter, architecture, cost model, patch queue, all 17 ADRs and their index.
`manifest.json` records source URI/version/hash; all 23 hashes were verified.
The source was a read-only local database query because no Agor connector was
available. No source Knowledge record was modified. Do not rewrite historical
snapshots; update the implementation map and current decision records instead.

## The live demo is already proven

Read `deploy/container/demo-proof.json`: Vast instance **57330173** ran the actual
native mesh node with a required canonical certificate, served Qwen2.5-0.5B
through its owner client, and was destroyed with independent absence confirmation.
Saved complete instance and volume inventories were empty. These are historical
observations, not fresh account queries. Do not rent again to prove the same demo.

Native client/server pins: `deploy/container/native-source.json`.
The companion worktree is
`/Users/sandro/.agor/worktrees/local/mesh-mp001-reviewed/prototype-keyless-main-integration`.
Client revision: `7a101840af79c7509789b9e3152ca6c4605186e8`.
No companion-source edits or native qualification reruns were made this hour.

The private GitHub repositories and tested release already exist:
`joabzzz/m8s`, `joabzzz/m8s-mesh-llm`, release `prototype-proof-2026-10-23`.
Keep tested release artifacts pinned to their proven sources. Subsequent source
integration is not an additional live qualification. New work remains local.

Final billing is still unsettled in saved evidence. Historical authorization was
USD16, with USD13.355168 conservatively reserved across attempts. Do not reset
claims/reservations, infer a current balance, or treat the difference as new
permission. No provider resource was created during this continuation.

## Source integration this hour

- Atomic exact-resource removal now combines node deletion, grant cancellation,
  retained absence receipt and `BillingUnknown` accounting. Failed SQL writes
  roll back together. Claim and nonce/revocation tombstones remain. Account-bound
  absence confirmation authenticates the original expected account before its
  exact-resource GET, retaining the same credential; this has source coverage only.
- Canonical launch metadata is persisted with the original managed intent.
  Recovery checks the immutable fingerprint and budget binding, returns original
  deadlines and the fingerprinted original creator account ID, and contains no
  create request or reminted claim. Stored account ID is not current authentication.
- `Store::vast_audit` and CLI `prototype audit` use a bounded, consistent snapshot.
  Reservation, preparation, consumed claim, unresolved invocation, recorded
  binding and recorded removal are distinguished. Billing/cancellation remain
  separate; none of this is live provider status or Ready.
- Unmanaged/mock and managed execution domains are selected durably before
  effects/accounting. They cannot be silently converted or reset after cleanup.
  The existing process lock is still required within a domain. Missing account
  rows cannot reset authorization when domain/period/managed history remains.
- The repo skill `.agents/skills/m8s-demo/SKILL.md` links the original roadmap and
  ADR implementation map in addition to the proven-demo runbook.

## Local checkpoint

Source/test checkpoint: `6eec8633258ac9f5841720f8dd8c516b6d3901f7` (inspect `git log` for the final documentation
commit). Earlier completed source slices include `205cb63` (atomic removal),
`c2d2214` (restart inputs), `0dcb561` (audit snapshot), `cbf717e` (execution domains),
`54f589c` (creator-account pin), and `464f23e` (account-bound absence).
`7dfe0b1` adds actual CLI audit tests; `6eec863` refuses account reset when
execution-domain or period history remains. Coherent checkpoints have ignored
recovery bundles under `bundles/`; the final bundle is
`bundles/m8s-roadmap-hour.bundle`.

### Final validation

The final selected profile passed **772 tests**, formatting and strict Clippy.
The `prototype-vast` CLI build also passed. The repo skill validator passed,
all 23 original-source hashes matched, and `git diff --check` was clean.
No provider calls, credential reads, rentals, pushes or native qualification
reruns occurred during this continuation. No Cargo command remains running at
handoff. Failed intermediate test logs are historical; the successful final logs:

- `/private/tmp/m8s-roadmap-wrapup-check.log` — SHA-256 `b3531b9e2c374f2df9f978752422e60d24a406730d43df1d93977673677eabc6`
- `/private/tmp/m8s-roadmap-wrapup-build.log` — SHA-256 `c6e8c752c685bc5312eec9ca6183af18f0e5c3fc611e63fd9f50eeb9dcebd2ac`

## Next meaningful P1 implementation

1. Complete authenticated creator-account restart observation/cleanup using the
   original identity and deadlines. Never turn an empty search, stored JSON or
   destroy acknowledgement into absence. A reconstructed capsule is not itself
   an armed cleanup worker. Do not add automatic replacement CREATE.
2. Integrate the native owner's signing interface without loading its permanent
   private key into m8s. The current native `auth sign-node` takes relative hours
   and writes a file; it is not an exact-time machine signing protocol. The live
   demo uses a fresh ephemeral owner and does not qualify permanent owner custody.
3. Compose a separate real daemon mode only after those boundaries are complete;
   preserve mock isolation and single-owner Store authority.
4. Build cross-attempt cumulative accounting and replacement admission before
   relaxing prototype one-shot guards. Original ADR-0008's fleet recovery policy
   is not permission to reset unresolved costs or consumed claims.

Larger 27B/70B models, Goose, multi-GPU layouts, owner-network-loss/node-self-destroy
qualification, signed image enforcement, SBOM and privacy audits remain open.
The default daemon is mock-only. Keychain qualification remains incomplete.
Existing E PASS and native endpoint acceptance must be reused, not repeated.

## Local checks

One Cargo process at a time, maximum two compiler jobs. The working environment:

```sh
cd /private/tmp/m8s-root-integration.VSK0VT
env PATH=/private/tmp/mp001-rustup/toolchains/1.98.0-aarch64-apple-darwin/bin:$PATH \
  CARGO_HOME=/private/tmp/mp017-primary-cargo RUSTC_WRAPPER= CARGO_NET_OFFLINE=true \
  RUST_TEST_THREADS=1 scripts/check --no-pty --no-acl-mutation
```

This selects deterministic/source checks, excluding four synthetic PTY and three
ACL-mutation tests. It is not unfiltered OS/endpoint qualification. Do not retry
the old Keychain/ACL/endpoint qualification or change OS security settings.

CLI build uses the same environment:
`cargo build --locked -j2 -p m8s-cli --features prototype-vast`.
Commit coherent slices and export ignored `bundles/m8s-<milestone>.bundle` with
`git bundle create --all`. Preserve the original local `origin` remote.

## New-session prompt

> Continue m8s source integration in
> `/private/tmp/m8s-root-integration.VSK0VT`. Read `AGENTS.md` and
> `docs/CONTINUATION_HANDOFF.md` first, then the original roadmap/ADR implementation
> map. Preserve all work. The live demo is proven and cleaned up; reuse that
> evidence. Follow the current local-only restrictions. Continue the next P1
> source slice with deterministic tests, without new rentals, credentials,
> qualification retries or pushes. Keep the one-shot, account, budget, TLS and
> exact-absence boundaries intact.
