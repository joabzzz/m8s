# Discussion primer: securely handing a rented box its invite + owner auth

For: Admin's discussion · Decides: [ADR-0006](agor://kb/document/01a15b4d-b4ae-765a-ba27-4491ae240bf9) · Evidence: [S1](agor://kb/document/01a15b4e-7d4e-740a-9a6d-e82f68e678ae) (Vast), [S2](agor://kb/m8s/research/spike-s2-mesh-llm-integration.md) (mesh-llm) · Status: open

## The problem in one picture

```
  owner (Mac mini, behind NAT)                    rented box (someone else's computer)
  has: owner key, Vast key                        has: nothing yet
         │                                                 ▲
         │  the only thing we control at launch:           │
         └──── env vars / onstart / image ─────────────────┘
                     ▲
                     └── readable by Vast (API returns extra_env + onstart)
                         and by the host operator (S1)
```

So **anything we put in env is effectively public to the provider.** The invite token and owner credential must not travel that way.

## What mesh-llm gives us today

- **Invite token** = how to *reach* the mesh, not permission to join. Signed tokens expire after 24h; none are single-use or revocable.
- **Admission** = owner-key trust policy (`--trust-policy`, `--trust-owner`).
- **Remote control** (`mesh-llm-control/1`) requires the **same owner key on both ends**, so the rented box would hold the owner key. That is a non-starter.
- **Mutating management API** is loopback-only.

## The proposal: sealed bootstrap + m8s-agent

1. Env carries only **public** info (the owner's iroh address) plus a **one-time nonce** (valid 10 minutes, single use, bound to the instance ID).
2. The agent in the container makes its own node key, dials the owner over iroh (encrypted, owner identity pinned), and presents nonce + instance ID + node key + image digest.
3. The owner checks:
   - the nonce is unused and fresh;
   - the instance is ours;
   - the caller's IP is the one Vast reports;
   - the image digest matches what we signed.
   If all pass, the owner **burns the nonce** and returns the real secrets over that channel: join token, admission for this node key, model config, control ACL.
4. After that, the agent accepts commands **only** from the owner's endpoint (plus any identities the owner delegates). It drives mesh-llm through loopback.

**If someone steals the nonce** (Vast or the host could read it), they must redeem it before the real container does, from the right IP. If they manage that, the real container's redemption fails loudly. m8s alerts, destroys the box and revokes the key.

## The honest limit

On a box the host fully controls, the host can read the container's memory after bootstrap, including the join token. Defences:

- keep join tokens short-lived and scoped to that node;
- treat community hosts as ZDR L1 ([ADR-0011](agor://kb/document/01a15b4e-5c46-729d-bb8f-7b1aaf99793a));
- for real protection, use **attested TEEs (L3)**, where the nonce check becomes "prove you are our measured image inside real confidential-computing hardware".

## Questions for the thread

1. Is the sealed bootstrap + m8s-agent (option B) acceptable for the prototype?
2. Delegation: when you "open control to the mesh", should that mean (a) a list of specific peer identities, (b) any admitted mesh member, or (c) a role?
3. Should we propose per-node owner certificates upstream to mesh-llm (NODE_OWNER_IDENTITY), so nodes can be revoked mesh-wide?
