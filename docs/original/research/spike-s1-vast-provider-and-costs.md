# Spike S1: Vast.ai provider, security model, and cost model

Spike S1 · Author: Metal-0 spike worker (Sonnet 5.5) · Date: 2026-10-20 · Status: complete

Desk research only (official Vast docs fetched 2026-10-20 plus third-party price blogs). Nothing was rented or tested. Items marked **[UNVERIFIED]** need a live check in the P0 plan. Not legal advice.

## TL;DR

- **Secrets must NOT go in Vast env/onstart.** `extra_env` and `onstart` are returned by the account API (`show instances`), and the host operator (root on the machine) can read container env. Deliver the mesh invite and owner auth out-of-band: container boots with only a short-lived, single-use enrollment nonce; m8s control plane pushes the real secrets over an authenticated channel (mesh-llm/iroh or TLS to a port) after the container proves itself. Treat even the nonce as burnable.
- **Dead-man's switch is feasible natively.** Every instance gets `CONTAINER_ID` + `CONTAINER_API_KEY`; the key is restricted to that instance (start/stop/destroy). A watchdog inside the container can run `vastai destroy instance $CONTAINER_ID` when heartbeats from the owner stop. Back it with an owner-side reaper and a Vast-side backstop (prepaid credit cap).
- **Always destroy, never stop.** Stopped instances keep billing storage; a stuck/stopped instance is a leak. Billing is per-second; you are not charged while status is `Loading`, but storage and bandwidth are charged regardless of state.
- **Offer filtering is rich:** `verified`, `datacenter`, `geolocation` (country-level, supports `notin`), `inet_down`, `reliability`, `direct_port_count`, `bw_nvlink`, `cuda_max_good`, `num_gpus`, `gpu_ram`, `type` (ondemand/bid/reserved). CLI default query silently adds `verified=true rentable=true external=false`.
- **No usable confidential computing on Vast.** Vast's own CC article (March 2024) says support is not complete, only experimental hosts. Secure cloud = ISO 27001 datacenters, not a TEE. Anything needing real confidentiality must go to Azure/GCP CC VMs later.
- **Budget math:** $20 buys ~57 h small (4090), ~16 h of 1x A100, ~5 h of 2xH100, ~1.2 h of 8xH100. A frontier cold start at 1 Gbps (~90 min, ~$25) alone exceeds the monthly budget. Frontier P0 must be a short, 10 Gbps-only, NVLink-topology probe or be skipped.
- **Credit exhaustion is a hard cap with teeth:** at zero balance instances stop and are scheduled for deletion unless a card auto-charges. Use prepaid credit with autobilling OFF as the real $20 ceiling.
- Egress/ingress is billed per byte per host (`inet_down_cost`), not shown in $/hr. Filter on it.

## Q1. Vast REST API and CLI: search, create, destroy, labels

Base: `https://console.vast.ai/api/v0/`. Auth: `Authorization: Bearer <key>`. Docs index: https://docs.vast.ai/llms.txt

