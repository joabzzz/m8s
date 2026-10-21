# Vast preflight (offline / unqualified)

Standalone Python 3 stdlib tooling; no changes to core/provider crates or Cargo.
**Default CLI is offline. No launch, destroy or payment implementation.**
An explicit opt-in read-only HTTPS runner exists but is NOT authorized for live use.

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
  result/header export, no key argument on a command line. Core client has no
  environment reads; opt-in runner supplier reads only VAST_API_KEY at runtime.
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

`https_readonly.https_client` constructs a real stdlib HTTPS adapter without
connecting or obtaining the key. Its fixed origin is `https://console.vast.ai:443`;
only exact GET account and POST fixed on-demand offer query are accepted. No
redirect following, retries, proxy discovery, base URL override or debug logging.
TLS verifies hostname/certificates with compiled-in CA paths, ignores certificate
environment overrides, and fails closed if trust loading fails. Socket I/O timeout
is 10 seconds, body read <=1 MiB+1, oversized/encoded/error/redirect replies fail
opaquely. This socket timeout is NOT an independent whole-process watchdog or
hard wall-clock bound against DNS/slow-drip traffic. Use an external bounded
supervisor before live execution. Client/runner exports only whitelisted facts.

`readonly_runner.py` without flags is offline and does not consume credentials.
`--authorized-readonly` is a future root-authorized opt-in, not an authorization
grant in itself. Runtime injection must be established securely before use; no
key value goes in argv, files, logs or tool arguments. Runner always reports
`live_authorized=false`, unknown topup and blocked pricing qualification. No
unauthenticated or authenticated provider calls have been made by these tests.

Python object introspection, traceback-local dumps and process memory are not
covered by repr redaction; no zeroization claim. Never give an account key to a
rented host. Test connection factories are trusted test-only injection seams,
not user-selected hosts/transports.

Evidence, official links, unresolved live gates and proposed independent cleanup
watcher design are in Knowledge:
[Metal-0-6 preflight](agor://kb/m8s/research/metal-0-6-vast-preflight.md).

## Local simulation watchdog and read-only supervisor

`local_watchdog.LocalWatchdog` accepts the exact in-memory `FakeProvider` only.
It holds a directory-scoped account-wide flock, fsyncs atomic journals containing
intent/reserves before hypothetical submission, never creates anything, and
reconciles exact owner/run with fresh complete instance+volume evidence. A
submitted uncertain create cannot be retried or inferred absent. Fake destroys
repeat until explicit absence; failures latch admission blocked. Aggregate initial
$2 reservations survive restart and are conservatively retained as spent at
cleanup (no refund without billing evidence). `poll` uses injected clock/pause;
exhaustion alerts/blocks rather than declaring success. Journal rows are retained.
Use one trusted canonical 0700 state directory; different directories do not
coordinate locks. Corrupt/partial state fails closed, not reset. This is not a
credential store or malicious-local-user boundary.

`readonly_supervisor.run_readonly(supplier, authorized=True)` is a future-explicit
root-authorized library entry, NOT executed by these tests. It runs only the fixed
read-only runner with a fresh minimal child environment, key inherited at runtime
from the injected supplier (never argv), 20-second monotonic I/O deadline, process
group SIGKILL BEFORE direct-child reap on ALL outcomes (including clean exit),
bounded 32KiB stdout and
strict output-schema whitelist. stderr discarded; partial output dropped on any
error. Reap target is bounded by two additional seconds; OS scheduling/spawn/reap
latency is not hard-real-time. Tests use standalone synthetic Python children,
including sleeping descendants, not the real provider runner. Default supervisor
CLI is offline. Secret suppliers and local executable/source tree are trusted.

**Local process/host loss defeats these local guards.** No independent remote or
provider watchdog is deployed/qualified; every live gate remains false. Fake
provider destroy lists are not evidence that real storage/instances were deleted.

Supervisor ownership contract: exclusive child reaper, default SIGCHLD handler,
non-reaping POSIX waitid(WNOWAIT) observation before group cleanup, then bounded
wait/reap. Never signal own process group, PID<=1, already-reaped PID or lost
ownership. Native os.waitid/WNOWAIT REQUIRED; no ctypes/libSystem/handwritten ABI fallback.
Unsupported interpreters fail before supplier/spawn; no early-reap fallback.
The check wrapper selects fixed installed /opt/homebrew/bin/python3.14 here,
otherwise python3, and verifies native capability before any tests. System
Python3.9 is explicitly tested to refuse without supplier calls or child spawn. Cleanup errors
discard even valid output. In this sandbox, signaling an exited zombie-only group
can return EPERM: that remains an opaque failure, never interpreted as success.
Actual regression children retain live stdout-closed descendants; kernel FIFO EOF
proves their writer closed after group kill. No provider process was used.
