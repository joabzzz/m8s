# ADR-0014 — Evals: reuse mesh-llm tooling; throughput at N concurrent users first; on-demand only

Status: **accepted** · Decided by: Admin (metrics, suites, reuse), Metal-0 (shape) · Research: [S4](agor://kb/m8s/research/spike-s4-harnesses-evals-orchestration.md) · Date: 2026-10-20

## Context

The goal is to learn how much compute a rented node adds to a mesh. Users decide what to run. mesh-llm has a team optimizing its eval tooling (agentic replay-matrix, `mesh-llm benchmark tune`), and Admin wants consistency with it.

## Decision

- **Primary metric: aggregate throughput (output tok/s) at N concurrent users**, swept over N in {1, 4, 8, 16, 32, …} until saturation. TTFT p50/p95 and $/1M output tokens are recorded alongside it.
- **Reuse mesh-llm eval tooling** for throughput and replay. m8s adds only orchestration: provision → run → collect → tear down. Results go into the [cost model](agor://kb/m8s/plans/cost-model.md).
- **Quality suites, all open source:**

| Suite | Measures | Tier focus |
|---|---|---|
| BFCL | tool calling | small, medium |
| Aider polyglot and/or SWE-bench Lite | coding | medium, frontier |
| FLORES-200 (subset) | translation | small |
| Canonical agent tasks (Goose/pi): web search, doc search, translation, build Pong from scratch | end to end | per tier |

- **On-demand only** (`m8s eval run --tier medium --suite throughput,bfcl`). No continuous canaries. Spot-checking whether a node lies about its model is a mesh-level problem.
- Evals run **from a mesh client** ([ADR-0010](agor://kb/m8s/decisions/0010-inference-over-the-mesh.md)), so results include real mesh transport overhead.

## Update from S2 (2026-10-20)

The concrete reusable throughput tool is mesh-llm's `scripts/run-llama-benchy-openai.sh`. It targets any `/v1` endpoint, including the owner's mesh-routed loopback, so it measures real mesh transport ([S2](agor://kb/document/01a15b51-c388-76f8-b6ed-8f10d51fdb83)).

`replay-matrix` and `benchmark tune` launch their own local servers. They are useful for per-node baselines inside the container, not for the mesh-level number. GuideLLM is a secondary option for N-concurrency sweeps.

## Consequences

- Eval runs cost rental time. Each run has a budget estimate shown before it starts, and it counts against the monthly cap ([ADR-0005](agor://kb/m8s/decisions/0005-placement-policy.md)).
- Full SWE-bench is too expensive for the $20 budget. Use small, fixed subsets until the budget grows.
