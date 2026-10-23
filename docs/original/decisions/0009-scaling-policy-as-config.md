# ADR-0009 — Scaling is owner-defined config; v0 triggers are manual, schedule and queue

Status: **accepted** · Decided by: Admin (direction), Metal-0 (shape) · Date: 2026-10-20

## Context

The right shape for scaling is unknown. It depends on how the org trades KV cache against model size, and on how many users connect. "A mesh node is going to know what they need best." Waiting for cold start is acceptable, even a feature: spin up for your timezone and set a close time. Start users with the *minimum aggregate compute* needed to get an agent going.

## Decision

- Each owner node has an **m8s config** (`m8s.toml`, mirrored into CRs if [ADR-0008](agor://kb/m8s/decisions/0008-orchestration-substrate.md) A is accepted). It declares tiers, schedules, triggers and cold behaviour. m8s ships opinionated **defaults**, never hard-coded policy.
- **v0 triggers:**
  1. **manual** (`m8s up/down/scale`);
  2. **schedule** (cron-like open and close windows, in the owner's timezone);
  3. **queue** (scale up when mesh-reported queue wait or in-flight requests for a tier's model exceed a threshold for a period, and scale down after an idle period).
- **Cold behaviour per tier.** What a caller experiences when the tier is at zero: `wait` (block until ready, with a timeout), `reject` (fail fast), or `fallback:<tier>` (route to a smaller tier's model). Note that routing happens in mesh-llm by model name, so fallback means *the client or harness* retries with another model. m8s exposes the hint.
- **Scale axes** (both expressible in config; v0 implements only replicas):
  - **out:** more replicas of the same tier;
  - **up:** a bigger single host (k NVLinked GPUs) for a bigger model or more KV cache ([ADR-0016](agor://kb/m8s/decisions/0016-one-model-per-host.md)).

```toml
# m8s.toml (sketch)
[budget]
monthly_usd = 20
max_hourly_usd = 3.0

[dead_mans_switch]
ttl = "1h"
max_lifetime = "12h"

[tier.medium]
provider = ["vast-secure", "vast-community"]   # preference order
gpu = { min_vram_gb = 80, count = 1 }
model = "Qwen3-Coder-Next-Q4_K_M"
replicas = { min = 0, max = 2 }
cold = "wait:15m"
interruptible = false

[[tier.medium.schedule]]
open = "0 8 * * 1-5"     # 08:00 local, weekdays
close = "0 18 * * 1-5"

[tier.medium.queue]
scale_up_when = "queue_wait_ms > 2000 for 5m"
scale_down_when = "inflight == 0 for 30m"
```

## Consequences

- The config schema is a public contract, so version it (`schema = 1`) from the start.
- The queue trigger depends on mesh-llm `/api/status` routing metrics or OTLP. The signal source is confirmed in [S2](agor://kb/m8s/research/spike-s2-mesh-llm-integration.md).
