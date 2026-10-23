# Architecture kickoff — Admin answers (2026-10-20)

> **Follow-up answers (2026-10-20, second round):**
> 1. [Export redaction: agent-tooling availability note omitted]
> 2. Defer the live frontier test. P0 uses popular mid-size models (Qwen 27B class, strong 70B class), preferring one download with several replicas and KV headroom.
> 3. Bandwidth: use each provider's native unit and range, whichever fits best, behind one consistent internal metric.
> 4. Sanctions denylist confirmed, plus China excluded.
> 5. The framework doesn't choose models; it gives them properties ([ADR-0017](agor://kb/document/01a15c08-e7bf-701e-a0e4-d203a169c2c2)).
> 6. [Export redaction: session-management note omitted]
> 7. [Export redaction: agent-naming clarification omitted]


Source: a coordinating-agent planning session, 2026-10-19/20. 48 questions were asked; this is the condensed record of Admin's answers. Decisions derived from it are in the [ADR index](agor://kb/document/01a1575c-3c01-775e-b5cb-4dab3b09c054).

## Goals and scope

- **First user:** someone who starts a private mesh network. Will likely also be tried on the public mesh.
- **First milestone:** a prototype that spins up compute containers and gets models serving on them. "Spin up three tiers, run Goose, tear down" is a good test. Scaling up (k containers, or one instance with k NVLinked GPUs) comes later.
- m8s **may later become a crate inside mesh-llm**.
- **Hardware:** anyone's hardware can join a mesh if eligible; other participants can be passive inference clients. Vast is the prototype provider; adapters for Lambda, AWS, GCP, Databricks, etc. come later. v0 must leave an **open adapter** that others can implement.
- **Exploration "done":** prototype + cost model + concise rough roadmap.
- **Upstreaming to mesh-llm:** on the table later. **No mesh-llm changes for now.** Revisit after the prototype.
- **Admin's relationship to mesh-llm:** volunteer on the dev team. Payments are a prototype and may change; Admin has seen a demo. Admin does not want to use a personal Lightning wallet, so mock it for tests. Payments get a separate deep-dive conversation.

## Architecture

- **Language:** Rust confirmed.
- **Operator + CRDs vs virtual-kubelet:** open. Admin wants a **separate conversation thread** to learn the differences.
- **Control plane host:** Admin's Mac mini.
- **Topology:** one mesh with all tiers, routed by model name. "We're really just contributing compute to a mesh."
- **Scope of control:** m8s is an extension of **one mesh node** that lets it spin up additional, more powerful mesh nodes. Provisioned containers serve only the node that owns them. **m8s does not operate other Mac minis or peers.**
- **Control channel:** open for discussion. Requirements: the owner node securely passes (1) the invite token and (2) an auth measure, so the originating node has proper access control. By default **the originating node is the only owner**; it may choose to open control to the mesh.
- **Tenancy:** single-tenant for the prototype. Multi-tenant concerns probably belong at the mesh level.

## Vast.ai and provisioning

- [Export redaction: account setup detail omitted.] **Budget: $20/month** for experiments.
- Community vs secure cloud is an **adapter-level choice**: `vast-community` and `vast-secure` are different providers.
- **Regions:** this matters a lot. Start with a denylist of US-sanctioned countries (e.g. Russia, Iran).
- **Bandwidth:** model latency isn't critical, but download speed is. Minimum **1 GiB down** (to confirm: Gbps vs GiB/s). No Vast preset containers, because we don't know what's in them.
- **Cold start:** waiting is expected and is a feature. You don't buy dedicated hardware; you spin up for your timezone and set a close time. Spin up/down is critical.
- **Interruptible instances:** the user can opt in. Default is non-interruptible. Per-provider docs should help an agent decide what to spin up.
- **Providers and harnesses:** an open provider/harness framework. Starter providers are the two Vast adapters. Starter harnesses are the **Goose SDK** (block/goose's "gdk") and **pi**.
- **Autoscaling:** start with a queue and schedules; keep it simple. Start users with the minimum aggregate compute needed to get an agent going.
- **Scale to zero / cold behaviour:** defined in each mesh node's m8s config.
- **Scaling philosophy:** the trade-off between KV cache and model size depends on the org's needs and user count, and the mesh node knows its needs best. Autoscaling must be configurable; we don't yet know what people want.
- **Dead man's switch (default on):** if the owner node dies or leaves the mesh, provisioned containers that lose contact for t = 1 hour are closed. No runaway spend.

## Tiers and models

- **Unanswered.** Admin's note started "Something with…" and the remaining tier questions (frontier definition, who picks the tier, multimodal, quantization) were left blank. See Open items.

## Performance evals

- **Primary metric:** total throughput at N concurrent users. Users decide what to run.
- **Quality suites:** tool calling (BFCL), coding (SWE-bench-lite / Aider) and translation (FLORES), plus any open-source evals that help.
- **Cadence:** on-demand only. Spot-checking whether nodes lie is the mesh's problem.
- **Tooling:** reuse what mesh-llm uses (replay-matrix etc.) for consistency with the mesh optimization team.

## Goose and agents

- **Example tasks:** web search, document search, translation, coding Pong from scratch. Defer to prior art.
- **Agent role:** mostly **customer**, likely also **operator** (an agent that uses the m8s CLI to change configs).
- **Critical:** customers run inference over the **iroh tunnel (the mesh)**, not a REST API. This may need an adapter.

## Payments

- **Skipped for the first prototype.** Payments are handled by mesh-llm. m8s only needs to decide whether inference may go to its hardware.

## Privacy and security

- **ZDR:** cryptographic where possible, but kept open. Not every provider will offer ZDR; we note each provider's level.
- **Threat model:** protect against (1) Vast itself, (2) the node controlling the Vast instance, (3) network observers.
- m8s is an open-source framework and **never operates other people's compute**, by design.
- **Tags:** nodes should be taggable (e.g. trusted). Possibly a mesh-level concern.
- **Compliance:** none for v0.
- **Audit:** an automated audit suite is wanted.
- **Image signing:** sign m8s images (cosign) and require release attestation: **yes**.
- **Secrets:** an open, adaptable trait. v0 can be as simple as `.env`, unless that is a bad idea for security.

## Operations and UX

- **Interfaces:** "bingo": CLI, TUI, web dashboard, API, plus k9s.
- **Observability:** stock Prometheus + Grafana + OTel is fine.
- **Alerts:** an open adapter (Slack, block/buzz, Discord later).
- [Export redaction: team/agent-roster and tooling-budget bullet omitted.]
- **Demo experiment:** after P3, the orchestrator records its own demo in a **cmux** terminal explaining how m8s works. It should show commands, k9s (or the kube-rs equivalent) or a web console, and the **mesh-llm console topology view** with new hardware joining the mesh live.

## Open items raised by this Q&A

1. **Tier questions unanswered:** preferred models and licensing, frontier definition, who picks the tier, multimodal in v0, quantization.
2. **"1 GiB down":** is it 1 Gbps (~125 MB/s) or 1 GiB/s (~8.6 Gbps)? Very different offer pools.
3. [Export redaction: agent-naming question omitted.]
4. [Export redaction: agent-tooling availability item omitted.]
