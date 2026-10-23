# Spike S3: Confidential Compute, ZDR and Privacy Audit for m8s

Spike S3 · Author: Metal-0 spike worker (Sonnet 5.5) · Date: 2026-10-20 · Status: complete

Method: desk research only. Items marked **[unverified]** come from model background knowledge or a single secondary source and were not confirmed against primary docs. Search was shallow (about 6 queries); treat prices as ballpark.

## TL;DR

- Cryptographic ZDR against the provider is only achievable with **CPU TEE + GPU CC** (AMD SEV-SNP or Intel TDX plus NVIDIA H100/H200/B200 CC mode). Nothing else stops a host root user from reading RAM/VRAM.
- Hopper CC encrypts CPU<->GPU PCIe via bounce buffers. HBM is not encrypted and NVLink is not encrypted on H100 (per the Phala-cited benchmark paper). Blackwell adds TDISP/IDE inline PCIe encryption and NVLink encryption, with 1/2/4/8-GPU passthrough CC on HGX B200/B300 (R590 driver). Check each claim against NVIDIA docs before relying on it.
- Overhead is low for inference: roughly under 5% for typical LLM queries, about 4-8% for small models on H100. Cost is mostly PCIe transfer, not compute.
- Rentable CC GPUs exist only at hyperscalers and TEE-native clouds: Azure NCCadsH100v5 (about $6.98/GPU-h list, 1x H100 NVL), GCP A3 confidential, Phala Cloud (H100/H200/B200). **No evidence that Vast.ai or RunPod offer CC** (marketplace hosts control firmware and host; searches found nothing). Treat Vast as L0/L1 only.
- Client-verifiable design is well-trodden (Tinfoil, Privatemode/Continuum, Phala dstack, Apple PCC): TLS/session key generated in TEE, bound into the attestation report, client verifies measurement against a signed/transparency-logged expected value. We can bind the **iroh endpoint key** the same way.
- Without CC, we can only raise the bar (tmpfs, no swap, egress allowlist, analytics off, encrypted cache). Honest label: **policy + audit, not cryptographic**. A malicious host can always read memory.
- Audit suite: most checks can run in-container (self) or from the owner node (black-box, canaries, egress); only host-side facts (swap, host disk, hypervisor) need host access or attestation. Output a signed JSON report.
- ZDR levels: L0 none, L1 policy+audit, L2 datacenter-verified, L3 hardware-attested TEE. Tags propagate to routing as hard filters on request privacy class.
- Image signing: cosign **keyless (GitHub OIDC) + Rekor** for CI-built images, verified by digest; independent of mesh-llm release attestation, which only covers the mesh-llm binary.
- Secrets v0: `SecretStore` trait; default macOS Keychain, with a 0600 `.env`/file fallback. `.env` is not horrible for a single-user Mac mini, but is weaker than Keychain.

## 1. GPU confidential computing state of the art

| Item | Hopper (H100/H200) | Blackwell (B200/B300) |
|---|---|---|
| CC mode | Yes, GPU in CC mode inside a CVM | Yes |
| PCIe protection | Software bounce buffers, AES-GCM encrypted | TDISP + IDE inline encryption, no bounce buffer latency (per NVIDIA blog via search) |
| HBM encryption | No (relies on on-package isolation) | **[unverified]** not encrypted either; check |
| NVLink | Not encrypted on H100 multi-GPU; multi-GPU CC uses PPCIe mode **[unverified detail]** | NVLink encryption; 1/2/4/8 GPU passthrough CC on HGX B200 with R590 TRD1 |
| CPU TEE needed | AMD SEV-SNP or Intel TDX | same (TDX/SEV-SNP; Intel Trust Authority documents B200 attestation, April 2026) |

