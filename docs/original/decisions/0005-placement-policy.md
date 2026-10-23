# ADR-0005 — Placement policy: sanctions denylist, bandwidth floor, our images only

Status: **accepted** · Decided by: Admin (list and bandwidth confirmed 2026-10-20) · Recorded by: Metal-0 · Date: 2026-10-20

## Context

Rented hosts can be anywhere. Admin does not want legal exposure from sanctioned jurisdictions. Model download speed dominates cold start, and Vast preset templates are opaque.

## Decision

m8s applies a **placement policy** before any offer is considered. The policy is configurable, but these defaults ship enabled:

1. **Region denylist.** Exclude hosts located in countries or regions under comprehensive US (OFAC) sanctions. The conservative default list comes from [Spike S1](agor://kb/m8s/research/spike-s1-vast-provider-and-costs.md). Starting candidates: Cuba, Iran, North Korea, Syria, Russia, Belarus, and occupied regions of Ukraine (Crimea, DNR, LNR). Offers with an **unknown location are excluded** by default. *Not legal advice; Admin owns the final list.*
2. **Bandwidth floor.** Minimum download bandwidth defaults to **1 Gbps**. Admin wrote "1 GiB down", which needs confirming as Gbps or GiB/s. The floor can be raised per tier.
3. **Our images only.** Only signed `m8s-node` images ([ADR-0011](agor://kb/m8s/decisions/0011-privacy-posture-and-zdr-levels.md)) may be launched. Provider preset templates are never used.
4. **Non-interruptible by default** ([ADR-0004](agor://kb/m8s/decisions/0004-provider-and-harness-adapters.md)).
5. **Hard spend guards**: a max $/hour per instance, a max total $/hour, and a monthly budget (initially **$20/month**). A launch that would exceed a guard is refused, not queued.

## Admin confirmation (2026-10-20)

- **Default denylist (accepted):** Cuba, Iran, North Korea, Syria, Russia, Belarus, occupied regions of Ukraine (Crimea, DNR, LNR), and **China** (excluded by Admin's policy). Offers with an unknown location are still excluded.
- **Bandwidth:** the floor uses each provider's native unit and range, whichever fits that provider best. Each adapter normalises to one internal metric (`inet_down_mbps`) so policy and reports stay consistent. The default floor is 1000 Mbps (1 Gbps), configurable per pool. The cost model always reports *measured* download speed next to *advertised* speed.

## Consequences

- Smaller offer pool and sometimes higher prices. Accepted.
- The policy engine runs in the core, so it applies to every provider. Adapters only map their location and bandwidth fields onto it.
- Changing the spend guards above the Admin-set budget needs Admin approval (SOP §7).
