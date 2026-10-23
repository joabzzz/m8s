# ADR-0007 — Dead man's switch: provisioned instances terminate when the owner is gone

Status: **accepted** (behaviour, decided by Admin) · Mechanism **proposed** (pending [S1](agor://kb/m8s/research/spike-s1-vast-provider-and-costs.md)) · Date: 2026-10-20

## Context

If the owner's machine dies, sleeps or leaves the mesh, rented containers keep billing. Admin wants protection from a "runaway train burning someone's credit card". Default: lose contact for **t = 1 hour** and the instances are closed. The guard cannot live only on the owner node, because that is the thing that might be dead.

## Decision: three independent layers, all on by default

```
 Layer 1: in-container (m8s-agent)       Layer 2: owner-side reconcile      Layer 3: provider / account
 ─────────────────────────────────       ─────────────────────────────      ───────────────────────────
 owner heartbeat every 60s over          on m8s start and every 5 min:      prepaid credit only / low balance
 m8s-control/1                           list_owned(label=owner-id),        hard max lifetime per instance
 no heartbeat for TTL (default 1h):      destroy anything not in the        (m8s config, default 12h)
   1. drain model, stop mesh-llm         desired state (orphan sweep)       provider auto-destroy if offered
   2. wipe tmpfs / model cache
   3. self-destroy via provider API
      (scoped per-instance credential)
   4. fallback: exit; container stops,
      layer 2 destroys the stopped box
```

- **TTL** is configurable per mesh in m8s config (`dead_mans_switch.ttl = "1h"`). Setting it to `off` needs an explicit flag and prints a warning.
- **Heartbeats** are authenticated: only control-ACL identities count ([ADR-0006](agor://kb/document/01a15b4d-b4ae-765a-ba27-4491ae240bf9)). Inference traffic from other peers does **not** keep an instance alive.
- **Stopped is not enough.** Some providers bill storage for stopped instances, so the goal state is *destroyed*.
- **Hard max lifetime.** Applies even when heartbeats are healthy. It catches a bug in which m8s keeps heartbeating forever. The owner can extend it explicitly.
- **Alerts** fire through the Alerter adapter ([ADR-0015](agor://kb/m8s/decisions/0015-interfaces-observability-alerts.md)) when layer 1 or layer 3 triggers.

## Mechanism confirmed (2026-10-21, with [ADR-0008](agor://kb/document/01a15b4d-b4f9-7254-adfb-37d41a4297e1))

- **Layer 1 credential:** Vast injects `CONTAINER_ID` and a per-instance **`CONTAINER_API_KEY`** into every container. The key is limited to start, stop and destroy on *that* instance ([Vast docker environment](https://docs.vast.ai/guides/instances/docker-environment)). Our main Vast key never enters a box.
- **Handshake gate:** in `m8s-boot/1`, the agent reports `container_id` and that the self-destruct key is present. m8sd requires `container_id == instance_id`. If either check fails, it refuses the bundle and destroys the box from the owner side (**fail closed**). After the handshake, the agent moves the key out of the environment so mesh-llm never sees it.
- **Always destroy, never stop.** Stopped Vast instances keep billing storage. The owner sweep destroys any owner-labelled instance that is stopped, exited, or not in the desired state, and keeps retrying until the provider list no longer shows it.
- **Outage recovery:** m8sd restores the desired state, limited to 3 launches per tier per hour. Beyond that the tier moves to `Paused` and an alert fires.

**P0 acceptance tests:**
1. On a throwaway instance, confirm that `CONTAINER_API_KEY` permits **destroy**, not just stop.
2. Pull the Mac's network, then confirm every instance is **destroyed (not stopped)** within TTL + 5 minutes, and that billing stops.

## Consequences

- Layer 1 needs a credential **inside** the container that can destroy *only that instance*. If a provider has no such scoped credential, the adapter must declare `self_destruct: false`. m8s then warns at launch and relies on layers 2 and 3.
- A wrongly shortened TTL can kill healthy nodes during a long owner outage. That is intended: spend safety wins over availability.
- The P0 acceptance test pulls the Mac mini's network and checks that every instance is destroyed within TTL + 5 minutes.
