# ADR-0010 — Inference goes over the mesh (iroh); the m8s API is control-only

Status: **accepted** · Decided by: Admin ("customers need to be able to use the iroh tunnel for inference, not the REST API") · Date: 2026-10-20

## Context

The kickoff draft had an `m8s-gateway` in front of mesh-llm's unauthenticated `/v1`. Admin rejected inference over REST. Customers, including Goose and pi agents, must reach models through the mesh transport (iroh/QUIC, end to end encrypted).

## Decision

- **Inference path:** client → its own mesh endpoint → iroh/QUIC → serving node. The supported client shapes are:
  - a local `mesh-llm client` (OpenAI API on the client's own loopback, transported over the mesh);
  - an embedded `MeshNode` client through the SDK and bindings.
- **No m8s inference gateway.** m8s never proxies prompts or completions, so it never sees them. This also helps the threat model ([ADR-0011](agor://kb/m8s/decisions/0011-privacy-posture-and-zdr-levels.md)).
- **m8s API = control plane only:** provisioning, scaling, status, evals, audits. It is bound to localhost on the owner node by default.
- **Harness adapters** ([ADR-0004](agor://kb/m8s/decisions/0004-provider-and-harness-adapters.md)) configure Goose and pi against the client's local mesh endpoint (for example `mesh-llm goose`). If an agent cannot run a mesh client, a thin **harness adapter** embeds one. That adapter runs on the client side, never inside m8s.
- **Who may infer** is decided by mesh admission (owner trust policy). Payments are out of scope ([ADR-0013](agor://kb/m8s/decisions/0013-payments-out-of-scope.md)).

## Consequences

- m8s cannot rate-limit or meter per caller. Per-caller quotas are a mesh-level concern, as Admin noted.
- Evals must run from a mesh client too, so the eval runner embeds a client.
- `/v1` on provisioned nodes binds to loopback only; nothing is exposed on public ports except mesh QUIC.
