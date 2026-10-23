# Qwen3.8 27B demo preparation

Issue [#6](https://github.com/joabzzz/m8s/issues/6) remains open. This is a
repeatable preparation procedure for a candidate live run, **not a successful
Qwen3.8 demo receipt**. The normal `m8sd` controller still uses development
providers. `m8s up`, `scale`, `status`, `logs`, and `down` do not currently start
this paid model. The explicit prototype lifecycle below does real provisioning,
certificate verification, inference and destruction when its prerequisites pass;
it does not satisfy the requested daemon-command acceptance on its own.

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

Keep the native ABI, ownership certificate checks and certified owner-client fix.
Do not replace the embedded runtime with an unrelated llama-server.

## Candidate hardware and scope

Choose Vast on-demand, one SM120 Blackwell GPU with at least 48,000 MiB VRAM,
64,000 MiB host RAM, 64 GB ephemeral disk and advertised download >=1,000 Mbps.
Compute must be <=1,500,000 micro-USD/hour; storage and transfer are additional.
The existing SM120 CUDA build recipe requires CUDA driver support >=12.8.
The 48 GB candidate leaves headroom above the 15.33 GiB weights for native
buffers and cache. It is a conservative selection, not measured VRAM proof.
[Unsloth's sizing guide](https://unsloth.ai/docs/models/qwen3.8) describes smaller
4-bit configurations; this demo does not claim those configurations were tested.

Native-download launches explicitly use `--ctx-size 4096 --parallel 1`. Do not
claim a 262K context demo. Neither multiple GPUs nor multiple nodes are selected;
split serving is unqualified. Text only: no vision projector is downloaded, no
image request is sent. The completion request disables thinking through
`chat_template_kwargs.enable_thinking=false` to reserve its 48 output tokens
for an answer. Verify that behavior against the selected runtime during native
qualification.

## Preparation and bounded run

1. Build from the stacked source with Rust 1.98. All local Cargo/native/container
   build commands in the coordinated session go through the shared helper:

   ```sh
   python3 /Users/sandro/Development/m8s/m8s-session-control/with-build-lock.py cargo build --locked -j 2 -p m8s-cli --features prototype-vast
   python3 /Users/sandro/Development/m8s/m8s-session-control/with-build-lock.py cargo build --locked -j 2 -p m8s-provider --features vast-node
   ```

   Preserve the model-free certified Mac owner client from the proven release,
   or build its pinned compatible successor outside the shared companion checkout.
   Prepare a Linux AMD64 CUDA image **before renting** using
   `deploy/container/Dockerfile.flint`, with
   `--build-arg MODEL_CONTRACT=qwen38-model-artifact.json`. The filename is
   historical; the recipe packages an SM120 runtime without weights. Use reviewed
   native and m8s source archives and record their hashes and the resulting image
   digest. Put only explicit public source inputs in the build context, never
   `.env`, run directories, owner keys or credentials. Qualify the packaged native
   runtime and exact GGUF locally/on already authorized hardware first. No
   qualified Qwen3.8 CUDA image is recorded yet; do not reuse the smoke image.

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
   with the coordinator. No concrete new run budget has been admitted yet.

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
   provisioning and loading. At 1,000 Mbps the weights alone have an ideal lower
   bound of about 132 seconds; no throughput guarantee follows from that rate.
   The prototype reserves a disk-sized transfer allowance; disk capacity alone
   does not bound cumulative traffic, upstream retries or image-pull charges.
   Establish an authoritative worst-case transfer bound (or enforce one) before
   admitting a paid run. Include storage for the entire runtime plus cleanup,
   independent cleanup, contingency and unsettled previous liabilities. Reserve
   at least 240 seconds for cleanup within the 1,800-second attempt ceiling.
   Refuse if the available credit cannot cover all retained and new liability.

4. After qualification and budget admission, run a fresh plan exactly once:

   ```sh
   target/debug/m8s prototype launch --plan /absolute/private/new-qwen38-plan.json
   ```

   Expected stage output includes cleanup armed **before CREATE**, exact-resource
   binding, listener progress, certificate verification, discovery of the exact
   served hash, owner-routed completion, and confirmed removal. A listener
   acknowledgment or `/v1/models` entry alone is not Ready. The final receipt
   must contain a sensible actual answer about mesh networks; do not replace it
   with canned expected text. The independent watchdog must survive owner loss.
   The node has its existing heartbeat/lifetime limits. Neither is a substitute
   for verified provider destruction. Do not retry an uncertain CREATE.

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
- Connect the production daemon lifecycle commands to the admitted live flow;
  the prototype one-shot is not `m8sd` + `up`/`scale`/`logs`/`down` acceptance.
- Admit the finite budget after all retained liabilities and native-download
  costs, arm independent cleanup, execute once and verify teardown.

[Sanitized preparation evidence](../deploy/container/qwen38-preparation-evidence.json)
records what was actually inspected. No Qwen3.8 resource was rented by the
preparation steps, so there is no new exact-resource teardown receipt or claimed
live completion. The historical small-model proof remains separate.
