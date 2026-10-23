# ADR-0006 — Bootstrap handshake and control channel (sealed bootstrap + m8s-agent)

Status: **accepted** · Decided by: **Admin** · Recorded by: Metal-0 · Date: 2026-10-20 · Discussion: [session](http://localhost:3030/ui/s/01a15c087d717284b12efc47/), [primer](agor://kb/document/01a15b50-d7e4-7222-a99c-e65646b2a12e) · Amends: [ADR-0002](agor://kb/document/01a15b4c-e13e-72a9-8cef-7554c25c0a12) · Patches: [mesh-llm patch queue](agor://kb/document/01a15c1a-14c0-7750-9cc2-2ede1ad0ad25)

## Decision (summary)

1. **Option B is accepted for the prototype:** an m8s-agent sidecar plus a sealed bootstrap. Admin's main owner key never leaves the Mac mini.
2. **Admission gap: fix it with a mesh-llm patch, not a fleet owner key.** Key-less node attestation ([MP-001](agor://kb/document/01a15c1a-14c0-7750-9cc2-2ede1ad0ad25)) is carried as a commit on the `m8s` branch of our local mesh-llm checkout, and the m8s-node image builds from that branch. During the sealed bootstrap, the Mac signs a short-lived cert for each node; no owner private key goes onto a rented box. The fleet-key interim is **not adopted**.
3. **mesh-llm patch process:** patches that m8s needs are stacked as commits on the `m8s` branch, each linked to the ADR that needs it, and tracked in the [patch queue](agor://kb/document/01a15c1a-14c0-7750-9cc2-2ede1ad0ad25) as future upstream PRs (Admin has no write access to mesh-llm yet). The procedure is in [SOP §5a](agor://kb/document/01a1575b-167a-721a-bb44-4562f6f36351). m8s may become a crate in mesh-llm in the long run, so patches stay small and upstream-shaped.
4. **Control delegation:** the control ACL is an **explicit list of endpoint IDs**, defaulting to `[owner]`. **Roles** (owner-signed, scoped, short-lived delegation claims) are the later design and need their own ADR. **"Any admitted mesh member" is rejected.**
5. **Bootstrap nonce:** a single-use, instance-bound, IP-bound nonce in provider-visible env is acceptable for L0/L1 providers. Its lifetime is **15 minutes**, up from the 10 proposed.

## Context

A freshly rented container must receive two things:

1. **Join material**, so it can reach and join the owner's mesh.
2. **Owner authority**, so that only the owner node, or identities the owner delegates to, can control it.

Facts that constrain the design:

- mesh-llm invite tokens are connectivity material, not authorization. Signed tokens last 24h, and none are single-use or revocable.
- Admission is owner-key based (`--trust-policy`, `--trust-owner`).
- mesh-llm's owner-control lane needs **the same owner key on both ends**. Shipping the owner private key to a rented box is unacceptable.
- The mesh-llm management API that can make changes is loopback-only.
- Provider env vars and onstart scripts are visible to the provider, and on community hosts probably to the host operator ([S1](agor://kb/document/01a15b4e-7d4e-740a-9a6d-e82f68e678ae)).
- The owner is a Mac mini behind NAT, so the container cannot dial it over plain HTTPS.

## Options

| | Option | Owner key leaves the Mac? | mesh-llm changes | Notes |
|---|---|---|---|---|
| A | Owner-control QUIC (`mesh-llm-control/1`) | **Yes** | none | Simplest, but a stolen owner key compromises the whole mesh |
| B | **m8s-agent sidecar** that accepts commands only from pinned endpoint IDs over iroh (`m8s-control/1`) and drives mesh-llm via loopback | No | MP-001 for admission | **Accepted** |
| C | SSH via provider | No | none | Provider holds the SSH path; awkward behind NAT; weak audit |
| D | Per-node owner certificates (NODE_OWNER_IDENTITY proposal) | No | upstream | Long-term direction. MP-001 is the minimal piece of it we carry now |

For the admission gap, two sub-options were considered:

| | Path | Outcome |
|---|---|---|
| i | Per-session rotated **fleet owner key** delivered through the sealed bootstrap | Not adopted. Its residual risk: a host that reads RAM could mint rogue fleet-owned nodes until rotation. |
| ii | **Key-less node attestation patch** (MP-001) on our `m8s` mesh-llm branch | **Adopted.** A thief gets only one node's key and cert, until it expires or is revoked. |

## Design

```
 owner node (Mac mini)                         rented container (m8s-node image,
 ─────────────────────                         mesh-llm built from the `m8s` branch)
                                               ────────────────────────────────
 1 m8s creates instance with env:
   M8S_OWNER=<owner endpoint id + relay addr>   (public info)
   M8S_BOOT=<one-time nonce, 15 min, bound to instance id>
                                               2 m8s-agent generates node key in tmpfs
                                               3 dials owner over iroh ALPN m8s-boot/1
                                                 (E2E encrypted; owner identity pinned)
                                                 sends {instance_id, nonce, node endpoint id,
                                                        image digest, [TEE quote]}
 4 m8s checks: nonce unused & fresh,
   instance id ours, source IP == provider-
   reported IP, image digest == signed digest
   → burns nonce, signs a short-lived node
     ownership cert for that endpoint id
     (auth sign-node), returns sealed bundle:
     join token, node cert, model + config,
     control ACL [owner, …], heartbeat TTL
                                               5 writes join-file + cert to tmpfs, starts mesh-llm
                                                 (--join-file, --node-ownership <cert>,
                                                  --trust-policy require-owned, analytics off)
 6 m8s-agent ⇄ owner: control + heartbeats on m8s-control/1 (ACL-pinned)
   commands: load/unload/drain/status/audit/self-destroy → loopback mesh-llm API
 m8s down / theft → destroy instance, revoke node cert + node id in trust stores
```

- **Theft window.** The nonce is visible to the provider. If anyone else redeems it first, the real container's redemption fails. m8s then raises an alert, destroys the instance and revokes the stolen node cert. The window is at most 15 minutes, single use, and IP-bound.
- **Admission.** The Mac mini can run `--trust-policy allowlist` trusting only Admin's owner id, because we don't use mesh-llm owner-control (which rejects `allowlist`). Rented nodes are admitted through their owner-signed cert. Cert lifetime is set to the node's planned lifetime plus margin, and is renewed over `m8s-control/1` if needed.
- **Delegation.** The control ACL is an explicit list of endpoint IDs in m8s config, defaulting to `[owner]`. A later role-based design would use owner-signed claims naming the holder, the allowed verbs (for example load/unload/drain/status, but not self-destroy or ACL changes) and a TTL. That pattern already exists in mesh-llm's `plugin_delegation`.
- **Join tokens.** Get them by polling the owner node's `/api/status` token. They are delivered by file and rotated by rewriting the join-file, because signed tokens are fixed at 24h. Better token minting is [MP-002](agor://kb/document/01a15c1a-14c0-7750-9cc2-2ede1ad0ad25) (candidate).
- **Upgrade path.** With a TEE (ZDR L3), attestation verification replaces the nonce check ([ADR-0011](agor://kb/document/01a15b4e-5c46-729d-bb8f-7b1aaf99793a)).

## Evidence (S1, S2)

- Vast returns `extra_env` and `onstart` through its API, so the env is provider-visible ([S1](agor://kb/document/01a15b4e-7d4e-740a-9a6d-e82f68e678ae)).
- Owner-control is rejected under `--trust-policy allowlist`, is single-owner only, and is direct-UDP only with relays disabled ([S2](agor://kb/document/01a15b51-c388-76f8-b6ed-8f10d51fdb83)). The m8s-agent channel runs on our own iroh endpoint with relays allowed, so none of this blocks us.
- Under `off` or `prefer-owned`, anyone holding the invite token can join and route inference. Under `require-owned`, the runtime today needs an owner keypair on the node and never loads a pre-signed cert (`auth sign-node` can mint one, but nothing consumes it). MP-001 closes this gap.

## Consequences

- We own a small agent binary in the image. It also hosts the dead man's switch ([ADR-0007](agor://kb/document/01a15b4e-5bfc-72c9-a718-77a74f349922)) and the audit runner.
- We carry a small mesh-llm patch stack on the `m8s` branch. [ADR-0002](agor://kb/document/01a15b4c-e13e-72a9-8cef-7554c25c0a12) is amended: the image runs mesh-llm built from that branch, not an unmodified release. Rebases onto upstream are deliberate and recorded in the patch queue.
- **MP-001 is a P0 blocker:** no rented node joins under an enforcing trust policy until MP-001 is on `m8s`.
- The owner node must be reachable over iroh, using relays if necessary, for bootstrap and heartbeats.
- Revocation lists are local per node. Until [MP-003](agor://kb/document/01a15c1a-14c0-7750-9cc2-2ede1ad0ad25) lands, m8s pushes trust-store updates itself, and short cert lifetimes limit exposure.
- Filing upstream PRs is an external action and needs Admin.
