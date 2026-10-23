# m8s — Roadmap v1

Status: v1 (2026-10-20), rough and expected to change · Owner: Metal-0 · [Charter](agor://kb/document/01a15b4b-ef89-75c0-a303-99bca9255a42) · [Architecture](agor://kb/document/01a15b4f-6f37-76b2-8ac8-e3c1be618392) · [Cost model](agor://kb/document/01a15b4f-b222-7153-ab6f-b55ace211d3e)

```
 P0 Prove it by hand ──▶ P1 Control plane ──▶ P2 Scale + evals ──▶ P3 Privacy + audit ──▶ 🎬 Demo ──▶ P4 Later
   (~$6 spend)            (≈$0–2)              (≈$8)                 (≈$2)                 (≈$2)
```

Spend estimates are against the $20/month budget. **The live frontier test is deferred** to P4 (Admin, 2026-10-20) ([cost model](agor://kb/document/01a15b4f-b222-7153-ab6f-b55ace211d3e)).

## P0: Prove it by hand

Goal: one rented node joins the private mesh and serves Goose, and we show it dies when the owner disappears.

- Repo bootstrap: first commit, workspace layout, register the repo in Agor ([ADR-0001](agor://kb/document/01a1576c-8eee-73e9-806c-d99a0e071e05)).
- **MP-001 (blocker):** key-less node attestation on the mesh-llm `m8s` branch ([brief](agor://kb/document/01a15c1c-ac26-7000-970b-de909358ba57), [patch queue](agor://kb/document/01a15c1a-14c0-7750-9cc2-2ede1ad0ad25)).
- `m8s-node` image v0: CUDA base, mesh-llm built from the `m8s` branch, minimal `m8s-agent` (bootstrap, supervise, heartbeat, DMS through `CONTAINER_API_KEY`).
- `m8s-bootstrap` on the owner: `m8s-boot/1` and `m8s-control/1` over iroh ([ADR-0006](agor://kb/document/01a15b4d-b4ae-765a-ba27-4491ae240bf9)).
- Minimal Vast client: offer survey (free), launch, destroy, labels.
- **Live spikes:**
  - iroh/UDP behaviour behind Vast port mapping;
  - model download speed against advertised bandwidth;
  - `CONTAINER_API_KEY` self-destroy;
  - multi-GPU use on one host (2x).
- P0 models: 27B class and 70B class. Compare replica layouts (b) and (c) ([ADR-0016](agor://kb/document/01a15b4e-cba5-72d2-8b24-019d3dc26fec)).
- Model profile catalog v0 ([ADR-0017](agor://kb/document/01a15c08-e7bf-701e-a0e4-d203a169c2c2)).
- **Exit criteria:**
  - a 27B-class node and a 70B-class node each join, serve, and complete a Goose task over the mesh;
  - `CONTAINER_API_KEY` permits **destroy** (tested on a throwaway instance);
  - the DMS fires with the network pulled, and every box ends up **destroyed, not stopped** ([ADR-0007](agor://kb/document/01a15b4e-5bfc-72c9-a718-77a74f349922));
  - `m8s down` leaves zero billable resources.

## P1: Control plane

- `Provider` trait plus `vast-community` and `vast-secure` adapters ([ADR-0004](agor://kb/document/01a15b4c-e18c-7445-a243-d3f2a4985da2)); placement and budget policy ([ADR-0005](agor://kb/document/01a15b4c-e1bf-7475-a333-a7ae1c135dc0)).
- `m8s-core` reconcile, orphan sweep, `m8s.toml` schema v1 ([ADR-0009](agor://kb/document/01a15b4d-b51a-7030-8be7-0c5368cbff72)).
- `SecretStore` with Keychain ([ADR-0012](agor://kb/document/01a15b4e-5c77-7297-bffc-cbcf31a63efc)).
- CLI with `--json` (`up/down/scale/status/logs/secrets`).
- `m8sd` daemon with SQLite over a Kubernetes-free `m8s-core`; outage recovery with the 3/h re-rent limit; `m8s top` ([ADR-0008](agor://kb/document/01a15b4d-b4f9-7254-adfb-37d41a4297e1)).
- **Exit criteria:** `m8s up <pool>` and `m8s down` are declarative and survive an `m8sd` restart; `m8s top` shows nodes live.

## P2: Scaling, evals, cost model

- Schedule and queue triggers, plus per-tier cold behaviour.
- `m8s eval`: a throughput sweep over N (mesh-llm tooling plus GuideLLM), BFCL, an Aider polyglot subset, a FLORES subset, and 4 canonical Goose/pi tasks per tier ([ADR-0014](agor://kb/document/01a15b4e-cb60-7391-baf0-388348c9db02), [S4](agor://kb/document/01a15b4e-43ee-7461-9bea-17f0edf8e5b3)).
- `Harness` trait: Goose and pi as subprocesses.
- TUI (`m8s top`), web dashboard, Prometheus/Grafana/OTel, Alerter ([ADR-0015](agor://kb/document/01a15b4e-cb84-7557-9c37-1faf9857c247)).
- **Exit criteria:** the cost model is filled with measured throughput for small and medium.

## P3: Privacy and security

- Hardened image: read-only rootfs, tmpfs, no swap or core dumps, analytics off, egress allowlist.
- cosign keyless signing, SBOM, digest pinning, `--require-release-attestation`.
- Audit suite A1–A10, a signed `m8s.audit/v1` report, and effective ZDR levels plus tags ([ADR-0011](agor://kb/document/01a15b4e-5c46-729d-bb8f-7b1aaf99793a), [S3](agor://kb/document/01a15b4c-9887-7452-a281-858ce1d16477)).
- **Exit criteria:** every node gets an audit report, and the docs make no overclaims.

## 🎬 Demo (after P3)

The orchestrator records its own cmux walkthrough. See the [demo plan](agor://kb/m8s/plans/demo-p3.md).

## P4: Later

- Payments: mock `wallet.v1` wallet, then a selling experiment ([ADR-0013](agor://kb/document/01a15b4e-cb32-77ff-afa6-bb74960a1048)), after a deep-dive thread with Admin.
- An L3 confidential-computing adapter (GCP A3 CC, Azure H100 CC, Phala) with attested node keys.
- Frontier tier at scale (≥10 Gbps hosts, a weights-cache strategy), and scale-up via NVLinked multi-GPU ([ADR-0016](agor://kb/document/01a15b4e-cba5-72d2-8b24-019d3dc26fec)).
- More providers (RunPod, Lambda, AWS, GCP, Databricks), more alert sinks, and an operator agent driving the CLI.
- Optional kube mode: a kube-rs `m8s-operator` as a native host process, plus k9s views ([ADR-0008](agor://kb/document/01a15b4d-b4f9-7254-adfb-37d41a4297e1) option A).
- An upstreaming proposal to mesh-llm (m8s as a crate, plus fixes for the gaps in architecture §7).
