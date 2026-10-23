> Current prototype status: the opt-in `vast-node` entrypoint is proven with
> canonical certificate bootstrap, real native inference, heartbeat supervision,
> and exact-resource cleanup. The owner CLI and independent watchdog completed
> the [live acceptance run](../../deploy/container/demo-proof.json). Earlier
> sections below describe the default local exerciser and library checkpoints;
> their limitations do not describe the opt-in live path. Container recipes run
> the agent under Tini, never directly as PID 1.

# m8s-agent local lifecycle slice

The default-off `secure-runtime` feature exposes
`node::runtime::redeem_and_run`: one pinned TLS redemption followed by native
identity/certificate checks and live heartbeat supervision. Invalid plans refuse
before redemption. Startup failure attempts drain, wipe, destroy, and explicit
absence lookup without allowing an earlier failure to suppress destruction.
The caller retains the scoped cleanup driver for independent retries; cancellation
and local child exit never establish resource absence. This library path does
not yet wire the executable entrypoint or establish provider destruction authority.

This binary is **not a production node entrypoint**. Default invocation fails
closed without spawning a child. It has no network listener, secure bootstrap,
certificate verifier, model readiness assertion, provider client or real destroy.

Local-only exercise (non-secret arguments only):

```sh
cargo run -p m8s-provider --features agent-runtime --bin m8s-agent -- --local 2 -- /bin/sleep 60
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

## Default-off sealed node material handoff

`--features sealed-node` adds a **library integration seam**, not a runnable
production node mode. The binary remains the existing local exerciser; its secure
path still refuses. No new listener, signer, key loader, provider driver, download
or automatically approving BootGate is supplied. The explicit container backend
below is separate from staging; no Docker invocation happens on import/default use.

`node::NodeMaterial::stage` consumes received bootstrap bytes plus explicit trusted
`MeshLaunch` inputs. Production accepts only the existing concrete canonical
`Mp001CertificateVerifier`, checks owner/node/validity/model/TTL binding, and
rechecks before returning. It does not prove that the publicly constructible
ReceivedBundle came from TLS. The coordinator must use the real redemption path
and existing authoritative Store burn, never a second ledger or fabricated bundle.
The generic verifier seam is private and used by synthetic file tests only.

Staging requires a fresh final directory under a canonical owned0700 parent;
existing paths are refused, never adopted/repaired/chmodded. Retained no-follow,
nonblock descriptors check ancestor ownership/write safety, exact private modes,
ACL absence (using the existing borrowed-FD checker), file type/link/inode/size
before/after fixed child creation and before sensitive bytes. Files are
`join-token`, `node-ownership.json`, `runtime.toml`, all0600 in0700. Any late
failure may leave partial private artifacts; no rollback, delete or wipe is
implied. Drop closes anchors, does not unlink. These checks are not malicious
same-UID confinement, encryption, or proof the backing mount is tmpfs. The real
container coordinator must provide a dedicated tmpfs and its qualified wipe path.

The already-created explicit **node** key is retained/checked as a private file,
not decoded, generated or treated as an owner key here. A canonical native-key
endpoint match is still a mandatory BootGate prerequisite. The accepted mesh
source has two important semantics: `--owner-required` demands an OWNER keystore,
so is deliberately absent; a bad pre-signed certificate can warn and start
Unsigned. Therefore both supervised and container commands now require the
additive `--node-ownership-required` flag alongside the certificate path. There
is no compatibility fallback that drops the flag: an older native parser must
refuse startup. The corresponding canonical implementation is source-reviewed
and focused-test green, but its local commit and native consistency check remain
blocked by tool policy; no qualified serving image is claimed here. The
coordinator must establish the real native key/endpoint before approving startup,
and still verify actual served identity/model and owner-routed response.
No Ready assertion is produced by staging or spawning.

`Supervisor::start_mesh` consumes and retains the material while using the same
owned process-group cleanup engine, existing BootGate, and fixed node heartbeat
binding. It rechecks canonical certificate/files and fresh DMS timing; explicit
TTL must match the bundle and hard lifetime must fit certificate remaining time.
It does not infer a DMS ACL from a wire identity: the coordinator installs the
actual received ACL in current control policy and its trusted DMS configuration.
Real verified heartbeat evidence retains all existing live policy checks.

The command uses actual accepted mesh flags: `serve --gguf` for local fixtures
or `serve --model` for native model resolution, protected join/cert
files, explicit config, `--trust-policy allowlist --trust-owner`, headless and
no default plugin installation. No owner-key/local-model-only/client/auto/publish
flag, public listener widening or raw token/config/key bytes enter argv/env.
Environment is cleared, then ONLY fresh HOME, explicit node-key path and the
source-supported analytics opt-outs MESH_LLM_ANALYTICS=0/DO_NOT_TRACK=1 set;
stdin/stdout/stderr are null, as for existing supervision. Native runtime/backend
packaging and actual secure entrypoint are still required. Opaque config is
retained, but **nonempty config refuses launch** until its native policy mapping
is implemented; it is never silently ignored or allowed to override trust/key
policy. Empty config is an explicit received input, not an invented live profile.

Pure owned-file/command tests do not run mesh, Docker, iroh or a model and do not
qualify authentication, inference, destruction, or billing. The public launch
path still requires an independently qualified gate and bounded teardown driver.

### Native public-ID preflight (still not admission)

`NodeMaterial::check_native_node_id(verifier, now, timeout)` executes the exact
selected mesh binary once as `auth node-id --key-path FILE --json`. Native mesh
owns key decoding; this adapter never parses a key or enables identity `host-io`.
The explicit timeout must be positive and at most60 seconds. One absolute deadline
covers preparation, capture and successful process exit; no retries. Spawn, kernel
scheduling and final direct-child kill/reap cannot be synchronously preempted.
Only256 stdout bytes are retained, stderr/stdin are null, environment is cleared
apart from fresh HOME and the two analytics opt-outs. Strict JSON requires exactly
one `node_id` field with lowercase64-hex matching the staged endpoint. Extra,
duplicate, malformed, oversized, mismatched or unsuccessful output refuses with a
fixed error. Retained anchors/canonical certificate are checked before and after.
Timeout/error kills and reaps only the retained direct child; no PID adoption or
process-group signaling. No key/public output/error chain is logged.

Success is a point-in-time native public-ID observation, **not an admission
permit**, TLS receipt, ongoing file integrity guarantee or proof of native
certificate enforcement. The existing BootGate remains mandatory. In particular,
this adapter does not fix native unsigned fallback, grant signing policy, wire a
secure node binary or authorize launch. Tests run only fresh fake programs and
synthetic key placeholders, never mesh or real keys.

### Container launch/request/stop integration contract

The accepted native source is `cecee5d23e33d2626a0bf4624bfa71dc1615d167`.
Its `mesh/deploy/docker/Dockerfile.client` is explicitly **not a worker image**;
`APP_MODE=worker` refuses, and its console mode's `--client --auto` is unsuitable.
A full Linux product needs both the backend-neutral release host and matching
native runtime. The actual source entrypoints are `just release-host-build` and
`just release-runtime-build cpu x86_64-unknown-linux-gnu` (or the explicitly
selected compatible architecture/backend); `just release-build` builds the
native/Skippy/host components (product-manifest composition is a separate step). The native packager produces `dist/native-runtimes`
artifacts with `manifest.json`, shared libraries, licenses and archive checksums.
Host/runtime release and ABI must match; a client executable alone is insufficient.
These are source-derived build interfaces, **not executed/qualified build commands
in this environment**. No image digest, GPU compatibility or model is invented.

The coherent qualification sequence is:

1. Explicitly select reviewed Linux host/runtime artifacts, pinned base image and
   separately approved local GGUF; package both host and runtime without baking
   keys, join tokens or owner material into an image. Native runtime installation
   supports explicit `runtime install ID --bundle-dir DIR --cache-dir DIR`; any
   eventual image build must disallow network fallback and verify its actual
   host/runtime contract rather than downloading a recommended runtime.
2. Create fresh owned container state with dedicated private tmpfs material and a
   read-only model mount, no host networking/privileged mode or host owner-key
   mount. Obtain the actual node endpoint, create the stopped container to obtain
   its actual ID, then direct sealed bootstrap and one-use Store burn bound to
   that ID; stage protected material. Run native public-ID preflight and
   establish native fail-closed certificate enforcement before BootGate approval.
3. Supervise actual mesh `serve` with the staged enforcing flags, retain
   authenticated heartbeat/DMS and exact scoped teardown. Verify the served
   identity/model and an owner-routed request, not merely an HTTP200 or process
   startup. `m8s prototype request --local-qualification` is a bounded HTTP leg;
   its output explicitly does not attest mesh identity or owner routing.
4. Stop the exact owned container/child, verify owned runtime termination and
   material teardown separately. Container stopped is not provider destroyed;
   any paid lifecycle still needs its independently authorized destroy/absence
   evidence and budget/watchdog prerequisites. Uncertainty must not trigger a
   fresh launch or be reported as absence.

Remaining integration prerequisites are concrete: the secure agent binary is
still unwired; native invalid-certificate fallback must be made fail closed for
this composition; actual direct TLS/bootstrap/canary must run on a permitted
runner; and approved matching Linux runtime/model/image artifacts are required.
No Docker build/run/pull, mesh endpoint, model inference or teardown qualification
has been performed by these source/pure-fixture checks. This contract deliberately
does not offer an executable insecure fallback container entrypoint.

### Implemented owned-container executor (unwired)

`node::container::ContainerSession` now implements actual Docker CLI
create/start/inspect/exec/stop/remove plumbing, not a plan/argv-only preview.
It starts **nothing by default**. Explicit trusted local create_stopped_for_qualification
creates only a stopped container and records its actual ID BEFORE bootstrap.
created_container_id() supplies plain observed metadata for the coordinator's
actual-ID-bound authoritative Store/direct TLS redemption; it is not provider
confirmation or admission. seal() stages received material only afterwards.
Public start refuses unsealed state and requires canonical certificate, fresh
DMS-bound checks and current external BootGate approval. No approving gate, CLI
launch route, new signer or production secure agent entrypoint is provided. The gate must qualify
**the selected immutable image**, including native hard refusal of invalid
certificates. The accepted native source's unsigned fallback is NOT qualified.
The adapter must not be used with that behavior just because staging succeeded.
DMS here is a checked prerequisite, not a background loop: the qualified coordinator
must actually retain/drive authenticated heartbeat, hard lifetime and scoped
cleanup/watchdog. No container-side DMS or owner canary is silently substituted.

Inputs explicitly select the local Docker executable/socket, preinstalled
`repository@sha256:...` image, fresh32-hex run ID, local GGUF and private UTF-8
prompt, token limit and request/command/stop bounds. No Docker context/environment
discovery, pulls, builds, models or credentials are supplied. Private run
material is mounted read-only; key/model are separate read-only mounts. Writable
HOME is a bounded private tmpfs; rootfs is read-only, capabilities dropped and
no-new-privileges enabled. The host Docker socket is never mounted. No published
port, listen-all, host network, privileged mode or raw secret argv is used.
The trusted selected local daemon is not newly authenticated by these checks;
path/inode checks are not confinement against malicious same-UID/root races.

A fixed0600 intent record precedes create. Exact returned64-hex ID plus observed
run/node labels, name and immutable image fence subsequent actions; no adoption
by name or automatic create/start retry. A protected created-ID record precedes
bootstrap/seal/start. This order avoids the impossible cycle of requiring a
container-ID-bound bundle before creating that container. These files are custody
metadata, **not an authoritative Store launch claim/paid ledger or universal
crash-durability proof**. Each intent/created-ID/start-attempt record completes
write_all, file sync_all and containing-directory sync_all before its protected
transition. Intent also syncs the existing parent of the newly created run
directory before Docker create. Failed intent write/sync prevents Docker create;
failed created-ID sync never exposes Created; failed start-attempt sync prevents
start and retains uncertainty/artifacts. No rollback, unlink, adoption or retry
is introduced. These calls are nonpreemptive and provide the filesystem's fsync
contract only, not a guarantee against every power/device/filesystem failure. Real composition must still use
existing durable launch/operation authority before effects. Failed create returns
CreateUncertain with retained material; missing ID cannot authorize destructive
commands. Start/reply/inspect uncertainty remains explicit. Teardown depends only
on retained Docker/socket identity and exact observed container ownership, not survival of join/model/prompt files, so failed bootstrap
or missing material cannot prevent emergency stop. Drop never calls
Docker or deletes material; the coordinator retains cleanup custody.

`request()` executes the existing `m8s prototype request` CLI **inside** the exact
observed-running container, to its own127.0.0.1 HTTP endpoint, avoiding listener
widening. Strict bounded projection decoding returns LocalCompletion metadata
only; it must retain not-verified identity/owner-routing and not-assessed
readiness labels. This is not an authenticated owner-routed canary. Every Docker
call uses the shared finite direct-child capture engine/one absolute command
deadline; output<=16KiB, no retries. There is no whole-lifecycle deadline claim.
Failed replies can follow committed effects. Teardown observes matching ownership,
stops if running, rechecks stopped, removes exact ID, then requires a successful
fresh all-container query with that full ID absent. Stop/rm acknowledgment or an
error alone is never absence. RemovedObserved means only this Docker observation,
not provider destruction, inference success, tmpfs erasure or host-material wipe.
After a lost remove reply, explicit observe_removed() can resolve the retained ID
with one fresh read-only query, without reissuing destruction or adopting a name.

`deploy/container/Dockerfile` is an actual **packaging recipe**, not a tested or
security-qualified serving image. It consumes only an explicitly reviewed Linux
base, selected Linux `cli/m8s`/`cli/m8s-agent` binaries, and a native-owner-composed
`product/` tree. The source's `cargo xtool product compose --bundle ... --host ...
--runtime ... --version ... --backend ...` owns its product-manifest; do not hand
encode that format. `product/mesh-llm`, product-manifest and native-runtimes stay
adjacent at `/opt/mesh`; the explicit native bundle environment selects them.
No network install step is included. Default entrypoint is the current agent,
which fails closed without its explicit local exerciser mode; the recipe does
NOT select that mode. The executor's native entrypoint override is permissible
only after the separate qualified-image gate above. No such gate is wired here.
No image build/run, real model request, TLS/owner canary or container removal has
been runtime-qualified. Fake owned executable tests are not those results.

### One-shot local demo lifecycle

After actual-ID-bound seal, `ContainerSession::run_once_and_remove` uses the same
canonical verifier, fresh DMS and external BootGate as `start`. It attempts start
once and performs at most one inference request. Whether start/request succeeds
or fails, it then takes the existing exact-ID-fenced stop/remove path. A model
completion is returned only after fresh all-container lookup confirms that ID is
absent. Failed start/request remains an error even after successful cleanup;
failed stop/remove/absence observation withholds the completion and retains
uncertainty. No create, start, request or destroy retry loop is added.

This explicit local operation does not create a gate, start an automatic cleanup
thread, survive caller/host loss, or qualify paid-provider destruction. Native
required-certificate support, actual secure entrypoint and real inference still
need their separate acceptance. Tests use owned fake Docker programs only.

### Explicit restart cleanup custody

`node::container::OwnedContainerStop::open` reopens only a caller-selected private
run and exact explicit IDs. It accepts new transport-bound intent v2 plus the
existing created-ID record, rechecks retained nofollow/ACL anchors and requires
exact daemon ID/run/node/name/image matches. It cannot start, seal, request,
adopt, prune or mint BootGate authority. One attempt per handle; lost replies,
failed inspect and nonempty/failed absence queries remain errors. Missing v2
transport evidence (including historical v1 intents) refuses recovery rather than
silently trusting a newly selected daemon. Existing retained live-session cleanup
continues to work independently of bootstrap/model material. Both paths share
one fenced inspection/removal algorithm, not separate destructive implementations.

## Blocking provider cleanup

`supervisor::worker::WorkerTeardown` dispatches bounded cleanup operations on a
dedicated thread. The control executor never waits for provider I/O: pending
operations return uncertainty, with at most one outstanding operation per effect.
Each operation must supply its own deadline and exact-resource authorization.
Dropping the dispatch handle does not join the worker or cancel queued cleanup.
This worker does not survive process death and is not the independent owner
watchdog required before a rental. Only explicit `ConfirmAbsent` evidence can
produce a confirmed-absence outcome.

## Explicit Vast node entrypoint

Build the executable with `cargo build -p m8s-provider --features vast-node --bin m8s-agent`.
The provider crate assembles the executable; this crate remains a provider-independent
library. For the local exerciser, use `-p m8s-provider --features agent-runtime`. This adds:

```text
m8s-agent --vast-node CONFIG NONCE_FILE NODE_KEY_FILE
```

This is a node-side entrypoint, not an owner-side LAUNCH command or Ready claim.
The launching owner must first journal the resource, reserve the full cost
liability and arm independently surviving cleanup. The node reads only Vast's
`CONTAINER_ID` and per-instance `CONTAINER_API_KEY`; it never loads `VAST_API_KEY`
or an owner signing key. Provider and native errors remain opaque.

All three inputs must be explicitly selected canonical absolute regular files,
mode 0600, owned by the effective user, with one link and no extended ACL. Their
parent directories must be private mode 0700 with no extended ACL. The nonce is
exactly 32 raw bytes; the node key uses the canonical 64-character lowercase hex
format (one trailing newline is accepted). The key must be the same fresh node
key associated with the owner's nonce. Secret bytes are not accepted in argv or
JSON. Unknown JSON properties are refused.

The bounded public config schema is `m8s.vast-node/v1`:

| Field | Meaning |
|---|---|
| `instance_id`, `container_id` | Positive Vast instance ID and its identical decimal string; must match `CONTAINER_ID` |
| `image_sha256` | Owner-admitted image digest, 64 lowercase hex characters |
| `owner_endpoint`, `owner_signing_key`, `mesh_owner_id` | Pinned public iroh identity, public MP001 signing key and canonical owner ID |
| `owner_address`, `bind_address` | Explicit direct UDP addresses with nonzero ports; owner address cannot be unspecified |
| `trust` | Current canonical public trust/revocation snapshot |
| `program`, `gguf`, `material_root` | Absolute native binary, optional local GGUF, and new private material directory; its private parent must already exist. Flint uses `gguf: null` and a pinned native model reference; `model_pin` (bytes + SHA-256) is then mandatory, the child's `HF_HUB_CACHE` is `<material_root>/hf-cache`, and a size/digest mismatch stops the child, drives teardown and deletes that cache. |
| `model` | Exact GGUF basename for local fixtures, or the admitted immutable Hugging Face reference when `gguf` is null. MeshLLM downloads the latter after bootstrap. |
| `heartbeat_ttl_seconds`, `max_lifetime_seconds` | Positive bounded lease; TTL cannot exceed lifetime, lifetime cannot exceed 1800 seconds |
| `port`, `console_port` | Distinct nonzero native listener ports |

Before redemption, an authenticated exact-instance GET must observe the supplied
instance as present. This establishes credential presence, not cryptographic proof
of DELETE permission. The control endpoint uses direct authenticated iroh, keeping
netmon and TLS enabled; it does not register a competing relay endpoint under the
native node's identity. The original transport receipt, exact canonical signature
verification and native node-ID check precede model-process startup.

Cleanup runs through the bounded worker. The supervisor handles child drain;
material wipe remains unconfirmed until provider destruction removes the ephemeral
filesystem. A dead per-instance key cannot certify owner-visible absence. The
external owner must independently confirm removal with its own credential and
reconcile the journal. The node never emits Ready or a successful inference claim.

Live endpoint, inference and destructive-provider qualification remain pending.
