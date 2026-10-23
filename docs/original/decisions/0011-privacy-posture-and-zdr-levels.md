# ADR-0011 — Privacy posture: threat model, ZDR levels, node tags, audit suite, signed images

Status: **accepted** (principles, and the L0–L3 taxonomy and wording, decided by Admin 2026-10-21; may be revisited later) · Mechanisms proposed · Research: [S3](agor://kb/document/01a15b4c-9887-7452-a281-858ce1d16477) · Date: 2026-10-20

## Context

Admin wants zero data retention (ZDR) to be **cryptographic** where possible. It is kept open because not every provider can offer it, so we record each provider's level. The adversaries are:

1. the provider (Vast, host operators);
2. the node that controls the rented instance;
3. network observers.

m8s is a framework and never operates other people's compute. The serving node sees plaintext prompts, and only a CPU TEE plus GPU confidential computing (CC) changes that.

## Decision

**1. ZDR levels.** Each adapter declares a *ceiling*. The audit suite computes the *effective* level per node.

| Level | Meaning | Protects against provider? | Example |
|---|---|---|---|
| **L0** none | No controls verified | No | unknown image or host |
| **L1** policy + audit | Our hardened image, tmpfs, analytics off, egress allowlist, audit passed | Accidental retention only | `vast-community` |
| **L2** verified DC + audit | L1, plus a datacenter with certifications | Contractually, not cryptographically | `vast-secure` |
| **L3** attested TEE | CPU TEE + GPU CC, measured image, attestation bound to the node's iroh key | **Yes**, cryptographically (within TEE trust assumptions) | Azure/GCP CC H100, Phala; future adapters |

**2. Threat model (v0):**

| Adversary | Control | Best level |
|---|---|---|
| Network observer | iroh/QUIC end-to-end encryption; no public `/v1` | strong at all levels |
| Provider / host operator | L1 hygiene, L2 contracts, L3 attestation | cryptographic only at L3 |
| Controlling (owner) node | m8s never proxies inference ([ADR-0010](agor://kb/m8s/decisions/0010-inference-over-the-mesh.md)); the image has no prompt logging; at L3, the client verifies the image measurement, so even the owner cannot swap in a logging image | cryptographic only at L3 |

**3. Node tags.** m8s attaches tags such as `provider=`, `zdr=L1`, `tier=` and `region=` to its nodes. Whether mesh-llm can gossip tags and route on them (for example a hard `min_zdr` filter) is a **mesh-level question**, investigated in [S2](agor://kb/m8s/research/spike-s2-mesh-llm-integration.md) and logged as an upstream item. Until then, tags live in m8s status and audit reports.

**4. Audit suite** (an automated, signed JSON report per node, schema `m8s.audit/v1`):

- canary prompts, then a scan of disk, logs, swap and caches for the canaries;
- egress capture against an allowlist;
- config assertions (analytics off, log store `metadata_only`);
- image digest and signature verification;
- attestation verification at L3;
- a transport check.

Each check is labelled with its vantage point (in-container, owner, or host). In-container results are self-attested, and the report says so.

**5. Signed images.** `m8s-node` images are signed with cosign (keyless, Rekor), with an SBOM. Launch and bootstrap verify by digest ([ADR-0006](agor://kb/document/01a15b4d-b4ae-765a-ba27-4491ae240bf9)). Nodes also run mesh-llm with `--require-release-attestation`. That covers the mesh-llm binary only; our image signature covers everything else.

## Update from S2 (2026-10-20)

mesh-llm has **no free-form gossiped node tags** and **no routing filter on tags** ([S2](agor://kb/document/01a15b51-c388-76f8-b6ed-8f10d51fdb83)). The only signed text is `node_label` in the owner certificate.

For the prototype:
- encode the tier, and where useful the ZDR level, in the **model names** m8s serves, for example `m8s-medium/Qwen3-Coder-Next`;
- keep the full tags in m8s status and in audit reports.

A `min_zdr` routing filter becomes an upstream proposal.

Also from S2:
- under trust policy `off` or `prefer-owned`, anyone holding the invite token can use the mesh;
- private meshes using m8s should run `require-owned` (see [ADR-0006](agor://kb/document/01a15b4d-b4ae-765a-ba27-4491ae240bf9)).

## Consequences

- Honest labelling: Vast nodes are **L1 or L2**, never "ZDR". Docs and UI must not overclaim.
- An L3 adapter is a post-prototype milestone. Its design (TEE generates the iroh key, report_data binds the node ID, client pins it) is sketched in S3.
- Admin accepted the L0–L3 taxonomy and the rule "never claim ZDR below L3" (2026-10-21, "okay for now").
