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

## Timed parent-owned development foundation (Phase A)

`DevelopmentCommand::fingerprint()` returns SHA-256 over a versioned domain,
fixed empty-environment/null-stdio/new-process-group mode, and length-prefixed
raw Unix program/argv bytes (including count and empty arguments). Construction
rejects NUL, relative programs, more than 256 arguments or more than 64 KiB total
program+argument bytes. The digest pins **configuration**, not executable file
contents; path replacement and trusted program behavior remain caller assumptions.
No command bytes or digest are added to the journal.

```rust,no_run
# use m8s_provider_local::{DevelopmentCommand, Local, LocalDeadlines};
# use m8s_provider::LaunchSpec;
# use std::{path::Path, time::{Duration, SystemTime}};
# fn example(directory: &Path, spec: &LaunchSpec) -> m8s_provider::Result<()> {
let command = DevelopmentCommand::new("/bin/sleep".into(), vec!["60".into()])?;
let mut local = Local::open_development(directory, spec.owner.clone(), command)?;
let stop_at = SystemTime::now() + Duration::from_secs(1);
let deadlines = LocalDeadlines::new(stop_at, stop_at + Duration::from_secs(2))?;
let launch = local.launch_development_until(spec, deadlines)?;
let stop = launch.stop_handle(); // cloneable, stop-only; cannot renew or launch
stop.request_stop();             // atomic latch + coalescing wake, no journal
// Keep `launch` for non-durable direct-child evidence. Dropping it requests stop.
# Ok(())
# }
```

`LocalDeadlines::new` captures Instant **before** high-resolution SystemTime once,
checks positive serving time, ordered cleanup and conversion overflow, and pins
both monotonic boundaries without rebasing. Periodic real wall comparisons allow
earlier stop after forward jumps/resume; rollback cannot extend the monotonic
boundary. There is no public fake clock, callback, ProcessOps or deadline renewal.
This is a local scheduling assertion, **not** Store claim authority/admission.

A dedicated detached worker owns the actual Supervisor **from spawn onward**.
Durable Intent still precedes its spawn request. A pre-spawn expired deadline
refuses spawn but leaves the intent uncertain; a late-returning spawn is stopped
before its reply. Response loss cannot discard ownership or authorize retry.
The launch call waits on `started.recv()` with **no independent caller timeout**.
There is no bounded launch-return promise: while the worker's own kernel spawn
primitive is blocked, that worker cannot poll or request stop. Kernel-spawn stalls
are explicitly outside the watchdog contract. Once spawn returns, it checks the
anchored deadline and requests stop immediately if late, before replying; it does
not abandon the child or retry create. A stop handle is returned only after this
synchronous launch wait and coordinator persistence complete.

Only one unresolved timed worker is allowed per Local; fresh untimed launches
are also blocked while it is unresolved. A timed launch is refused while this
Local retains any untimed Supervisor. Retrying the timed method never reissues
move-only launch ownership; the ordinary Provider API can still observe an
existing UID but cannot renew its deadline.

The worker uses nonblocking retained-identity Supervisor operations on each poll,
with a 50 ms maximum polling interval under continued scheduling and promptly
returning OS primitives. A first stop request wakes it earlier. Repeated stop
requests do not enqueue work or create a busy loop. It never acquires caller,
journal or SQL locks, performs storage I/O, or blocks sending a notification.
Only the coordinator locks the existing Store for validation/persistence. The
worker merely retains an Arc of that **same** Store/lock/directory anchor; it
never reopens the lock. Drop of Local or LocalLaunch requests stop without join.
The detached worker keeps this anchor and Supervisor until **direct-child reap**;
persistent errors retain ownership and resources rather than claiming absence.
Stopped workers do not acquire durable destroy authority on their own.

Timed `destroy` requests stop **before** validation, serialization or fsync, unlike
the legacy untimed ordering. The existing durable Intent already records the
possible effect. The coordinator alone persists Destroying, then observes retained
DirectChildReaped evidence before persisting Absent. Deadline, natural exit and
Drop alone are only Stopped (or uncertainty), never absence. Journal failure or
restart still blocks fresh effects with unresolved records, even if cleanup
happened in the old process. No PIDs are persisted/adopted or recovered by scanning.

`LocalLaunch::termination_state` exposes only local non-durable evidence;
`instance()` is the original launch snapshot, not live readiness. The cleanup
miss flag latches conservatively if a poll reaches the cleanup boundary before
reap has been observed, including a late first observation. Missing the cleanup
boundary never stops cleanup attempts or turns uncertainty into success.

**This is a 50 ms stop-request polling bound, not an exit deadline or hard real-time
promise.** Parent death/abort, process-wide suspension, host loss, scheduler
starvation, kernel stalls and trusted programs escaping their group are outside
the contract. Instant suspend behavior is platform dependent; there is no
suspend-resistant containment claim. Threads do not survive parent loss. A trusted
program can use host/network rights; no sandbox, remote/provider watchdog,
certificate, served model, canary, Ready, cloud absence or billing proof is added.
No Phase B Store executor, daemon/CLI integration or paid route is implemented.

Additional tests use controlled synthetic children and private cfg(test) bounded
channel gates: caller mutex and journal/store stalls; stop/deadline/late spawn;
reply disconnect; Drop lock retention until reap; natural exit with a controlled
owned-group member; cleanup miss; no restart adoption; raw-byte digest boundaries;
fractional anchored conversion, rollback, forward change and overflow. A small
private worker-engine seam wraps only Supervisor stop/observation and real runtime
clock/park operations; production still owns the actual Supervisor.
Deterministic scripts hold uncertainty, simulated kill denial and never-reaped
states for eight fixed polls beyond cleanup while verifying the worker alone
retains the exact Store lock. They allow exit only after explicit direct-reaped
evidence, with no later process action, and exercise a full reply channel.
These are worker-state/scheduling tests, not injected kernel or real kill-error
qualification. Existing Supervisor ECHILD/kill-denial/non-reap tests separately
cover its exclusive-reaper operations; no PID or signalling logic is duplicated.
