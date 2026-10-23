# ADR-0003 — m8s extends one owner node; one mesh for all tiers

Status: **accepted** · Decided by: Admin · Recorded by: Metal-0 · Date: 2026-10-20

## Context

The kickoff covered several questions:

- Should m8s manage the owner's other machines (Mac minis)?
- Should each tier have its own mesh?
- Who controls a provisioned node?
- Is m8s multi-tenant?

## Decision

- **Unit of ownership: one mesh node** (the *owner node*). m8s is an extension of that node. It provisions extra mesh nodes on rented hardware, and those nodes are controlled only by the owner node.
- **m8s does not manage anything it did not provision.** That excludes other Mac minis, other peers, and the public mesh.
- **One mesh, all tiers.** Provisioned nodes join the owner's (private) mesh and serve models. Clients pick by model name; mesh-llm routing does the rest. "Tier" is an m8s provisioning concept (hardware class + model), not a separate network.
- **Default control: owner only.** The owner may opt in to delegating control to other identities, such as mesh members. This is an explicit config, not a default.
- **Single-tenant** for the prototype. Quotas and tenancy are probably mesh-level concerns.
- Control plane host: **Admin's Mac mini**, the same machine as the owner node.

## Consequences

- The owner node holds the provider credentials, the owner identity and the provisioned-instance inventory. Losing it must not cause runaway spend ([ADR-0007](agor://kb/m8s/decisions/0007-dead-mans-switch.md)).
- Provisioned nodes are ordinary mesh peers from everyone else's point of view. Anyone admitted to the mesh can send them inference.
- m8s CRDs and inventory are scoped to the owner's own resources. No cluster-wide discovery of peers.
