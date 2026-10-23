# Nebius provider

`m8s_provider::nebius::Nebius<T>` implements the consolidated provider contract.
The default build has deterministic fixtures and mapping only. Enable
`nebius-readonly-https` for `nebius::readonly::ReadonlyHttps`. No gRPC dependency
is needed: the documented REST gateway exposes the same protobuf API. All network,
TLS and secret-store dependencies remain optional. The agent has no provider dependency.

`Nebius::read_only` refuses `launch` and `destroy` before any transport or token
supplier call. `Nebius::fixture_only` enables a deliberately incomplete synthetic
create protocol and synthetic delete operation for tests. The HTTPS transport does
not implement either mutation, even if passed to the fixture constructor. There is
no Nebius launch command, disk/network/image provisioning, token exchange, or paid
resource operation in this change.

## Authoritative evidence checked 2026-10-23

- [REST overview](https://docs.nebius.com/rest-api/index): gateway generated from
  the same protobuf definitions as gRPC. [Authentication](https://docs.nebius.com/rest-api/authentication)
  specifies an IAM access token in the Authorization bearer header. User access
  tokens last 12 hours; service accounts exchange signed JWTs for access tokens.
  This client accepts a caller-supplied `SecretValue` at each read; it does not
  discover credentials, invoke the CLI or refresh tokens itself.
- [OpenAPI](https://api.nebius.cloud/openapi.json), SHA-256 at inspection:
  `1b497bcb799fb4a9940adca1ef2cd7e1b8cd4452776aca854db85cc1316c8e50`.
  Fixed origin `https://api.nebius.cloud`, GET routes `/compute/v1/instances`,
  `/compute/v1/platforms`, `/quotas/v1/quota-allowances`. Query fields are camelCase
  `parentId`, `pageSize`, `pageToken`; replies use `items`, `nextPageToken`.
  All pages are consumed to a terminal token. Repeating tokens, failed pages,
  duplicate instance IDs, foreign project rows, and page/row/body bounds fail closed.
  ProtoJSON may omit empty arrays/tokens and zero-valued counters; unknown top-level
  page fields (including error envelopes) fail closed.
- [Instance service](https://github.com/nebius/api/blob/main/nebius/compute/v1/instance_service.proto)
  is project scoped and returns long-running Operations for create/delete.
  [Metadata](https://github.com/nebius/api/blob/main/nebius/common/v1/metadata.proto)
  supports arbitrary string labels. The instance-list API has no server-side label
  filter: fetch the entire selected project's inventory, then filter the exact
  `m8s-owner` value locally. Owned rows require version `1`, valid `m8s-node-uid`
  and `m8s-tier`. Near-prefix owners are foreign, never adopted or deleted.
- [Regions](https://docs.nebius.com/overview/regions): Finland `eu-north1`,
  Iceland `eu-north2` (private), France `eu-west1`/`eu-west2`, Spain `eu-south1`,
  Israel `me-west1`, US `us-central1`/`us-north1`, UK `uk-south1`/`uk-south2`.
  Offers carry the exact region and ISO alpha-2 country; unknown regions are refused.
- [Types and presets](https://docs.nebius.com/compute/virtual-machines/types) and
  [project platform discovery](https://docs.nebius.com/compute/virtual-machines/list-platforms)
  define the static catalog intersected with project platforms/presets. GPU memory
  uses conservative decimal GB to bytes, not an assumed GiB conversion.
- [Compute pricing](https://docs.nebius.com/compute/resources/pricing) currently
  lists USD PAYG GPU-hour rates effective October 1, 2026: H100 4.50, H200 5.40,
  B200 8.50, B300 9.50; RTX PRO 6000 is 1.80. Multiply by preset GPU count with
  integer micro-USD arithmetic. Spot became dynamic October 8 and is excluded.
  L40S is excluded because CPU/RAM have separate charges. This is a dated catalog
  estimate for the listed compute resources, excluding storage, networking, taxes,
  discounts and ILS billing. It is not a live billing quote or bounded launch liability.
- [Quota API schema](https://github.com/nebius/api/blob/main/nebius/quotas/v1/quota_allowance.proto)
  exposes regional limit, usage, allocation state and usage state. A platform's
  `spec.gpuCountQuotaType` identifies the GPU quota by metadata name. Only active,
  known usage with enough remaining quota admits a preset. Missing/frozen/unknown
  quota does not imply availability. [Compute quotas](https://docs.nebius.com/compute/resources/quotas-limits)
  apply even to stopped VMs. Fixtures use synthetic quota names, not invented real IDs.

## Semantics and limits

### Hardware is regional

A project in one region cannot select every GPU in the global catalog. Discover
platforms in the selected project before choosing hardware or requesting quota.
Quota approval does not provision a VM and does not establish physical capacity.

| Hardware | Documented regions (2026-10-23) |
| --- | --- |
| B300 | `us-north1`, `uk-south1`, `eu-west2` |
| RTX PRO 6000 | `us-central1`, `uk-south2`, `eu-south1` |
| L40S | `eu-north1` |

For example, a live `us-north1` project discovery listed B300 and CPU platforms,
while the same account's `us-central1` project listed RTX PRO 6000. A small-model
test does not need a B300; selecting another regional project can be necessary
to use cheaper hardware. Include CPU/RAM in L40S quotes and check dynamic spot
prices separately. See [platforms](https://docs.nebius.com/compute/virtual-machines/types).

Tenant and project quota records are distinct. The live account had an active
one-B300 tenant allowance while its project record omitted `limit` and `state`.
Do not interpret that project record as proof that no tenant quota was approved,
or silently mark an unspecified record active. The adapter's current
project-only quota filter remains conservative and can return no offers here.

The [one-shot GPU smoke tool](../../tools/nebius-smoke/README.md) checks cloud
provisioning, GPU model execution and cleanup separately. It does not enable
production mutations on `ReadonlyHttps` or claim authenticated m8s routing/Ready.

On 2026-10-23, the tool ran `HuggingFaceTB/SmolLM2-135M-Instruct` on one
`gpu-b300-sxm` VM with PyTorch 2.9.0/CUDA 13.0. It returned **Hello!** using
GPU-resident model parameters after a CUDA kernel check. Independent cleanup and
a subsequent owner read confirmed zero instances, disks and IP allocations in
the selected project. The first create was cleaned up too early
because the runner interpreted the provisional `STOPPED` state as a failure;
that check was corrected to inspect terminal operation status before the retry.
Both attempts and exact cleanup identities are in the
[sanitized evidence](../../tools/nebius-smoke/evidence-2026-10-23.json).
This establishes a provider/GPU smoke, not authenticated m8s routing, production
launch support, a performance benchmark, or settled billing.

Search is a catalog and GPU-quota eligibility check, not a reservation or proof of
physical capacity. It does not qualify VM-count, disk, IP or other resource quotas.
No Internet throughput or reliability evidence is available, so both are zero;
normal core placement therefore rejects these unqualified offers on bandwidth.
A future live launch path must requalify pricing, all resources/quotas, image trust,
capacity, bandwidth and independently bounded exact-resource cleanup.

Caller persists launch intent **before** effects. Fixture launch uses the exact
owner and `node_uid`, reconciles a complete inventory first, adopts only one matching
owner/node/tier/platform/preset, and records uncertainty before sending create.
Retries reconcile but never submit that attempt again, even after deletion. A fresh
adapter can adopt a labeled VM after restart; in-memory suppression cannot preserve
an invisible uncertain attempt across restart, so the controller's durable intent
remains mandatory. Labels are mutable ownership evidence within an explicitly
selected trusted project, not cryptographic authorization or provider idempotency.

An accepted operation is not a VM observation or confirmation of absence. Stopped
remains Present; deleting, stopping, error and unknown statuses fail closed because
the core has no matching state. Describe confirms absence only after a successful,
fully consumed, validated project inventory excludes that exact ID. No HTTP error,
404, delete acknowledgement or partial list becomes Absent. Paginated API reads
are not an atomic snapshot under concurrent external mutation; this is no stronger
than the supplier's list consistency. External label/project mutation is not supported.

The synchronous HTTPS client has fixed GET routes, TLS roots, bounded JSON reads,
timeouts, no redirects, proxies, retries, compression or TLS key logging. It rejects
ambiguous known fields and duplicate labels, discards unrelated VM spec fields
(including cloud-init), marks Authorization sensitive, and maps errors to static
provider errors. Provider/client/wire bodies have no Debug or Serialize implementation.
No credentials or account resources were used in validation. Deterministic mapping
and transport-boundary tests do not claim live API, TLS or resource lifecycle proof.
