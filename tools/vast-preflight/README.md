# Vast preflight (offline / unqualified)

Standalone Python 3 stdlib tooling; no changes to core/provider crates or Cargo.
**No live runner, launch, destroy, payment, credential reader or default transport.**

```sh
tools/vast-preflight/check
```

The check wrapper removes `VAST_API_KEY` and `CONTAINER_API_KEY` from the test
process environment without reading or printing either value. Tests also check
variable NAMES only and refuse unsafe direct invocation. It compiles Python
sources in memory without writing bytecode.

These are synthetic fixture/unit tests, not provider admission, live pricing,
cleanup execution, image signature verification or security qualification.
The root `scripts/check` does **not** run these tests; run both at integration.

## Library surface

- `ReadOnlyClient(exchange, secret_supplier)`: runtime dependency injection;
  fixed account/offer read allowlist, sanitized facts, opaque failures. No raw
  result/header export, no key argument on a command line, no environment reads.
- `dry_run(account, offer, plan, gates, now_s)`: pure conservative micro-USD
  budget/deadline simulation. Passing result still has `live_authorized=False`.
- `cleanup_step(intent, snapshot, now_s, abort=False)`: pure watcher decision;
  returns exact-owner/run destroy IDs or unknown/storage/absence status. It does
  **not** execute effects, manage a journal/lock or deploy an independent watcher.

`Gates` contains external evidence assertions, not built-in verification. Default
values refuse simulation. Do not set them true just to make a launch pass.
`AccountFacts.auto_topup` remains unknown: the documented account balance endpoint
is not an authoritative topup-state contract. No undocumented flag is trusted.

Costs conservatively interpret raw storage rates as $/GB/hour, transfer as $/GB,
plus `dph_total` for run AND cleanup grace, plus reserve. These are diagnostic
bounds only when unit sources conflict. `RawOfferUnits` preserves raw decimals
and `UnitEvidence` records source URL, unversioned retrieval evidence and units.
Default official provenance is unresolved: **dry_run refuses even when every
Gates flag is true**. Test-only resolved provenance is explicitly synthetic.
No current live unit qualification exists. Monthly storage cannot qualify without
an explicit reviewed hourly rate. Explicit `MB/s * 8 -> Mbps` conversion (or
identity for explicit Mbps) precedes threshold comparison; unknown units fail. Location requires exactly two uppercase letters; all UA
is rejected conservatively because occupied regions cannot be inferred safely.
Fixtures contain fake prices and markers, never actual credentials.

Injected transport is trusted secret-handling code. Before any authorized use it
must enforce fixed official HTTPS origin, verified TLS, no redirects/proxies,
bounded bodies/timeouts, and no debug/header/response logging. No transport ships
here; tests inject memory-only exchange functions. Python object introspection,
traceback-local dumps and process memory are not covered by repr redaction; no
zeroization claim is made. Never give an account key to a rented host.

Evidence, official links, unresolved live gates and proposed independent cleanup
watcher design are in Knowledge:
[Metal-0-6 preflight](agor://kb/m8s/research/metal-0-6-vast-preflight.md).
