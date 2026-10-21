# m8s-agent local lifecycle slice

This binary is **not a production node entrypoint**. Default invocation fails
closed without spawning a child. It has no network listener, secure bootstrap,
certificate verifier, model readiness assertion, provider client or real destroy.

Local-only exercise (non-secret arguments only):

```sh
cargo run -p m8s-agent -- --local 2 -- /bin/sleep 60
```

It starts a direct child with an empty inherited environment, null stdin/stdout/
stderr, and no shell interpolation. At TTL expiry or early child exit it latches
DMS teardown, requests SIGKILL for the dedicated process group and attempts a
nonblocking direct-child reap step. It exits **2** with provider absence
unconfirmed. A pending local termination is not completed cleanup or destruction. No plaintext heartbeat channel exists. Local mode
never renews its lease and is not evidence of real admission/provider behavior.

## Integration boundaries

- `BootGate` is a trusted adapter boundary. `UnavailableBoot` rejects everything;
  `LocalOnlyBoot` is explicitly for local tests. A real adapter must authenticate
  iroh's pinned owner, validate the sealed policy/certificate and complete the
  instance/self-destruct capability checks before permitting a spawn. The gate
  does not claim Ready: served model and owner-routed canary are still required.
- `Supervisor::heartbeat` takes an **already authenticated transport endpoint**,
  not a claimed message identity. Existing owner/delegate ACL and hard lifetime
  checks apply. Do not connect stdin/TCP plaintext or inference traffic to it.
- `shutdown` latches teardown; `tick(injected_seconds)` runs it. Call ticks
  serially on a monotonic clock. Local stop/reap pending or failure never skips
  the independent Destroy/ConfirmAbsent attempts. Drop performs one best-effort
  nonblocking step only; it does not wait for a live process or start a reaper.
  The OS init must reap orphan descendants.
- `TeardownDriver` must bound each operation's I/O time and scope wipe to the
  instance's dedicated material. The scheduler performs at most one effect batch
  per due tick with exponential delay capped by `RetryTeardown`. Uncertainty
  keeps retrying; successful destroy is not confirmed absence. This interface
  cannot enforce execution deadlines on arbitrary synchronous driver code.
- The binary's destroy/wipe/drain driver is deliberately unavailable. Tests use
  fake destroy outcomes only. No secret extraction or environment mutation is
  implemented. Child environment clearing prevents inherited credentials from
  reaching the local child, but is not a production key-lifecycle implementation.

## Known qualification gaps

Containment of descendants that escape the process group, graceful model
API draining, protected tmpfs material and wipe, real authenticated transport,
bootstrap policy binding, per-instance credential custody, alerts and actual
provider destroy/absence are not implemented here. Use a qualified container init
for orphan reaping; do not deploy this binary as container PID 1. Do
not use local process exit or fake driver tests to assert stopped billing.

Process-group signalling keeps the leader unreaped with `waitid(WNOWAIT)` until
its group is signalled, preventing PID reuse. The supervisor must be the sole
child reaper; external waitpid or SIGCHLD auto-reaping fails closed. Own-group,
zero/negative and broadcast-group targets are rejected. Only ESRCH is accepted
as local group disappearance, not provider absence. Accepted SIGKILL does not
prove exit: a process can remain alive with a pending signal. OS permission
failures remain cleanup uncertainty and never prove destruction; provider teardown
attempts still run independently. Some
macOS sandbox policies deny signalling an already-exited group leader; this
capability remains unqualified rather than being silently treated as success.

## Local executable termination signals

Only the local executable installs `signal-hook` flag handlers for SIGTERM and
SIGINT, before spawning its child. The library never installs handlers. A handler
sets an atomic flag only; the normal main loop observes it (25ms polling interval),
latches `shutdown`, and attempts one DMS teardown batch, including process-group
SIGKILL and nonblocking direct-child reap progress. This is a request for forced
local teardown, not guaranteed completed cleanup or graceful
forwarding of the received signal or successful model-API draining. Repeated
signals are idempotent requests, not an immediate-exit escalation that could
orphan the child. Registration failure prevents launch. Registered callbacks are
removed when the executable's run scope finishes; this is not a library API for
restoring process-wide signal dispositions.

Local mode exits 2 after that batch, without indefinite provider retries. A
local teardown request still reports **provider absence unconfirmed**. Library
local pending/errors remain separate from the provider outcome, not absence. Tests send real SIGTERM/SIGINT (including repeated mixed
signals) to a local agent with a synthetic child/grandchild and check bounded
exit plus group-member disappearance. These are local OS tests, not secure
admission or provider qualification.

