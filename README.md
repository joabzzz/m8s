# m8s — mock-only Sprint 1 slice

Rust desired-state controller for owner-selected GPU pools. This implementation
cannot contact a live provider, admit a mesh node, or serve inference.

## Build and checks

Rust 1.92+; Unix host; C compiler for bundled SQLite. Builds use two jobs.
No Docker, GPU, credentials or model download is needed.

```sh
export CARGO_HOME="$PWD/.cargo-home" # writable cache for restricted sessions
cargo build --workspace --locked -j 2
./scripts/check                    # fmt, tests, clippy -D warnings
./scripts/mock-e2e                 # daemon + Unix socket + CLI, mock resources
```

For explicitly non-PTY selection, use `./scripts/check --no-pty`: the same fmt,
workspace/all-feature/locked tests and strict all-target Clippy (two jobs), but
with exactly four current synthetic PTY cleanup names excluded. Its notice marks
this as filtered, **not** an unfiltered full pass. No flags preserves the standard
profile unchanged; `--help` is effect-free. This is test selection, not sandbox or
access authorization: ignored operational tests remain ignored, and new
operational tests require profile review/maintenance. It is not a universal
future capability guarantee. The regression below guards the inventory against
the current `crates/m8s-top/tests/terminal_cleanup.rs` four names.

Run `python3 scripts/test-check-profiles` to check argv, selection and fail-fast
behavior using only an owned fake Cargo on controlled PATH; it never runs real
Cargo/compiler/tests/PTY. This lightweight regression is separate from checks.

## Try the mock

Choose a short, private state-directory path (Unix sockets have length limits).
The daemon creates it with mode 0700 and its socket with mode 0600. Existing
non-private directories are refused. A process lock prevents two daemons from
using the same directory. The Store library requires one exclusive reconciler;
SQLite reservations alone do not make multiple controller replicas safe.

```sh
target/debug/m8sd /tmp/m8s-demo owner-example
# In another terminal:
target/debug/m8s --socket /tmp/m8s-demo/control.sock up qwen27 --json
target/debug/m8s --socket /tmp/m8s-demo/control.sock scale qwen27 2 --json
target/debug/m8s --socket /tmp/m8s-demo/control.sock status --json
target/debug/m8s --socket /tmp/m8s-demo/control.sock logs --json
target/debug/m8s --socket /tmp/m8s-demo/control.sock down --json
```

Reconcile runs every second in this mock demo. Commands persist desired state;
they do not synchronously assert teardown completion. After `down`, poll status
until `nodes` is empty. Restart using the same directory and owner label:
desired state, instance handles, mock resources and launch limits survive.
Different owner labels for an existing database are refused.

**Booting is intentional.** Secure bootstrap, cert signing, served-model checks
and owner-routed canary are unavailable. No mock node is labelled Ready. Every
response states `mode: mock-only` and the bootstrap limitation.

## Safety boundaries

- A SQLite transaction reserves a launch intent and launch-limit slot before
  the provider side effect. A returned instance id is saved immediately.
- An ambiguous launch is adopted by matching exact owner, node uid and tier.
  If no id can be recovered, its intent remains unresolved and is not retried.
  Even `down` cannot falsely declare it gone. Manual investigation is needed;
  do not remove its row to make status green.
- Only explicit per-instance `Observation::Absent` releases a finalizer. A failed
  list/describe, stopped instance, or acknowledged destroy never counts as Gone.
- Destroy-pending nodes count toward capacity, preventing overlapping replacement
  spend. Stopped/exited owner-labelled orphans are tracked and destroy is retried.
- Sliding 3 launches/tier/hour history is separate from node rows and persists
  after finalization and restart. Saturation emits `Paused` events; desired
  state stays stored and reconciliation resumes when a slot expires.
- Core policy and model planner are pure libraries: micro-USD, bytes, Mbps and
  injected seconds. Planner layouts are fit estimates, not runtime certification
  or automatic model recommendations.
- Nonce ledger stores SHA-256 hashes, exact instance/IP/image bindings and a
  15-minute single-use TTL. It is a tested component, not a bootstrap service.
- `m8s-agent` contains only a deterministic DMS engine. Its authenticated-identity
  input must eventually come from pinned iroh. Drain/wipe failures do not defer
  destroy; only confirmed absence ends retries. No effect executor is shipped.
- Secret values have no Debug/Display/Serialize and are zeroized on drop.
  Explicitly selected stores include injected read-only environment, hardened
  **experimental binary-format** Unix file persistence and injected keyring. No ambient discovery or silent
  fallback occurs. The opt-in macOS OS adapter is compile-checked only; tests
  use synthetic files and fake keyring drivers, never the real OS keychain.
  See crates/m8s-secrets/README.md for strict file/ACL/format and coordination
  requirements. The binary FileStore does **not** complete ADR-0012's accepted
  TOML fallback: Needs Admin card 01a15cbc-5bc1-7026-8894-8792bd61053d asks whether
  to retain TOML or accept binary. No secret-retrieval CLI or default backend is wired.

## Fixture-only provider mapping

The separate m8s-provider-vast crate exercises hand-written S1-derived API fixtures
through an injected in-memory transport. It has no network client, credentials,
default transport or daemon dependency. Community/secure filters, exact decimal
units, labels, launch handles, complete snapshots and owner-checked destroy are
tested offline. See its README for schema assumptions and live-validation gaps.
This does not enable a live launch path.

## Not implemented / not claimed

No live Vast transport, live account access, signed-image gate,
iroh bootstrap/control or sealed bundle, MP-001 signer integration, mesh supervision,
actual DMS self-destroy, default secret-backend selection/secret CLI, config loader,
schedules/queue triggers, `top`, eval/audit/harness, node image, or tiny-model mesh E2E.
The daemon refuses launches from any Provider whose id is not `mock`.
Core placement/budget policy is not a substitute for durable live budget reservations;
the mock path incurs no money and does not claim to enforce live spend.

Before any live work: finish secure bootstrap/admission, signed-image verification,
durable budget reservations, scoped self-destroy transport and supervision; integrate
and verify patched mesh. With Admin present and explicit authorization, test a
throwaway instance's destroy credential, lost-network DMS (destroyed, not stopped,
within TTL + 5 minutes), provider uncertainty recovery, zero billable resources
after down, real bandwidth and 27B/70B serving/canary. No live acceptance criterion
has been met by mocks.
