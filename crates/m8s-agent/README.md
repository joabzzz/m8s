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
DMS teardown, kills the dedicated process group and reaps the direct child and exits **2**: provider absence is
unconfirmed, not destroyed. No plaintext heartbeat channel exists. Local mode
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
  serially on a monotonic clock. Drain failure cannot suppress local termination
  or provider teardown attempts. Drop is best-effort process-group cleanup; the OS init reaps orphan descendants.
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

OS SIGTERM/SIGINT forwarding, containment of descendants that escape the process
group, graceful model
API draining, protected tmpfs material and wipe, real authenticated transport,
bootstrap policy binding, per-instance credential custody, alerts and actual
provider destroy/absence are not implemented here. Use a qualified container init
for OS termination handling; do not deploy this binary as container PID 1. Do
not use local process exit or fake driver tests to assert stopped billing.

Process-group signalling keeps the leader unreaped with `waitid(WNOWAIT)` until
its group is signalled, preventing PID reuse. The supervisor must be the sole
child reaper; external waitpid or SIGCHLD auto-reaping fails closed. Own-group,
zero/negative and broadcast-group targets are rejected. Only ESRCH is accepted
as group disappearance. OS permission failures remain cleanup errors and never
prove destruction; provider teardown attempts still run independently. Some
macOS sandbox policies deny signalling an already-exited group leader; this
capability remains unqualified rather than being silently treated as success.