- **Attestation**: GPU has a fused per-device key; evidence bundle (GPU report plus CPU TEE quote) can be verified by NVIDIA NRAS (remote), by Intel Trust Authority, or locally with the nvtrust verifier SDK against NVIDIA's Reference Integrity Manifests (RIM). Local verification avoids trusting a remote service for the GPU leg. CPU leg verified separately via AMD KDS / Intel PCS certificate chains.
- **Overhead**: arXiv 2409.03992 (Phala-hosted) reports under 5% for most LLM queries, near zero for large models/long sequences, 4-8% for Llama2 7B. Blackwell benchmark paper exists (arxiv 2608.26575 as returned by search; **[unverified]** ID looks odd).
- **Multi-GPU 8x**: supported on HGX B200/B300 per NVIDIA R590 notes. On H100, 8x via PPCIe mode **[unverified]**. Practical consequence: for big models, prefer Blackwell CC; for H100 stay single-GPU or small TP.

Sources: https://arxiv.org/html/2409.03992v2 · https://developer.nvidia.com/blog/hardware-rooted-ai-security-that-wont-slow-you-down/ · https://docs.trustauthority.intel.com/main/articles/concept-gpu-attestation.html · https://docs.nvidia.com/580trd1-trusted-computing-solutions-release-notes.pdf · https://arxiv.org/pdf/2608.26575 · https://superprotocol.com/resources/gpu-cpu-tee-requirements

## 2. Who rents CC GPUs

| Provider | Offer | Ballpark on-demand | Notes |
|---|---|---|---|
| Azure | NCCadsH100v5: 1x H100 NVL 94GB, 40 vCPU Genoa SEV-SNP, 320 GiB | about $6.98/GPU-h (East US, list) | GA; real CC. https://learn.microsoft.com/en-us/azure/virtual-machines/sizes/gpu-accelerated/nccadsh100v5-series |
| GCP | A3 confidential VMs with H100 (TDX) | A3-high about $3/GPU-h plus CC surcharge **[unverified]** | https://cloud.google.com/confidential-computing/confidential-vm/pricing |
| Phala Cloud | H100/H200/B200 CC, dstack, on-demand or 1/6-month | varies **[unverified]** | Aggregator over TEE hosts; has dstack attestation tooling. https://docs.phala.network/phala-cloud/confidential-ai/confidential-gpu/overview |
| Verda (ex-DataCrunch) | Advertises confidential computing | unknown | https://verda.com/confidential-computing **[unverified]** |
| AWS | No GPU CC instance found. Nitro Enclaves have no GPU access; SEV-SNP exists on CPU instances only | n/a | **[unverified]** |
| Lambda, CoreWeave, RunPod, Vast | No CC offering found in search | Vast H100 about $0.90-1.87/h, RunPod about $2-2.8, Lambda about $3 (non-CC) | Re-check; CoreWeave/Lambda may add |

Implication: CC supply is 2-7x the price of marketplace GPUs and mostly single-GPU H100 NVL. Fits v0.5+ as an L3 adapter (Azure/GCP/Phala), not Vast.

## 3. End-to-end attested inference designs

Patterns:
- **Tinfoil**: TLS keypair generated inside enclave (tfshim); attestation doc carries key fingerprint; client SDK fetches expected measurements from GitHub releases + Sigstore (Rekor), verifies attestation locally. https://docs.tinfoil.sh/verification/attestation-architecture
- **Privatemode / Continuum**: attestation service plus proxy that verifies workers and does key exchange; clients can delegate verification to a proxy and encrypt prompts end-to-end. https://docs.edgeless.systems/continuum/attestation/trust-anchors
- **Phala dstack**: CVM image measured (RTMR/MRTD), app-compose hash in event log, KMS releases keys only to attested CVM; RA-TLS-style certs. **[unverified detail]**
- **Apple PCC**: nodes publish measurements to an append-only transparency log; client only talks to nodes whose measurement is logged; no admin shell, no persistent storage. **[from background]**
- **RA-WEBs / RA-TLS**: put quote hash in TLS cert extension or report_data. https://arxiv.org/pdf/2411.01340

### Sketch: binding to mesh-llm iroh identity

iroh NodeId = ed25519 public key. Inside the TEE the node generates the secret key at boot (never persisted, never injected by the owner).

