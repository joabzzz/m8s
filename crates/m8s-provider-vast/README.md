# Fixture-only Vast adapter

This crate is a **mapping test harness**, not a production provider transport.
There is no HTTP client, base URL, credential input, default transport, retries,
or daemon wiring. Construct it only with an injected in-memory FixtureTransport.
No account or container credentials are required or read.

## Fixture provenance and assumptions

Hand-written synthetic fixtures are based on the supplied desk-research document
[Spike S1](agor://kb/m8s/research/spike-s1-vast-provider-and-costs.md), fetched by
that research on 2026-10-20. This implementation made **no provider calls** and did
not fetch fresh provider docs. Original references recorded by S1:

- [Search offers](https://docs.vast.ai/api-reference/search/search-offers)
- [Create instance](https://docs.vast.ai/api-reference/instances/create-instance)
- [Show instances](https://docs.vast.ai/api-reference/instances/show-instances)
- [Destroy instance](https://docs.vast.ai/cli/reference/destroy-instance)

These are references, not evidence of live compatibility. Bodies and response
envelopes are deliberately narrow and still require official-schema/live
verification with Admin before any actual transport is written or enabled.

## Contract under test

- Search POST /api/v0/bundles/: explicit verified/rentable/non-external/ondemand,
  datacenter false for community / true for secure, minimum 1000 Mbps, denylist.
  Local validation reapplies these filters. Country must be strict ISO alpha-2;
  all UA excluded because these fixtures lack trustworthy subdivisions.
- gpu_ram is interpreted conservatively as **decimal MB** from the S1 field
  description, not inflated to MiB. Exact real unit semantics need verification.
  inet_down uses Mbps. Capacities/bandwidth round down, micro-USD price rounds up.
  JSON decimal lexemes are preserved with arbitrary_precision; no f64 math.
- Create PUT /api/v0/asks/ID/: pinned image syntax, entrypoint args mode,
  target running, disk GB, cancel_unavail, owner/node/tier label.
  Reads **new_contract**, never assumes an id field. No env/onstart, secrets,
  templates, registry login or arbitrary provider knobs accepted.
- Labels are versioned exact strings m8s:v1:OWNER:UID:TIER with bounded safe
  components; no prefix-owner matching. Secure is verified datacenter, **not TEE**.
- Describe uses documented GET /api/v0/instances/ snapshot. Only a fixture with
  explicit complete_instance_snapshot=true, valid full envelope and no
  pagination/count/duplicate-id inconsistency may confirm an absent id.
  HTTP 404, auth errors, outages, malformed bodies and incomplete lists never
  imply absence. A real transport would need an independently proven complete
  snapshot contract, not a boolean set blindly on any HTTP 200.
- Loading/created maps to Starting, never Ready or Gone. Unknown/destroyed status
  is uncertainty, not absence. Stop/exit remain present. Foreign/malformed labels
  cannot authorize deletion.
- Destroy first checks exact owner against the snapshot and only sends DELETE;
  success is an acknowledgement, **not** absence. Poll describe separately.
- Repeated owner+node UID in one adapter lifetime is cached; uncertain submission
  is not retried. Changing its offer/tier is refused. This cache is NOT durable:
  a future orchestrator must persist intents before launch and adopt labels after
  restart, and must never reissue an uncertain intent based on an empty snapshot.

## Not production-ready

Pinned image syntax is not signature verification. This crate does not enforce
durable spend reservations, real provider-side idempotency, secure bootstrap,
container identity/IP attestation, self-destruct credentials, or confidentiality.
It is intentionally absent from m8sd dependencies and the daemon continues to
refuse any provider id other than mock. There is no stop endpoint.
