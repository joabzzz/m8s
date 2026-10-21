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
| GET | `/api/v0/users/current` | none |
| POST | `/api/v0/bundles` | exact `readonly::offer_query()` |
| GET | `/api/v0/instances/{positive-canonical-u64}` | none |
| GET | `/api/v1/instances` | none |

Fixed `https://console.vast.ai`, no URL/base overrides, query strings, redirects,
proxy discovery, retries, credentials in URLs, launch/destroy/payments or arbitrary
bodies. Unsupported methods/paths/bodies fail BEFORE secret supplier/backend.
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
