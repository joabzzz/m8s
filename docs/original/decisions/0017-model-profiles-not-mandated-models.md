# ADR-0017 — m8s doesn't pick models; it describes them with model profiles

Status: **accepted** · Decided by: Admin ("this framework shouldn't choose, but we might want to have properties for different models"), shape by Metal-0 · Date: 2026-10-20

## Context

The kickoff draft tied each tier (small, medium, frontier) to specific models. Admin doesn't want the framework choosing models. Owners choose, and m8s should make that choice easy and safe by describing each model's properties.

## Decision

- **Tiers are owner-defined, named pools.** Each pool is a hardware spec plus a model reference ([ADR-0009](agor://kb/document/01a15b4d-b51a-7030-8be7-0c5368cbff72)). m8s ships **example presets**, not mandates. `small`, `medium` and `frontier` are just example names.
- **Model profiles** are data, not code. They live in a catalog file that users can extend (`models/*.toml`). m8s uses them to plan placement and to check whether a model fits.

```toml
[model."qwen3.6-27b-q4km"]
source        = "hf://<org>/<repo>@<rev>"   # pinned revision
family        = "qwen3"
params_b      = 27
active_params_b = 27            # MoE: active per token
quant         = "Q4_K_M"
file_gb       = 16.5
kv_mb_per_1k_tokens = 64        # per sequence, for KV planning (measured or estimated)
max_context   = 131072
capabilities  = ["tools", "reasoning"]   # tools | vision | audio | reasoning | code
license       = "apache-2.0"
license_class = "permissive"    # permissive | restricted | non-commercial
runtime       = { engine = "mesh-llm/skippy", status = "certified" }  # certified | community | untested
notes         = "Popular all-rounder; good tool calling."
```

- **Placement planner** (fit check, never a recommendation engine). Given a host offer and a profile, it computes:
  - how many replicas fit: one per GPU, or several per GPU if they fit;
  - KV-cache headroom per replica, expressed as concurrent sequences at a target context length;
  - estimated download time and cost ([cost model](agor://kb/document/01a15b4f-b222-7153-ab6f-b55ace211d3e)).
- Evals ([ADR-0014](agor://kb/document/01a15b4e-cb60-7391-baf0-388348c9db02)) write **measured** numbers back into a profile's sidecar: tok/s at N, real KV usage, quality scores. Over time the catalog carries real data instead of estimates.
- **Licence and runtime status are visible but not enforced.** The owner decides; the CLI warns on `untested` or `restricted`.

## Consequences

- The catalog becomes a shareable community asset, much like provider docs ([ADR-0004](agor://kb/document/01a15b4c-e18c-7445-a243-d3f2a4985da2)). Agents can read it to make suggestions.
- Tier-routing hints in model names ([ADR-0011](agor://kb/document/01a15b4e-5c46-729d-bb8f-7b1aaf99793a)) come from the pool name, not the profile.
