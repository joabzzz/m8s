# m8s — Cost model v0

Status: v0, built from desk research ([S1](agor://kb/document/01a15b4e-7d4e-740a-9a6d-e82f68e678ae)). Prices are third-party ranges, **not live quotes**. Throughput is unknown until P2 evals. · Owner: Metal-0 · 2026-10-20

## Formulas

```
cold_start_cost   = $/h × (image_pull + model_download + load) / 3600     # Vast doesn't bill while 'Loading', but
                                                                          # bills storage + bandwidth in every state
model_download_s  ≈ model_GB × 8 / effective_Gbps                         # assume ~70% of advertised inet_down
session_cost      = cold_start_cost + $/h × serving_hours + bandwidth_GB × $/GB
break_even_$per1M = ($/h × 1e6) / (throughput_tok_s@N × 3600 × utilization)
hours_per_budget  = budget / $/h
```

## Desk price ranges (on-demand, Vast; from S1)

| Config | $/h range | Tier | Hours per $20 (midpoint) |
|---|---|---|---|
| 1x RTX 4090 (24 GB) | 0.15–0.45 | small | ~57 h |
| 1x A100 80 GB | 0.8–1.5 | medium (low) | ~16 h |
| 1x H100 80 GB | 1.5–2.5 | medium | ~10 h |
| 2x H100 | 3–5 | medium (large) | ~5 h |
| 8x H100 | 14–20 | frontier | ~1.2 h |

## Cold start: the frontier problem

| Model size | @ 1 Gbps | @ 10 Gbps | Cost on 8x H100 @ ~$17/h |
|---|---|---|---|
| 20 GB (small) | ~4 min | <1 min | — (on a 4090: ~$0.02) |
| 50 GB (medium) | ~10 min | ~1 min | — (on an H100: ~$0.35) |
| 150 GB | ~30 min | ~3 min | — |
| 400 GB (frontier) | **~90 min** | ~8 min (+load ≈ 25 min total) | **~$25 at 1 Gbps**; ~$7 at 10 Gbps |

**Implication:** with a $20/month budget, one frontier cold start on a 1 Gbps host costs more than the entire budget. The frontier tier needs:

- a **much higher bandwidth floor** (≥10 Gbps), and
- **either** a larger budget **or** a deferred live frontier test (desk-validate only in P0).

**Admin decision (2026-10-20):** the live frontier test is deferred. P0 spends on mid-size models instead (see below).

## Throughput side (filled in P2)

| Tier | Model | tok/s @N=1 | @N=8 | @N=32 | Saturation N | $/1M out tok @50% util |
|---|---|---|---|---|---|---|
| small | TBD | | | | | |
| medium | TBD | | | | | |
| frontier | TBD | | | | | |

## P0 spend plan v2 (≈ $8 of $20; download once, many replicas)

0. **Free:** query offers to fill a real price table. Record how many offers meet the 1 Gbps floor, and the measured vs advertised download speed.
1. **27B class** (e.g. Qwen 27B Q4_K_M, ~17 GB, ~3 min at 1 Gbps):
   - **Layout (b):** a 2x 4090/3090 host with 2 replicas and one download, for ~2 h. **≈ $1–2**
   - **Layout (c):** compare on the same host with 1 replica and many parallel slots.
   - Also covers the bootstrap, a Goose task and the DMS test.
2. **70B class** (a strong dense 70B at Q4_K_M, ~40–43 GB, ~7 min at 1 Gbps):
   - **Layout (c):** 1x 80 GB (A100/H100) with 1 replica and ~35 GB of KV headroom, **or**
   - **Layout (b):** 2x 48 GB with 2 replicas.
   - ~2 h, with a throughput sweep at N and a BFCL subset. **≈ $3–5**
3. **Reserve ≈ $10** for retries and the P2 evals. **Frontier is deferred** ([ADR-0016](agor://kb/document/01a15b4e-cba5-72d2-8b24-019d3dc26fec)).

The headline output is a table of **aggregate tok/s at N, by layout (a/b/c), × $/h**. It answers whether many smaller replicas beat one big KV pool for our workloads.

## Spend safety

- Use prepaid Vast credit, with no card and autobilling off, as the hard cap (S1). m8s budget guards ([ADR-0005](agor://kb/document/01a15b4c-e1bf-7475-a333-a7ae1c135dc0)) and the dead man's switch ([ADR-0007](agor://kb/document/01a15b4e-5bfc-72c9-a718-77a74f349922)) sit on top.
- Always **destroy**, never stop. Stopped instances still bill storage.