**Offer search** (`POST /api/v0/bundles/` / `vastai search offers`; https://docs.vast.ai/api-reference/search/search-offers , https://docs.vast.ai/cli/reference/search-offers). Operators: `eq neq gt lt gte lte in notin` (CLI: `== != > < >= <= in notin`; string values use underscores, e.g. `RTX_3090`).

| Need | Field |
|---|---|
| GPU model / count / VRAM (MB) | `gpu_name`, `num_gpus`, `gpu_ram`, `gpu_total_ram` |
| Interconnect | `bw_nvlink` (NVLink bandwidth), `pci_gen`, `pcie_bw` (no explicit SXM-vs-PCIe field; infer from gpu_name e.g. `H100_SXM` vs `H100_PCIE`, and check `bw_nvlink`) |
| Network | `inet_down`, `inet_up` (Mbps), `inet_down_cost`, `inet_up_cost` ($/GB), `static_ip`, `direct_port_count` |
| Trust | `verified`, `verification` (verified/unverified/deverified), `reliability` (0-1), `datacenter` (datacenter-only), `external` |
| Location | `geolocation` (country code) |
| Software | `cuda_max_good`, `driver_version`, `compute_cap`, `ubuntu_version` |
| Host resources | `disk_space`, `disk_bw`, `cpu_ram`, `cpu_cores_effective` |
| Pricing/type | `dph_total`, `min_bid`, `storage_cost`, `type`, `duration` (max/min rental duration), `rentable`, `machine_id`, `host_id` |

Note: the CLI says defaults are `external=false rentable=true verified=true` unless `-n` is passed. `vast-community` adapter must explicitly set `verified` appropriately (see Design). Whether API (not CLI) applies the same default **[UNVERIFIED]**; always send explicit filters.

**Create**: `PUT /api/v0/asks/{offer_id}/` (https://docs.vast.ai/api-reference/instances/create-instance). Body: `image`, `label`, `disk` (GB), `runtype` (ssh, jupyter, args, ssh_proxy...), `target_state`, `price` (bid, interruptible), `onstart` (limit 4048 chars; gzip+base64 for longer), `args`/`args_str`, `env` (docker flags string, `-e K=V -p 1234:1234/udp`), `image_login`, `template_hash_id`, `cancel_unavail`, `vm`, `volume_info`. Response returns `new_contract` = instance id (not `id`).

**Show**: `GET /api/v0/instances/` returns `id, label, actual_status, cur_state, intended_status, public_ipaddr, ports, direct_port_*, onstart, extra_env, image_uuid, gpu_name, dph_total, duration, start_date, end_date, geolocation, country_code, verification` (https://docs.vast.ai/api-reference/instances/show-instances).

**Destroy**: `DELETE /api/v0/instances/{id}/` (CLI `vastai destroy instance ID`); irreversible, same as the DESTROY button (https://docs.vast.ai/cli/reference/destroy-instance). Stop/start via `PUT /instances/{id}/` with state.

**Labels**: free-text `label` set at create (and updatable via `vastai label instance`). Use `m8s:<cluster-id>:<role>` for orphan reconciliation. It is a single string, no key-value tags.

**On-demand vs interruptible vs reserved** (https://docs.vast.ai/guides/instances/choosing/instance-types.md , https://docs.vast.ai/guides/instances/pricing.md):
- On-demand: fixed price, exclusive GPU, high priority, never interrupted by other renters, but **has a host-set maximum duration** (`duration`/`end_date`); at expiry the contract ends and the instance is **stopped** (data persists, storage billed).
- Interruptible: you set a bid (`price`/`--bid_price`); higher bids get priority; paused when outbid or when an on-demand renter arrives; no advance notice; data kept while paused; auto-resumes. Typically 50%+ cheaper. Paused instances still cost storage. Bad fit for mesh nodes serving traffic; maybe OK for non-critical small tier.
- Reserved: prepay for up to 50% discount; convertible from on-demand. Not needed in v0.

## Q2. Custom images, registry auth, onstart, env, and secret visibility

- **Own image**: any Docker Hub/GHCR/other registry URI in `image`. Three launch modes (https://docs.vast.ai/documentation/instances/docker-environment): Entrypoint (`runtype=args`; runs the image ENTRYPOINT/CMD plus `args`), SSH, Jupyter. SSH/Jupyter modes **inject setup scripts and replace the image entrypoint**; custom env vars are not visible in SSH/tmux/Jupyter sessions unless exported to `/etc/environment`. **Recommendation: use Entrypoint mode** so PID 1 is our binary and env is clean (loses SSH; add dropbear/SSH ourselves only for debugging, or use `ssh_proxy` runtype in P0 for debugging).
- **Private registry**: `image_login` (API) / `--login '-u USER -p TOKEN HOST'` (CLI) takes docker-login arguments. Docs page on this is thin **[UNVERIFIED in practice]**. That credential is stored in the instance record. Use a read-only, single-image pull token, or keep the image public (no secrets baked in).
- **onstart**: shell script, 4048 chars max via API (compress for more). In Entrypoint mode onstart is run by Vast's wrapper **[UNVERIFIED]**; prefer putting logic in our entrypoint.
- **Env injection**: `-e K=V` in the `env` string (also templates and account-level env vars; `vastai create env-var` values are "encrypted before storage", https://docs.vast.ai/api-reference/accounts/create-env-var, but they are still injected in plaintext into the container).

**Who can see what (CRITICAL)**

| Party | Sees env / onstart? | Evidence |
|---|---|---|
| Account owner (dashboard, API) | Yes: `extra_env`, `onstart` returned by `show instances`; template contents visible in console | show-instances schema |
| Anyone with the account API key (incl. leaked/scoped read key with instance read) | Yes, same as above | same |
| Vast.ai platform | Yes (they store and relay it) | inherent |
| Host operator | Effectively yes: root on the machine can `docker inspect` or read the container's /proc; community hosts are individuals | Vast Security FAQ says containers are unprivileged and isolated from other clients but **explicitly does not address host-level access risk or env var security** (https://docs.vast.ai/guides/reference/faq/security.md) |
| Other tenants | No (namespaced containers) | same FAQ |

Conclusion: there is **no secrets mechanism that protects against the host operator** and none that protects against the Vast control plane. Vast's own advice: do not store credentials in instances. Anything placed on a rented box must be assumed readable by that host and must be short-lived and narrowly scoped.

## Q3. Identity from inside the container, self-destruct, limits, billing

Injected env (https://docs.vast.ai/documentation/instances/docker-environment): `CONTAINER_ID`, `CONTAINER_API_KEY`, `PUBLIC_IPADDR`, `VAST_TCP_PORT_<n>`, `VAST_UDP_PORT_<n>`, `GPU_COUNT`, `VAST_CONTAINERLABEL`, `DATA_DIRECTORY`, `SSH_PUBLIC_KEY`, Jupyter vars.

- `CONTAINER_API_KEY` is a per-instance key; third-party summary of Vast docs says it can only start/stop/destroy that specific instance (https://docs.vast.ai/guides/instances/docker-environment). Docs explicitly show `vastai destroy instance $CONTAINER_ID --api-key $CONTAINER_API_KEY` from inside. Exact permission set **[UNVERIFIED]**; can it read `show instance` (to see its own label) or rewrite its label? Test in P0.
- The key is also visible to the host operator, but its blast radius is only that one instance (good).
- **Self-destroy**: yes, supported. HTTP form: `DELETE https://console.vast.ai/api/v0/instances/$CONTAINER_ID/` with `Authorization: Bearer $CONTAINER_API_KEY` **[UNVERIFIED exact HTTP form; CLI form is documented]**. Ship a static `vastai`-compatible HTTP call in our binary, not the Python CLI.
- **Provider-side auto-destroy / max duration**: on-demand offers carry a host-set `duration`/`end_date`; expiry stops (does not destroy) the instance. No documented user-settable TTL or scheduled stop/destroy at create time **[UNVERIFIED; search for `end_date`/`vastai` schedule endpoints]**. Vast has "schedule maintenance" and cron-like job features in the API for stop/start (`schedule job` endpoint) **[UNVERIFIED]**, which may offer a server-side backstop; test in P0.
- **Billing** (https://docs.vast.ai/guides/reference/faq/billing.md): per-second for active rental; not charged while `Loading`; storage charged every second the instance exists, in any state (including stopped); bandwidth charged per byte in any state. Destroy to stop all charges.
- **Zero credit**: docs say instances stop and "will be scheduled for deletion" unless balance becomes positive; with a saved card, auto-charge covers negative balance (protects instances); accounts with spend history get a grace buffer proportional to average daily spend (https://docs.vast.ai/guides/reference/billing.md). Docs are slightly inconsistent (FAQ says stopped, not deleted). Treat as: no card + no autobilling = instances die when $20 runs out. No account-level hard spend limit exists in docs besides prepaid balance **[UNVERIFIED]**.

## Q4. Networking

(https://docs.vast.ai/guides/instances/connect/networking.md)

- Instances share a host public IP; each gets a subset of ports mapped **randomly** external-to-internal. Mapping via `-p EXT:INT[/udp]` in `env`, or `EXPOSE` in the image. **Limit: 64 open ports per container.**
- **UDP works**: `-p 8082:8082/udp`; env `VAST_UDP_PORT_<n>`. QUIC/iroh therefore possible, but the external port differs from the internal one unless an **identity port** (docs say ports above 70000 keep external=internal; odd range, verify) is used. Important for iroh: the node must advertise its *external* UDP port; iroh does STUN-like address discovery and relay fallback, so it should still connect via relay at worst **[UNVERIFIED]**.
- Container can discover: `PUBLIC_IPADDR` and `VAST_TCP_PORT_*` / `VAST_UDP_PORT_*` env vars (read these to configure advertised addresses). `show instances` also returns `ports`, `public_ipaddr`, `direct_port_start/end`.
- `direct_port_count` offer filter ensures enough directly mapped ports; `static_ip` filter exists.
- Hairpin/NAT between co-located instances on the same host or same public IP: not documented **[UNVERIFIED]**. Do not rely on it; frontier tier is one instance with 8 GPUs, so no inter-instance traffic needed.
- Caddy/portal on many Vast base images fronts web apps; with our own image in entrypoint mode this does not apply (per deploy-linux-gpu SKILL notes in the mesh-llm repo).

## Q5. Confidential computing and what "secure" means

- Vast CC article (2024-03-15): "Complete support for this feature is not yet available"; some hosts are experimenting (https://vast.ai/article/Absolute-Security-with-NVIDIA-Confidential-Computing). No later GA evidence found. **Assume none** for planning; no `cc_mode` or TDX/SEV offer filter exists in the field list.
- **Datacenter status** (`datacenter=true`, aka secure cloud): requires business registration, identity verification, Datacenter Hosting Agreement, >= 5 GPU servers listed, and exactly **one** active third-party cert: **ISO/IEC 27001** (https://docs.vast.ai/host/datacenter-status.md). Vast's FAQ also mentions Tier 3/4 datacenter standards for vetted providers. No SOC 2, HIPAA, or TEE guarantee today. Docs say per-cert badges (HIPAA, GDPR, Tier) may come later.
- **verified** (machine-level): passed Vast's machine verification (hardware/identity/performance checks, https://docs.vast.ai/host/understanding-verification.md). It is NOT a privacy guarantee. It's the CLI default.
- Net: secure != confidential. A datacenter host is a vetted business under contract and less likely to be malicious, but its root can still read your memory. Model weights are public-ish; the secrets (mesh token, owner auth) need the short-lived design below.

## Q6. Geography

- Granularity: **country code** in `geolocation` (two-letter, e.g. `US`, `DE`); docs examples use `in`/`notin` country lists (https://docs.vast.ai/search , https://docs.vast.ai/cli/reference/search-offers). Show-instances also exposes `country_code`. No state/city field **[UNVERIFIED]**; the `geolocation` string sometimes appears as "City, CC" in the UI **[UNVERIFIED]**, so parse the last two letters defensively and exclude on both.
- Exclude with `geolocation notin [XX,YY,...]`.

**Conservative default denylist** (not legal advice; sources: OFAC program pages, e.g. https://ofac.treasury.gov/sanctions-programs-and-country-information and university compliance summaries from the web search):

| Basis | Jurisdictions | Country codes |
|---|---|---|
| OFAC comprehensive (long-standing) | Cuba, Iran, North Korea, Crimea / so-called DNR / LNR regions of Ukraine | CU, IR, KP, (UA subregions: not separable by country code; see note) |
| Syria | US terminated the Syria sanctions program in mid-2025 and later relaxed related statutes, but export-control and listing risk remains. Keep on denylist by default | SY |
| Conservative additions (not OFAC-comprehensive; policy/export-control/risk) | Russia, Belarus (heavy sectoral sanctions and export controls, GPU diversion concerns), also consider China (CN; Vast exposes CN hosts) as a trust/data-governance choice | RU, BY, (CN optional) |

Note: Ukraine regions Crimea/DNR/LNR cannot be excluded by country code; also exclude `UA` entirely in the default. Additional watch list (arms embargo/high risk, optional): MM, VE, AF, SD, SS, LY, YE, IQ, LB, ZW, CD, SO. Re-check OFAC at each release; list changes.

## Q7. Typical $/hr

Vast has no fixed price list; hosts set prices. I could not pull live offers (no API key allowed). Numbers below are triangulated from third-party posts of differing dates and **disagree** (one aggregator even quotes an $10.84 H100 median, which is implausible). Treat as planning ranges, and replace with live `search offers` output in P0 step 0 (free).

| GPU (per GPU, on-demand) | Community / unverified | Secure / datacenter | Sources |
|---|---|---|---|
| RTX 3090 | $0.09-0.25 | rarely listed | deploybase (Mar 2026), gpucost.org |
| RTX 4090 | $0.15-0.45 | ~$0.40-0.70 | same + Thunder Compute blog |
| RTX 5090 | $0.50-0.90 | ~$0.7-1.0 [UNVERIFIED] | Thunder Compute / spheron summary |
| L40S | ~$0.5-0.9 [UNVERIFIED] | ~$0.7-1.2 [UNVERIFIED] | not found in sources |
| A100 80GB | $0.80-1.50 | ~$1.2-1.8 | deploybase, spheron |
| H100 PCIe | $0.90-1.50 | $1.50-2.27 | spheron (Jul 2026) |
| H100 SXM | $1.50-2.50 | $1.80-2.50 | spheron |
| H200 | <$3.00-3.50 | $3.50-5.50 | spheron |
| B200 | scarce | $6-12 (scarce) | spheron, deploybase ($5-7.5) |
| 8x H100 SXM node | n/a | $14-20/hr total | spheron |
| 8x H200 node | n/a | ~$28-44/hr [derived] | derived from per-GPU |

Interruptible is ~50%+ cheaper (examples: 4090 ~$0.15, H100 ~$0.67/GPU, A100 ~$0.29 from one aggregator; low confidence).

**inet_down >= 1 Gbps frequency**: no published distribution found. Anecdotally consumer/home hosts are often 100-950 Mbps and datacenter hosts 1-10 Gbps. **[UNVERIFIED]**; P0 step 0 should run `inet_down >= 1000` counts per GPU tier and report the fraction. Expect a significant price premium and thinner supply at small tiers on community, and good coverage at datacenter. 8-GPU nodes are mostly datacenter, so likely satisfy 1 Gbps. `inet_down` is also self-reported/measured by a host speedtest and may overstate real HF/CDN throughput.

## Q8. Cost model

**Download time (ideal line rate; real-world HF/CDN throughput typically 50-80% at 1 Gbps and often capped at ~2-5 Gbps on 10 Gbps hosts [UNVERIFIED])**

| Model size | 1 Gbps ideal | 1 Gbps @70% | 10 Gbps ideal | 10 Gbps @ ~4 Gbps eff. |
|---|---|---|---|---|
| 20 GB | 2.7 min | 3.8 min | 16 s | 40 s |
| 50 GB | 6.7 min | 9.5 min | 40 s | 1.7 min |
| 150 GB | 20 min | 28.6 min | 2 min | 5 min |
| 400 GB | 53 min | 76 min | 5.3 min | 13 min |

Add image pull (~8 GB CUDA image: 1-2 min at 1 Gbps; free while Loading), mesh join (<1 min), VRAM load (1-2 min small, 3-5 medium, 8-12 frontier).

**Per-tier cold-start (1 Gbps @70%) and tier-hours per $20** (price assumptions mid-range; all inputs from Q7 and flagged low confidence):

| Tier | Config / price | Model | Cold start (billed after active) | Cold-start cost | Hours per $20 |
|---|---|---|---|---|---|
| small | 1x 4090 community, $0.35/hr | 20 GB | ~8 min | ~$0.05 | ~57 h |
| small (secure) | 1x L40S/4090 DC, $0.70/hr | 20 GB | ~8 min | ~$0.09 | ~28 h |
| medium-80 | 1x A100 80GB, $1.20/hr | 50 GB | ~15 min | ~$0.30 | ~16 h |
| medium-160 | 2x H100 SXM, $4.00/hr | 150 GB | ~37 min | ~$2.50 | ~5 h |
| frontier | 8x H100 SXM, $17/hr | 400 GB | ~90 min | ~$25 | ~1.2 h |
| frontier at 10 Gbps | same | 400 GB | ~25 min | ~$7 | 1.2 h |
| frontier 8x H200 | ~$35/hr | 400 GB | ~25 min @10 Gbps | ~$15 | ~0.57 h |

Bandwidth fees (`inet_down_cost`, $/GB) are extra and host-specific; at e.g. $0.01/GB, 400 GB = $4. Filter `inet_down_cost <= 0.02` **[UNVERIFIED typical values]**.

Key implication: model caching on the control plane side does not help (new host each time) unless we use a private mirror/S3 close to the host; a regional object-store mirror with 10 Gbps would cut frontier cold start to minutes. Consider a Vast **volume** (persistent on a specific host) only if we deliberately reuse the same host, but that conflicts with ephemerality.

## Q9. Other providers (future adapters)

| Provider | API maturity | CC GPU |
|---|---|---|
| RunPod | REST + GraphQL, serverless and pods, secrets feature, mature | No GPU CC offering known [UNVERIFIED] |
| Lambda | Simple REST (launch/terminate, SSH keys), on-demand only, good H100/B200, limited availability | No CC known |
| Hyperbolic | Marketplace-style API, newer; H100/H200 on-demand | No CC known |
| TensorDock | REST marketplace API, community-hosted hosts, host-trust issues similar to Vast community | No |
| AWS | Mature (EC2, IAM, instance profiles); P5 H100/H200 nodes | No H100 GPU TEE GA found [UNVERIFIED]; Nitro does not cover GPUs |
| GCP | Mature; A3 confidential VMs with H100 + Intel TDX (https://cloud.google.com/blog/products/identity-security/how-confidential-accelerators-can-boost-ai-workload-security) | Yes (H100, TDX) |
| Azure | Mature; NCC H100 v5 confidential VMs, GA (https://mc.merill.net/message/AZ-ga-confidential-vms-with-nvidia-h100-tensor-core-gpus) | Yes (H100, AMD SEV-SNP CPU TEE) |
| Databricks | Not a raw GPU rental API: workspace compute / serving endpoints; poor fit for mesh node | Depends on underlying cloud [UNVERIFIED] |

The three hyperscalers also offer real instance identity and scoped IAM, which solves the secret-delivery problem more cleanly (attested identity + KMS).

## Design implications for m8s

**Provider trait fields** (suggested):
- `provider_id` (`vast-community`, `vast-secure`), `trust_class` (community | verified_dc | confidential), `supports_udp`, `supports_self_destroy`, `max_ports`.
- `OfferQuery { tier, min_gpus, min_vram_total_mb, gpu_names[], require_nvlink, min_inet_down_mbps (default 1000), max_inet_cost_per_gb, min_reliability, countries_deny[], max_price_per_hr, price_mode (ondemand|bid), min_disk_gb, min_direct_ports, cuda_min }`.
- `Offer { offer_id, machine_id, host_id, gpu_name, num_gpus, vram_total, dph_total, inet_down/up, inet_*_cost, reliability, geolocation, verification, datacenter, bw_nvlink, max_duration_hours, direct_ports, driver/cuda }`.
- `Instance { id, label, status, public_ip, port_map(tcp/udp), started_at, dph_total, accrued_cost_estimate }`.
- Ops: `search`, `create(offer, image, env, onstart, label, image_login)`, `get`, `list_by_label_prefix`, `destroy` (idempotent), `balance()`, `self_destroy_credentials` (what to inject).
- Persist `accrued_cost` and an `orphan reconciler` loop keyed on the `m8s:` label prefix.

**Dead-man's switch (recommended: layered)**
1. In-container watchdog (primary): m8s agent in the image expects an authenticated heartbeat from the owner node (over the mesh/iroh channel) at least every N min; if none for 60 min (configurable, shorter during cold start e.g. 30 min cap to boot) it calls destroy with `CONTAINER_API_KEY` and exits. Retry with backoff; log destroy result.
2. Owner-side reaper (secondary): on owner boot and every few minutes, list instances by label and destroy any not in the desired state.
3. Backstops: pick offers with `duration` (max rental) <= planned lifetime where possible; prepaid-only balance with autobilling OFF so the worst case is bounded; alert on `balance` and on any instance older than tier TTL.
If the host has blocked outbound HTTPS to console.vast.ai the watchdog fails, so the owner-side reaper plus the credit cap remain essential.

**Secret delivery (recommended)**
- Never put the mesh invite token or owner auth in `env`, `onstart`, `args`, template, or image. Assume Vast and the host see all of those.
- Boot with only: a random per-instance `ENROLL_NONCE` (short TTL, single use), the control plane's public key/endpoint, and non-secret config.
- Container generates an ephemeral keypair, contacts owner over an authenticated channel (owner's iroh endpoint ID is public; container proves knowledge of nonce, owner replies with the mesh invite encrypted to the container's key, scoped to least privilege and time-limited, ideally a per-node invite that can be revoked by node id).
- Use a revocable per-node mesh credential so a compromised host can be cut off; rotate on teardown.
- Be explicit in docs that `vast-community` has no confidentiality against the host, and `vast-secure` only reduces risk. For workloads needing confidentiality, require a `confidential` trust_class provider (Azure/GCP).
- Registry: public image (no secrets inside) or read-only pull token.

**Offer-filter defaults**
- `rentable=true`, `type=ondemand`, `inet_down >= 1000` (and ideally `>= 2000` for frontier), `inet_up >= 200`, `reliability >= 0.98` (secure) / `>= 0.95` (community), `direct_port_count >= 4`, `cuda_max_good >= 12.4` (match the image), `disk_space >= model + 40 GB`, `geolocation notin [CU,IR,KP,SY,RU,BY,UA]` (+ CN by policy), `inet_down_cost` cap.
- `vast-secure`: `datacenter=true verified=true`. `vast-community`: `datacenter=false`; set `verified=true` by default (explicit), with a separate opt-in flag for `verified=false` (cheaper but higher failure rate and risk).
- Frontier: `num_gpus=8`, `gpu_name in [H100_SXM, H200]`, `bw_nvlink` present/high, datacenter only, single offer (one host).
- Sort by `dph_total` ascending, tie-break by reliability; cap `max_price_per_hr` per tier so a mispriced host never gets rented.

## Recommended $20 P0 plan (target spend ~ $14, reserve ~$6)

Setup: add $20 prepaid credit, **no card on file / autobilling off** (hard cap); create a scoped API key (`manage_instances`, no billing); fresh account key never goes into containers.

| Step | Action | Est. cost |
|---|---|---|
| 0 | Free: run `search offers` / `POST /bundles` for every tier. Record price percentiles, share with `inet_down>=1000`, `datacenter`, `bw_nvlink`, `direct_port_count`, `inet_down_cost`, countries. Fills the Q7 table with real numbers. | $0 |
| 1 | Community small: 1x 3090/4090, `inet_down>=1000`. Boot our image in entrypoint mode, check env/ports/UDP, `show instances` leaks (`extra_env`), `CONTAINER_API_KEY` self-destroy via HTTP, label updates, billing granularity, join the mesh with the enrollment flow, serve 20 GB model. ~1 h. | ~$0.5 |
| 2 | Dead-man's test: kill heartbeat, confirm self-destroy fires at the shortened timeout (e.g. 10 min); confirm owner reaper catches a forced-failure case (block outbound). | ~$0.3 |
| 3 | Secure small: `datacenter=true` 1x L40S/4090/A100, same flow, compare cold start, download speed, port behavior. ~1 h. | ~$1 |
| 4 | Medium: 1x A100 80GB or 2x H100 PCIe (secure), 50-150 GB model, measure real download throughput (1 vs 10 Gbps hosts) and VRAM load. ~1.5 h each, two hosts. | ~$6 |
| 5 | Frontier probe, time-boxed: 8x H100 SXM datacenter, `inet_down>=10000`, <= 20 min: run `nvidia-smi topo -m`, confirm NVLink, test 100 GB pull from mirror/HF. Skip serving a full 400 GB model. Proceed only if hourly <= $20 (cost <= ~$7 incl. startup). | ~$6-7 |
| 6 | Teardown audit: confirm zero instances, check storage/bandwidth line items in the invoice. | $0 |

Total ~$14-15 expected; stop and re-plan if any step exceeds 2x its estimate. Defer interruptible (bid) testing to a later month; defer a full frontier serve to a larger budget or to a mirror-backed 10 Gbps run.

## Open questions / unverified

1. Exact permissions of `CONTAINER_API_KEY` (read own instance? edit label? HTTP form for destroy).
2. Whether the REST search applies `verified=true` by default like the CLI.
3. Any API for server-side scheduled stop/destroy or max lifetime at create time; what `duration` means for on-demand in practice and the stop-vs-destroy behavior at expiry.
4. Zero-credit behavior discrepancy: docs say both "scheduled for deletion" and "stopped, not deleted"; grace period length.
5. `image_login` format and docs coverage for private GHCR; whether registry creds are visible in `show instances`.
6. Whether `onstart` still runs in Entrypoint mode and whether it is shown in the dashboard to team members.
7. Identity-port range (>70000 looks invalid for TCP/UDP; probably an internal mapping) and iroh hole-punching/relay behavior behind Vast's port mapping.
8. Hairpin/NAT between co-located instances (not needed for v0).
9. Live price distribution (all Q7 numbers are third-party, mixed dates, inconsistent), share of offers with `inet_down >= 1 Gbps`, L40S/5090 secure pricing, typical `inet_down_cost`.
10. Current Vast confidential computing status after March 2024; any host-advertised CC.
11. Whether `geolocation` has sub-country granularity; behavior for Crimea/DNR/LNR hosts.
12. AWS/RunPod/Lambda/Hyperbolic/TensorDock/Databricks CC status in table is from general knowledge, not fetched.
13. Sanctions list status (Syria especially) changes; confirm with counsel before production.
