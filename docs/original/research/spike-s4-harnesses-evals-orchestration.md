# Spike S4: Agent harnesses, evals, orchestration (Goose, pi, benchmarks, models, kube-rs vs virtual-kubelet, cmux)

Spike S4 · Author: Metal-0 spike worker (Sonnet 5.5) · Date: 2026-10-20 · Status: complete

Method: desk research only (web search/fetch + read-only look at <workspace>/mesh-llm). Items marked **[unverified]** came from secondary sources or search snippets and need a hands-on check. Web-derived model sizes/benchmarks are from late-2026 blog/HF pages and move fast.

## TL;DR

- **Goose** moved: canonical repo is now `github.com/aaif-goose/goose` (Linux Foundation Agentic AI Foundation, donated by Block Nov 2025). Rust, Apache-2.0, ~55k stars. Headless via `goose run -t/-i/--recipe --output-format json --no-session`. Embedding is via its ACP server (Agent Client Protocol, JSON-RPC) plus a TypeScript SDK wrapping it; there is no obvious pure-Rust "gdk" embed story **[unverified]**.
- **pi** moved to `github.com/earendil-works/pi` (was badlogic/pi-mono), MIT, TypeScript, npm `@earendil-works/pi-coding-agent` (Node 22.19+). Modes: TUI, print (`-p`), `--mode json`, `--mode rpc` (JSONL over stdio), TS SDK. Custom OpenAI-compatible provider = a few lines in `~/.pi/agent/models.json`. mesh-llm already writes this for us (`mesh-llm pi --write`).
- **Both harnesses are already wired to mesh-llm** (`mesh-llm goose`, `mesh-llm pi`, nightly `--agent-smokes opencode,pi,goose`). m8s should drive them as black-box subprocesses (CLI/RPC), not embed SDKs, in v0.
- **Task suites:** do not depend on Goose bench or Terminal-Bench for v0. Use a small in-house canonical task pack (4 tasks per tier, scripted pass checks), and use BFCL / Aider polyglot / FLORES as the quality gates.
- **Throughput metric:** reuse mesh-llm's `scripts/run-llama-benchy-openai.sh` and `replay-matrix` for agentic-realistic load; add GuideLLM (Apache-2.0, vllm-project) as the generic N-concurrent-users sweep. Skip llmperf (stale) and genai-perf (superseded by AIPerf).
- **Quality evals:** BFCL (`bfcl-eval`, `--skip-server-setup` against any OpenAI endpoint) is cheap; Aider polyglot (225 exercises, Docker) is the right coding gate; SWE-bench Lite is heavy (Docker, >=120 GB disk) so defer; FLORES-200 is a ~30-line script with sacrebleu chrF++.
- **Models:** small = Qwen3.6-27B / Gemma-4-E4B; medium = Qwen3-Coder-Next (~48-53 GB), MiniMax-M2.x (~138-152 GB) or DeepSeek-V4-Flash (155 GB Q4); frontier = DeepSeek-V4-Pro (~850 GB UD-Q4_K_XL, MIT), GLM-5.2 (MIT, 372-475 GB at Q4), Kimi K2.6 (584 GB, modified MIT). Frontier models are NOT in mesh-llm's catalog/auto-packs today; need catalog entries and a skippy/llama.cpp support check.
- **Orchestration recommendation for v0:** kube-rs operator with CRDs (option A), finalizers for guaranteed teardown, `additionalPrinterColumns` so `k9s` shows the fleet. Virtual-kubelet has no Vast/RunPod provider and breaks too many pod semantics. A plain Rust daemon + SQLite is the fallback/simplest, but loses the k9s story the Admin wants.
- **cmux** = macOS Ghostty-based terminal for agents, scriptable by CLI + Unix socket. For a recorded demo, drive panes from a script via the cmux CLI and record with the screen recorder; use VHS `.tape` files for deterministic GIF/MP4 fallbacks. Note GPL-3.0 client and a BSL server component.

## 1. Goose (block/goose -> aaif-goose/goose)

