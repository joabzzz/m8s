# local-development provider

**Explicit synthetic development only.** This adapter runs a real local child
process, not a cloud/GPU instance. It is a future local lifecycle integration
seam, not a substitute for authenticated mesh E2E. It is not wired into the
CLI/daemon. No model is downloaded or served by this crate.

## Opt in

Create a dedicated private (`0700`) state directory and call
`Local::open_development(directory, owner, DevelopmentCommand::new(...))`.
The executable must be absolute; argv is pinned at construction, never derived
from launch intent fields. Both must be trusted, nonsecret local configuration.
A suitable synthetic process is `/bin/sleep` with argv `60`. If deliberately
running `m8s-agent --local`, its admission remains synthetic; production agent
boot remains fail closed. This adapter never supplies bootstrap material or
owner keys. Do not use it to launch production mesh workloads.

The process is run by the verified `m8s-agent::supervisor` library, with
`LocalOnlyBoot` explicitly selected. This is **not** certificate verification or
authentication. `InstanceState::Running` only means a child was spawned, never
Ready, model service, owner-routed canary, secure admission, or transport success.
The reused supervisor's DMS driver returns only Failure: it implements no external
wipe/destroy/absence or cloud watchdog. The adapter uses injected synthetic time
zero only when explicitly shutting down; no authenticated heartbeat or lifetime
qualification is implied.

## Offer and development policy

`search` returns only `local-cpu-unmeasured`: zero GPUs, zero VRAM bytes,
unknown country/region, zero Mbps and zero reliability basis points (unmeasured,
not measured zero throughput/reliability). No GPU, cloud bandwidth, secure cloud,
privacy level or measured hardware claim is made. Zero hourly micro-USD means no
provider billing; local electricity/storage are not estimated. Non-interruptible
means no spot-market/preemption contract, not guaranteed host availability.

Normal location/GPU/bandwidth constraints should reject this offer. A future
caller must explicitly choose a CPU-only development policy rather than relaxing
production filters globally. Launch accepts only the exact local offer. No core,
Provider trait, daemon policy, CLI routing or readiness checks are changed here.

## Ownership, durability and uncertainty

- One constructor-pinned exact owner and exclusive file lock per directory.
  `list_owned` matches the full owner string, never a prefix. Foreign launch
  owners are invalid. Unknown IDs cannot be destroyed or reported absent.
- IDs are directory-namespace-hashed monotonic identities, not PIDs. Keep the
  directory for the entire lifecycle. Never clone/reset/delete its contents,
  reuse it for another owner, or move a copy into service concurrently. The
  directory is descriptor-opened with O_DIRECTORY/O_NOFOLLOW/O_CLOEXEC and
  fstat-checked for the effective UID and exact 0700 mode. Lock/state/temp files
  are opened relative to that descriptor with O_NOFOLLOW/O_CLOEXEC, and checked
  as regular effective-UID-owned 0600 files with one hard link. FIFOs, file
  symlinks, wrong modes/owners and hard links are rejected, never repaired.
  The lock descriptor remains stable through all supervisor Drop cleanup.
  Device/inode checks at API/persistence boundaries reject directory-anchor,
  lock or state replacement. Ancestors must be trusted and the directory must
  belong exclusively to this OS user; these checks are not an adversarial
  concurrent-filesystem sandbox or protection against malicious same-UID code.
- An owner/node_uid intent (including tier) is fsynced before spawn. A retry with
  the same identity returns its existing instance only with a retained process
  handle; a changed tier is invalid. Spawn failure leaves unresolved intent and
  blocks blind retry. ANY unresolved non-absent record without a retained
  handle blocks ALL fresh launches, including a different node_uid; retained
  owned handles can still be observed and destroyed. Tombstones alone do not
  block a different UID. No process search or PID probe is performed.
