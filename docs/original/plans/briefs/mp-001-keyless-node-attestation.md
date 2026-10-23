# Brief MP-001 — Key-less node attestation (mesh-llm patch)

Status: **Implemented/Sol-reviewed source snapshot; blocked on canonical import and runtime admission qualification** (2026-10-21). Reviewed clone commit `6b4dd72fd2445ca459caf495ae18b2fe3df9dd5a`, not on canonical `m8s`. [Review evidence](agor://kb/document/01a15cbc-6b00-7793-aae1-8cc71daa9ed1) · [Export redaction: worker session/branch links and worker/model roster omitted] · Owner: Metal-0 · Queue: [patch queue](agor://kb/document/01a15c1a-14c0-7750-9cc2-2ede1ad0ad25) · Decision: [ADR-0006](agor://kb/document/01a15b4d-b4ae-765a-ba27-4491ae240bf9) · Evidence: [S2](agor://kb/document/01a15b51-c388-76f8-b6ed-8f10d51fdb83) · P0 blocker

## Goal

A mesh-llm node that has **no owner keypair** can load a certificate pre-signed by the owner (the output of `mesh-llm auth sign-node`). The node verifies the certificate against its own endpoint id, reports itself as owner-**Verified**, and is admitted by peers running `--trust-policy require-owned` or `allowlist`.

This lets m8s admit a rented box without putting any owner private key on it. The Mac mini signs a short-lived certificate for each node during the sealed bootstrap.

## Where to work

| | |
|---|---|
| Repo | `<workspace>/mesh-llm` (local only; never push, never open upstream PRs) |
| Base | branch **`m8s`** at `9d97fbfa58c428fb95263a0de719bed959f2510a` (tag `m8s-base-2026-10-21`, upstream main 2026-10-20 "ci(plugins): register the capsules plugin ownership domain (#2449)") |
| Worker branch | `metal-0-1-mp-001-keyless-node-attestation` off `m8s`. Metal-0 reviews it and squashes it onto `m8s` as **one commit**. |

## Starting points (verify; from a quick grep)

- Cert signing: `mesh/crates/mesh-llm-commands/src/auth.rs` (`run_sign_node`, `sign_node_certificate`, `save_node_ownership`, `verify_node_ownership`).
- Identity and ownership types: `mesh/crates/mesh-llm-identity/src/ownership.rs`.
- Runtime ownership at startup, which currently re-signs only when it holds the owner keypair: `mesh/crates/mesh-llm-host-runtime/src/mesh/node.rs`, `mod.rs`, `announcements.rs`, `peer_state.rs`, `connections.rs`.
- Existing admission tests to extend: `mesh/crates/mesh-llm-host-runtime/src/mesh/tests/gossip/owner_admission.rs`, `tests/admission/helpers.rs`.
- CLI parser: `mesh/crates/mesh-llm-cli/src/parser/commands.rs`. Config schema: `mesh/crates/mesh-llm-config`.

## Requirements

1. A new flag `--node-ownership <file>` plus the matching config key. Also auto-load the default node-ownership path if it exists and there is no owner keystore. Follow the existing conventions for data dir and env-var overrides.
2. On startup **without** an owner keypair and **with** a certificate:
   - parse and verify the signature against the owner public key in the certificate;
   - check that `node endpoint id == own endpoint id`;
   - check that `now < expiry`;
   - check that the certificate isn't in the local revocation list.
   If every check passes, announce the ownership exactly as a self-signed node would, so peers see it as **Verified**.
3. If any check fails, log a clear reason and start as **Unsigned**. Never crash, and never fall back silently to a looser state.
4. If an owner keypair **is** present, behaviour is unchanged: the keypair wins, and a warning is logged if a certificate file is also present.
5. A helper to print a node key's endpoint id, so the owner can pre-sign without the node running. For example `mesh-llm auth node-id [--key-path …] [--json]`, or the closest existing pattern.
6. Generic naming: nothing m8s-specific, so the patch can go upstream.
7. Docs: update the relevant section of `mesh/docs/MESHES.md` or `NODE_OWNER_IDENTITY.md`, plus the CLI docs.

## Tests (must pass)

- Valid pre-signed certificate, no keypair → Verified, and admitted under `require-owned` and `allowlist`.
- Expired certificate → Unsigned and rejected.
- Endpoint id mismatch → Unsigned and rejected.
- Revoked certificate id or node id → Unsigned and rejected.
- Keypair plus certificate → the keypair path is used; warning logged.
- `cargo test` for the touched crates, `cargo clippy` clean on them, and `cargo fmt`.

## Out of scope

- Token minting (MP-002), revocation gossip (MP-003), tags (MP-004), Dockerfile (MP-005).
- Any m8s code. Any push, PR or upstream contact.

## Definition of done and report

Reply to Metal-0 with:
- the commit SHA on the worker branch and the files changed;
- the exact test commands and their results;
- a draft upstream PR description (title, motivation, design, test plan) saved as `MP-001-PR.md` at the branch root (not committed to `m8s`);
- risks and open questions.

Metal-0 then reviews, squashes onto `m8s`, and updates the patch queue to `on m8s`.

## Resource notes

The mesh-llm build is heavy (Rust plus a patched llama.cpp). Prefer `cargo test -p <crate>` on the touched crates over a full `just build`. This counts as one heavy environment against the SOP §2 budget.
