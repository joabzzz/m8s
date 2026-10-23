# Vast adapter / optional read-only HTTPS

Default features remain empty; deterministic fixture mapping still works using
`FixtureTransport` (a compatibility alias of `Transport`). No core provider deps.

Opt in with `--features readonly-https`. `readonly::ReadonlyHttps::new(supplier)`
builds a synchronous reqwest 0.13.5 client without obtaining a key or performing
provider I/O. Supplier is explicitly injected `FnMut() -> Result<SecretValue>`;
there is no environment/file/default-secret lookup, executable, or CLI. Never
supply a key in argv, logs, fixture files or a rented host. Build/use on a blocking
thread, outside an async runtime. No live authorization is implied.

Only these exact requests are accepted by `Transport::exchange`:

| Method | Path | Body |
|---|---|---|
| GET | `/api/v0/users/current/` (slashless input also accepted) | none |
| POST | `/api/v0/bundles` | exact `readonly::offer_query()` |
| GET | `/api/v0/instances/{positive-canonical-u64}` | none |
| GET | `/api/v1/instances` | none |

Fixed `https://console.vast.ai`, no URL/base overrides, query strings, redirects,
proxy discovery, retries, credentials in URLs, launch/destroy/payments or arbitrary
bodies. Unsupported methods/paths/bodies fail BEFORE secret supplier/backend.
The account route is sent with its canonical trailing slash; a read-only live
check observed a 301 without it and HTTP 200 with it. Redirects remain disabled.
TLS uses explicit rustls AWS-LC crypto provider, static WebPKI roots, normal
certificate/hostname verification, no key logging. No global crypto-provider
installation, insecure verifier or environment trust override. Timeout/connect
bounds 10s; response <=1MiB; JSON content type, identity encoding, unique security
headers, unique JSON keys and success envelopes enforced. No real TLS handshake
has been tested; deterministic backend fixtures are not TLS/provider qualification.

Authorization HeaderValue is sensitive. Client/request/reply backend structs have
no Debug/Serialize; transport compile-fail tests prevent Debug/Serialize. Errors
are opaque `provider unavailable` without raw reqwest/I/O/body/URL causes. Raw
provider fields are not returned: account outputs balance micro-USD + unknown
topup, instance outputs ID/structured owner label/status, offers output numeric
unqualified diagnostics only. Secret reflection in parsed strings/keys is refused.
SecretValue zeroizes its buffers; reqwest/header internals may copy bytes, so no
complete process-memory-zeroization guarantee is made.

## Deliberate fail-closed gaps

### Explicit scoped destruction

The separate `scoped-destroy-https` feature exposes
`readonly::destroy::ScopedDestroyHttps`. Construction binds one positive instance
ID and an injected credential supplier without network I/O. Each `destroy()` call
sends one DELETE to that exact ID, using the same bounded TLS client as reads;
there are no automatic retries, redirects, proxies, or configurable URLs. The
caller must establish durable ownership before construction. This type does not
establish provider-side credential scope or arm an independent watchdog.

Only HTTP 200 with a bounded, duplicate-free JSON `success: true` envelope yields
`DestroyAcknowledged`. An acknowledgement is not absence: 404, timeouts, empty
inventories and ambiguous responses never authorize removing journal rows. The
owner must independently confirm resource removal. `confirm_absent()` sends a
separate GET to the same fixed ID and returns a private absence receipt only for
HTTP 200 with explicit `instances: null`. The official CLI uses this exact
response to identify a removed resource. Present rows (including stopped),
missing fields, mismatched IDs, errors and 404 never establish absence. Use the
creating owner account for this check; a destroyed instance credential may lose
visibility. This does not claim account-wide inventory completeness. The read-only `Transport`
continues to reject every mutation, even when this feature is enabled.

Protocol references: [Vast destroy instance API](https://docs.vast.ai/api-reference/instances/destroy-instance)
and [official CLI exact-instance lookup](https://github.com/vast-ai/vast-cli/blob/master/vast.py).
This implementation has synthetic coverage only; live qualification is pending.

### Remaining inventory and launch gaps

Current official offer response example shows an object while historical fixture
mapping expects a list. Only bounded explicit lists are supported, NOT guessed
object wrapping. Offer prices remain unqualified (official units conflict).
List v1 exposes pagination; this slice does not paginate/qualify completeness.
**Every reply has complete_instance_snapshot=false**, including empty rows/count0;
404/error/partial inventory never confirms absence. No volume/storage-cleanup or
provider watchdog qualification.

Existing `Vast::fixture_only` mapping retains its historical trailing-slash v0
snapshot and offer-filter assumptions. These are NOT silently rewritten into the
live routes: unsupported fixture requests fail closed against ReadonlyHttps.
Therefore this transport is NOT a qualified live `Provider` implementation and
cannot make existing fixture launch/destroy paths operational. Use its explicit
read-only request surface only after root's separate authorization/injection review.

Focused checks (jobs1, no provider calls):

```sh
cargo fmt -p m8s-provider-vast -- --check
cargo test -p m8s-provider-vast --all-features --locked -j1
cargo test -p m8s-provider-vast --no-default-features --locked -j1
cargo clippy -p m8s-provider-vast --all-features --all-targets --locked -j1 -- -D warnings
```

Official evidence and remaining live gates:
[Knowledge](agor://kb/m8s/research/metal-0-6-rust-readonly-transport.md).
