# ADR-0016 — Each served model fits on one rented host; scale up with NVLinked GPUs, scale out with replicas

Status: **proposed** (pending spikes [S1](agor://kb/m8s/research/spike-s1-vast-provider-and-costs.md) and [S2](agor://kb/m8s/research/spike-s2-mesh-llm-integration.md), then a live test in P0) · Date: 2026-10-20

## Context

mesh-llm can split large models across nodes (Skippy stage splits). Its own docs say Vast NAT hairpinning pushes co-located containers onto a ~200 ms relay. Benchmarks drop from 68 tok/s on one node to 12 tok/s across three. Admin's scaling picture: "spin up k containers, or prepare another instance with k GPUs NVLinked".

## Proposed decision

- **No cross-host splits on rented hardware.** Each provisioned node serves models that fit entirely in that host's VRAM, with headroom for KV cache.
- **Scale up** means a bigger host: more GPUs, NVLink, single machine. This is how the frontier tier works (for example 8x H100/H200).
- **Scale out** means more replicas of a tier, and mesh routing spreads requests.
- Splits remain allowed between the owner's *own* LAN machines. m8s simply doesn't create them.

## Open questions for the spikes

- How well does mesh-llm/Skippy use 8 GPUs inside one host (layer split vs tensor split)?
- Do Vast offers expose NVLink topology?
- Which frontier models fit in 640 GB and 1.1 TB at usable quantization?

## Update from S2 (2026-10-20)

[S2](agor://kb/document/01a15b51-c388-76f8-b6ed-8f10d51fdb83) found the following:

- `split_mode` and `main_gpu` are wired. `tensor_split` and `placement` are documented but **not wired**.
- Nothing documents 8-GPU behaviour.
- MiniMax M2.7 is supported.
- DeepSeek V3 is supported only through package-backed stages.
- The **ds4 plugin for DeepSeek V4 is Apple Silicon only**.

The frontier tier on Vast therefore needs a live multi-GPU test (2x first) before we commit to 8x hosts. The leading frontier candidates are MiniMax M2.7, GLM-5.x and Kimi K2.x, pending llama.cpp support checks.

## Admin direction (2026-10-20)

**The live frontier test is deferred.** P0 uses popular, well-performing mid-size models: the Qwen 27B class and strong 70B-class dense models.

Prefer **one download, several replicas, KV headroom** over one big model. There are three replica layouts, and P0 measures throughput at N for each:

```
 (a) one host per replica     (b) multi-GPU host, one replica   (c) one big GPU, one replica,
                                  per GPU, model file shared        many parallel slots + big KV
 ┌────┐ ┌────┐ ┌────┐          ┌──────────────────────┐          ┌──────────────────────┐
 │GPU │ │GPU │ │GPU │          │ GPU0  GPU1  GPU2  GPU3│          │ H100 80GB            │
 │ M  │ │ M  │ │ M  │          │  M     M     M     M  │          │  M + KV for N seqs   │
 └────┘ └────┘ └────┘          └──── one download ─────┘          └──── one download ────┘
 3 downloads, 3 bills          1 download, N mesh nodes           1 download, 1 mesh node
```

- Layout (b) runs one mesh-llm process per GPU (`CUDA_VISIBLE_DEVICES`), each with its **own node key**. Nodes that share a key collapse into a single peer.
- The m8s placement planner ([ADR-0017](agor://kb/m8s/decisions/0017-model-profiles-not-mandated-models.md)) computes replicas and KV headroom for each layout.

## Consequences

- The frontier tier is the most expensive and scarcest. It needs a solid cold-start story and a strict budget.
