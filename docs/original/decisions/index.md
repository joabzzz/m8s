# m8s — ADR index

Owner: Metal-0 · Rules: [SOP §5](agor://kb/document/01a1575b-167a-721a-bb44-4562f6f36351)

ADRs live at `agor://kb/m8s/decisions/NNNN-<slug>.md`. Each contains context, options, decision, consequences, status, and who decided. Metal-0 approves ADRs on its own authority. Decisions that are hard to reverse, costly, scope-changing, or touch security/privacy are escalated to Admin.

Statuses: `proposed` · `accepted` · `superseded by NNNN` · `rejected`

| # | Title | Status | Decided by | Date |
|---|---|---|---|---|
| [0001](agor://kb/document/01a1576c-8eee-73e9-806c-d99a0e071e05) | Local workspace and repo, no remote | accepted | Admin | 2026-10-19 |
| [0002](agor://kb/document/01a15b4c-e13e-72a9-8cef-7554c25c0a12) | Rust, with mesh-llm consumed as an unmodified dependency | accepted, **amended** by 0006 (small patches on the `m8s` branch) | Admin | 2026-10-20 |
| [0003](agor://kb/document/01a15b4c-e163-71d4-97c1-e66da2bbb578) | m8s extends one owner node; one mesh for all tiers | accepted | Admin | 2026-10-20 |
| [0004](agor://kb/document/01a15b4c-e18c-7445-a243-d3f2a4985da2) | Open Provider and Harness adapters; vast-community and vast-secure are separate | accepted (trait shape proposed) | Admin / Metal-0 | 2026-10-20 |
| [0005](agor://kb/document/01a15b4c-e1bf-7475-a333-a7ae1c135dc0) | Placement policy: sanctions denylist, bandwidth floor, our images only | accepted (list incl. China; bandwidth per provider) | Admin | 2026-10-20 |
| [0006](agor://kb/document/01a15b4d-b4ae-765a-ba27-4491ae240bf9) | Bootstrap handshake and control channel (sealed bootstrap + m8s-agent) — [discussion](http://localhost:3030/ui/s/01a15c087d717284b12efc47/) | **accepted** (key-less attestation patch MP-001, explicit control ACL, 15-min nonce) | Admin | 2026-10-20 |
| [0007](agor://kb/document/01a15b4e-5bfc-72c9-a718-77a74f349922) | Dead man's switch (3 layers) | accepted (mechanism proposed) | Admin | 2026-10-20 |
| [0008](agor://kb/document/01a15b4d-b4f9-7254-adfb-37d41a4297e1) | Orchestration: Rust daemon + SQLite (`m8sd`) by default; kube-rs operator optional later; virtual-kubelet rejected — [discussion](http://localhost:3030/ui/s/01a15c087db477d1822c3214/) | **accepted** (no minikube in v0; per-tier re-rent limit; teardown = destroy via Vast `CONTAINER_API_KEY`) | Admin | 2026-10-20 |
| [0009](agor://kb/document/01a15b4d-b51a-7030-8be7-0c5368cbff72) | Scaling is owner-defined config; v0 triggers manual, schedule, queue | accepted | Admin / Metal-0 | 2026-10-20 |
| [0010](agor://kb/document/01a15b4e-5c1f-7453-bd9a-c455fd58a1ee) | Inference over the mesh (iroh); m8s API is control-only | accepted | Admin | 2026-10-20 |
| [0011](agor://kb/document/01a15b4e-5c46-729d-bb8f-7b1aaf99793a) | Privacy posture: threat model, ZDR levels L0–L3, tags, audit, signed images | accepted (taxonomy accepted 2026-10-21) | Admin | 2026-10-20 |
| [0012](agor://kb/document/01a15b4e-5c77-7297-bffc-cbcf31a63efc) | SecretStore trait; Keychain default, 0600 file fallback — [discussion](http://localhost:3030/ui/s/01a15c087e0177449258b553/) | **accepted; amended 2026-10-21** (latest Admin direction retains bounded versioned `secrets.toml`; prior binary approval superseded; security controls unchanged; m8s never holds mesh owner key; host-side secrets; auto top-up off; wallet outside m8s) | Admin | 2026-10-20; amended 2026-10-21 |
| [0013](agor://kb/document/01a15b4e-cb32-77ff-afa6-bb74960a1048) | Payments out of scope; m8s only gates admission | accepted | Admin | 2026-10-20 |
| [0014](agor://kb/document/01a15b4e-cb60-7391-baf0-388348c9db02) | Evals: reuse mesh-llm tooling; throughput at N first; on-demand | accepted | Admin | 2026-10-20 |
| [0015](agor://kb/document/01a15b4e-cb84-7557-9c37-1faf9857c247) | Interfaces (CLI/TUI/web/API/k9s), stock observability, Alerter | accepted | Admin | 2026-10-20 |
| [0016](agor://kb/document/01a15b4e-cba5-72d2-8b24-019d3dc26fec) | One model per rented host; scale up via NVLink, out via replicas; frontier deferred | proposed (pending P0 live test) | Admin direction | 2026-10-20 |
| [0017](agor://kb/document/01a15c08-e7bf-701e-a0e4-d203a169c2c2) | m8s doesn't pick models; model profiles + placement planner | accepted | Admin | 2026-10-20 |

🚩 = waiting on Admin.

mesh-llm patches needed by these ADRs are tracked in the [mesh-llm patch queue](agor://kb/document/01a15c1a-14c0-7750-9cc2-2ede1ad0ad25) ([SOP §5a](agor://kb/document/01a1575b-167a-721a-bb44-4562f6f36351)).
