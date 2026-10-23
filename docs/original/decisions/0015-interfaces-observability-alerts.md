# ADR-0015 — Interfaces (CLI, TUI, web, API, k9s); stock observability; Alerter adapter

Status: **accepted** · Decided by: Admin · Date: 2026-10-20

## Decision

- **Interfaces:** all of them, layered on one API:
  - `m8s` **CLI**, scriptable, with `--json` everywhere. This is the operator-agent surface.
  - **TUI** (`m8s top`, ratatui).
  - **Web dashboard**, served locally by the m8s API.
  - **k9s**: only in the optional kube mode (`m8s-operator`, [ADR-0008](agor://kb/document/01a15b4d-b4f9-7254-adfb-37d41a4297e1) option A, not built in v0). In v0, `m8s top` is the k9s-style dashboard.
  - The **mesh-llm console** (:3131) for mesh topology.
- **Observability:** stock **Prometheus + Grafana + OTel Collector**, run as optional local services (e.g. Docker Compose or Homebrew). There is no minikube in v0. mesh-llm exports OTLP (it has no Prometheus endpoint), so the collector bridges it. m8s exposes its own metrics (instances, spend $/h, heartbeat age, provisioning latency).
- **Alerter trait.** v0 backends are a log or desktop notification. Slack, block/buzz and Discord come later. Alert events:
  - dead man's switch fired;
  - spend guard hit;
  - bootstrap nonce stolen;
  - node unhealthy;
  - audit failed.

## Consequences

- The CLI is the contract the other interfaces and agents build on. Design it first, in P1.
- The observability stack adds RAM on the Mac mini. It is optional in config, with a lightweight mode that has no Grafana.
