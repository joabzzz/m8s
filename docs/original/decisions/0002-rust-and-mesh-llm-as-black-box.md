# ADR-0002 — Rust, with mesh-llm consumed as an unmodified dependency

Status: **accepted, amended 2026-10-20** by the [ADR-0006](agor://kb/document/01a15b4d-b4ae-765a-ba27-4491ae240bf9) decision · Decided by: Admin · Recorded by: Metal-0 · Date: 2026-10-20

> **Amendment (Admin, 2026-10-20).** mesh-llm is no longer strictly unmodified. Small, upstream-shaped patches that m8s needs are stacked as commits on the **`m8s` branch** of `~/Development/m8s/mesh-llm`. Each one links to its ADR and is tracked in the [mesh-llm patch queue](agor://kb/document/01a15c1a-14c0-7750-9cc2-2ede1ad0ad25) as a future upstream PR. The m8s-node image runs mesh-llm built from that branch, and the owner-side path deps point at it. Procedure: [SOP §5a](agor://kb/document/01a1575b-167a-721a-bb44-4562f6f36351). Larger forks remain out of scope.

## Context

m8s has to drive mesh-llm nodes. That means minting join material, reading status and controlling models. mesh-llm is a Rust workspace with an embeddable SDK (`mesh-llm-sdk`: `MeshNode::builder()…start()`, `invite_token()`, `status()`). Admin is a volunteer on the mesh-llm dev team, and m8s may later become a crate inside mesh-llm.

## Options

1. Rust, embedding `mesh-llm-sdk` and the CLI binary as-is.
2. Rust, with a fork of mesh-llm that adds the missing features (GPU Dockerfile, token mint API, `/v1` auth).
3. Another language that shells out to the `mesh-llm` CLI.

## Decision

Option 1.

- m8s is written in **Rust** (edition 2024, crate at `~/Development/m8s/m8s`).
- The m8s **binary** on the owner side may depend on mesh-llm crates (SDK, membership types) as path dependencies on `~/Development/m8s/mesh-llm`, pinned to a known commit.
- The m8s-node **image** runs the released `mesh-llm` binary, with our own config and entrypoint.
- **No changes to mesh-llm during the prototype.** Any gap is worked around inside m8s and logged in the upstream wishlist in [architecture v0](agor://kb/m8s/designs/architecture-v0.md) §Upstream gaps.

## Consequences

- Workarounds we expect:
  - a sidecar agent for authenticated control ([ADR-0006](agor://kb/m8s/decisions/0006-bootstrap-and-control-channel.md));
  - our own CUDA image;
  - token minting through an embedded seed node.
- Keep the m8s crate layout close to mesh-llm conventions (workspace crates, `just`) so that moving it in later is cheap.
- If mesh-llm's internal APIs churn, pinned path deps will break. Pin a commit and bump it deliberately.