```
 TEE (CVM + GPU CC)                         Client / peer mesh node
  gen ed25519 sk -> NodeId
  report_data = H("m8s-v1" || NodeId || nonce_or_epoch || gpu_evidence_hash)
  quote = TDX/SNP quote(report_data) + NVIDIA GPU evidence
  publish AttestationBundle {NodeId, quote, gpu_evidence, event_log, image_digest, sig_chain}
                                            |
   <-------- QUIC/iroh connect (NodeId) -----+
   serve bundle on ALPN "m8s/attest/1" (with client nonce for freshness)
                                            verify:
                                              1 quote chain -> AMD/Intel root
                                              2 report_data matches NodeId + nonce
                                              3 measurement == value signed by m8s release (cosign/Rekor)
                                              4 GPU evidence: CC mode on, RIM ok, driver version >= min
                                              5 TCB not revoked, quote age < N min
                                            then pin NodeId as tag=attested, send prompts
```

Notes:
- Because iroh already authenticates the peer by NodeId, binding the NodeId into report_data gives channel binding for free; no new TLS layer needed.
- Prompts then terminate inside the TEE. Caveat: mesh-llm routes via intermediate nodes and splits models across nodes. Pipeline-split to a non-attested peer leaks activations. Policy: attested requests only route to attested nodes end-to-end, no cross-node layer splits to untagged peers.
- Attestation gossip: add the bundle hash and tag to the node's gossiped descriptor; verifiers cache by (NodeId, epoch).
- Measured image must exclude owner-injected secrets; config comes from a measured manifest or sealed key release (KMS releases only to the measured image).
- Reproducible/measured builds are a prerequisite for meaningful step 3.

## 4. Best-effort controls without CC (community hosts)

Controls: read-only rootfs; models/KV/logs/tmp on tmpfs or an ephemeral LUKS volume with a key generated in-container and held in RAM only; swap off (cannot control host swap; at least `mlock`, `RLIMIT_CORE=0`, `PR_SET_DUMPABLE=0`); `MESH_LLM_ANALYTICS=0`, `log_store=metadata_only`, no prompt logging; egress allowlist (iptables/nftables in-container, or only mesh relay + model source); pull model weights at boot, encrypt cache with an ephemeral key; wipe on teardown (destroy instance, shred volumes, cryptographic erase by dropping key); no SSH keys beyond owner; drop capabilities, seccomp.

Honest limits:
- Host root can read process memory, GPU VRAM (via driver or debugger), `/proc`, ptrace, `docker exec`, snapshot the disk, or swap in the container runtime. Vast community hosts are random individuals; Vast shared containers run under the host's Docker daemon.
- In-container egress firewalls are enforced by the thing the host controls.
- Ephemeral keys protect only data at rest after teardown, not a live attacker.
- Result: this reduces **accidental retention and opportunistic snooping**, not a determined host. Label L0/L1.

## 5. Audit suite design

Checks (ID, what, where it can run):

| ID | Check | Runs from |
|---|---|---|
| A1 | Canary prompt: send unique token (UUID) via mesh; after, scan disk, logs, `/tmp`, HF cache, swap file, core dumps, shell history for token | inside container (agent) |
| A2 | Canary scan of telemetry: run local sink/mock for PostHog host; confirm zero requests; confirm `MESH_LLM_ANALYTICS=0` | inside |
| A3 | Egress capture: record outbound connections during run (conntrack/`ss`), compare to allowlist | inside (weak); owner node via relay-side logs (partial); host (strong) |
| A4 | Transport: pcap on the node interface; verify only QUIC to known peers, no plaintext canary bytes | inside (sees only its own netns), owner node for its own link |
| A5 | Config assertions: analytics off, log_store metadata_only, rootfs read-only, swap off (`/proc/swaps`), tmpfs mounts, caps dropped | inside |
| A6 | Image provenance: `cosign verify` by digest, SBOM (syft, SPDX/CycloneDX) match, SLSA provenance attestation (`cosign verify-attestation`) | owner node before launch + inside at boot |
| A7 | Binary provenance: mesh-llm `--require-release-attestation` | inside |
| A8 | CC attestation: verify CPU quote + GPU evidence (nvtrust local verifier) | owner node / verifier peer |
| A9 | Host facts (swap on host, disk encryption, snapshots, other tenants) | needs host access; provider declaration only |
| A10 | Model/KV residue after teardown: relaunch on same host and scan for prior canary | owner node (black-box, Vast re-rent); probabilistic |

