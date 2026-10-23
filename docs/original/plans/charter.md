# m8s — Project charter

Status: v1 (2026-10-20) · Owner: Metal-0 · Approver: Admin · Source: [kickoff Q&A](agor://kb/document/01a15b4b-a774-720a-99c8-03d2cbce9eec)

## What m8s is

**m8s** (pronounced "mates") is an open-source Rust control plane that extends **one mesh-llm node** with on-demand cloud compute. The owner node rents GPU machines from a provider, boots our own signed container image running mesh-llm, and securely hands it the credentials it needs to join the owner's private mesh. The new node serves a model, and m8s tears it down when it is no longer needed, including automatically when contact with the owner is lost.

> Most people already have *some* hardware. m8s is the software layer that lets a mesh borrow much bigger hardware for minutes or hours, instead of buying it.

```
  your mesh node (Mac mini) ── m8s ──▶ provider API (Vast.ai, …)
          │                                   │ rent + boot m8s-node image
          │◀════════ iroh/QUIC mesh ═════════▶│ joins your mesh, serves a model
  clients (Goose, pi, apps) infer over the mesh, never through m8s
```

## Who it's for

- **First user:** a person running a **private mesh** (and later the public mesh) who wants more compute than their own machines have.
- **Agents:** primarily inference **customers** (Goose, pi). Later, also **operators** that drive the m8s CLI.

## In scope (exploration phase)

1. **Prototype:** provision → join mesh → serve → evaluate → tear down. The tiers below are **example pools**: owners choose the models, and m8s describes them with model profiles ([ADR-0017](agor://kb/document/01a15c08-e7bf-701e-a0e4-d203a169c2c2)). P0 uses 27B- and 70B-class models, and frontier is deferred.
   - **small:** translation, lookup, basic tool calling
   - **medium:** coding, spreadsheets, general reasoning
   - **frontier:** the largest models possible on a single host
2. **Cost model:** $/hour versus throughput at N concurrent users, per tier and provider.
3. **Rough roadmap:** concise, phased. See [roadmap](agor://kb/m8s/plans/roadmap.md).
4. **Open extension points:** `Provider`, `Harness`, `SecretStore` and `Alerter` traits, so others can add providers and tools.
5. **Privacy/security:** a threat model, per-provider ZDR levels, an automated audit suite, and signed images.
6. **Demo:** after P3, a self-recorded cmux walkthrough that shows hardware joining the mesh live.

## Out of scope (for now)

- Payments and Lightning. m8s only decides whether inference may reach its hardware. A mock wallet comes later.
- Managing other people's nodes, or other machines of the owner's. m8s controls only what it provisioned.
- Multi-tenancy, compliance programs, and operating anyone's compute as a service.
- Large changes to mesh-llm or a long-lived fork. **Small, upstream-shaped patches are in scope.** They are stacked on the `m8s` branch of mesh-llm and tracked as future upstream PRs in the [patch queue](agor://kb/document/01a15c1a-14c0-7750-9cc2-2ede1ad0ad25) ([ADR-0006](agor://kb/document/01a15b4d-b4ae-765a-ba27-4491ae240bf9)). m8s may become a crate in mesh-llm in the long run.
- Cross-host model splits on rented hardware.

## Success criteria (exploration phase)

- From the Mac mini, `m8s up <tier>` brings a node from each tier into the private mesh. A Goose or pi agent completes a canonical task over the mesh against it. `m8s down` leaves **zero billable resources**.
- If the owner goes offline, every provisioned instance terminates itself within the configured TTL (default 1h).
- An eval run reports aggregate throughput at N concurrent users and quality scores per tier, and feeds a cost model.
- The audit suite produces a signed report per node, stating its ZDR level honestly.
- All spend stays within Admin's budget ($20/month for experiments) unless Admin approves more.

## Constraints

- **Language:** Rust. Repo: `~/Development/m8s/m8s`, local only ([ADR-0001](agor://kb/document/01a1576c-8eee-73e9-806c-d99a0e071e05)).
- **Control plane host:** Admin's Mac mini (designed to run on almost any machine). `m8sd` is a Rust daemon with SQLite, and `m8s top` is the dashboard. kube-rs plus k9s is an optional later mode ([ADR-0008](agor://kb/document/01a15b4d-b4f9-7254-adfb-37d41a4297e1)).
- **Images:** only our own images. No provider preset templates.
- **Placement:** sanctioned regions are excluded, with a minimum bandwidth floor.

## Key links

- [Architecture v0](agor://kb/m8s/designs/architecture-v0.md)
- [ADR index](agor://kb/document/01a1575c-3c01-775e-b5cb-4dab3b09c054)
- [mesh-llm patch queue](agor://kb/document/01a15c1a-14c0-7750-9cc2-2ede1ad0ad25)
- [Roadmap](agor://kb/m8s/plans/roadmap.md)
- [Cost model](agor://kb/m8s/plans/cost-model.md)
- [Status](agor://kb/document/01a1575c-3c1f-7036-84be-7c5f7b6deb12)
