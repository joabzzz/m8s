# Fresh bounded demo run

Paths below are from the repository root. This procedure performs a paid rental
only when the user requests it and the account/budget boundaries are established.
The successful historical run is evidence, not a reusable launch plan.

## 1. Reuse the tested artifacts

Read `deploy/container/native-source.json` and
`deploy/container/listener-agent-evidence.json` for exact pins. The Linux AMD64
runtime is already published with its model and actual mesh node. Download the
Mac ARM64 CLI and native client from the private release when appropriate:

https://github.com/joabzzz/m8s/releases/tag/prototype-proof-2026-10-23

Verify `SHA256SUMS` before extraction. These are unsigned development artifacts;
the m8s CLI is a debug build. They were qualified on the development Mac.
Alternatively build the CLI with at most two compiler jobs:

```sh
cargo build --locked -j 2 -p m8s-cli --features prototype-vast
```

A default build lacks live launch support. Do not rebuild the native runtime
unless its source/platform actually changed. The pinned native source is in
`joabzzz/m8s-mesh-llm`; preserve required-certificate behavior.

## 2. Establish the inputs without exposing secrets

Use the explicitly selected `.env` path containing `VAST_API_KEY`; never search
unrelated credential files or print its contents. Confirm the expected account
ID from user context or an authorized projected account read. Do not assume the
historical account ID belongs to a new operator.

Read the retained attempt ledgers and the latest saved account evidence before
allocating a cap. Separate actual spend from retained uncertain liabilities.
The prototype currently has per-attempt Stores, not an atomic cross-attempt
account budget: the coordinator must conservatively account for all prior
attempts. Never zero prior liabilities, reset claims, or infer available spending
from the account credit alone. Keep one concurrent rental, no persistent volumes,
and sufficient cleanup and contingency reserves. No agent top-ups.

For a new environment, owner-check is available; skip already-passed unchanged
qualification. The optional authorized provider preflight command is read-only
but makes actual provider calls:

```sh
m8s prototype vast-preflight --credential-file /absolute/selected.env --account ACCOUNT_ID --disk-gb 10
```

Launch performs its own fresh account/inventory/price observation. Preflight
requires complete empty instance and volume inventories; do not delete unrelated
resources or relax that rule to make a demo proceed.

## 3. Prepare a new private plan

The exact versioned schema and validation live in
`crates/m8s-cli/src/prototype/launch.rs`; budget types live under
`crates/m8sd/src/budget.rs`. Read them rather than inventing fields. A plan contains:

- `schema: m8s.vast-demo/v1`; fresh absolute `directory` that does not exist;
  absolute native `mesh` and selected `credential_file` paths.
- Immutable `image` digest from the proven image receipt; `offer_id: null` for
  fresh qualifying selection, or a newly observed exact offer ID. Never reuse
  the historical offer ID as if it were current.
- `expected_account_id`, `disk_gb`, unique `owner`, `node_uid`, `tier`.
- `policy: AdminBudget`, including current authorization window, calendar period,
  cumulative accounting baseline, one-instance limit, hourly/all-in caps,
  runtime bound, cleanup reserve and contingency.
- `request: BudgetRequest`, with matching owner/node, fresh run/request labels,
  runtime/cleanup/contingency and quote fields. Live preflight replaces prices
  and observation timestamps; it does not extend approval or reset spending.

Use restrictive permissions for the plan and its parent. Current hard limits
include at most USD6 per attempt, one instance, at most 1800 seconds for runtime
plus cleanup, and at least 240 seconds reserved for cleanup. These implementation
ceilings do not grant spending authority. Budget validation may impose stricter
limits. Choose a cap within the remaining cumulative authorization.

The CLI currently uses a fixed bounded demonstration prompt and one completion;
it is not a persistent interactive chat server. Do not extend rental lifetime
by disabling cleanup to make a playground. Implement an explicitly bounded
interactive mode separately if requested.

## 4. Run once and retain evidence

```sh
m8s prototype launch --plan /absolute/private/new-attempt.json
```

Capture sanitized CLI stage output. Never log credential-bearing provider
responses, signed log URLs, SSH stdin frames, native private material or keys.
The run arms independent cleanup before consuming the durable CREATE claim.
Do not repeat the command or reuse/reset its directory after failure.

The successful path verifies certificate delivery, discovers the exact served
model through the owner client, requests inference, then destroys the resource.
Only after independent provider-confirmed absence does it release the completion.
Retain `demo-evidence.json`, `cleanup-evidence.json`, `preflight-evidence.json`,
`watchdog.events.jsonl`, and `state.db` privately. Do not publish the whole run
directory: partial failures may leave private material needing scoped cleanup.

## 5. Resolve failures and finish

```sh
m8s prototype audit --directory /absolute/existing-run --owner EXACT_OWNER
```

Audit is local inspection, not provider absence proof. Destroy acknowledgment,
local process exit and stopped state are not absence. An uncertain CREATE may
have rented a resource. Keep independent cleanup running; use the exact owner,
immutable image and resource identity to observe/recover. A missing receipt is
not permission to rerent. Do not bypass denied operations or change security
settings; report the exact blocked operation if external access is missing.

After explicit removal, confirm complete instance and volume inventories and
record a timestamped billing snapshot if authorized. Unsettled billing retains
its reservation. Remove only this run's temporary private keys/registry configs
and stop its owned local processes; preserve the user's credential file, login,
source, caches, unrelated resources and accounting journals. Report the image,
model, instance ID, actual completion, cleanup evidence and billing uncertainty.