Limit: in-container checks are self-attested; a malicious host can lie to the agent. Only A8 (hardware-rooted) and owner-side black-box checks (A6 pre-launch, A10) escape that. Run the suite at boot, periodically, and on each re-provision.

### Report schema (signed JSON)

```json
{
  "schema": "m8s.audit/v1",
  "report_id": "uuid",
  "generated_at": "2026-10-20T12:00:00Z",
  "node": { "node_id": "iroh-ed25519-hex", "provider": "vast", "offer_id": "...", "declared_zdr_level": "L1" },
  "image": { "digest": "sha256:...", "cosign_verified": true, "rekor_log_index": 123, "sbom_digest": "sha256:...", "slsa_level": 3 },
  "mesh_llm": { "version": "...", "release_attestation": "verified" },
  "attestation": { "tee": "none|tdx|sev-snp", "gpu_cc": false, "report_data_binds_node_id": false, "evidence_ref": null },
  "checks": [
    { "id": "A1", "name": "canary_residue", "status": "pass|fail|skip|warn", "vantage": "in_container|owner|host", "trust": "self_attested|black_box|hardware_rooted", "evidence": "sha256:...", "detail": "..." }
  ],
  "computed_level": "L1",
  "tags": ["audited"],
  "expires_at": "2026-10-20T18:00:00Z",
  "signer": { "kind": "node|owner", "key_id": "..." },
  "signature": "ed25519:..."
}
```

Signed by owner key (and counter-signed by node key); `computed_level` is derived by the owner, never trusted from the node.

## 6. ZDR level taxonomy

| Level | Meaning | Evidence | Typical adapters |
|---|---|---|---|
| L0 | No claim; community host | none | Vast community |
| L1 | Provider policy (no retention) plus our audit suite passes | provider ToS/DPA + A1-A7 | Vast secure cloud, RunPod secure |
| L2 | Datacenter-verified: known operator, SOC2/ISO, contractual ZDR, single tenant VM, no host-access for others | certification references + contract | AWS/GCP/Azure/Lambda non-CC |
| L3 | Hardware-attested TEE: CPU TEE + GPU CC, measured m8s image, NodeId bound | A8 verified, fresh | Azure CC, GCP CC, Phala |

Rules: adapters declare a **ceiling**; the audit computes the **effective** level (min of declared and verified). Tags: `zdr:L0..L3`, `attested`, `trusted`, `audited`. Request carries `min_zdr`; the router treats it as a hard filter (never fall back downward silently), plus an optional soft preference. Tags are advertised in the gossiped node descriptor with the signed audit report hash; clients re-verify L3 themselves instead of trusting the tag.

## 7. Image signing

- **Keyless cosign** (Fulcio cert from GitHub Actions OIDC, entry in Rekor): no key to leak or rotate; identity is the workflow (`--certificate-identity` + `--certificate-oidc-issuer`). Best default for an OSS project with public CI. Weakness: depends on Sigstore public infrastructure and needs online Rekor verification (or bundles for offline).
- **Key-based**: simple, offline, but key custody burden; use KMS/hardware token if chosen. Reasonable as a second signature for releases.
- Recommendation: keyless by default, publish the expected identity in the repo and in the client. Attach SBOM and SLSA provenance (`slsa-github-generator` or GitHub artifact attestations) as cosign attestations. Always verify by **digest**, never tag.
- Verification points: owner node before provisioning (pins digest in launch request); in-instance entrypoint can verify itself only weakly (host can bypass). Strong check = measured digest in TEE report (L3).
- Relation to `--require-release-attestation`: that flag checks provenance of the **mesh-llm binary** peers run. It does not cover our image, entrypoint, or config. Layered: image signed (ours) contains mesh-llm binary (attested upstream); node tag `attested` requires both.

Docs: https://docs.sigstore.dev/ **[unverified: not fetched]**

## 8. Secrets for the owner Mac mini control plane

