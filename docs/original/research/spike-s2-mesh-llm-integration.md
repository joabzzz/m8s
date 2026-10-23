# Spike S2: mesh-llm integration research for m8s

Spike S2 · Author: Metal-0 spike worker (Sonnet 5.5) · Date: 2026-10-20 · Status: complete

Scope: read-only code research of `<workspace>/mesh-llm` (workspace v0.78.0, HEAD `9d97fbfa5`). Paths below are relative to that repo root. Tags: **[V]** = verified by reading code/docs; **[I]** = inference; **[U]** = unverified / needs a live test. Nothing was built or run.

## TL;DR

- **Customers can consume over the mesh with no m8s REST API.** `mesh-llm client`, the embedded SDK `MeshNode` (client mode) and the Node/Python/Swift/Kotlin bindings all expose a local OpenAI `/v1` (default `:9337`) that routes by `model` over iroh/QUIC to the serving peer. Goose, pi, OpenCode and Claude Code are supported by `mesh-llm goose|pi|opencode|claude` (writes provider config pointing at `127.0.0.1:9337`). [V]
- **Default admission is open to anyone holding the invite token.** With `trust-policy` = `off` or `prefer-owned`, passive clients may use `ROUTE_REQUEST` and `TUNNEL_HTTP` streams before admission, so the token is effectively a bearer credential for inference (`mesh/crates/mesh-llm-host-runtime/src/mesh/peer_state.rs:243-283`). Only `require-owned` / `allowlist` close that, and then every customer needs an owner keystore + node certificate. [V]
- **BLOCKER 1: a node cannot be owner-attested without holding the owner private key.** Certificates are Ed25519-signed per node endpoint id and `auth sign-node` can mint one offline, but the runtime never loads a pre-signed cert; it re-signs at every start and only if it has the keypair (`mesh/crates/mesh-llm-host-runtime/src/mesh/node/startup.rs:238-253`, `mesh/peer_state.rs:30-48`). The owner-control listener is not started unless the node itself is a verified owner (`mesh/node.rs:1111-1113`). [V]
- **BLOCKER 2: owner-control does not work under `--trust-policy allowlist`** (returns `UnsupportedTrustPolicy`, `crypto/control_plane.rs:~106`), and control is strictly single-owner (remote owner_id must equal local owner_id). No multi-owner ACL. [V]
- **BLOCKER 3 (risk): owner-control is direct-address only, no relay.** It is a separate QUIC UDP listener (default bind `127.0.0.1:0`) with relay disabled by design (`mesh/node.rs:1119-1143`). A Vast container needs a reachable UDP port plus `owner_control.advertise_addr`. Vast UDP port mapping is not documented in the repo. [V for code, U for Vast]
- **Tokens:** legacy invite = base64 JSON `EndpointAddr`, no expiry, no scoping. Signed bootstrap token (24h hard-coded, `mesh/mod.rs:44`) only exists on requirement-aware meshes and needs the originator node to hold the owner key. No single-use, no lifetime knob. A `--join-file` is re-read on every rejoin, so rotation = rewrite the file. [V]
- **No free-form node tags are gossiped.** Closest signed channel: `node_label` and `hostname_hint` inside the owner-signed node certificate. Routing is by model name; no tag/label filter exists. Encode tier in model names, not tags. [V/I]
- **Multi-GPU, one host:** `split_mode`/`main_gpu` are wired for single-node loads, `tensor_split` config key and `placement` are documented as *unwired*, advertised VRAM is the sum across GPUs, 8-GPU behaviour has no test evidence in docs. ds4 (DeepSeek V4) plugin is Apple-Silicon-only early access. [V/U]
- **Ready/drain signals exist and are good:** `GET /health` (liveness), `GET /api/status` (`runtime.daemon_state`, `llama_ready`, `peers`, `token`), SSE `GET /api/events`, `GET /api/runtime/intents`, `drain-model` over owner-control, SIGTERM => graceful shutdown + PEER_LEAVING broadcast. [V]
- **Immediate m8s workaround (my recommendation):** use a dedicated *fleet owner key* shipped to containers (accepting the blast radius), `trust-policy=require-owned`, and file the upstream gaps listed in section "Design implications".

## Q1. Client access over iroh

**How a passive client consumes inference [V]**
- `mesh-llm client --join <token>` (or `--auto`, `--discover`) starts an API-only node: no GPU/model, local OpenAI API on `:9337`, management API/console on `:3131` (`mesh/docs/MESHES.md`, `mesh/docs/CLI.md` common options `--client`). The node runtime mode `client` never loads models (`mesh/docs/USAGE.md:287-308`).
- Each node exposes the same `/v1`; the proxy peeks at the `model` field of POST bodies and forwards to the host serving that model over a QUIC tunnel (`mesh/docs/design/DESIGN.md`, "Multi-Model"). Passive nodes use stream `0x05 ROUTE_REQUEST` (route table) and `0x04 TUNNEL_HTTP` (raw TCP relay to the remote HTTP API); see the stream table in `DESIGN.md`. Proxy buffers exactly one HTTP request per connection (`Connection: close`), so clients must not rely on keep-alive.
- `GET /v1/models` lists the union of local and mesh-routable models; unknown model => 404 `model_not_found`; draining/paused => 503 (`website/src/docs/pages/runtime-lifecycle.md`).

**SDK embedded client [V]**
- Rust: `MeshNode::builder().client().join_token(t).start().await?; node.openai_client()?` (`mesh/docs/sdk/rust.md`, `mesh/crates/mesh-llm-sdk/src/embedded_node.rs:20-60,336`). Note: `openai_client()` is a plain `reqwest` client against the *embedded node's own local HTTP listener* (`api_base_url()`), i.e. the SDK starts the full host runtime in-process and talks to its loopback `/v1`. So SDK == `mesh-llm client` embedded; there is no lighter wire client. Builder knobs include `owner_key(path)`, `owner_required`, `node_label`, `trust_policy`, `trust_owner`, `bind_ip/port`, `iroh_relay(s)`, `config_path`, `isolated_config`.
- Bindings: Node (`@mesh-llm/sdk`, `Node.create({mode:'client', joinTokens:[t]})`), Python (`from meshllm import Node`, external repo `Mesh-LLM/mesh-llm-python-sdk`), Swift (SwiftPM `MeshLLM`, arm64 Apple only), Kotlin (`mesh/sdk/kotlin`). All: `node.inference.listModels() / chatCompletions() / streamChatCompletions()` (`mesh/docs/sdk/*.md`). Serve/combined modes need an installed native runtime.
- `serve_only()` nodes reject `openai_client()` (SDK boundary).

