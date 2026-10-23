# Qwen3.8 27B live demo

A [live owner-routed canary](../deploy/container/qwen38-owner-canary-proof.json)
returned a sensible completion from the exact pinned model on instance57431708,
then real `m8s down` led to provider-confirmed destruction. The first canary was
sent separately after discovering an API/content identity mismatch; a corrected
automatic coordinator repeat is pending. Issue[#6](https://github.com/joabzzz/m8s/issues/6)
remains open until that integrated verification finishes. The default `m8sd` controller remains development-only.
An explicit `--vast-demo` mode now connects the normal control commands to the
real bounded prototype coordinator: one admitted attempt, one replica, no
replacement or automatic CREATE replay. Source tests and image builds are
qualification steps, not successful Qwen3.8 serving evidence.

## Earlier startup failures: four exact resources destroyed

The [live-attempt receipt](../deploy/container/qwen38-live-attempts.json) records
instances **57423542**, **57426719**, **57429936** and **57430645**. All reached certificate
delivery/control, then failed before exact model discovery or inference. The third attempt reports the closed
bootstrap category `transport` after the first authenticated heartbeat.
All have independently confirmed provider absence and retained removal records;
no paid resource remains. Issue #6 acceptance is **not met**.

The original paid-attempt allowances total 2,857,776 micro-USD. The user later
explicitly removed the requirement to keep those full allowances held and
authorized available funds to finish. The [authorized reconciliation](../deploy/container/qwen38-authorized-reconciliation.json)
retains 817,893 micro-USD of conservative closed-run exposure for those two
provider-confirmed-absent attempts, plus all 13,355,168 micro-USD of older holds.
Original journals, consumed claims and the earlier declined proposal remain as
history; this is not final billing. The 22:05:26 UTC authenticated observation
found 16,204,104 micro-USD credit, no instances/volumes, and 2,031,043 micro-USD
available after these liabilities. That funded the third attempt with a
1,900,000 cap. Later attempts and reconciliations below retain additional
exposure. The current next allowance is 1,200,000 from 1,251,890 observed
available at 22:23 UTC; launch must still requalify account, inventory and prices.

The [latest qualified image](../deploy/container/qwen38-product-image-qualification.json)
contains the agent with fixed public NVIDIA probe/loader paths after clearing its
environment. Driver mounts appear after build-time `ldconfig`; neither personal
PATH nor arbitrary environment/credentials are inherited. Native stderr is
classified only into fixed categories in memory; raw output is not logged.
Node startup errors are emitted before invoking destruction, because later
reporting may lose SSH. The daemon retains closed bootstrap categories in
`m8s logs`. These changes do not extend the lease or delay teardown, and remain
offline qualification rather than successful GPU serving proof.

## Earlier admission failure: blocked before CREATE

The [sanitized attempt receipt](../deploy/container/qwen38-blocked-attempt.json)
records the 2026-10-23 20:31 UTC outcome. Real daemon `up`, idempotent `scale 1`,
`status`, `logs` and `down` ran, but no resource was created and no model loaded.
A cumulative-cap bug rejected the launch after reserving 2,369,054 micro-USD.
The fix checks the incremental allowance against the prototype's 6,000,000
micro-USD ceiling before reserving; historical spending stays in the cumulative
policy and aggregate budget checks. It does not repair or release the failed
attempt's reservation or replay its consumed controller claim.

At that earlier 20:31 UTC checkpoint, provider inventories were empty for both
instances and volumes. Credit was 16,218,436 micro-USD; historical holds of 13,355,168 plus the new retained
2,369,054 left only 494,214 unreserved. That could not cover the required
1,000,000 contingency; the later explicitly authorized uninvoked cancellation
is recorded above. This paragraph describes the earlier checkpoint, not current
spendable credit. No top-up, reservation reset or live success is
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
The artifact receipt form above is converted at the native CLI boundary to
`repo@revision:filename`; the packaged native client resolves that exact selector
to the pinned download URL in a metadata-only check. The metadata command also
normalizes artifact receipt syntax; neither form proves an actual GPU load.
The lower-level model-ref parser uses the colon selector. This conversion
removes ambiguity but is not a proven explanation of the failed live startup.

Only the public first MiB was fetched for GGUF header inspection: it identifies
`qwen35`, GGUF v3, 866 tensors and 262,144 context. The whole-file hash has not
been independently checked. The raw GGUF blob hash is not the native HF source identity. The pinned native
`HuggingFaceGgufIdentity` hashes canonical JSON containing repository, immutable
commit and the ordered filename/size/blob-hash entries. For this exact artifact,
the derived response/routing content identity is:

```text
local-gguf/sha256-7756e8943d5ec98b1cd76895d33d20b8c8bf7609a71540fc9b6fa512bdedd0de
```

OpenAI discovery and requests use the canonical alias instead:

```text
unsloth/Qwen3.8-27B-GGUF@4ca720788d1e01f1bff70c033e0d0028fd02e502:UD-Q4_K_M
```

The response's model field must equal the7756 content identity. The first live
owner canary proved this mapping; the coordinator now distinguishes discovery
from response identity. No alternate model or generic `mesh` route is accepted.

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

The pinned packager records the NVCC architecture target `120a`, while the
pinned NVIDIA probe converts compute capability 12.0 to `120`. The resolver
compares strings exactly and rejects that mismatch. The agent-only overlay
normalizes this one bundle's hardware requirement to `120`, after verifying
all declared native library hashes and exact release/ABI/toolkit. It changes
no library bytes or ABI assertion. [NVIDIA's CUDA target documentation](https://developer.nvidia.com/blog/nvidia-blackwell-and-nvidia-cuda-12-9-introduce-family-specific-architecture-features)
explains that the `a` NVCC suffix runs only on the exact compute capability;
[NVIDIA lists RTX PRO Blackwell as 12.0](https://developer.nvidia.com/cuda/gpus).
The normalization refuses any unexpected architecture, release, ABI or checksum.

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
   [packaging receipt](../deploy/container/qwen38-product-image-qualification.json).
   It passed the actual native product integrity startup gate for release0.76.1 /
   ABI0.1.71; CPU-only qualification is not a GPU/model load receipt. Reuse this
   immutable image for unchanged pins. To update the agent **before renting**, use
   `deploy/container/Dockerfile.agent-update` and its explicit public inputs,
   including the native product integrity qualifier. It reuses the compiled
   native runtime and repairs enclosing product digests after normalization; no
   native Rust/CUDA rebuild is needed. `Dockerfile.flint` is the historical full
   native build recipe, not the qualified final overlay. Record source archive
   hashes and the resulting immutable image digest. Put only explicit public source inputs in the build context, never
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
   the qualified image and reconciled, finite policy. Carry forward audited liabilities; preserve original journals and consumed claims.
   The executable schema and budget validator remain authoritative.

   For a read-only fresh account/inventory/offer check:

   ```sh
   target/debug/m8s prototype vast-preflight --credential-file /Users/sandro/Development/m8s/.env --account 32349 --disk-gb 64 --model qwen3.8-27b
   ```

   Preflight prices and advertised hardware are observations. Native model
   download happens **during billed rental time**, along with image pull,
   provisioning and loading. At 500 Mbps the weights alone have an ideal lower
   bound of about 264 seconds; no throughput guarantee follows from that rate.
   The Qwen profile reserves 64 GB of transfer independently of its 64 GB
   disk: two complete 16.465 GB download paths plus the <13 GB unpacked image
   leave >18 GB for metadata/framing. The pinned native artifact resolver
   downloads exactly the selected non-sharded GGUF, not all variants. Its native Hub and Xet request retry counts are fixed to zero, using
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
   Retain conservative unsettled liability bounds and original journals, and record
   a fresh account timestamp. Full allowances may be replaced only through an
   explicitly authorized, evidenced reconciliation; provisional charges alone
   are not final billing. Exact-owner
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

## Follow-up startup diagnosis

The third attempt57429936 ended with independently confirmed absence at
1792793232, 36 seconds after reservation. Under the updated user authorization,
[its separate reconciliation](../deploy/container/qwen38-authorized-reconciliation-third.json)
retains360417 micro-USD including the entire64GB transfer bound and100000 extra
contingency. Original allowance1751182, journals and consumed claim remain.
At22:12:27 UTC complete instance/volume inventories were empty and credits
16169767 minus14533478 reconciled liabilities left1636289 micro-USD.

Native JSON fatal output can use stdout. Agent077b377 classifies both channels
with bounded destructive in-memory handling and logs an explicit child-exit
category before destruction. This corrects a diagnostic gap, not a proven
GPU startup cause. No raw native output is published.

## Reproduced package integrity blocker

A no-credential, no-certificate, network-disabled native `serve` probe reproduces
`product runtime manifest checksum mismatch` on the normalized images through
8e4faa54. The earlier normalizer changed the nested runtime manifest without
updating its enclosing product manifest and complete runtime-tree digests. The
actual native startup gate rejected the package before model download; help,
metadata and installed-runtime listing were insufficient qualification.

The corrected normalizer validates the original product/host/tree and every
native library, changes the120a-to120 metadata, then updates both product
digests using the native composer's canonical file-tree algorithm. Host/library
bytes, ABI0.1.71 and release0.76.1 remain unchanged. This preserves integrity
enforcement. The image build now runs `tests/qualify_product_integrity.py` with
no network, credentials, certificate or weights. A deliberately synthetic GPU
profile selects the bundle in the CPU build container; reaching the expected
driver-library load failure proves native product checks passed. This synthetic
profile is never used on a rental and is not CUDA/inference proof.

The corrected image's actual native startup qualification now passes the
product integrity gate and reaches the expected library-load failure in its
CPU-only build container. This check is part of Dockerfile.agent-update.
[Fourth-run reconciliation](../deploy/container/qwen38-authorized-reconciliation-fourth.json)
retains363585 micro-USD for47seconds plus full64GBtransfer and100000 extra
contingency; original allowance/journal remains. At22:23:00 UTC credit16148953
minus14897063 retained liabilities left1251890. The next finite allowance is
1200000, including1500seconds runtime,240cleanup and400000contingency. Native
download remains included; fresh preflight must still qualify the actual quote.

The user also authorized reconciling previously closed historical allowances.
[The fifth/historical receipt](../deploy/container/qwen38-authorized-reconciliation-fifth-and-historical.json)
retains485510 micro-USD for the612-second fifth run (full64GBtransfer plus100000
extra contingency) and201654 for already-destroyed historical57330173 (its
original full10GBtransfer, conservative original storage rate,64seconds,100000
extra contingency). Original journals and consumed claims remain unchanged.
At22:38:22 UTC complete inventories were empty and15994476 credit minus13868190
liabilities left2126286. The integrated repeat is capped at1900000, with1500
seconds runtime,240cleanup,1000000contingency and the same64GBtransfer bound.
