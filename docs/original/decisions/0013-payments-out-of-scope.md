# ADR-0013 — Payments are out of scope for the prototype; m8s only gates admission

Status: **accepted** · Decided by: Admin · Date: 2026-10-20

## Context

mesh-llm has a Lightning payments proof of concept:

- metered protocol v3, with per-model, per-token pricing in msat;
- the client pays the serving node directly;
- the only wallet plugin is a Lexe plugin;
- no mainnet payment has been made on v3 yet.

Admin does not want to use a personal Lightning wallet, and wants a separate deep-dive later.

## Decision

- **No payment features in the prototype.** Payments are mesh-llm's job.
- m8s's only responsibility is **whether inference may reach hardware m8s provisioned**. That is expressed through mesh admission (owner trust policy) and node config, for example mesh-llm's default buyer policy `free_only` and no seller pricing configured.
- **Wallets never go on rented nodes.** Seed phrases on ephemeral third-party hosts are a non-starter.
- **When payments return (P4+):** a **mock wallet** that implements mesh-llm's `wallet.v1` plugin contract, used for tests (no real sats). Selling inference from rented capacity then becomes a cost-model question ([cost model](agor://kb/m8s/plans/cost-model.md)).

## Consequences

- The cost model reports $/1M tokens break-even, but no revenue side yet.
- There is a separate thread to schedule with Admin: the payments deep-dive (custody, pricing, fraud).