**Encryption [V for transport claim, I for end-to-end topology]**
- README and `MESHES.md` state QUIC end-to-end encrypts traffic between mesh nodes (prompts, responses, split activations) and iroh relays only forward ciphertext. [V as documented]
- Client->host is a direct iroh connection to the node that serves the model (relay-fallback only), so the TLS endpoint is the serving node. [I from DESIGN.md; not traced in code]. Caveats: (a) the serving node (Vast host) sees plaintext; there is no confidential compute or attestation of the remote runtime (`NODE_OWNER_IDENTITY.md` explicitly says certified-build admission is not runtime attestation); (b) for split-model setups the stage-0 host relays activations to other stages (not relevant for m8s since cross-host splits are out).

**Goose / pi [V]**
- `mesh-llm goose [--model M] [--port 9337]` writes `~/.config/goose/custom_providers/mesh.json` and launches Goose; `mesh-llm pi [--host H:P] [--write]` writes every mesh model into `~/.pi/agent/models.json`; same for `opencode` and `claude` (`mesh/docs/AGENTS.md`, `mesh/docs/CLI.md:743-775`). They target an HTTP `/v1` URL, so the harness machine needs a local `mesh-llm client` (auto-started for loopback targets, cleaned up on exit) or can use `--host` against any reachable node's `:9337`. Manual config: base URL `http://127.0.0.1:9337/v1`, api key arbitrary (`mesh`).
- Caveat: `:9337` of a remote node has no auth. Pointing `pi --host` at the owner Mac mini's `:9337` bypasses mesh admission entirely.

## Q2. Owner identity and delegation

**Doc status vs code [V]**
- `mesh/docs/design/NODE_OWNER_IDENTITY.md` is headed `Status: proposal`, but phases 1 and 2 are implemented: `SignedNodeOwnership`, `TrustPolicy {off, prefer-owned, require-owned, allowlist}`, `TrustStore` with trusted/revoked owners, revoked cert ids and revoked node ids, all in `mesh/crates/mesh-llm-identity/src/ownership.rs` (types :16-100, `sign_node_ownership` :246, `verify_node_ownership` :287). Gossip carries `owner_attestation` (`mesh/crates/mesh-llm-membership/src/peer_state.rs:115,370`). CLI: `auth init|status|sign-node|renew-node|rotate-node|verify-node|revoke-owner|revoke-node|rotate-owner|trust add/list/remove` (`mesh/crates/mesh-llm-commands/src/auth.rs:50-135`). The doc's Phase 3 "provisioning" is the part that is missing (see below).
- Certificate: claim = {version, cert_id, owner_id, owner_sign_public_key, node_endpoint_id, issued_at, expires_at, node_label?, hostname_hint?}, signed over canonical bytes with domain tag `mesh-llm-node-ownership-v1:`. Default lifetime 7 days (`DEFAULT_NODE_CERT_LIFETIME_SECS`), CLI `--expires-in-hours` default 168.
- Admission: `reject_direct_peer_for_policy` runs on every direct gossip frame (`mesh/gossip/admission.rs`, `mesh/gossip.rs:479`); `policy_accepts_peer` => `off`/`prefer-owned` accept anything, `require-owned`/`allowlist` need `status == Verified` (`membership/peer_state.rs:49`). `allowlist` additionally requires owner_id in the local trust store. Revocation lists are local only. Trust store file is loaded at startup and merged with `--trust-owner` (`runtime/startup_models.rs:157-161`).

**Q: can the owner issue a per-node cert for a remote node WITHOUT shipping the owner private key?**
- Cryptographically yes, operationally not yet. `auth sign-node --node-key <file> --out <file>` signs a cert for an arbitrary node key file using the owner keystore (`commands.rs:234-255`; `sign_node_certificate` in `auth.rs`). So m8s next to the owner could generate a node key, sign a cert and ship `key` + `node-ownership.json`. [V for signing, I for key generation path]
- But the runtime does not consume such a file. `init_owner_runtime` only produces an attestation `if owner_config.keypair.is_some()` and then *always re-signs fresh* (`load_or_refresh_owner_attestation`, comment: "Always sign a fresh attestation on startup when the owner key is available") and overwrites the cert file. Without a keypair, `owner_attestation = None`, `owner_summary` is Unsigned, and `maybe_start_control_listener` returns early because `local_verified_owner_id()` is None (`mesh/node.rs:1111`). `load_node_ownership` is only used by the `auth` CLI. [V]
- Therefore today the minimal path that works is: ship an owner keystore (`MESH_LLM_OWNER_PASSPHRASE` env or unencrypted) to the container and run with `--owner-key <path> --owner-required --node-label <label>`. Mitigation: use a separate *fleet owner identity* (`auth init --owner-key fleet.json`), not the admin's personal one, because a Vast host operator has root on the box and can read it. [V/I]