SIGKILL cannot be caught: killing the agent that way, aborts, host failure, or
uninterruptible kernel operations can defeat best-effort cleanup/exit deadlines.
The 3-second test bound is observed local behavior, not a guarantee under kernel
failure. OS signal permission failures, escaped groups and orphan-reaping limits
remain as documented above. A qualified init/owner-side destroy watcher is still
required; no signal test proves provider destruction or stopped billing.

## Nonblocking stop/reap and spend containment

`tick` never uses `Child::wait`, sleeps or poll-until-exit loops. Process operations
use `waitid(WNOHANG | WNOWAIT)` for exit evidence without releasing the reserved
leader PID, request group SIGKILL once accepted, and call `Child::try_wait`
(WNOHANG) **only after exit was observed and group signalling was accepted**.
No numeric group signal is sent after the leader is reaped. Failed signalling
retains the unreaped leader for a possible later safe retry. External waitpid or
SIGCHLD auto-reaping is unsupported; observed ownership loss fails closed.

Provider `TeardownState` and `local_termination_state()` are separate:

- `Running`: no local teardown requested yet.
- `StopPending`: request accepted/in progress, no confirmed direct-child reap.
- `DirectChildReaped`: direct child only; not proof every descendant or host is gone.
- `Uncertain`: local OS operation failed; never absence.

`child_exited()` performs one nonblocking step and returns true only after direct
child reap. It may report an OS error; `tick` instead records local `Uncertain`
and continues the independent provider effect batch/backoff, so a watchdog must
not abort its containment loop merely because local cleanup failed. Failed or
pending local stop cannot complete Drain successfully. A successful destroy
request still does not satisfy ConfirmAbsent. Only explicit driver-confirmed
provider absence stops provider retries; local reap polling can continue afterward.
A kill-pending child never fabricates that confirmation.

Retain and tick the supervisor until `DirectChildReaped`, or explicitly transfer
reaping responsibility to a qualified owner before dropping it. Drop does one
bounded best-effort stop/poll step; a pending child/zombie can remain after Drop.
The short local executable is deliberately not a persistent provider watchdog or
reaper. Parent exit/container init is still needed for orphan adoption/reaping.

The private process-operation seam is injectable only in unit tests: synthetic
never-exits-after-kill, kill denial and lost-reaper cases prove Destroy retries and
backoff progress without creating any real unkillable process. Other tests verify
exit-before-reap ordering, no operations on reused PIDs, independent provider/local
evidence, and ordinary local child/grandchild/signal regression behavior.

These are nonblocking APIs, not preemptive syscall deadlines. WNOHANG prevents
intentional waiting for child exit; the OS syscalls, trusted teardown drivers and
adapter destructors must still return promptly. A hung kernel, arbitrary blocking
driver or dead process cannot be made bounded by this synchronous coordinator.
No paid/real provider, secure transport, wipe or readiness path is qualified here.

## Optional authenticated control bridge

`authenticated-control` (off by default) enables a path dependency on bootstrap's
real `iroh-transport`, using its sole `tls-aws-lc-rs` backend. It starts no network
endpoint and installs no process-global handlers. The executable is unchanged.

`Supervisor::start_with_control_node(..., local_node: EndpointId)` pins an
immutable local TLS target from **trusted local owner/boot configuration**, not
from the incoming event. This is configuration, **not proof of secure bootstrap
or admission**. Existing `start()` remains explicitly unbound; verified dispatch
on it returns `ControlError::Unbound` without consuming/applying authorization.
The production `UnavailableBoot` remains closed. LocalOnlyBoot is synthetic only.

`verified_heartbeat(evidence: VerifiedHeartbeat<'_>, now)` consumes one opaque
bootstrap event against the stored target. Bootstrap owns all current live-policy,
revocation, connection/ALPN, lease, freshness and single-use checks, retaining its
authorization read guard through the short DMS callback. Only the canonical
TLS peer and verified receive timestamp reach DMS; caller dispatch time cannot
extend the lease. DMS still enforces its own ACL, monotonic time, TTL and hard
lifetime. Configure its ACL with canonical iroh endpoint strings and bound its
hard lifetime by the relevant verified certificate expiry. No permissions are
cached or renewed here. Rejection never calls the DMS callback.

The raw-string `heartbeat` library API remains a trusted compatibility seam,
not a new stdin/network entrypoint or authentication bypass for adapters. No
production event constructor or synthetic evidence factory is exposed.

Focused checks: `cargo test -p m8s-agent -j1` and the same with
`--features authenticated-control`, plus all-target Clippy for both. Feature unit
helpers exercise configuration refusal/routing and DMS only; compile-fail docs
check evidence construction/reuse. They do **not** exercise real protocol
receipt/consumption. Existing bootstrap supported-runner TLS fixtures remain
explicitly ignored, with actual execution blocked here by AF_ROUTE netmon EPERM.
No retry/bypass was attempted. Real client/admission, certificate qualification,
served model and owner-routed canary remain required; this bridge claims no Ready.