| Option | Pros | Cons |
|---|---|---|
| `.env` 0600 gitignored | zero deps, simple | plaintext at rest; leaks via backups, crash dumps, accidental commits/log; readable by any same-user process |
| macOS Keychain (`security-framework` crate) | encrypted at rest, ACL per app, Secure Enclave possible for keys, no files | macOS-only; headless daemon access/unlock prompts need care (use login keychain with launchd user agent or `kSecAttrAccessible` settings) |
| 1Password CLI (`op`) | good UX, audit trail, shared | needs account/daemon and biometric or service-account token (which is another secret) |
| SOPS + age | git-friendly encrypted config, reviewable | age private key still needs storing somewhere |
| HashiCorp Vault | dynamic secrets, audit | overkill and ops burden for one Mac mini |

Recommendation: define `trait SecretStore { get, put, delete }` with backends `keychain` (default on macOS), `file` (0600 TOML/env, refuse to start if permissions looser, warn at startup), `env` (for CI), and `op` as optional. Owner signing key: generate in Keychain, non-exportable if possible (Secure Enclave supports P-256 only, so ed25519 would need software key stored in Keychain). **.env is not horrible** for v0 on a single-user machine with FileVault, provided 0600, gitignored, never logged, and with a short-lived provider API key where possible. Do not ship secrets into instances: pass only a per-boot enrollment token.

## Threat-model table

| Adversary | Control | Guarantee level |
|---|---|---|
| Provider/host operator (community) | tmpfs, ephemeral keys, egress allowlist, analytics off | None cryptographically (best-effort, L0/L1); detects only careless retention |
| Provider/host operator (datacenter) | Contract/DPA, single-tenant VM, audit | Policy (L2) |
| Provider/host operator | CPU TEE + GPU CC, measured image, NodeId bound attestation | Cryptographic given TEE trust (L3); side-channels and vendor roots of trust remain |
| Provisioning mesh node/owner | Key generated in TEE, no owner-injected secrets, client verifies measurement | Cryptographic (L3); at L0-L2 the owner can exec and read |
| Network observers | iroh QUIC E2E encryption, no plaintext canary check (A4), egress allowlist | Cryptographic for content; metadata (IP, timing, sizes) visible |
| Malicious mesh peer / split-model peer | route attested only to attested, no split with untagged peers | Policy plus verification |
| Supply chain | cosign keyless, SBOM, SLSA, digest pinning, release attestation | Provenance, not behavior |
| Telemetry | MESH_LLM_ANALYTICS=0, A2 audit | Config plus audit |

## Design implications for m8s

1. Make ZDR a first-class typed field: `ZdrLevel` on provider adapters (ceiling) and on nodes (effective, audit-derived); `min_zdr` on requests.
2. Do not market Vast as private. v0 default L0/L1 with explicit labelling; do not promise "cryptographic" until an L3 adapter exists.
3. Build the audit suite as an in-image agent plus owner-side verifier; share the report schema now so L3 plugs in later.
4. Reserve an ALPN (`m8s/attest/1`) and a descriptor field for attestation bundles; design NodeId generation inside the instance so the owner never holds the node secret key.
5. Image: keyless cosign + SBOM + SLSA, digest-pinned in provisioning; deterministic/reproducible builds as a goal for later measured boot.
6. Disable analytics by default in our image and assert it.
7. `SecretStore` trait with Keychain default and file fallback.
8. Upstream asks for mesh-llm: attestation hook for NodeId binding, a request-level policy to refuse layer splits to untagged peers.

## Open questions

- Does mesh-llm allow supplying/deriving the iroh secret key from an in-TEE source, and where does it persist it?
- Is HBM encryption present on Blackwell? Confirm in NVIDIA docs (unverified).
- Does any marketplace (Vast, RunPod) plan CC hosts? If so, how are hosts' TEE roots verified?
- GCP A3 CC GPU pricing and region/availability; AWS roadmap for GPU CC.
- Threat acceptance: are side-channel and TEE.fail style physical attacks on DDR5 TEEs (reported 2025, **[unverified]**) in scope for L3?
- Metadata privacy: who may see which node served which request?
- Sharded models: how are cross-node activations protected when all nodes are L3 but belong to different owners?
- Does the Mac mini need to retain provider API keys long-term, or can they be short-lived/scoped?
