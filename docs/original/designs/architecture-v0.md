# m8s — Architecture v0

Status: draft v0.2 (2026-10-21) · Owner: Metal-0 · [Charter](agor://kb/document/01a15b4b-ef89-75c0-a303-99bca9255a42) · [ADR index](agor://kb/document/01a1575c-3c01-775e-b5cb-4dab3b09c054) · [Patch queue](agor://kb/document/01a15c1a-14c0-7750-9cc2-2ede1ad0ad25) · Research: [S1](agor://kb/document/01a15b4e-7d4e-740a-9a6d-e82f68e678ae) · [S2](agor://kb/document/01a15b51-c388-76f8-b6ed-8f10d51fdb83) · [S3](agor://kb/document/01a15b4c-9887-7452-a281-858ce1d16477) · [S4](agor://kb/document/01a15b4e-43ee-7461-9bea-17f0edf8e5b3)

Changes since v0.1:
- `m8sd` (a Rust daemon with SQLite) replaces the in-minikube operator. There is no Kubernetes in v0 ([ADR-0008](agor://kb/document/01a15b4d-b4f9-7254-adfb-37d41a4297e1)).
- Secrets stay on the host ([ADR-0012](agor://kb/document/01a15b4e-5c77-7297-bffc-cbcf31a63efc)).
- Admission works through MP-001 per-node certificates, with no owner key on rented boxes ([ADR-0006](agor://kb/document/01a15b4d-b4ae-765a-ba27-4491ae240bf9)).
- Models are described by profiles; m8s does not choose them ([ADR-0017](agor://kb/document/01a15c08-e7bf-701e-a0e4-d203a169c2c2)).

## 1. Big picture

```
 ┌──────────────────────── owner host (Admin's Mac mini; any machine) ────────────────────────┐
 │                                                                                            │
 │  mesh-llm (owner node)        ◀─ loopback ─▶  m8sd (one Rust process, launchd user agent)  │
 │   console :3131                                 m8s-core reconcile loop (30s / on notify)   │
 │   owner keystore (passphrase,                   Provider adapters (vast-community/secure)   │
 │   unlock secret in Keychain)                    bootstrap+control server (iroh ALPN         │
 │   signs MP-001 node certs on request            m8s-boot/1, m8s-control/1)                  │
 │                                                 policy: placement, budget, re-rent, DMS     │
 │  Keychain: vast/api-key, session secrets        state: ~/.m8s/state.db (SQLite)             │
 │                                                                                            │
 │  m8s CLI · m8s top (TUI) · web ── m8s-api (unix socket / localhost) ──┘                    │
 │  optional: OTel collector → Prometheus/Grafana                                             │
 └──────────────┬──────────────────────────────────────────────────┬──────────────────────────┘
                │ provider REST (HTTPS, scoped key)                 │ iroh/QUIC (E2E, relays OK)
                ▼                                                   ▼
      ┌──────────────────┐            ┌──────────────── private mesh (require-owned) ───────────┐
      │ Vast.ai API      │  launch →  │  m8s-node             m8s-node             …            │
      │ vast-community   │            │  ┌──────────────────────────────┐                      │
      │ vast-secure      │            │  │ m8s-agent: boot, control,    │ CONTAINER_API_KEY     │
      └──────────────────┘            │  │ heartbeat, DMS, audit        │ (self-destroy only)   │
                                      │  │ mesh-llm serve (m8s branch,  │ MP-001 node cert      │
                                      │  │ /v1 loopback only)           │ (no owner key)        │
                                      │  └──────────────────────────────┘                      │
                                      │  Goose / pi / apps infer via local `mesh-llm client`   │
                                      └────────────────────────────────────────────────────────┘
```

Key properties:
- **Inference never touches m8s** ([0010](agor://kb/document/01a15b4e-5c1f-7453-bd9a-c455fd58a1ee)).
- **No owner private key ever reaches a rented box** ([0006](agor://kb/document/01a15b4d-b4ae-765a-ba27-4491ae240bf9)).
- **m8s never reads the mesh owner key.** It asks mesh-llm to sign ([0012](agor://kb/document/01a15b4e-5c77-7297-bffc-cbcf31a63efc)).
- **Every box can destroy itself, and teardown always destroys** ([0007](agor://kb/document/01a15b4e-5bfc-72c9-a718-77a74f349922), [0008](agor://kb/document/01a15b4d-b4f9-7254-adfb-37d41a4297e1)).

## 2. Components

| Component | Where | Job |
|---|---|---|
| `m8s-core` | lib | Desired-state model, `reconcile(desired, observed) -> action`, placement/budget/re-rent policy, GpuNode state machine. **No Kubernetes, no provider deps.** |
| `m8sd` | host process | Tokio loop over `m8s-core`; SQLite state; provider adapters; bootstrap/control server; orphan sweep. Holds all secrets (via `SecretStore`). |
| `m8s-provider` + `m8s-provider-vast` | lib | `Provider` trait; `vast-community` and `vast-secure` ([0004](agor://kb/document/01a15b4c-e18c-7445-a243-d3f2a4985da2)). |
| `m8s-secrets` | lib | `SecretStore`: Keychain via `keyring` (default), 0600 file fallback, env (CI) ([0012](agor://kb/document/01a15b4e-5c77-7297-bffc-cbcf31a63efc)). |
| `m8s-models` | lib + data | Model profile catalog and placement planner (fit, replicas, KV headroom, download estimate) ([0017](agor://kb/document/01a15c08-e7bf-701e-a0e4-d203a169c2c2)). |
| `m8s-agent` | inside image | Redeems the nonce and reports `container_id`; receives the sealed bundle (MP-001 cert, join token, model config, control ACL); moves `CONTAINER_API_KEY` out of the environment; supervises mesh-llm; executes control commands through loopback; runs the DMS timer and audit checks. |
| `m8s-node` image | registry | CUDA base, mesh-llm built from the **`m8s` patch branch**, `m8s-agent` as entrypoint, read-only rootfs, tmpfs, egress allowlist, analytics off, cosign-signed. |
| `m8s-harness` | lib/CLI | `Harness` trait: Goose and pi as subprocesses, configured via `mesh-llm goose` / `mesh-llm pi`. |
| `m8s-eval` | CLI | `run-llama-benchy-openai.sh` through the mesh, GuideLLM, BFCL, Aider and FLORES subsets ([0014](agor://kb/document/01a15b4e-cb60-7391-baf0-388348c9db02)). |
| `m8s-audit` | agent + host | Signed `m8s.audit/v1` report ([0011](agor://kb/document/01a15b4e-5c46-729d-bb8f-7b1aaf99793a)). |
| `m8s` CLI / `m8s top` / web | host | Interfaces ([0015](agor://kb/document/01a15b4e-cb84-7557-9c37-1faf9857c247)). |
| *(optional, later)* `m8s-operator` | host process | kube-rs wrapper over `m8s-core` against any cluster; k9s views. Not built in v0. |

## 3. State (SQLite tables; these would be the CRDs in optional kube mode)

```
mesh_pool   (1 row)        owner endpoint id, budget, DMS ttl, max lifetime, placement policy, control ACL
tier        (per pool)     name, provider prefs, gpu spec, model profile ref, replicas{min,max,desired},
                           layout (a|b|c), cold behaviour, schedules, queue trigger, re-rent limit (3/h)
gpu_node    (per instance) tier, provider, offer, instance_id, container_id, $/h, phase, node endpoint id,
                           cert id + expiry, zdr effective, heartbeat_at, launches_this_hour
nonce       (per launch)   instance_id, hash, issued_at, ttl 15m, redeemed_at, redeemer_ip
eval_run / audit_run       inputs, status, report ref
event       (append-only)  everything above, for m8s top / logs / alerts
```

`gpu_node` phases are:

`Pending → Searching → Launching → Booting → Bootstrapped → Loading → Ready → Draining → Destroying → Gone`

with `Failed`, `Orphaned` and `Paused` (re-rent limit hit) as side states. **A row is deleted only after the provider confirms the instance is gone.** That is the daemon's equivalent of a finalizer.

## 4. Lifecycle: `m8s up <tier>`

```
 CLI → m8s-api → UPDATE tier SET desired=1 → notify m8sd:
   1 policy: region denylist (incl. CN), inet_down ≥ floor, budget headroom, re-rent limit  (0005)
   2 planner: profile × offers → fit + replicas + KV headroom; rank by $/h × reliability  (0017)
   3 mint nonce (15m, single use); launch signed image digest; labels(owner-id, node uid);
     record instance_id immediately                                                     (0006)
   4 agent → m8s-boot/1: nonce + container_id + node pubkey + image digest
     m8sd checks nonce fresh/unused, container_id == instance_id, IP == provider IP,
     digest == signed, CONTAINER_API_KEY present → else refuse + destroy (fail closed)
   5 m8sd asks mesh-llm to sign a short-lived MP-001 cert for the node key → sealed bundle
   6 agent starts mesh-llm (--node-ownership cert, --join-file, require-owned, analytics off)
   7 Ready = cert verified + model served + canary request routed via owner node          (S2)
   8 heartbeats 60s; DMS on agent (1h → drain, wipe, DESTROY via CONTAINER_API_KEY)
 m8s down → drain → destroy → poll provider until gone → revoke cert → delete row
 outage recovery: restore desired state, max 3 launches/tier/hour, else Paused + alert
```

## 5. UX sketches

```
$ m8s up qwen27
✔ policy   region∉{CU,IR,KP,SY,RU,BY,UA-occ,CN} · inet_down≥1000 Mbps · $14.20 left this month
✔ plan     qwen3.6-27b-q4km on 2× RTX 4090 (layout b) → 2 replicas · ~18 parallel seqs @8k each
✔ offer    vast-community · $0.71/h · us-central · rel 99.1%
✔ launch   m8s-node@sha256:9c1e… (cosign verified) · instance 41823
✔ boot     nonce redeemed from 203.0.113.7 · container 41823 ✓ · self-destruct key ✓
✔ admit    node cert signed by owner (expires 13h) · require-owned ✓
⠋ loading  17.2 GB  ▕███████████▌    ▏ 118 MB/s
```

```
┌ m8s top ─ owner mini-01 ─ mesh m8s-prod ──────────────── $1.92/h · $5.80 of $20 ─┐
│ POOL       NODES  MODEL                     LAYOUT  TOK/S@N8  QUEUE  ZDR  CLOSES    │
│ qwen27     2/2    qwen3.6-27b-q4km          b       212       0.0s   L1   18:00     │
│ dense70    1/1    <70B-class>-q4km          c       141       0.6s   L2   18:00     │
│ ── nodes ─────────────────────────────────────────────────────────────────────────  │
│ qwen27-41823a  vast-community  4090×2  $0.71  Ready  hb 8s   cert 12h  age 41m      │
│ dense70-41907  vast-secure     H100    $1.21  Ready  hb 11s  cert 13h  age 22m      │
│ DMS ok · re-rent 0/3h · max lifetime 12h   [u]p [d]own [e]val [a]udit [l]ogs [c]onsole│
└───────────────────────────────────────────────────────────────────────────────────┘
```

## 6. Pools (examples only, owners choose; [ADR-0017](agor://kb/document/01a15c08-e7bf-701e-a0e4-d203a169c2c2))

| Example pool | Hardware | Model class | Layout | Notes |
|---|---|---|---|---|
| qwen27 | 2× 24 GB | Qwen 27B class, Q4_K_M (~17 GB) | b | P0 |
| dense70 | 1× 80 GB or 2× 48 GB | strong 70B dense, Q4_K_M (~42 GB) | c or b | P0 |
| frontier | 8× H100/H200, one host | MiniMax M2.7 / GLM-5.x / Kimi K2.x | — | **deferred** (cost, untested 8-GPU) ([0016](agor://kb/document/01a15b4e-cba5-72d2-8b24-019d3dc26fec)) |

## 7. Upstream gaps

These are tracked in the [patch queue](agor://kb/document/01a15c1a-14c0-7750-9cc2-2ede1ad0ad25):

- **MP-001** key-less node attestation: approved, P0 blocker.
- **MP-002** token mint API: candidate.
- **MP-003** revocation propagation: candidate.
- **MP-004** signed tags plus route filter: candidate.
- **MP-005** GPU Dockerfile: candidate.

Not patched, by design:
- owner-control under `allowlist`;
- owner-control over relay;
- `/v1` auth (it stays loopback);
- a Prometheus endpoint (we bridge OTLP).

Also open: an owner keystore is plaintext unless a passphrase is set. m8s requires the passphrase ([0012](agor://kb/document/01a15b4e-5c77-7297-bffc-cbcf31a63efc)).
