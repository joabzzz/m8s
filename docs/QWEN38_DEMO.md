# Qwen3.8 27B live demo

On 2026-10-23, **m8sd + m8s up/scale/status/logs/down** served the exact model
through mesh-llm on Vast instance **57433800**. The automatic owner-routed canary
returned a sensible completion and the coordinator exited successfully after
provider-confirmed destruction. Fresh full inventories contained no instances
or volumes. See the [live proof](../deploy/container/qwen38-live-proof.json) and
[final account observation](../deploy/container/qwen38-final-account-evidence.json).

> A mesh network is a distributed network where each device acts as both a host
> and a relay, allowing data to travel along multiple paths for improved
> connectivity and redundancy.

This uses the real, opt-in `m8sd --vast-demo` controller. The default controller
remains a development controller. This mode admits one bounded attempt and one
replica; it does not implement fleet replacement or production readiness.

## Pinned model and runtime

The model is [Qwen/Qwen3.8-27B](https://huggingface.co/Qwen/Qwen3.8-27B/tree/1d4bf0f2ff6012fd82039f2fa52739d0dd7c60c0),
Apache 2.0, dense 27B. Its `qwen3_5` architecture is supported by the selected
runtime. This demo is text-only, context 4096, parallel 1, no draft model; vision,
262K context and distributed splits are unqualified.

| Selection | Exact value |
| --- | --- |
| Plan model selector | `qwen3.8-27b` |
| Control tier | `qwen38-27b` |
| GGUF repository revision | `unsloth/Qwen3.8-27B-GGUF@4ca720788d1e01f1bff70c033e0d0028fd02e502` |
| File | `Qwen3.8-27B-UD-Q4_K_M.gguf` — 16,464,440,224 bytes |
| Raw GGUF blob SHA-256 | `322e194ff79741c7baa497c240f677f54b201b0efab44ca8e50f122b39123482` |
| OpenAI discovery/request ID | `unsloth/Qwen3.8-27B-GGUF@4ca720788d1e01f1bff70c033e0d0028fd02e502:UD-Q4_K_M` |
| Verified response/content ID | `local-gguf/sha256-7756e8943d5ec98b1cd76895d33d20b8c8bf7609a71540fc9b6fa512bdedd0de` |

The native HF identity hashes canonical repository, commit and ordered
file/path/size/blob-hash metadata. It differs from the raw GGUF hash. The CLI
checks the exact discovery alias and the exact response identity separately;
it never routes this demo through the generic `mesh` alias. The
[identity regression](../deploy/container/tests/test_qwen38_identity.py) binds
these values to the actual verified owner canary. The full payload was not
independently rehashed by the operator.

Use the immutable image:

```text
ghcr.io/joabzzz/m8s-prototype@sha256:5266fe6e7e824cd4e343b084d74c9b251cdec130d12b72fbb8349f940f8048a6
```

The [image receipt](../deploy/container/qwen38-product-image-qualification.json)
records native source 962aeb2faabe74f5fa809b8b61eaad2f9b816e18,
llama.cpp a37cc46104eb50900d5ab4a9ba02a0abdac039db, patched revision
6085189ee2b42cad0a296bebdab486cae3a58dbe, runtime 0.76.1 and ABI 0.1.71.
The Mac owner uses the certified model-free client at
7a101840af79c7509789b9e3152ca6c4605186e8, SHA-256
48ca217a81c77a4bc7f40d8729e9324a0d6e32d5b5e3bcc7c64082415757d8cb.
Do not substitute a release package with a different ABI.

A rental downloads weights through mesh-llm's native Hub/cache path. The image
contains native binaries, not Qwen weights. No Rust/CUDA compilation occurs on
the rental. Reuse the qualified image for unchanged pins; agent updates use
`deploy/container/Dockerfile.agent-update`, preserving compiled native libraries.
The build verifies original host/library/tree hashes, normalizes 120a-to-120 probe
metadata, updates both enclosing product digests and exercises the actual native
product integrity gate. CPU-only qualification is not GPU inference proof.

## Prerequisites and finite budget

- One on-demand SM120 RTX PRO 5000/6000 node, at least 48,000 MiB VRAM,
  128,000 MiB host RAM, 64 GB ephemeral disk, advertised download >=500 Mbps and
  CUDA-driver support >=12.8. A single 48 GB GPU node returned the first real
  canary; peak VRAM usage was not measured. Smaller-RAM host failures remain
  unresolved, so automatic selection follows the proven larger-RAM profile.
- Direct UDP mappings and host-key-pinned SSH, certificate-required native
  client/server and the owner signer. Keep canonical iroh TLS and trust checks.
- Explicitly selected credential file containing `VAST_API_KEY`. This session
  used `/Users/sandro/Development/m8s/.env`; never source it or put its value in
  argv, logs, state, build contexts or commits. Tests use synthetic credentials.
- Fresh authoritative credits and complete instance/volume inventories, exact
  absence of prior rentals, and an allowance after all remaining liabilities.
  Historical snapshots are not current funds or renewed authorization.

The profile budgets compute/storage through runtime plus at least 240 seconds
cleanup, contingency, image pull and native download. Its 64 GB transfer reserve
covers two full 16.465 GB paths plus <13 GB unpacked image and >18 GB overhead; Hub/Xet
retries and automatic draft download are disabled. Storage is normalized from
USD/GB/month using a conservative 28-day month and upward rounding. Runtime plus
cleanup cannot exceed 1800 seconds. Independent cleanup must be armed before CREATE.

The user explicitly authorized audited conservative reconciliation of destroyed
runs instead of retaining full launch allowances. The original journals and
consumed claims remain unchanged; [reconciliation receipts](../deploy/container/qwen38-authorized-reconciliation-sixth.json)
retain uncertain exposure and do not invent final billing. Never reset a claim,
replay an uncertain CREATE, or launch overlapping rentals.

## Repeat the controlled flow

1. Obtain the pinned owner client from the [proven release](https://github.com/joabzzz/m8s/releases/tag/prototype-proof-2026-10-23)
   or its qualified source build; verify its hash. Build the local controller/CLI.
   In the coordinated development workspace, every Cargo/build/check command
   uses the shared helper (Rust 1.98, max two jobs, no alternate target directory):

   ```sh
   python3 /Users/sandro/Development/m8s/m8s-session-control/with-build-lock.py cargo build --locked -j2 -p m8s-cli -p m8sd --features m8s-cli/prototype-vast,m8sd/prototype-vast
   ```

2. Read the [fresh-run procedure](../.agents/skills/m8s-demo/references/run-demo.md)
   and [CLI contract](../crates/m8s-cli/README.md#one-attempt-vast-daemon-control-explicit-prototype-vast).
   Make a read-only fresh qualification using the selected file/account:

   ```sh
   target/debug/m8s prototype vast-preflight --credential-file /absolute/selected.env --account ACCOUNT_ID --disk-gb 64 --model qwen3.8-27b
   ```

   Copy the [inert plan](../.agents/skills/m8s-demo/assets/qwen38-plan.template.json)
   outside Git into an owned private directory. Fill fresh dates, account,
   unique owner/node/request IDs, owner-client path, image digest, current
   reconciled baseline and finite budget. Set `model` to `qwen3.8-27b`, `tier`
   to `qwen38-27b`, and `control_directory` to a fresh absolute private path.
   The attempt `directory` is separate and must not exist. Zero template caps
   and unknown liability deliberately refuse. Freeze these inputs before up.

3. Start the daemon and issue real control commands from another terminal:

   ```sh
   target/debug/m8sd --vast-demo --state-dir /absolute/private/CONTROL --plan /absolute/private/plan.json --m8s /absolute/target/debug/m8s
   target/debug/m8s --socket /absolute/private/CONTROL/control.sock up qwen38-27b --json
   target/debug/m8s --socket /absolute/private/CONTROL/control.sock scale qwen38-27b 1 --json
   target/debug/m8s --socket /absolute/private/CONTROL/control.sock status --json
   target/debug/m8s --socket /absolute/private/CONTROL/control.sock logs --json
   ```

   Status reports `mode=vast-prototype` and real retained journal state, never
   fresh provider absence or invented Ready. Scale 1 is idempotent; larger
   replicas refuse. Restart observes/request-down only, without CREATE replay.
   Image pull, native download and loading consume the original lease.

4. Wait for these log stages in order:

   ```text
   certificate-delivered-control-established
   owner-routed-model-observed
   owner-routed-inference-completed
   completion-held-until-down-or-deadline
   ```

   The coordinator sends one bounded mesh-network prompt through the model-free
   owner client. It validates the response model and nonempty completion. Logs
   expose only closed labels, not raw native output, tokens or private material.
   The answer remains withheld until provider-confirmed removal.

5. Stop through the same controller:

   ```sh
   target/debug/m8s --socket /absolute/private/CONTROL/control.sock down --json
   target/debug/m8s --socket /absolute/private/CONTROL/control.sock logs --json
   ```

   Down acknowledges a request, not absence. Wait for
   `exact-instance-absence-confirmed`, `cleanup-evidence.json` and the final
   `demo-evidence.json` in the private attempt directory. Require matching exact
   instance/image, `provider_confirmed_absent=true`, `certificate_verified=true`,
   `owner_routed_inference=true`, and a sensible completion. Check complete
   account instance/volume inventories again. Preserve journals, consumed claims
   and uncertain billing; publish only sanitized evidence. Stop only this
   controller after checking its process identity. A failure consumes its attempt;
   replacement requires prior exact absence and a new finite admitted plan.

The previous `qwen27` README label was a mock tier, not an upstream model mapping.
The mock example now uses `mock-demo`; it is not evidence for this live demo.
