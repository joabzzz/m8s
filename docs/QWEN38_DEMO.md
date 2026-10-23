# Qwen3.8 27B demo preparation

Issue [#6](https://github.com/joabzzz/m8s/issues/6) remains open until the live
receipts below exist. The default `m8sd` controller remains development-only.
An explicit `--vast-demo` mode now connects the normal control commands to the
real bounded prototype coordinator: one admitted attempt, one replica, no
replacement or automatic CREATE replay. Source tests and image builds are
qualification steps, not successful Qwen3.8 serving evidence.

## Latest attempt: blocked before CREATE

The [sanitized attempt receipt](../deploy/container/qwen38-blocked-attempt.json)
records the 2026-10-23 20:31 UTC outcome. Real daemon `up`, idempotent `scale 1`,
`status`, `logs` and `down` ran, but no resource was created and no model loaded.
A cumulative-cap bug rejected the launch after reserving 2,369,054 micro-USD.
The fix checks the incremental allowance against the prototype's 6,000,000
micro-USD ceiling before reserving; historical spending stays in the cumulative
policy and aggregate budget checks. It does not repair or release the failed
attempt's reservation or replay its consumed controller claim.

Fresh provider inventory is empty for both instances and volumes. Credit is
16,218,436 micro-USD; historical holds of 13,355,168 plus the new retained
2,369,054 leave only 494,214 unreserved. This cannot cover the required
1,000,000 contingency, so another live attempt is blocked pending explicit
accounting reconciliation. No top-up, reservation reset or live success is
claimed. The published image is qualified offline, not on a rented GPU.

Budget `instance_hourly` and `total_hourly` include transfer and contingency
amortized over serving runtime; they are **not** compute-only rates. For example,
a 2,800,000 all-in ceiling over 900 seconds implies at most 11,200,000 micro-USD
amortized hourly. The separate Qwen compute filter remains 2,000,000/hour.
Always admit the actual refreshed quote within the unreserved all-in allowance.

## Exact model and runtime

Use **Qwen/Qwen3.8-27B**, not Qwen3.5, Qwen3-32B, or the historical Qwen2.5 0.5B
fixture. The [official pinned configuration](https://huggingface.co/Qwen/Qwen3.8-27B/blob/1d4bf0f2ff6012fd82039f2fa52739d0dd7c60c0/config.json)
uses `Qwen3_5ForConditionalGeneration` / `qwen3_5`. The
[official model card](https://huggingface.co/Qwen/Qwen3.8-27B/blob/1d4bf0f2ff6012fd82039f2fa52739d0dd7c60c0/README.md)
describes a dense, Apache-2.0, vision-capable 27B model with 262,144 native context.

The fixed prototype selector is `"model": "qwen3.8-27b"`. It resolves through
MeshLLM's native download/cache path:

```text
unsloth/Qwen3.8-27B-GGUF@4ca720788d1e01f1bff70c033e0d0028fd02e502/Qwen3.8-27B-UD-Q4_K_M.gguf
```

The [pinned quantization](https://huggingface.co/unsloth/Qwen3.8-27B-GGUF/tree/4ca720788d1e01f1bff70c033e0d0028fd02e502)
is 16,464,440,224 bytes. Its upstream LFS SHA-256 is
`322e194ff79741c7baa497c240f677f54b201b0efab44ca8e50f122b39123482`.
Only the public first MiB was fetched for GGUF header inspection: it identifies
`qwen35`, GGUF v3, 866 tensors and 262,144 context. The whole-file hash has not
been independently checked. The expected routed API identity is:

```text
local-gguf/sha256-322e194ff79741c7baa497c240f677f54b201b0efab44ca8e50f122b39123482
```

Source inspection of companion MeshLLM `9d97fbfa58c428fb95263a0de719bed959f2510a`
finds the `qwen35` family and explicit Qwen3.8 recognition. Its llama.cpp pin is
`620d2225cc436ab0b001e3200e590e3b286ef9ab`. This suggests no new architecture
implementation is needed at that revision; it does **not** establish that the
historically published CPU image or a newly built CUDA image can load this file.
An additional isolated `runtime list --available --json` check against the
existing certified owner build refused the published v0.78.0 Metal package:
the build expects release 0.76.1 / Skippy ABI 0.1.71; the catalog offers 0.78.0 /
ABI 0.1.66. No runtime was installed and no local model load was claimed. This
checks a potential local qualification route; the proven model-free owner client
does not load that package. Build a matching native runtime/host pair for model
qualification rather than changing the manifest's ABI assertions.

The published Qwen image uses the integrated native source at
`962aeb2faabe74f5fa809b8b61eaad2f9b816e18`: llama.cpp
`a37cc46104eb50900d5ab4a9ba02a0abdac039db`, patched revision
`6085189ee2b42cad0a296bebdab486cae3a58dbe`. Its actual loader contains
`qwen35`. The matching host/runtime pair now builds and passes the offline
installed-runtime check with release 0.76.1 / ABI 0.1.71. See the
[image qualification receipt](../deploy/container/qwen38-image-qualification.json)
for source/archive hashes, immutable image digest and owner-client checksum.
GPU loading and a routed completion remain separate live gates.

Keep the native ABI, ownership certificate checks and certified owner-client fix.
Do not replace the embedded runtime with an unrelated llama-server.

## Candidate hardware and scope

Choose Vast on-demand, one SM120 Blackwell GPU with at least 48,000 MiB VRAM,
64,000 MiB host RAM, 64 GB ephemeral disk and advertised download >=500 Mbps.
Compute must be <=2,000,000 micro-USD/hour; storage and transfer are additional.
Both transfer directions must be <=4,000 micro-USD/GB, so low hourly pricing
cannot select a host whose native-download fees exhaust the bounded run budget.
The prototype explicitly supports reported host locations US, NL, CZ and FR.
The existing SM120 CUDA build recipe requires CUDA driver support >=12.8.
The 48 GB candidate leaves headroom above the 15.33 GiB weights for native
buffers and cache. It is a conservative selection, not measured VRAM proof.
[Unsloth's sizing guide](https://unsloth.ai/docs/models/qwen3.8) describes smaller
4-bit configurations; this demo does not claim those configurations were tested.

Native-download launches explicitly use `--ctx-size 4096 --parallel 1 --no-draft`.
Automatic draft-model detection is disabled so it cannot add another weight download. Do not
claim a 262K context demo. Neither multiple GPUs nor multiple nodes are selected;
split serving is unqualified. Text only: no vision projector is downloaded, no
image request is sent. The completion request disables thinking through
`chat_template_kwargs.enable_thinking=false` to reserve its 48 output tokens
for an answer. Verify that behavior against the selected runtime during native
qualification.

## Preparation and bounded run

1. Build this branch with Rust 1.98. All local Cargo/native/container
   build commands in the coordinated session go through the shared helper:

   ```sh
   python3 /Users/sandro/Development/m8s/m8s-session-control/with-build-lock.py cargo build --locked -j 2 -p m8s-cli -p m8sd --features m8s-cli/prototype-vast,m8sd/prototype-vast
   python3 /Users/sandro/Development/m8s/m8s-session-control/with-build-lock.py cargo build --locked -j 2 -p m8s-provider --features vast-node
   ```

   Preserve the model-free certified Mac owner client from the proven release,
   or build its pinned compatible successor outside the shared companion checkout.
   Use the published immutable CUDA image from the
   [packaging receipt](../deploy/container/qwen38-image-qualification.json). It
   passed offline CLI and installed-runtime checks for release 0.76.1 / ABI
   0.1.71; this is not a GPU/model load receipt. To rebuild **before renting**, use
   `deploy/container/Dockerfile.flint`, with
   `--build-arg MODEL_CONTRACT=qwen38-model-artifact.json`. The filename is
   historical; the recipe packages an SM120 runtime without weights. Use reviewed
   native and m8s source archives and record their hashes and the resulting image
   digest. Put only explicit public source inputs in the build context, never
   `.env`, run directories, owner keys or credentials. Verify the packaged
   host/runtime ABI and required-certificate option before renting. The exact
   GPU model load is part of the bounded paid qualification; compilation alone
   does not qualify it. Do not reuse the smoke image or bypass ABI assertions.

2. Read only the operator-selected credential file. For this session the user
   clarified `VAST_API_KEY` in `/Users/sandro/Development/m8s/.env`; it is never
   sourced, printed, copied or passed in argv. Account 32349 was authenticated
   at 2026-10-23 17:17:47 UTC: credit 16,218,436 micro-USD rounded down, complete
   instance inventory empty and volume inventory empty. This is a timestamped
   observation, not current spending authority or settled billing.

   The historical ledger retains 13,355,168 micro-USD and reports 25,000
   micro-USD of charges without billing finality. Do not subtract the reported
   charge from a hold, reset a consumed claim or treat the old $16 authorization
   as renewable. Current credit minus those holds is at most 2,863,268 micro-USD,
   before later liabilities and reserves. Reconcile subsequent private journals
   with the coordinator. This earlier observation predates the retained reservation described above;
   it must not be reused as current unreserved budget.

3. Read the [fresh-run safety procedure](../.agents/skills/m8s-demo/references/run-demo.md)
   and [CLI launch contract](../crates/m8s-cli/README.md#one-attempt-vast-demo-default-off).
   Copy the [inert Qwen template](../.agents/skills/m8s-demo/assets/qwen38-plan.template.json)
   outside Git into an owned private directory. Its zero dates/caps, unknown
   liability and zero image digest deliberately prevent use. Replace them with
   the qualified image and reconciled, finite policy. Preserve cumulative holds.
   The executable schema and budget validator remain authoritative.

   For a read-only fresh account/inventory/offer check:

   ```sh
   target/debug/m8s prototype vast-preflight --credential-file /Users/sandro/Development/m8s/.env --account 32349 --disk-gb 64 --model qwen3.8-27b
   ```

   Preflight prices and advertised hardware are observations. Native model
   download happens **during billed rental time**, along with image pull,
   provisioning and loading. At 500 Mbps the weights alone have an ideal lower
   bound of about 264 seconds; no throughput guarantee follows from that rate.
   The Qwen profile reserves 256 GB of transfer independently of its 64 GB
   disk. Its native Hub and Xet request retry counts are fixed to zero, using
   the pinned clients' public settings, to prevent retry amplification. Include
   the immutable image's download size, weights, metadata and transfer overhead
   in that reserve; fail admission if it cannot cover the selected flow.
   Search Offers defines `storage_cost` as USD/GB/month. The adapter converts
   it to micro-USD/hour using a conservative 28-day month and upward rounding;
   `allocated_storage` binds the requested disk size in the price search.
   Include storage for the entire runtime plus cleanup,
   independent cleanup, contingency and unsettled previous liabilities. Reserve
   at least 240 seconds for cleanup within the 1,800-second attempt ceiling.
   Refuse if the available credit cannot cover all retained and new liability.

4. For daemon control, add `"control_directory": "/absolute/private/NEW-CONTROLLER"`
   to the private plan. It must match `--state-dir`; the attempt `directory`
   remains a separate fresh directory. Start the explicitly selected binaries:

   ```sh
   target/debug/m8sd --vast-demo --state-dir /absolute/private/NEW-CONTROLLER --plan /absolute/private/new-qwen38-plan.json --m8s /absolute/target/debug/m8s
   # Another terminal, same private socket throughout:
   target/debug/m8s --socket /absolute/private/NEW-CONTROLLER/control.sock up qwen38-27b --json
   target/debug/m8s --socket /absolute/private/NEW-CONTROLLER/control.sock scale qwen38-27b 1 --json
   target/debug/m8s --socket /absolute/private/NEW-CONTROLLER/control.sock status --json
   target/debug/m8s --socket /absolute/private/NEW-CONTROLLER/control.sock logs --json
   # After logs show completion-held-until-down-or-deadline:
   target/debug/m8s --socket /absolute/private/NEW-CONTROLLER/control.sock down --json
   ```

   Startup freezes the validated plan/executable fingerprint without opening
   credentials or contacting Vast. `up` consumes a durable single-use controller
   claim before starting the actual prototype CLI. The CLI owns the authoritative
   intent/budget Store and arms independent cleanup before provider CREATE.
   Scale 1 is idempotent while active; scale 0 requests down; larger counts
   refuse. Restart permits stored observation and down, never launch replay.

   Status reports `mode=vast-prototype`; nodes come from the real attempt Store,
   not a mock adapter. Stored status is not fresh provider absence and no Ready
   phase is invented. Logs retain only an exact list of static stage labels.
   Wait for `completion-held-until-down-or-deadline`, which follows verified
   certificate delivery, exact model discovery and a real routed completion.
   The answer remains private until verified removal. Down before completion
   cancels the work and requests cleanup. Down after completion releases the
   result to the existing exact-resource destruction/finalization path. Down's
   immediate acknowledgment is not absence: poll logs and verify the receipt.

   The original admitted deadline, node DMS and detached watchdog still apply.
   Omitting `control_directory` preserves the one-shot prototype's automatic
   cleanup. Neither mode retries an uncertain CREATE or extends an approval.
   A sensible actual answer about mesh networks must appear in the final
   `demo-evidence.json`; never substitute canned expected text.

5. Preserve private attempt state, sanitized completion, preflight, watchdog,
   certificate and cleanup receipts. Publish only reviewed projections. Check
   complete instance and volume inventories again; destruction acknowledgment
   and stopped state are insufficient. Container disk remains chargeable until
   destruction; persistent volumes survive instance deletion. See
   [Vast storage rules](https://docs.vast.ai/guides/instances/storage/types).
   Keep all unsettled holds and record a fresh account timestamp. Exact-owner
   audit, when needed:

   ```sh
   target/debug/m8s prototype audit --directory /absolute/private/existing-attempt --owner EXACT_OWNER
   ```

   Audit reads local journals; it does not contact Vast or prove absence.

## Remaining issue acceptance

- Qualify the exact model and packaged runtime, then record a real routed answer.
- Exercise the explicit daemon mode's `up`/`scale`/`status`/`logs`/`down` against
  the live model. It remains a one-attempt development integration, not a
  production fleet scheduler.
- Reconcile the retained reservation before admitting another finite budget;
  preserve all liabilities and claims, then arm independent cleanup and verify
  actual serving and exact-resource teardown.

[Sanitized preparation evidence](../deploy/container/qwen38-preparation-evidence.json)
records what was actually inspected. No Qwen3.8 resource was rented by the
preparation steps, so there is no new exact-resource teardown receipt or claimed
live completion. The historical small-model proof remains separate.
