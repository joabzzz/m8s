# ADR-0008 — Orchestration substrate: Rust daemon + SQLite by default; kube-rs operator optional later

Status: **accepted** · Decided by: **Admin** · Date: 2026-10-20 · Discussion: [education thread](http://localhost:3030/ui/s/01a15c087db477d1822c3214/) · [primer](agor://kb/document/01a15b50-d7b4-76fc-9619-087c7122252e) · Research: [S4 §6](agor://kb/document/01a15b4e-43ee-7461-9bea-17f0edf8e5b3)

## Context

The original proposal was single-node Kubernetes (minikube) on the Mac mini, kube-rs controllers, and k9s for visibility. The rented machines are **not** Kubernetes nodes. They are external resources reached through provider APIs. In the education thread Admin said:

- k9s was wanted only as a pleasant dashboard to watch; it is not critical.
- minikube's 2–4 GB of RAM is overhead. m8s should be light enough to run on almost any hardware.
- A no-Kubernetes mode is welcome, and the plumbing can drop the extras.

All three options use the same reconcile pattern: read desired state, observe reality, take one idempotent step, record status, requeue. They differ only in where state is stored, what wakes the loop, and what the user sees.

## Options

- **A. Operator + CRDs (kube-rs).** `MeshPool`, `Tier`, `GpuNode` and `EvalRun` custom resources. Finalizers guarantee teardown, and k9s shows our printer columns.
- **B. virtual-kubelet provider.** Each provider appears as a fake Node, and each Pod becomes a rental. Pod semantics (logs, exec, probes, networking, restarts) must be emulated. The library is Go and has no Rust implementation, no Vast or RunPod provider exists, and the Deployment controller re-creates failed pods, which quietly re-rents GPUs.
- **C. Plain Rust daemon + SQLite.** One process, one loop, one state file. No cluster.

## Decision

1. **C is the default v0 substrate.** `m8sd` is a single Rust process: a tokio loop that wakes on notify or every 30 s, with state in `~/.m8s/state.db` (SQLite), the `m8s` CLI and `m8s-api` over a local socket, and `m8s top` as the k9s-style dashboard ([ADR-0015](agor://kb/document/01a15b4e-cb84-7557-9c37-1faf9857c247)). The plumbing stays minimal, with no reimplemented watch or informer machinery. Only two guarantees are kept:
   - a `gpu_nodes` row is not deleted until the provider confirms the instance is **gone** (our "finalizer");
   - every rental is labelled with the owner ID, so the orphan sweep can find it.
2. **All logic lives in `m8s-core`**: desired-state model, `GpuNode` state machine, and placement, budget and DMS policy. It has no Kubernetes dependency. `m8sd` is a thin wrapper around it.
3. **A is an optional later mode**, not built in v0. `m8s-operator` (kube-rs) wraps the same `m8s-core`. When built, it runs as a **native host process** (e.g. launchd) against any cluster's API through kubeconfig, **not as a pod**, so provider and owner secrets stay in the host SecretStore/Keychain ([ADR-0012](agor://kb/document/01a15b4e-5c77-7297-bffc-cbcf31a63efc), [ADR-0006](agor://kb/document/01a15b4d-b4ae-765a-ba27-4491ae240bf9)).
4. **B is rejected.**
5. **Recovery restores the goals.** After an owner outage, including one longer than the DMS TTL, m8s reconciles back to desired state by default and re-rents until each tier's GPU and replica targets are met. One guard applies: a **per-tier re-rent limit** (default: at most 3 launches per tier per hour, configurable). When it is hit, the tier pauses and an alert fires through the Alerter. Budget caps and the hard max lifetime ([ADR-0007](agor://kb/document/01a15b4e-5bfc-72c9-a718-77a74f349922)) still apply.
6. **Teardown always means destroy, never stop.** On Vast, a *stopped* instance is still yours and keeps billing for storage. Only *destroy* ends all charges.
   - The in-container `m8s-agent` self-destroys using Vast's injected per-instance `CONTAINER_API_KEY`, which can only start, stop or destroy that one instance, together with `CONTAINER_ID`. The owner's main provider key never enters a box.
   - **Handshake check (m8s-boot/1):** the agent reports `container_id` and `self_destruct: ready`. The owner checks that `container_id` equals the recorded `instance_id`. If the check fails, the owner fails closed: it refuses the join bundle and destroys the instance from the owner side.
   - After the check, the agent removes the key from the environment of child processes, so mesh-llm never sees it.
   - The owner-side sweep destroys any owner-labelled instance that is stopped, exited or not in desired state, and keeps retrying until the provider list reports it gone.

## Consequences

- **No minikube in v0.** The extra RAM is about zero, and m8s runs on any machine that can run a Rust binary. k9s is not part of v0. `m8s top` is the dashboard.
- [Architecture v0](agor://kb/document/01a15b4f-6f37-76b2-8ac8-e3c1be618392) needs updating. Replace `m8s-operator (in minikube)` with `m8sd`. §3 becomes SQLite tables with the same shape (MeshPool → Tier → GpuNode, EvalRun, AuditRun) and the same `GpuNode` phases. CRDs become the A-mode projection of that model.
- The mention of k9s in [ADR-0015](agor://kb/document/01a15b4e-cb84-7557-9c37-1faf9857c247) becomes "optional, A-mode only".
- A Mac mini death is handled the same way under any substrate: the owner process dies with the machine. Protection while the owner is dead comes from the [ADR-0007](agor://kb/document/01a15b4e-5bfc-72c9-a718-77a74f349922) layers, not from the orchestrator.
- **To verify in P0 / S1:**
  - `CONTAINER_API_KEY` permits **destroy**, not just stop, from inside the container.
  - The P0 acceptance test (pull the owner's network) ends with every instance **destroyed**, not stopped or exited, within TTL + 5 min.
- Sources for Vast behaviour: [Vast FAQ: instances](https://docs.vast.ai/guides/reference/faq/instances) (stop keeps storage billing; destroy ends it) and [Vast docker environment](https://docs.vast.ai/guides/instances/docker-environment) (`CONTAINER_API_KEY`, `CONTAINER_ID`).
