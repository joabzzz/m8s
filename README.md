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

## Try the mock

Choose a short, private state-directory path (Unix sockets have length limits).
The daemon creates it with mode 0700 and its socket with mode 0600. Existing
non-private directories are refused. A process lock prevents two daemons from
using the same directory.

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
- Secret values have no Debug/Display/Serialize and are zeroized on drop. Only an
  explicitly injected read-only environment store exists; it never scans ambient
  variables. No secret-retrieval CLI, file fallback or Keychain backend exists.

## Not implemented / not claimed

No Vast transport (even fixture-only), live account access, signed-image gate,
iroh bootstrap/control or sealed bundle, MP-001 signer integration, mesh supervision,
actual DMS self-destroy, OS Keychain/file SecretStore, secret CLI, config loader,
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