- State writes use a private new temporary file, fsync, rename and directory
  fsync. Reads and serialized writes are bounded to 1 MiB, records (including
  tombstones) to 256, and owner/node_uid/tier labels to 256 UTF-8 bytes. Duplicate
  JSON record IDs are rejected during parsing rather than overwritten in a map.
  Record exhaustion requires future lifecycle design, not tombstone deletion.
  Any write error poisons the adapter until reopened. An interrupted
  `state.pending`, missing existing state, malformed state, lock failure or OS
  error fails closed. Do not automatically delete an interrupted journal.
- Natural child exit is **Stopped**, not Absent. Destruction intent is persisted
  before cleanup. `destroy` is an acknowledgement; only a subsequent `describe`
  or complete `list_owned` can persist an absence tombstone after retained-handle
  cleanup/reaping has been observed. Tombstones prevent UID resurrection.
- Local Absent proves the retained **direct child was reaped**, after successful
  owned-group SIGKILL (or OS ESRCH), under the exclusive-reaper/group contract.
  It does NOT prove all descendants were reaped, immediate termination of an
  uninterruptible group member, escaped-descendant cleanup, billing cessation,
  cloud absence, model-cache wipe or Ready. OS waits can block; this is not a
  bounded teardown guarantee. The supervisor preserves the unreaped leader
  identity until group signalling, avoiding PID-reuse kills.
- After restart/handle loss, **all non-absent records are uncertain**, including
  previously stopped records or acknowledged destruction. Describe/destroy and
  complete owner listing fail; neither retry nor fresh-UID launch can spawn. Numeric
  PIDs are neither persisted nor adopted/signalled. Known durable absence alone
  survives restart. There is deliberately no automatic recovery or journal
  erasure API; manual recovery requires out-of-band evidence and future design.
- Drop attempts group cleanup without persisting destroy/absence. Abrupt parent
  death can leave children; no unattended watchdog guarantee is offered.

## Process/environment boundary

The inherited environment allowlist is **empty** (`env_clear`), stdin/stdout/
stderr are null, and no provider credential, keyring, owner key or secret store is
accessed. Config, adapter and process state have no Debug/Serialize interface
that exposes executable argv. The journal contains only nonsecret ownership and
lifecycle metadata, never a process ID, executable/argv or secret material.

A trusted executable can still access host files/network using its OS rights;
this adapter does not create a security sandbox or enforce executable behavior.
Choose only a synthetic local program with no remote calls, downloads or secret
access. It must not daemonize or escape its dedicated process group. External
SIGCHLD auto-reaping/waitpid is unsupported and yields uncertainty; host init
must reap orphan descendants. No host process scans, arbitrary PID signalling,
broad kills, Docker, network-policy or sandbox bypasses are used.

## Verification

Focused checks (own Cargo home; one compiler job):

```sh
CARGO_HOME="$PWD/.cargo-home" cargo fmt -p m8s-provider-local -- --check
CARGO_HOME="$PWD/.cargo-home" cargo test -p m8s-provider-local --locked -j 1
CARGO_HOME="$PWD/.cargo-home" cargo clippy -p m8s-provider-local --all-features --all-targets --locked -j 1 -- -D warnings
```

Tests use real synthetic local children plus private temporary journals: duplicate
UID, exact ownership, no fake GPU offer, natural exit with owned-group descendant,
explicit destroy/observed absence, group descendant containment, confirmed
absence restart, unresolved restart, spawn/storage failure, ECHILD, malformed/
missing/oversized state, duplicate IDs/record caps, UID/mode/nlink checks,
symlink/FIFO refusal, anchor/lock replacement, descriptor CLOEXEC and empty
inherited environment. Restart tests refuse different-UID launches in every
non-absent phase while allowing retained-handle cleanup. No credentials are read,
requested or set. Host sandbox denial of signalling an already-empty group has
been observed on macOS: the dedicated empty-group test verifies uncertainty,
**not** successful empty-group cleanup. This limitation is not reinterpreted as
absence; unsandboxed qualification remains separate. Full `scripts/check` and
cross-platform qualification belong to the integration owner, not this lane.

Architecture references: ADR-0004 adapter separation; ADR-0006 secure bootstrap
unchanged; ADR-0007 stopped is not destroyed; ADR-0010 no inference gateway.