| Item | Finding | Source |
|---|---|---|
| Home | Moved from block/goose to **aaif-goose/goose** at the Linux Foundation AAIF (with MCP, AGENTS.md) | https://github.com/aaif-goose/goose ; https://www.arcade.dev/blog/goose-the-open-source-agent-that-shaped-mcp |
| Language / license | Rust core, Apache-2.0, ~55.5k stars; desktop app + CLI | https://github.com/aaif-goose/goose |
| Providers | 15+ (Anthropic, OpenAI, Gemini, Ollama, OpenRouter, Azure, Bedrock), 70+ MCP extensions | same |
| SDK / embedding | Agent Client Protocol server (`goose-acp`, JSON-RPC 2.0) and a TypeScript SDK that wraps it, built on the ACP TS SDK. Custom distributions (preconfigured providers/extensions) supported. | https://mintlify.com/block/goose/advanced/acp-protocol ; https://agentclientprotocol.com/libraries/typescript **[unverified: exact SDK package names]** |
| Custom provider | JSON file in `~/.config/goose/custom_providers/` (mac/Linux). mesh-llm writes `mesh.json` automatically via `mesh-llm goose`. Manual alternative: `GOOSE_PROVIDER=openai GOOSE_MODEL=<id> OPENAI_HOST=http://127.0.0.1:9337 OPENAI_API_KEY=mesh` | local: mesh/docs/AGENTS.md lines 28-60 |
| Headless | `goose run -t "text"`, `-i file.md`, `--recipe recipe.yaml`, `--params k=v`, `--no-session`, `--output-format json`, `--max-turns`, `--provider/--model`, `--with-builtin developer` | https://goose-docs.ai/docs/guides/running-tasks |
| Recipes | YAML workflow files; `goose recipe validate/list`, `goose schedule` (cron) | https://goose-docs.ai/docs/guides/goose-cli-commands |
| Bench | `goose bench` exists (JSON config of models x eval selectors; `goose bench selectors` lists suites) but the docs page 404'd on direct fetch and it appears lightly maintained. Treat as inspiration only. | https://goose-docs.ai/docs/tutorials/benchmarking (search snippet) **[unverified]** |
| Storage | Sessions in SQLite `sessions.db` | goose CLI docs |

For m8s: spawn `goose run --no-session --output-format json -t <task>` with `OPENAI_HOST` pointed at the mesh. Parse JSON output and check the workspace for pass criteria.

## 2. pi coding agent

- Now `github.com/earendil-works/pi` (monorepo packages `pi-ai`, `pi-agent-core`, `pi-coding-agent`, `pi-tui`, ...). Previously badlogic/pi-mono (Mario Zechner). MIT. Minimal harness: 4 built-in tools (read, write, edit, bash); no sub-agents/plan mode by design; extensions, skills, prompt templates as npm packages. https://github.com/badlogic/pi-mono (redirects) ; https://github.com/earendil-works/pi **[unverified redirect]**
- Install: `npm i -g @earendil-works/pi-coding-agent` (Node 22.19+) or `curl -fsSL https://pi.dev/install.sh | sh`.
- Modes: interactive; `-p` print; `--mode json`; `--mode rpc` (JSONL stdin/stdout, events like `message_update`, `agent_end`, `tool_execution`); TypeScript SDK. `--no-session` for one-offs.
- Custom provider: `~/.pi/agent/models.json`:
  ```json
  {"providers":{"mesh":{"baseUrl":"http://host:9337/v1","api":"openai-completions","apiKey":"mesh","models":[{"id":"Qwen 3.6 27B"}]}}}
  ```
  `mesh-llm pi --write [--host h:9337]` writes this (incl. `contextWindow`/`maxTokens`) and `pi --provider mesh --model <id>` selects it (local mesh/docs/AGENTS.md lines 131-210). Docs warn to enable `compat` flags only for verified endpoint quirks. https://raw.githubusercontent.com/badlogic/pi-mono/main/packages/coding-agent/docs/models.md
- Headless for tasks: `pi -p "<task>" --provider mesh --model <id> --no-session` or RPC for a long-lived worker with streamed events (good for the TUI/web dashboard to show live tool calls).

## 3. Prior-art task suites and recommended canonical tasks

Prior art (all open-source):