**Q: (a) admitted under trust-policy, (b) accept control only from the owner**
- (a) Admission of the container by the owner node: owner node runs `--trust-policy allowlist --trust-owner <fleet_owner_id>` (strong) or `require-owned` (weak: any valid self-signed owner passes). Admission of the owner node by the container: container runs `--trust-policy require-owned` (see conflict below).
- (b) Control: `verify_control_plane_peer_ownership` requires `remote_owner_id == local_owner_id` where the local owner summary must itself be verified, and rejects `TrustPolicy::Allowlist` outright (`crypto/control_plane.rs:85-140`). So the container must run `off|prefer-owned|require-owned` to accept control. Combined with BLOCKER 1, the container needs the owner key. The remote cert is verified with `TrustPolicy::Off` and still honours revoked owner / cert / node-id lists. [V]
- Effect of the BLOCKER 2 conflict: a GPU container cannot simultaneously be controlled by the owner and use an `allowlist` that excludes arbitrary other owners. With `require-owned`, any party able to mint its own owner keystore and present a valid cert gets admitted (and, because of `stream_allowed_before_admission`, may then route inference through it). Customers therefore cannot be cleanly separated from "anyone with the token" until the allowlist/control conflict is fixed upstream. Mitigation today: keep the join token secret and short-lived, plus `require-owned`, plus per-customer onboarding (customer's owner_id added to trust stores only on nodes where allowlist is feasible, i.e. NOT the controlled container fleet). [I]

**Q: delegate control to other owners (multiple --trust-owner / control ACLs)?**
- No. `--trust-owner` feeds only admission (`allowlist`); control compares exactly one `owner_id`. No ACL structure exists in `OwnerControlConfig` (`bind`, `advertise_addr` only; `mesh/crates/mesh-llm-config/src/model.rs:80`). A pattern for scoped owner delegation exists for plugin signing (`mesh/crates/mesh-llm-identity/src/plugin_delegation.rs`: owner-signed claim bound to a node cert, 24h max), which is a model for a future control-delegation claim. [V]

**Control transport specifics m8s must respect [V]**
- m8s never speaks `mesh-llm-control/1` itself. It calls its *local* owner node's loopback management API: `POST /api/runtime/control/{load-model,ensure-model,unload-model,drain-model,get-config,scan-refresh,apply-config}` with body `{endpoint, model|instance_id, profile}`; the local node dials the target (`mesh/crates/mesh-llm/src/commands/runtime.rs`, `website/src/docs/pages/api-reference.md:169-230`). Loopback-only (`ensure_loopback_control_caller`).
- `endpoint` is a control endpoint token read from the *target's* `GET /api/runtime/control-bootstrap` (loopback on the target), pinning one node. The token is just `base64url(JSON EndpointAddr)` of the control listener (`node.rs:1143`, `connections.rs:529`); m8s could synthesize it from node id + advertised address, [I] but upstream docs say never infer. Intents are session-only (lost on restart; config.toml must carry persistent state).

## Q3. Token minting

- **Legacy invite** (`Node::invite_token`, `mesh/node_identity.rs:137-175`): `base64url(JSON EndpointAddr)` of the node (id + relay URL + direct/public addrs). No expiry, no secrecy semantics ("not a bearer authorization credential" per `MESHES.md`, though see passive-client caveat above). Changes whenever the node's advertised addresses change. [V]
- **Signed bootstrap token**: only on *requirement-aware* meshes (`[mesh_requirements]` min/max version, protocol bounds or release attestation; or SDK `signed_join_tokens(true)`, which sets `min_protocol_version = 1`; `sdk/embedded_config.rs:425-440`). `sign_requirement_bootstrap_token` signs {addrs, genesis policy, mesh_id, policy_hash, `expires_at = now + 24h`} with the *originator's owner key* (`node_identity.rs:57-79`; const `SIGNED_BOOTSTRAP_TOKEN_LIFETIME_MS = 24h` in `mesh/mod.rs:44`). The node must be the policy originator (`requirement_origin_owner`), so the originator needs `owner_key`. If no valid signed token can be produced the node returns an *empty string* rather than a legacy token ("refusing to emit legacy invite token"). Tokens are cached and re-minted when expired. Requirements are immutable: changing them creates a new mesh id. [V]
- **Programmatic minting from SDK**: `MeshNode::invite_token() -> Option<&str>` is read once from `/api/status.token` at start (`sdk.rs:211,365`). For a fresh token later, poll `GET /api/status` on `node.console_url()` and read `token`. There is no `mint_token(ttl)` API. [V]
- **Knobs:** none for lifetime (hard-coded 24h), scope, audience or use-count. **Single-use: not possible.** **Revocation of an issued token:** not possible except by rotating to a new mesh (requirements change) or by trust-policy admission. [V]
- **Join-file rotation** (`MESHES.md` "Join without putting the token in argv"): `--join-file` / `MESH_LLM_JOIN_FILE` / default `<config dir>/invite.token` are re-read on *every rejoin attempt*; `--join` and `MESH_LLM_JOIN` are not. So m8s should deliver tokens by file, and a signed-token mesh needs a refresh channel to containers inside 24h (via Vast exec/ssh, or the container pulling from m8s). A node that is already connected does not need the token again until it loses connectivity. [V]

## Q4. Multi-GPU single host, MoE, VRAM

- **Runtime:** Skippy embeds a patched llama.cpp (`skippy/llama_cpp/patches/`, pinned by `upstream.txt`); CUDA lanes are separate release artifacts (cuda12, cuda13, cuda13-sm120 Blackwell; `mesh/docs/SDK.md` table, `mesh-llm runtime install cuda12|cuda13|exact:<id>`).
- **Advertised capacity** = sum of per-GPU VRAM (`survey.gpu_vram = per-GPU vec; vram = sum`, `mesh/crates/mesh-llm-system/src/hardware/mod.rs:638-652`), plus for *discrete* GPUs a 90% credit of surplus system RAM (`vram + 0.9*(ram - vram)`) unless the `host_ram_offload` config opts out (`GpuConfig.host_ram_offload`, default off for fit decisions; `model.rs:88-99`). Treat advertised `my_vram_gb` for rented boxes carefully; `--max-vram` caps it.
- **Config surface** (`mesh/website/src/docs/pages/config-reference.md:209-225`, `skippy/docs/CONFIGURATION.md:107-117`):
  - `hardware.device` (wired; e.g. `CUDA0`), `hardware.gpu_layers` (wired), `hardware.split_mode` = auto|none|layer|row|tensor (status *partial*: single-node resolution only, multi-node stages use auto), `hardware.main_gpu` (partial), `gpu.assignment` auto|pinned, `[gpu] parallel` slots.
  - `hardware.tensor_split` and `hardware.placement` (pooled/separated): **unwired** (pass validation, then fail at model load). The CLI `--tensor-split` is hidden and "deprecated" (`mesh-llm-cli/src/parser/commands.rs:802`). `hardware.cpu_moe/n_cpu_moe`: unwired.
  - `split_mode = "auto"` is passed to the native runtime as the tri-state default (`skippy-runtime/src/config.rs:55-70`), so the effective behaviour is llama.cpp's default for visible devices (layer split across all GPUs). [I; no doc or test states 8-GPU behaviour]
- **Known limits for 8-GPU hosts:** nothing documented. Cross-host splitting (Skippy stages) is supported by the product but out of scope for m8s. Within a host, `split_mode=tensor` and `row` exist as enum values. Recommend a live smoke test on an 8-GPU Vast box before promising large tiers. [U]
- **MoE / large models:** Skippy family matrix (`skippy/docs/FAMILY_STATUS.md`): MiniMax M2.7 supported (`unsloth/MiniMax-M2.7-GGUF:UD-Q2_K_XL`, sharded GGUF, 62 layers), DeepSeek3 (`unsloth/DeepSeek-V3.2-GGUF:UD-Q4_K_XL`) supported **via package-backed layer stages** ("full GGUFs too large for cheap local baseline"), GLM-4.7 Flash / GLM4-MoE, Hunyuan-A13B, EXAONE-MoE 236B (package-only), Qwen3-MoE/Qwen3-VL-MoE, OLMoE, PhiMoE, Granite-MoE.
- **DeepSeek V4 / ds4:** `mesh-llm plugins install Mesh-LLM/ds4-plugin`; "Early access: Apple Silicon, Mesh v0.77.0+"; DeepSeek V4 Flash Q2 ~81 GiB of disk and unified memory; the engine maps weights into memory; runs on one node only (`mesh/docs/plugins/dwarfstar.md`). **Not a CUDA path today.** [V]
- **VRAM rule of thumb** (`.agents/skills/deploy-linux-gpu/SKILL.md`): `model_bytes + KV` must fit; KV default Q8_0; heavy-KV models ~130 KB/token at Q8_0; with `--auto` the planner targets up to 4 slots dividing KV; pin context with `--ctx-size`. Context/slots are auto-scaled to VRAM (`mesh/docs/specs/context-and-slots-auto.md`).
- **Tuning tool:** `mesh-llm benchmark tune` (Q7) to pick ctx/batch/mmap/mlock/speculative settings on the actual box.

## Q5. Node tags / capabilities

Gossiped per-node metadata (`PeerAnnouncement`, `mesh/crates/mesh-llm-membership/src/peer_state.rs:81-140`): role, models (served/hosted/available/requested), `vram_bytes` + itemized `memory`, `gpu_name`, `hostname`, `gpu_vram`, bandwidth/TFLOPS hints, `version`, `served_model_descriptors` (capabilities such as tool-use/vision/reasoning per model), `served_model_runtime` (ctx length etc.), `advertised_model_throughput`, `inference_admission_state` (ACCEPTING/…/ALL_PAUSED), `owner_attestation` (with `node_label`, `hostname_hint`), release attestation, `lightning_offers` (pricing). [V]

- **No arbitrary key/value tags or region on nodes.** `--region` and `--mesh-name` are discovery-listing (Nostr/mDNS) fields for a *mesh*, not per-node gossip (`network/discovery.rs:144`). `--name` is a display name. [V]
- **Only signed free-text channel:** `node_label` (<= whatever string) and `hostname_hint` inside the owner-signed certificate, visible per peer in `/api/status.peers[].owner`. Because the owner signs it, a node cannot forge it. m8s could pack `provider=vast-secure;zdr=attested;tier=medium` into `--node-label`, but it only works if the node is cert-attested (BLOCKER 1 applies) and **m8s must treat it as informational: no routing code reads it.** [V/I]
- **Routing filter:** `mesh-llm-routing` has affinity/cache-aware modules only; routing is by model name, then load/latency/cache affinity, and local-first or cheapest paid peer when payments are enabled (`lightning-payments.md` "Approval and routing"). No owner/label filter. [V]
- **Recommendation:** encode tier into distinct model names (e.g. serve `Qwen3-8B` on small, `Qwen3-32B` on medium, `MiniMax-M2.7` on large) — the mesh already routes by name — and keep provider/ZDR tags in m8s' own registry keyed by node id (`/api/status.peers[].id`). Upstream ask: signed `node_tags` in the certificate claim plus a route filter.

## Q6. Payments mocking and gating

**wallet.v1 contract (`mesh/crates/mesh-llm-wallet/src/contract.rs`) [V]**
- Ops: `wallet_open`, `wallet_balance`, `wallet_transactions`, `wallet_create_invoice`, `wallet_pay`, `wallet_lookup`, `wallet_wait_for_payment`, `wallet_wait_for_arrival`. JSON shapes are additive; `WalletIdentity {wallet_id, provider, network}`; `OpenRequest{directory}`; `OpenResponse{identity, created, features}`.
- **The host currently requires `network == "mainnet"` for paid inference** (`contract.rs:57`; enforced in `mesh/crates/mesh-llm-host-runtime/src/network/payments/wallet_plugin.rs:191`). A regtest/fake wallet reporting `regtest` will be rejected by the host for paid flows. A fake can report `"mainnet"` (never touching real funds) to pass the check. [V/I]
- **Effort to write a fake wallet plugin: low.** Implement two traits in `mesh-llm-wallet`: `WalletBackend { provider_name(), open(dir) -> OpenedWallet }` and `WalletProvider { balance, transactions, create_invoice, pay -> Result<Transaction, PayError>, lookup, wait_for_payment, (wait_for_arrival default) }`, then `plugin_server::wallet_plugin(backend)` + `PluginRuntime::run` (feature `plugin-server`). ~150-250 lines. The hard parts are BOLT11 `Invoice` values (the `invoice.rs` type validates at the trust boundary, so the fake needs structurally valid BOLT11 strings; existing test helper `invoice_with_expiry` in `mesh/crates/mesh-llm-payments/src/tests.rs` shows how) and honouring `PayError::NotSubmitted` vs `Uncertain` semantics and idempotent `pay` repeats.
- **Existing test doubles:** `MockWallet` in `mesh/crates/mesh-llm-payments/src/tests.rs:80-110` (toggles for lost responses, pending, lookup unavailable, fee override, etc.; in-process, not a plugin binary) and `FakeProvider`/`FakeBackend` in `mesh/crates/mesh-llm-wallet/src/plugin_server.rs:318-360` (always declines to pay, `network: "regtest"`). No ready-made standalone fake wallet plugin and no regtest harness in the repo. Real wallet: external `Mesh-LLM/lexe-wallet` (Lexe 0.1.24, mainnet only, "example purposes only"). [V]
- **Gating "may inference route to my hardware":**
  - Free serving is default; a node with no wallet and no seller prices serves free (`lightning-payments.md` "Build defaults are not spending policy"). Spending policy default `free_only`; `mesh_payment` request field can only restrict.
  - Seller side pricing is configured via `mesh-llm wallet` and gossiped as `lightning_offers` (field 51); paid providers return HTTP 402 for unpaid ordinary remote inference. Loopback-originated requests are the only ones that can spend (a customer's own client node pays; QUIC ingress cannot).
  - Admission-level gating for m8s's "owner may open control to the mesh" and "who may route to my GPUs" is the owner trust policy (`require-owned` / `allowlist`) plus invite-token secrecy. There is no per-customer entitlement or quota, and `/v1` is unauthenticated (`api key` ignored). A "pricing=free-only" setting does not exist on the provider side; free is the absence of pricing.
- **Implication:** for the demo, m8s can run entirely free (no wallet). For paid-tier tests, build a ~200-line fake wallet plugin that reports `mainnet`, and drive the real ledger/metering path.

## Q7. Evals reuse

- `mesh/evals/README.md` + `mesh/evals/agentic-replay-engines.example.json`: whole-session **replay matrix** (`just automation-run automation replay-matrix plan|run|report`). It builds Mesh refs from git worktrees, launches `mesh-llm serve --model <m> --log-format json` locally, replays Thoughtworks agentic-coding trajectories (parquet; needs `trajectory-reader` build) at client concurrency 1/2/4, and reports decode throughput after first content, end-to-end output throughput, **TTFT**, realized in-flight concurrency, slot utilization, failures, cache %, with SVG charts, CSV/JSON/MD and artifact hashes. `--engine-config` (the example JSON) adds llama.cpp/vLLM/SGLang arms as external OpenAI servers (executable, model, tokenizer, context_size, max_concurrency). It *launches its own server*, so it cannot target an existing remote node as-is. [V]
- `mesh/evals/moa-openrouter`: preserved MoA trace evidence (JSONL) used by Rust tests; recording needs paid OpenRouter calls; not useful for m8s. [V]
- `mesh-llm benchmark tune [--model|--models <local gguf>] [--ctx-sizes ..] [--batch-sizes ..] [--json] [--apply] [--launch-args]` (`mesh/docs/CLI.md:519-545`, `.agents/skills/benchmark-tune/SKILL.md`): spawns isolated trial `mesh-llm serve` children, reports per-candidate **decode tok/s** (single request, `--max-tokens` default 128) with ctx/batch/ubatch/mmap/mlock/flash-attn/speculative sweeps; JSON includes per-trial timings (`setup_ms, readiness_ms, request_ms`). Needs the model already on local disk of the machine it runs on. Good as an on-box calibration step in the image, not as a fleet benchmark. [V]
- `scripts/run-llama-benchy-openai.sh` (env: `BASE_URL` default `http://127.0.0.1:9337/v1`, `MODEL`, `PP=128`, `TG=16`, `DEPTH`, `CONCURRENCY`, `RUNS`, `FORMAT`, `SAVE_RESULT`; uses `uvx llama-benchy`): **the best fit for m8s**. It benchmarks any OpenAI `/v1` endpoint, including the owner node's mesh-routed `:9337`, at configurable prompt/gen sizes and concurrency (TTFT and tok/s per llama-benchy). Doc: `skippy/docs/LLAMA_BENCHY.md`. [V]
- `skippy/evals/*`: `skippy-competitive-benchmark.json` (llama-benchy pp=512, tg=8/64/256, concurrency 1..256, pinned Thoughtworks subset; driven by `replay-matrix competitive-plan|run|report`), `skippy-capacity-acceptance.json` (cache eviction acceptance gates with p95 TTFT/makespan deltas), `skippy-usecase-corpus.json`, `skippy-coding-agent-loop.jsonl`, `corpora/` (prompt sets), `moe/prompts`. These are release-engineering gates for Skippy internals, only the corpus files are cheaply reusable. [V]
- **How m8s could drive it:** after a node is Ready (Q10), run `scripts/run-llama-benchy-openai.sh` with `BASE_URL=http://127.0.0.1:9337/v1 MODEL=<name> CONCURRENCY="1 4 16" SAVE_RESULT=...` on the owner host (traffic crosses iroh to the new node), then store TTFT/tok/s per node in m8s. Pin the model so routing hits the new node (or temporarily drain others). For a quality/agent check, run the tool-call reliability probe: `cargo xtool automation stability tool-call --base-url http://127.0.0.1:9337/v1 --models <m> --attempts 3` (`mesh/docs/AGENTS.md`). `mesh-llm benchmark tune --json` inside the container is optional pre-flight to choose ctx/batch.

## Q8. Console and topology view

- **What it shows [V]:** embedded React UI (`mesh/crates/mesh-llm-ui`) served from the management port `:3131`. Network/dashboard features include a `MeshViz` topology canvas (`features/network/components/MeshViz*.tsx`: layout, node hover cards with GPU/models, lifecycle animations for nodes joining/leaving, traffic animation via `useMeshVizTraffic`), a Peers table, model catalog, plus Chat, Logs (request ledger), Configuration, Plugins and Status tabs. Joining nodes appear live (UI is fed by SSE: `LiveStatusConnector.tsx`, `runtime-events-v1-sse.ts`).
- **Run on the owner node for a demo [V]:** `mesh-llm serve ... ` (console on by default, `--console 3131`); `--headless` hides the UI but keeps `/api/*`. Loopback by default; to view from another machine use `--listen-all` (affects HTTP API/console only, not QUIC) and put it behind Tailscale/SSH tunnel because **/v1 and the management API have no auth**; control routes remain loopback-only regardless. Open `http://localhost:3131`.
- **Streams for m8s [V]:**
  - `GET /api/events` (SSE, `Content-Type: text/event-stream`): sends the full `/api/status` JSON immediately, then a new full snapshot whenever status/models/routing/processes/inventory/plugins change (`api/routes/runtime.rs:1675-1715`). Simple to consume; payload is the whole status each time (peers list including new nodes, models, `daemon_state`).
  - `GET /api/runtime/events/v1` (cursor-based SSE with event names `runtime_state`, `runtime_event`, `runtime_health`, `runtime_replay_gap`; cursor `rt1:<uuid>:<seq>`; resumable) is the richer, versioned contract (fixtures in `mesh/crates/mesh-llm-runtime-event-contracts/fixtures/runtime_events_v1/`). Prefer this for a robust integration; it is allow-listed and privacy-scrubbed.
  - `GET /api/status` peers array (id, role, hosted models, GPU, VRAM, owner summary), `GET /api/discover`.
- Caveat: these show what the *owner node* sees. A passive client or node outside the gossip scope may not be listed. Idle transitive clients are filtered out of peer lists (`gossip.rs:~1170`).

## Q9. Headless container config

**Best non-interactive invocation [V/I]**
```bash
export MESH_LLM_ANALYTICS=0 DO_NOT_TRACK=1 MESH_LLM_NO_DEFAULT_PLUGINS=1
export MESH_LLM_OWNER_PASSPHRASE=...   # only if the fleet keystore is encrypted (BLOCKER 1)
exec mesh-llm serve \
  --model unsloth/Qwen3.6-27B-GGUF:UD-Q4_K_XL \
  --join-file /var/m8s/invite.token \
  --owner-key /var/m8s/fleet-owner.json --owner-required --node-label m8s-vast-xyz \
  --trust-policy require-owned \
  --headless --console 3131 --port 9337 \
  --bind-port 47916 \
  --config /var/m8s/config.toml
```
- Do NOT pass `--auto` (it joins the *public* mesh via Nostr; also switching private<->public wipes node/mesh identity per `MESHES.md`). Use `--join-file`. `--headless` only hides the web UI (SKILL.md calls out misuse as a recurring mistake; here it is legitimately used to shrink the surface).
- `--bind-ip/--bind-port` pin QUIC to a fixed UDP port for Vast port forwarding; `--max-vram`, `--ctx-size`, `--name`, `--llama-flavor cuda` available. Relay: default managed iroh relays; override flags (`--relay URL`, `--relay-auth`) are hidden lab flags. SDK exposes `iroh_relay(s)`/`iroh_relay_auth`.
- **config.toml sketch** (`website/src/docs/pages/config-toml.md`, `mesh/docs/USAGE.md`):
```toml
version = 1
[runtime]
mode = "serve"
startup_failure_policy = "fail_fast"   # let supervisor restart; default best_effort leaves a degraded daemon
drain_timeout_secs = 30
drain_timeout_max_secs = 300
[gpu]
assignment = "auto"
[owner_control]
bind = "0.0.0.0:7447"                  # UDP; default is 127.0.0.1:0
advertise_addr = "<vast_public_ip>:<mapped_udp_port>"
[analytics]
enabled = false
[telemetry]                             # optional: send metrics to an m8s OTLP collector
enabled = true
endpoint = "https://m8s-otel.example"
export_interval_secs = 15
[[models]]
model = "unsloth/Qwen3.6-27B-GGUF:UD-Q4_K_XL"
[models.model_fit]
ctx_size = 65536
[models.hardware]
split_mode = "layer"
```
- **Telemetry/analytics:** `[telemetry]` is off unless `enabled=true` with an explicit endpoint; OTLP/HTTP metrics only (`mesh_llm_requests_inflight`, `mesh_llm_loaded_models`, `mesh_llm_model_launch_*`, `mesh_llm_model_request_total`, route attempts; no prompts/hostnames/raw node ids; `docs/plugins/telemetry.md`). **Analytics** (maintainers' PostHog) is *on by default in official builds*: disable with `[analytics] enabled=false`, `MESH_LLM_ANALYTICS=0`, `DO_NOT_TRACK=1`, or `mesh-llm analytics disable` (`website/src/docs/pages/analytics.md`). Disable it in the image for ZDR claims.
- **Runtime install for CUDA [V with a doc discrepancy]:** the installer (`install.sh`) fetches the host archive and then runs `mesh-llm setup`, which owns runtime selection; the `MESH_LLM_INSTALL_FLAVOR` env var is **deprecated and ignored** by current `install.sh` (the Vast skill still mentions it; the skill is stale). Use `mesh-llm runtime install cuda12|cuda13|exact:meshllm-native-runtime-linux-x86_64-cuda13-sm120` or `--bundle-dir` for offline images; env overrides `MESHLLM_NATIVE_RUNTIME_ARTIFACT_DIR|_DIR`, `MESH_SDK_NATIVE_RUNTIME_DIR`, `MESH_LLM_NATIVE_RUNTIME_MANIFEST_URL`. Release archives: Linux CUDA, CUDA Blackwell, ARM64 CUDA (README). For a baked image: install host + `runtime install cudaNN` at build time so first boot downloads only the model.
- **No GPU Dockerfile in repo** (only `mesh/deploy/docker/Dockerfile.client`, CPU/no-model, entrypoint `entrypoint.sh` with `APP_MODE`, and a Fly console image). The client image shows the expected ports (3131, 9337) and `--listen-all` usage.
- **Model download:** mesh-llm resolves/downloads HF refs itself on first start into the HF cache (`~/.cache/huggingface`); set `HF_HOME` to a persistent volume; pre-baking weights is possible but not required. Use immutable refs (`@<sha>`) for reproducibility.
- **Vast notes from `.agents/skills/deploy-linux-gpu/SKILL.md` (summary):** mesh-llm resolves the model itself (do not pre-download/scp); installer lands `~/.local/bin/mesh-llm`, not on non-interactive PATH, use full path; RTX 50xx needs CUDA 13 + recent driver; model load takes minutes (poll VRAM + ports `9337`/`3131` bound; empty `/v1/models` early is normal); keep alive with Vast's supervisor (`/etc/supervisor/conf.d/mesh-llm.conf`, `autorestart=true`, `startsecs=10`, `stopwaitsecs=30`) or `systemd --user` via `mesh-llm setup --service` (+ `loginctl enable-linger`); first SSH after boot may fail with a retry banner (retry on exit 255); Vast images run extra services so idle load average is normal; Vast pre-allocates external ports and fronts web apps with Caddy, so 9337/3131 are not publicly reachable by default, use the documented external port mapping or an SSH `-L`; logs in `/var/log/mesh-llm.log`, `~/.mesh-llm/runtime/<pid>/logs/skippy-native.log`; identity `~/.mesh-llm/key`; stop with `mesh-llm stop` / `supervisorctl stop`. The skill's example uses `--auto` (public mesh): not what m8s wants.

## Q10. Health and lifecycle signals

- **Liveness:** `GET :3131/health` => always 200 once the management process answers: `{status, mode: worker|client|serving, mesh:{status: standalone|connected|disconnected, admitted_peer_count, connected_peer_count}, serving:{status: healthy|starting|idle|not_applicable|degraded|unhealthy, models:[...]}}`. Not an inference readiness result. The OpenAI port also has `/health`, `/healthz`, `/readyz` (`api-reference.md:32-85`). [V]
- **`runtime.daemon_state` (in `/api/status`):** `starting` -> `ready_idle` -> `ready_proxying` / `ready_serving`, with `degraded` (terminal model failure) and `stopping`; precedence `stopping > degraded > ready_serving > ready_proxying > ready_idle > starting`. Capability flags: `worker_capable, local_serving, proxying, plugin_ingress, accepting_local, accepting_remote`. Also `llama_ready`, `node_state/node_status`, `local_instances`, `runtime.lifecycle_instances[{instance_id, model_ref, state}]`, `runtime.intent_summary`. [V]
- **Recommended m8s "Ready" predicate [I]:** (1) container `/health` 200 and `serving.status == healthy` with the expected model in `serving.models`; (2) `daemon_state == ready_serving` and `llama_ready`; (3) from the OWNER node's view: the new node id appears in `/api/status.peers` as admitted with `hosted_models` containing the model and `inference_admission_state` ACCEPTING (or unspecified); (4) end-to-end canary: a 16-token chat completion for that model through the owner's `:9337` whose response `model` matches. Use (3)/(4) as the true Ready since it proves mesh admission + routing (model load can take minutes; early empty `/v1/models` is normal).
- **Failure signals:** `daemon_state=degraded`, `serving.status in {degraded, unhealthy}`, `/api/runtime/intents[].last_error`, `startup_failure_policy=fail_fast` exit (supervisor restarts), peer missing from owner's peers (stale after ~PEER_STALE_SECS, dead-peer TTL; `membership/peer_state.rs`), 404/503 on canary. Node health on owner side is also tracked by heartbeat / PEER_DOWN (`0x06`) broadcasts.
- **Drain semantics [V]:** `drain-model` (owner-control or local `DELETE /api/runtime/instances/<id>`): rejects new work for the instance (503 `service_unavailable` to routers), lets admitted work finish, unloads at zero in-flight, force-cancels at `drain_timeout_secs` (default 30, owner-command cap `drain_timeout_max_secs` 300). Acknowledgement means *queued*, not done; poll `/api/runtime/intents` and `/api/status.runtime.lifecycle_instances` (`draining` -> gone) and `/v1/models`. Alternative coarse control: `runtime.activity` pause (`pause_all`) with `PUT /api/runtime/activity/override` advertises `ALL_PAUSED` so peers route away (loopback API only, requires access to the container). Owner-control intents are session-only.
- **Graceful leave [V]:** SIGTERM/SIGINT handlers are installed process-wide (`runtime/shutdown_signal.rs`, fix #1812); shutdown triggers `broadcast_leaving` which sends `PEER_LEAVING (0x07)` on every connection (`mesh/heartbeat.rs:953-969`) so peers drop the node immediately instead of waiting for stale timeouts. `mesh-llm stop` does the same for tracked instances. Vast `destroy` is SIGKILL-like: always drain, then SIGTERM, wait for `stopping`/process exit (`stopwaitsecs`), then destroy.

## Design implications for m8s

### Bootstrap / auth handshake recommendation

Goal: only the owner node controls the container; container joins the private mesh; no leakage of the admin's personal identity.

1. **Create a dedicated fleet owner identity** on the Mac mini: `mesh-llm auth init --owner-key ~/.m8s/fleet-owner.json --no-passphrase` (or passphrase delivered via `MESH_LLM_OWNER_PASSPHRASE`). Run the *owner/control node itself* with `--owner-key` = this fleet identity (control requires `remote_owner_id == local_owner_id`), `--owner-required`, `--node-label owner-mini`. This confines the blast radius of a Vast-host key theft to the m8s fleet (the Admin's personal owner key never leaves). Rotate/revoke the whole fleet identity on compromise (`auth rotate-owner`, `revoke-owner` in the owner node's trust store).
2. **Mesh creation**: owner node creates an unpublished mesh (no `--publish`, no `--auto`). Preferred: make it requirement-aware (`[mesh_requirements]` with `min_protocol_version=1`, or SDK `signed_join_tokens(true)`) so tokens are signed with 24h expiry; otherwise legacy token. `trust-policy`: `require-owned` on both sides (not `allowlist`, see gap 2).
3. **Per-container provisioning (m8s controller)**: (a) m8s calls vast.ai to create the instance with the m8s image, exposing UDP for QUIC (`--bind-port`) and for `owner_control` (see gap 3), and TCP none (keep 9337/3131 closed); (b) m8s writes into the container (via Vast env/onstart or a one-shot fetch over TLS from m8s, never persisted in image): `invite.token` (join file), fleet owner keystore, `config.toml` with `owner_control.advertise_addr` = `<public ip>:<mapped port>` (known after Vast reports port mapping), model ref, `node_label` (e.g. `m8s:<tier>:<provider>`); (c) container starts `mesh-llm serve` as in Q9.
4. **Control**: m8s reads the container's control endpoint token (it can compose it from node id + advertised addr, or fetch `GET /api/runtime/control-bootstrap` via Vast exec), then calls the *local owner node* `POST /api/runtime/control/{ensure-model,drain-model,unload-model}`. Wait for Ready (Q10). Never expose the owner node's `:3131`.
5. **Token hygiene**: deliver tokens only through the join-file path (not argv/env logs). If signed tokens are used, refresh the file before the 24h expiry for all live containers.
6. **Customer path**: customers run `mesh-llm client`/SDK with the invite token. Until upstream fixes, with `require-owned` they also need *any* owner keystore (self-generated is fine) because passive streams are blocked pre-admission under enforcing policies; m8s should hand each customer a token + instructions to `auth init`. If m8s later wants named customers, move the owner/customer-facing edge to an `allowlist` node (the Mac mini) and keep the fleet containers `require-owned`: the Mac mini can run `allowlist` only if it does not need to control containers... which it does (gap 2). So fix gap 2 first.

### Required upstream gaps (to file/track against Mesh-LLM/mesh-llm)

1. **Key-less node attestation:** `--node-ownership <file>` (or auto-load `~/.mesh-llm/node-ownership.json`) used when no owner keystore is present; verify it against actual endpoint id; mark owner_summary Verified; start the control listener with it (needs the trusted owner id, not a keypair). Also `mesh-llm auth node-id [--node-key f]` and node-key generation to pre-sign offline. (Phase 3 "provisioning" in `NODE_OWNER_IDENTITY.md`.)
2. **Control under `allowlist`:** allow `TrustPolicy::Allowlist` in `verify_control_plane_peer_ownership` (owner equality already enforced), and add an explicit control ACL (`[owner_control] allowed_owners = [...]`).
3. **Owner-control over relay or an explicit UDP-less path:** control listener disables relay due to shared endpoint id (`node.rs:1119-1143`). A relay-capable control (distinct control endpoint id, or ALPN on the main endpoint) is needed for NATed/Vast containers without UDP port maps.
4. **Token API:** `POST /api/token` (loopback) / SDK method to mint signed tokens with configurable TTL, audience and single-use nonce; revocation list for tokens; make invite tokens bearer-unusable for passive streams unless owner-attested (or document that they are).
5. **Node tags:** signed `node_tags`/`capabilities` map in the owner certificate claim or announcement + route filter by tag.
6. **Multi-GPU:** wire `hardware.tensor_split` and `placement`; document and test 8-GPU single-host loads (layer vs row vs tensor); CUDA multi-GPU smoke in CI.
7. **GPU Dockerfile + entrypoint** (CUDA 12/13 + `mesh-llm runtime install cudaNN` baked, non-root, HEALTHCHECK on `/health`), and update the stale `MESH_LLM_INSTALL_FLAVOR` mention in `deploy-linux-gpu` skill.
8. **Wallet:** regtest/fake network acceptance behind a flag (host forces `mainnet`), and a reference fake wallet plugin.
9. **Revocation propagation:** revoke lists are local per node; m8s must push trust-store updates to nodes (or upstream a gossip/CRL).
10. **Vast UDP:** verify Vast.ai exposes UDP port mappings (outside this repo) or provide a TCP/WebSocket transport option for control.

### Image entrypoint sketch (`/entrypoint-m8s.sh`)

```sh
#!/bin/sh
set -eu
# inputs injected by m8s: M8S_MODEL, M8S_NODE_LABEL, M8S_CONTROL_ADVERTISE (ip:port), M8S_BOOT_URL, M8S_BOOT_TOKEN
export MESH_LLM_ANALYTICS=0 DO_NOT_TRACK=1 MESH_LLM_NO_DEFAULT_PLUGINS=1 HF_HOME=/workspace/hf
mkdir -p /var/m8s && umask 077
# one-shot fetch of join token + keystore + config over TLS from m8s (short-lived bearer, single use)
curl -fsS -H "Authorization: Bearer $M8S_BOOT_TOKEN" "$M8S_BOOT_URL" | tar -x -C /var/m8s
nvidia-smi -L || { echo no-gpu >&2; exit 1; }
exec mesh-llm serve --model "$M8S_MODEL" \
  --join-file /var/m8s/invite.token \
  --owner-key /var/m8s/fleet-owner.json --owner-required --node-label "$M8S_NODE_LABEL" \
  --trust-policy require-owned \
  --headless --console 3131 --port 9337 --bind-port 47916 \
  --config /var/m8s/config.toml
# image: nvidia/cuda runtime base + mesh-llm host + `mesh-llm runtime install cuda13`; run under tini; HEALTHCHECK curl -f localhost:3131/health;
# stop: SIGTERM, stop_grace_period >= 60s (drain + PEER_LEAVING); m8s drains via owner-control before destroy.
```

## Open questions

1. Does Vast.ai (the specific offers m8s targets) allow inbound UDP port mappings? If not, can the mesh QUIC data path still work via iroh relay (likely yes) and how is owner-control reached (no relay)? Needs a 15-minute live test on one cheap Vast box.
2. Is node key generation/injection supported offline (`MESH_LLM_...` env or `~/.mesh-llm/key` format) so m8s can learn a node's endpoint id before boot? `auth rotate-node` exists but was not traced.
3. With `require-owned` on the container, can the Mac mini owner node (fleet identity) and customers be admitted without further config? Expected yes; needs a 3-node local smoke test (owner, container stand-in, client).
4. Exact 8-GPU behaviour for `split_mode=auto` on CUDA (does it layer-split all 8 visible devices; does the planner's fit estimate use summed VRAM correctly; any per-device overhead)? Needs a live run.
5. Where does a Vast host operator's visibility end: with a keystore and plaintext prompts on the box, ZDR tags (`zdr=attested`) can only be provider-asserted, never cryptographically attested by mesh-llm (release attestation is build provenance only).
6. Are passive-client streams really admitted pre-gossip for SDK clients under `off`/`prefer-owned` in the current release (comment says yes at `peer_state.rs:243-283`; confirm with a test from a non-owner client)?
7. Token refresh model for 24h signed tokens: will m8s push new join files to all live containers, or run a legacy (non-expiring) token and rely on trust policy?
8. Does `/api/events` need auth if the owner node is exposed via `--listen-all` for the demo console? (No auth in code; use a tunnel.)
