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

The default-off `watchdog` feature also builds `m8s-vast-watchdog`. This is an
owner-side cleanup executable, not a launcher or a Ready capability. Its only
arguments are `--plan CANONICAL_ABSOLUTE_PATH`; the creating account's credential
is supplied through an anonymous stdin pipe, which the launcher must close.
Never pass a key through argv, environment, a fixture, or a log. The launcher
must clear the child's environment and retain its original budget reservation.

The private plan uses schema `m8s.vast-watchdog/v1` and exactly these fields:
`owner`, `node_uid`, `tier`, `image` (immutable repository digest), `created_at`,
and `destroy_at` (absolute Unix seconds). The original lifetime is at most 1800
seconds, and arming must occur within 30 seconds of creation. The plan is a
current-user-owned 0600 regular single-link file in a private 0700 directory,
with no extended ACL. Trusted ancestors remain a caller responsibility.

The watchdog creates and syncs `watchdog.events.jsonl` exclusively, detaches into
its own session, then writes `armed ` followed by the canonical public plan JSON
and a newline. The launcher must match the complete receipt before submitting a
create. Existing journals refuse rearming; they are
never removed or truncated. Resource binding requires positive exact owner label
and image matching; an empty lookup is unresolved. SIGTERM, SIGINT, SIGHUP or
the original deadline trigger cleanup. Monotonic elapsed time prevents clock
rollback from extending the lease. Each provider request has a ten-second bound;
lookup and cleanup cycles are separated by three seconds. A separate exact GET
must confirm absence, even after DELETE acknowledgement.

After 180 seconds of unresolved cleanup (plus any already-running bounded API
calls), it records uncertainty and exits nonzero. Provider outages, watchdog
failure, host loss and storage billing are **not** contained by this process.
Keep the original liability and require reconciliation; never infer zero cost,
release a reservation or create a replacement. The executable has source/unit
coverage plus a passing synthetic launcher-process-group-loss qualification.
Live provider qualification remains unfinished. It is not yet wired into a
complete owner LAUNCH command.

`ArmedCleanup::arm_cli` re-executes the current m8s binary's fixed cleanup-worker
route with an empty environment. It supplies the key through stdin and bounds
the complete, plan-bound acknowledgment to ten seconds. Its opaque handle retains
the same account key for one create dispatch, preventing create/cleanup account
substitution. Drop requests early cleanup without killing the detached worker.
This is a process receipt, not provider absence or guaranteed billing containment.

The default-off `m8sd/prototype-vast` executor consumes the existing durable
create claim. It requires a reserved budget, one lifetime attempt in that owner
Store, a cap at most USD6, one instance and at least 240 seconds of cleanup
reserve. It checks the cleanup handle before and after the database invocation
burn, never retries create, and retains uncertain liability even for a successful
create acknowledgment. Only a separate exact instance observation can bind the
resource and advance Launching to Booting. Ready, real inference and settlement
remain separate obligations. Tests use synthetic accounting and never dispatch
to Vast.

`ProvisionPacket` and `ProvisioningSession` implement the owner half of the
restricted SSH handoff. The host public key carries its exact instance ID; the
packet must match that observed instance and immutable image. A fresh private
known-hosts file pins the host key. SSH has no ambient config/agent/passwords,
forwarding, multiplexing, or command selection; the only remote command is
`provision-v1`. The nonce and node private key travel in the bounded stdin frame,
separately from public JSON, with no secret arguments or child output logging.
The retained SSH child is not an authentication/readiness receipt: the original
TLS bootstrap must still succeed and the caller must retain provider cleanup.
Only generated literal paths without OpenSSH expansion tokens are accepted.

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