| Suite | Fit | Notes |
|---|---|---|
| Terminal-Bench 2.0 (89 tasks, Harbor harness, LiteLLM backend) | Coding/terminal agents | Heavy (Docker per task, long horizon). Good later for frontier tier. https://tbench.ai/docs ; https://docs.litellm.ai/docs/projects/Harbor |
| tau2-bench | Tool use + policy in multi-turn | API-model only; expose local model as OpenAI endpoint. https://evalscope.readthedocs.io/en/latest/third_party/tau2_bench.html |
| BFCL v4 agentic (web search, memory, format sensitivity) | Tool calling, web search | See Q4. https://gorilla.cs.berkeley.edu/leaderboard.html |
| GAIA / BrowseComp (1,266 Qs) | Web research | Too hard for small/medium; BrowseComp needs good search tool; frontier only. Dataset gating (GAIA on HF) **[unverified]** |
| Aider polyglot (225 Exercism tasks) | Coding | See Q4. |
| Goose bench | Goose-specific | See Q1. |

None cover Admin's four tasks end-to-end, so define a **canonical task pack** of ~4 scripted tasks per tier with machine-checkable pass criteria (our own, YAML, run identically through Goose and pi):

| Tier | Task | Pass criteria |
|---|---|---|
| Small | T1 Translate 20 FLORES-200 devtest sentences EN->FR/ES/DE/JA | chrF++ >= threshold set from a baseline model; no extra commentary (format check) |
| Small | T2 Lookup: given a local 5-doc corpus, answer 10 factual Qs via a `grep`/search tool | >= 8/10 exact-or-contains matches |
| Small | T3 Basic tool call: call a mock `get_weather(city)` JSON tool then summarize | valid tool-call JSON schema + final answer contains tool output (mirrors mesh-llm `stability tool-call` probe) |
| Small | T4 Web search: single-hop question using a stub/recorded search MCP server | answer contains ground-truth string; at most N tool calls |
| Medium | T5 Pong from scratch: "build Pong in Python/pygame or single-file HTML" | file exists; `python -c` import / headless smoke runs 5s without exception; unit test of paddle/ball physics functions; bonus: screenshot check **[our own spec]** |
| Medium | T6 Spreadsheet: given CSV, produce a pivot/summary CSV with a script | output CSV equals golden file |
| Medium | T7 Doc search/RAG: 50-doc corpus (markdown), 10 multi-hop questions | >= 7/10 correct; cites file names |
| Medium | T8 Fix a seeded bug in a small repo (Aider-polyglot-style single exercise) | hidden unit tests pass |
| Frontier | T9 Terminal-Bench subset (5 tasks, Apache-2.0) | task verifiers pass (precedent: https://github.com/minghinmatthewlam/openbench) |
| Frontier | T10 Multi-file feature in a repo (SWE-bench Lite-style, 10 instances) | FAIL_TO_PASS tests pass |
| Frontier | T11 Web research multi-hop (BrowseComp-lite, 10 Qs) | answer match; step budget |
| Frontier | T12 Pong with 2-player + AI + score + tests in one session | tests pass + lint clean |

Run each task with N=3 seeds and report pass@1 plus tokens, wall time and tool-call count.

## 4. Eval suites and load generators

| Suite | How to run vs OpenAI-compatible endpoint | Cost/time/hardware |
|---|---|---|
| **BFCL v3/v4** | `pip install bfcl-eval`; `.env`: `REMOTE_OPENAI_BASE_URL`, `REMOTE_OPENAI_API_KEY`, `REMOTE_OPENAI_TOKENIZER_PATH`; `bfcl generate --model <name> --test-category <cats> --skip-server-setup`, then `bfcl evaluate ...`. Needs chat-style endpoint with tool calling. https://github.com/ShishirPatil/gorilla (berkeley-function-call-leaderboard) | CPU-only client; thousands of short calls; ~tens of minutes to a couple hours depending on categories and concurrency **[unverified]**. v4 web-search category needs a SerpAPI key (a paid external dependency) **[unverified]**; skip that category in v0. |
| **Aider polyglot** | Aider's `benchmark/benchmark.py <run> --model openai/<id> --edit-format whole|diff --threads N --exercises-dir polyglot-benchmark` inside Docker; set `OPENAI_API_BASE`/`OPENAI_API_KEY`. https://raw.githubusercontent.com/Aider-AI/aider/main/benchmark/README.md ; https://aider.chat/docs/llms/openai-compat.html | 225 tasks; reports pass rate, seconds_per_case, cost. Executes LLM code: **must run in Docker, on an isolated box**. Hours at low concurrency; use `--threads` to double as a load test. No GPU on client. |
| **SWE-bench Lite (300) / Verified (500)** | `swebench.harness.run_evaluation` on predictions produced by an agent (mini-swe-agent, OpenHands, or pi/Goose) pointing at the endpoint. https://www.swebench.com/SWE-bench/faq/ | x86_64, >=120 GB disk (Verified Mini ~5 GB), Docker. Long agent trajectories (tens of K tokens each) make this the most expensive; defer past v0. Apple-silicon Mac mini cannot run it natively (x86 images) **[unverified]**, so run the harness on the Vast box or a Linux CI node. |
| **FLORES-200** | Dataset `facebook/flores` (HF; CC-BY-SA 4.0 **[unverified]**). Loop devtest sentences through `/v1/chat/completions` with a fixed translation prompt; score with `sacrebleu` chrF++ (and BLEU with flores200 tokenizer). Use a 10-language subset x 1012 sentences, or 200 sentences per direction for speed. | Minutes to an hour; CPU client. WMT24++ is a newer alternative (55 languages) https://huggingface.co/papers/2502.12404 |

**Throughput at N concurrent users**

| Tool | Verdict |
|---|---|
| mesh-llm `scripts/run-llama-benchy-openai.sh` + `corpus JSON` (mesh/docs/BENCHMARKS.md:54) | Reuse: already used for throughput evidence |
| mesh-llm `automation replay-matrix run` (mesh/evals/README.md) | Reuse: replays real agentic-coding trajectories (Thoughtworks dataset) at client concurrency 1/2/4, compares Mesh refs and external llama.cpp/vLLM/SGLang arms; emits JSONL/CSV/SVG TTFT+throughput charts. It starts its own `mesh-llm serve` though, so it measures a node it launches, not an already-running remote mesh **[needs check]**. |
| mesh-llm `benchmark tune` (mesh/docs/CLI.md:519) | Per-node local tuning (ctx, batch, flash-attn, speculative) before serving; run once per GPU/model combo, store the winning flags |
| mesh-llm `stability tool-call` / `stability nightly` | Reuse as the correctness smoke (tool-call streaming, Goose/pi/opencode agent smokes) against any `/v1` URL |
| **GuideLLM** (vllm-project, Apache-2.0) | Add: generic sweep/concurrent/throughput profiles against any OpenAI endpoint, TTFT/ITL distributions, JSON/CSV/HTML. https://github.com/vllm-project/guidellm (CLI flag syntax in README differs across versions; verify) |
| `vllm bench serve` | Alternative: `--max-concurrency` against any OpenAI endpoint; fine but tied to vLLM install |
| NVIDIA AIPerf | Successor of genai-perf (which is being phased out). Heavier; optional. https://developer.nvidia.com/blog/benchmarking-llm-inference-at-scale-with-aiperf |
| llmperf (Ray) | Skip (stale/archived) **[unverified]** |

**Reuse strategy:** primary metric = aggregate output tokens/s at N in {1,2,4,8,16,32} concurrent streams, measured by GuideLLM (synthetic) and by benchy/replay-matrix (agentic-shaped). Quality gate = BFCL (small), FLORES (small), Aider polyglot (medium), Aider polyglot + T9/T10 (frontier). Wrap everything in an `m8s eval` runner that shells out to these tools and writes one JSON record per (tier, model, GPU SKU, quant, N).

## 5. Model shortlist per tier

VRAM = approximate GGUF file size at Q4-class quant plus ~15% KV headroom (mesh-llm's own auto-pack uses `vram*0.85`). Sizes: mesh catalog (local file mesh/crates/mesh-client/src/models/catalog.json) or HF pages cited.

| Tier | Model | Q4 size | License | In mesh catalog / auto-pack? |
|---|---|---|---|---|
| Small (<=24 GB) | Qwen3.6-27B (UD-Q4_K_XL) | 16.4 GB | Apache-2.0 (per onyx.app) | catalog yes; auto-pack uses Qwen3.5-27B (min VRAM 24) |
| Small | Qwen3.6-35B-A3B (MoE, fast) | 20.8 GB | Apache-2.0 **[unverified]** | catalog yes |
| Small | GLM-4.7-Flash (MoE) | 18 GB | MIT **[unverified]** | catalog + auto-pack (min 50 GB tier) + FAMILY_STATUS supported |
| Small | Gemma-4-E4B-it | 4.6 GB | Gemma terms (restrictive-ish) | catalog + auto-pack (8 GB tier) + FAMILY_STATUS |
| Small | Qwen3-8B / 4B | 5 / 2.5 GB | Apache-2.0 | auto-pack fallback |
| Medium (80-160 GB) | Qwen3-Coder-Next | 48-53 GB | Apache-2.0 **[unverified]** | catalog + auto-pack (min 63 GB) + FAMILY_STATUS (Qwen3Next, KvRecurrent) |
| Medium | MiniMax-M2.5 (Q4_K_M) / M2.7 | 138-152 GB | modified MIT-style per onyx (M3) **[unverified for M2.x]** | M2.5 catalog + top auto-pack (min 179 GB); M2.7 in FAMILY_STATUS (UD-Q2_K_XL only) |
| Medium | Qwen3-235B-A22B | 142 GB | Apache-2.0 | catalog |
| Medium | DeepSeek-V4-Flash 284B/13B active | 155 GB (UD-Q4_K_XL) | MIT | not in catalog; arch `deepseek4` not in FAMILY_STATUS; HF: https://huggingface.co/unsloth/DeepSeek-V4-Flash-0731-GGUF |
| Frontier (640 GB-1.1 TB, 8-GPU) | DeepSeek-V4-Pro 1.57T/48B active | ~850 GB (UD-Q4_K_XL) | MIT | not in catalog; fits 8xH200 (1128 GB) only just, not 8xH100 (640 GB). https://huggingface.co/unsloth/DeepSeek-V4-Pro-0813-GGUF |
| Frontier | Kimi K2.6 | ~584 GB (UD-Q4_K_XL) | Modified MIT | not in catalog; fits 8x80 GB with small KV **[tight]** |
| Frontier | GLM-5.2 744B | 372-475 GB at Q4 | MIT | not in catalog; fits 8x80 GB comfortably. https://unsloth.ai/docs/models/tutorials/glm-5.2.md |

Licensing buckets: permissive = MIT/Apache (DeepSeek, GLM, Qwen3.x, Hunyuan); modified/attribution terms = Kimi, MiniMax; restrictive terms = Gemma. For a private mesh the practical risk is low, but record it per model in the m8s catalog.

Cross-check findings: mesh-llm auto-packs top out at MiniMax-M2.5 (needs >=179 GB VRAM); FAMILY_STATUS (updated 2026-08-28) lists DeepSeek3 (V3.2 package-backed), GLM4.7-Flash, Qwen3Next, MiniMax M2.7, EXAONE-MoE 236B; no entry for DeepSeek V4 (`deepseek4`), GLM-5.x or Kimi. Frontier tier therefore needs (a) a llama.cpp/skippy support check and (b) m8s-owned model refs, since the auto-pack won't choose them. Also single-host 8-GPU frontier probably runs plain llama.cpp/`mesh-llm serve` on one host; skippy stage-splitting is for multi-node.

## 6. kube-rs operator vs virtual-kubelet (educational)

**Mental model.** Kubernetes is a database of desired state plus controllers that make reality match. Both options use that idea but at different layers.

| | A: Operator + CRDs (kube-rs) | B: virtual-kubelet provider |
|---|---|---|
| What you define | New kinds, e.g. `GpuNode`, `MeshModel`, `EvalRun`. You choose the spec/status fields. | A fake **Node** that accepts normal **Pods**; your provider implements create/delete/get-status of a pod |
| Who writes the logic | A Controller: watch CR -> `reconcile(obj)` -> call Vast API -> patch `.status` -> requeue | A Go library interface (PodLifecycleHandler: CreatePod, DeletePod, GetPodStatus, GetPods, logs, exec) |
| Fit for "rent a machine, run mesh-llm in it" | Natural: the CR *is* the rental (offer id, price cap, region, image, state, ssh/ip, cost accrued) | Awkward: a Pod is "a container on a node"; our unit is a *machine lease*. Mapping Pod->Vast instance hides price/region/offer concepts in annotations. |
| Language | Rust (kube-rs, `Controller`, `finalizer` helper) | Go library; Rust would need a hand-rolled kubelet API shim **[no Rust impl known]** |
| Existing providers | n/a | Official/community: Azure ACI, HashiCorp Nomad, AWS Fargate, Alibaba ECI, OpenStack Zun, IoT Edge, Tensile-kube. **None found for Vast.ai or RunPod** (searched). https://github.com/virtual-kubelet/virtual-kubelet |
| What breaks | n/a | Pod networking (virtual pods cannot reach other nodes' pods), `kubectl logs/exec` depend on provider, volumes/init containers vary, probes, DaemonSets, service endpoints, GPU resource accounting, and the pod would not be where the GPU actually is (the model container runs on a remote host that is not routable from the cluster) |
| k9s UX | CRs appear as first-class resources | Appears as pods on a virtual node; no model/lease detail |

**Reconcile loop and status.** `Controller::new(Api<GpuNode>)` + `.owns(...)`/`.run(reconcile, error_policy, ctx)`. Reconcile must be idempotent: read spec, look up the real Vast instance (store `instance_id` in `.status` immediately after create to avoid leaks on crash), drive a state machine (`Searching -> Creating -> Booting -> Loading -> Ready -> Draining -> Terminating`), patch `.status.phase/conditions/endpoint/costAccrued/lastError`, return `Action::requeue(30s)` while transitional. Enable the `status` subresource.

**Finalizers = guaranteed teardown.** Add a finalizer (e.g. `m8s.io/instance`) on first reconcile *before* creating the external instance. On `kubectl delete`, K8s sets `deletionTimestamp` but keeps the object until the controller destroys the Vast instance and removes the finalizer. kube-rs provides `kube::runtime::finalizer` (Apply/Cleanup events). For external APIs, use finalizers (not ownerReferences). https://kube.rs/controllers/relations/ ; https://book.kubebuilder.io/reference/using-finalizers

**Owner refs.** Only garbage-collect *in-cluster* children automatically (e.g. a `MeshModel` owns its `GpuNode`s, an `EvalRun` owns its Jobs/Pods). They do not clean external instances; finalizers do. Foreground deletion order lets parents wait for children.

**Leases.** Two meanings: (1) leader election via `coordination.k8s.io/Lease` if you ever run >1 controller replica (not needed on single-node minikube; run 1 replica, `Recreate` strategy); (2) our own *rental lease*: a `spec.maxLifetime`/`idleTimeout`/`budget` that the reconciler enforces, plus a periodic orphan sweeper that lists Vast instances labelled `m8s-*` and terminates ones with no CR. This is the backstop against leaked spend.

**CRDs in k9s.** Typing `:` + the kind's plural/short name/alias (e.g. `:gpunodes`, `:gn`) opens the custom resource list; `:crd` lists CRDs; `:aliases` shows names. Columns come from the CRD's `additionalPrinterColumns` (the same ones `kubectl get` shows), so declare Phase, Model, GPU, $/hr, Age, Endpoint there; `kubectl-tree`/`describe` for status conditions. k9s docs page checked didn't state this explicitly **[unverified: confirm in k9s in spike]**. kube-rs: `#[kube(printcolumn = "...")]` on the `CustomResource` derive generates them.

**Alternative C: plain Rust daemon + SQLite.** Simplest, no cluster dependency, easiest to test (state is local, one process), full control over a rental state machine. Cost: re-implement watch/retry/finalizer semantics, no k9s/kubectl for free, no declarative GitOps; custom CLI/TUI/web must do all visibility. Reasonable if minikube proves flaky on the Mac mini.

**Recommendation (v0):** Option A with a minimal CRD set (`GpuNode` or `Rental`, `ServedModel`/`Tier`, `EvalRun`), a finalizer on every CR that owns an external resource, a hard budget/TTL field, and an orphan sweeper. Keep the Vast client behind a `Provider` trait so RunPod etc. can follow. Keep core state machine logic in a plain Rust crate that doesn't import kube (so it is unit-testable and could back option C). Do not adopt virtual-kubelet.

## 7. cmux

- cmux (github.com/manaflow-ai/cmux **[repo owner assumed; several unrelated forks named cmux exist]**) is a macOS-only, native Swift/AppKit terminal built on libghostty, aimed at running many coding agents (Claude Code, Codex, Gemini...) in parallel: vertical tabs showing branch/PR/cwd, notification rings, splits, built-in scriptable browser, SSH workspaces, session restore.
- Scripting: CLI and Unix-domain socket API to create workspaces/tabs, split panes, send keystrokes, open URLs in the browser. https://github.com/manaflow-ai/cmux ; https://betterstack.com/community/guides/ai/cmux-terminal/
- License: client GPL-3.0-or-later; server components BSL 1.1 (fine for demos, check before bundling).
- Demo plan: a shell script that (1) creates a cmux workspace, (2) splits panes: `k9s` | `m8s tui` | agent (`goose run ...` / `pi`) | browser on the dashboard, (3) uses `send` to type commands at human pace and `sleep` for beats, then (4) record the screen with macOS screen capture (`screencapture -v` / QuickTime / OBS). cmux has no documented built-in recorder **[unverified]**.
- Deterministic fallback: VHS (charmbracelet; `.tape` files, needs ttyd+ffmpeg, outputs GIF/MP4/WebM) or asciinema (`.cast`). VHS renders in its own headless terminal, so it cannot capture cmux panes, but it's good for README clips. https://github.com/charmbracelet/vhs
- Because real Vast provisioning takes minutes, pre-warm one GPU node and stage a fast 'tier switch' story, or speed up the recording in editing.

## Design implications for m8s

1. **Harness adapter = subprocess.** `trait Harness { fn run(task, endpoint) -> Result }` with Goose (`goose run --output-format json`) and pi (`-p` / `--mode rpc`) impls; mesh-llm's launcher/`--write` already produces correct provider config, so m8s can call those or write the same files.
2. **Pin harness versions** (Goose moved orgs; pi moved orgs/packages). Record versions in each eval result.
3. **One eval record schema** (tier, model, quant, GPU SKU, ctx, N concurrent, tok/s, TTFT p50/p95, quality scores, $/Mtok, $/hr) fed to CLI/TUI/web.
4. **Model catalog in m8s** with size, quant, license class, min VRAM, supported-by-skippy flag. Frontier entries need manual validation.
5. **Tier-to-hardware map:** small = 1x24 GB; medium = 1x H200/2x A100-80 or 4x4090 (80-160 GB); frontier = 8x H200 for DeepSeek-V4-Pro, 8x H100/A100-80 for GLM-5.2 or Kimi K2.6. Vast availability of 8-GPU hosts is a risk.
6. **CRDs with finalizers + TTL + orphan sweeper** to make cost safety a first-class feature.
7. **Security:** Aider/SWE-bench run LLM code; use Docker on rented boxes, never on the Mac mini owner node.

## Open questions

1. Does mesh-llm's replay-matrix support targeting an already-running remote mesh `/v1` URL, or only self-launched `mesh-llm serve`? (README says it launches the server.)
2. Do llama.cpp / skippy support `deepseek4`, GLM-5.x and Kimi-K2.6 architectures at the pinned llama.cpp commit? Need a certification run per FAMILY_CERTIFY.md.
3. Goose SDK: exact package/crate to embed, and stability of ACP; is `goose bench` still maintained?
4. Does Vast.ai reliably offer 8x80 GB+ hosts with >=1 TB local disk and fast model download (HF -> instance)? Model weight transfer time (850 GB) may dominate cold start; consider pre-built volumes or mirrored storage.
5. BFCL v4 web-search category requires an external search API; which free/self-hosted search backend do we use for 'web search' canonical task?
6. License review for Kimi K2.6 / MiniMax modified-MIT terms and Gemma terms for the intended use.
7. Does k9s render CRD `additionalPrinterColumns` as expected on the chosen minikube/k9s versions? (Quick hands-on check.)
8. Which cmux repo/license is the one the Admin means (manaflow-ai/cmux assumed)?
