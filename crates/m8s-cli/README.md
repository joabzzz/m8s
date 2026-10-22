# m8s CLI

Mock-control commands retain their existing `--socket PATH` grammar. Secret
commands are local-only and do not contact the daemon or any provider:

```text
m8s secrets --file ABSOLUTE_PATH set NAME [--stdin] [--expires-at UNIX_SECONDS] [--json]
m8s secrets --file ABSOLUTE_PATH list [--json]
m8s secrets --file ABSOLUTE_PATH rm NAME [--json]
m8s secrets --file ABSOLUTE_PATH check [--json]
m8s secrets --help
```

`--file` and `--json` may appear anywhere after `secrets`, once each. File
selection is always explicit. **Without `--file`, the command fails closed:**
Keychain remains the intended macOS default, but independent CLI-process
serialization and real OS qualification are not implemented. This CLI never
constructs a Keychain driver or silently substitutes a file/env backend.

The operator must provision an owned, exactly 0700 parent directory outside
all Git worktrees and supply a canonical absolute path (`/private/tmp`, not the
`/tmp` symlink, on macOS). No directory creation, tilde expansion, permission
repair or default-path/env discovery occurs. Data and sibling lock require
owner 0600, no extended ACLs, symlinks/hardlinks or special files. Reads/updates
retain descriptor-relative no-follow validation, locking and atomic fsync.
See [m8s-secrets](../m8s-secrets/README.md) for filesystem constraints, the strict
bounded TOML schema, crash behavior and advisory-lock limitations. Hex values
on disk are **encoding, not encryption**; protect disks/backups accordingly.

## Input and output

- `set NAME` refuses redirected/non-TTY stdin; local terminal entry uses
  `/dev/tty` with no echo. Each fixed prompt is emitted only after no-echo is
  active and the restoration guard is armed. Enter the same value twice. A mismatch, cancellation,
  empty value, oversized input or read failure prevents storing.
- Terminal Ctrl-C/Ctrl-D/Ctrl-Z cancels. Backspace edits without echo. Attributes
  are restored on ordinary success/failure/cancellation; abrupt kill/host failure
  can require `stty sane`. Never type a secret in argv, chat or Agor.
- `--stdin` is explicit opt-in and reads **exact bytes through EOF**, including
  a final newline, once (no confirmation). Arbitrary bytes, including NUL and
  non-UTF8, are preserved. Both input paths limit values to 64 KiB.
- `--expires-at` is signed Unix seconds and must be in the future at set time.
  Omission means no expiry. At exact expiry get/list/check hide the entry;
  `rm` still removes it. There is no automatic physical-erasure guarantee.
- No `get`, `show`, `--value` or positional value is accepted. Rejected args,
  paths, parser diagnostics and values are never included in errors.
- `list` returns active names/expiry only. `rm` reports whether a record existed.
  `check` verifies **local storage only** and explicitly reports owner-keystore,
  provider-permission, signing and OS Keychain checks as not performed.
- `--json` writes status/metadata JSON to stdout, including a fixed error object
  on failure. Prompts and generic errors go to stderr; values go nowhere.
  Names are validated metadata, not a place to put secret material.

Tests use dedicated synthetic temp files and injected keyring/input mocks. The
library's dedicated synthetic PTY checks verify no echo and restoration. An
explicit ignored CLI PTY qualification test covers real terminal confirmation,
mismatch and cancellation on hosts permitting `/dev/tty` access:
`cargo test -p m8s-cli actual_terminal -- --ignored`. This sandbox denies opening
`/dev/tty` even for a synthetic controlling PTY; that qualification is blocked,
not a passing terminal E2E. The CLI fails closed instead of changing input paths. They never access real
Keychain items, ambient credential env, the mesh owner keystore or a provider.
Fake/mock tests do not establish OS Keychain qualification. Default setup,
lifecycle expiry sweeps and real owner/provider/signing checks remain deferred.

## Read-only dashboard in-process

```sh
m8s --socket /private/path/control.sock top [--once] [--json] [--timeout-ms 1..10000]
m8s top --socket /private/path/control.sock [--once] [--json] [--timeout-ms 1..10000]
```

Both forms call the same explicit `m8s_top::app` parser/runner as standalone
`m8s-top`, in this process; no executable lookup, shell or child dispatch.
Only Status is sent, never a provider/resource command. Socket private-path,
peer-UID, one absolute I/O deadline, redacted errors and terminal RAII behavior
are identical to [m8s-top](../m8s-top/README.md). Parsing/imports have no socket,
terminal or signal effects; interactive execution is explicit opt-in.

Non-TTY invocation automatically takes one text snapshot. `--once` and `--json`
never initialize the terminal; both can be combined (JSON wins). Each is allowed
once; repeated `--timeout-ms` uses the last valid 1..10000 value. Top JSON is the
dashboard projection, not the existing status command's raw API envelope.
Missing evidence remains unknown/null; reported Ready is unverified and stale
status is never proof. No secure admission, billing cessation or readiness claim.

Existing control/secrets grammar and JSON are unchanged. As before, `m8s`
rejects any invalid UTF-8 argument (including a socket path) with an opaque
error; standalone `m8s-top` still accepts OS-byte paths. Dedicated synthetic
sockets assert Status-only top payloads, safe error output and legacy command
compatibility. Synthetic PTYs check CLI alias q/SIGINT/SIGTERM cleanup using the
standalone fixture, with no access to the user's controlling terminal.

## Effect-free owner-intent plan

```text
m8s plan --local-development --config ABSOLUTE_CONFIG --catalog ABSOLUTE_CATALOG --tier ID [--json]
m8s plan --help
```

All four input flags are required exactly once, in any order. `--json` is
optional once. Unknown flags/positionals, duplicates, socket, execution and
approval overrides fail before files are opened. No discovery or interpolation.
Use the shipped `m8s-config/examples/m8s.toml` and `m8s-models/models/examples.toml`
with explicit absolute paths, or your own documents using those existing schemas.

Exit 0 means **intent validated, not authorized or executable** under the fixed
`ValidationLimits::default()` conservative envelope, reported separately from
configured budget ceilings. This is not Admin approval, a cost quote, balance,
reservation, executable local plan or final configuration/public-contract approval.
Every accepted tier requires GPUs and Vast preferences; the CPU/unmeasured local
adapter cannot satisfy these. `local_execution_supported=false`,
`inference_served=false`, readiness `not_assessed`, authorization `not_checked`,
billing `unknown` and requirements `not_verified` never become deployment evidence.
DMS durations, cold/fallback, queue, timezone and cron are descriptive only: no
watchdog, scheduler, next occurrence or automatic tier selection is performed.

Unix only: both explicit inputs must be regular files, at most 65536 bytes,
opened with final-component no-follow/nonblocking checks and bounded cap+1 reads.
Ancestor resolution is trusted; this is not protection against hostile parents,
concurrent regular-file edits or OS/filesystem metadata stalls. Other platforms
fail closed. No directory creation, repair, file writes, environment/credential
lookup, Keychain, daemon socket, child, provider, download/cache or Store access.
Only normalized validated intent is rendered; catalog source/license/runtime free
text, input paths, argv and raw parser diagnostics are never echoed. Text and
`m8s.local-intent-plan/v1` JSON use the same private typed view, fully rendered
and bounded to 64KiB before stdout. JSON failures use fixed opaque categories;
output I/O failure is nonzero (a broken stream may still receive partial bytes).
No fingerprint, UID, Offer, claim, launch journal or Ready status is minted.

## Synthetic local lifecycle demo (Unix, opt-in)

Build/run in the workspace, or install with the same default-off feature:

```sh
cargo install --path crates/m8s-cli --features local-development
# Provision an owned 0700 parent, outside Git; use its canonical absolute path.
mkdir -m 700 /private/tmp/my-m8s-demo
m8s local-demo --synthetic-accounting --state-dir /private/tmp/my-m8s-demo/cancel-new --synthetic-cap-micro-usd 10000 --scenario cancel
cargo run -p m8s-cli --features local-development -- local-demo --synthetic-accounting --state-dir /private/tmp/my-m8s-demo/deadline-new --synthetic-cap-micro-usd 10000 --scenario deadline
```

Each state directory must be **new**, under an existing canonical parent (on
macOS use `/private/tmp`, not the `/tmp` symlink). No reset, repair or rerun:
choose a fresh directory per invocation. All four flags are required once;
cap is an integer 1..1000000 micro-USD of **synthetic** accounting, not spend
authorization. Too-small caps fail before child submission and retain state.

The shared library walkthrough launches only fixed `/bin/sleep 15`, with a
3-second runtime and 2-second cleanup grace. `cancel` pairs durable cancellation
with local stop; `deadline` observes the original deadline then explicitly
cleans up. NDJSON reports `LocalOnlyAbsent`, billing `unknown`, budget
`Uncertain` and the full reserved hold retained; SQLite and journal are kept.
Failure is nonzero, never a successful-run claim. Default builds refuse before
state/child effects; `m8s local-demo --help` explains the opt-in.

This is not production deployment, Admin approval, billing cessation, model
serving or Ready evidence. No provider, credential/Keychain, daemon or bootstrap
path is invoked; existing security and runtime gates remain unchanged.

## Inspect retained synthetic demo state (Unix, opt-in)

Using the same feature-enabled binary and already-provisioned parent above,
choose a new state path, run once, then inspect without rerunning the demo:

```sh
m8s local-demo --synthetic-accounting --state-dir /private/tmp/my-m8s-demo/inspect-new --synthetic-cap-micro-usd 10000 --scenario cancel
m8s local-inspect --state-dir /private/tmp/my-m8s-demo/inspect-new
m8s local-inspect --help
```

`--state-dir CANONICAL_ABSOLUTE_PATH` is required exactly once; no other flags
except standalone `--help`. Help works without the feature and touches no state.
Default-off/non-Unix builds refuse inspection before filesystem access.

One bounded JSON line (`m8s.local-inspect/v1`) reports the retained typed
`Uncertain` budget, full synthetic hold (30 micro-USD), consumed operation and
`cleanup_required`. It is an **immutable historical snapshot, not live policy**:
accounting `synthetic-not-approval`, billing `unknown`, readiness `not-assessed`,
child_state `not-checked`. No journal is opened and no current child absence,
settlement, refund, billing cessation or readiness is inferred.

Only completed, quiescent records with the fixed synthetic demo owner/context
are accepted. Missing/incomplete/tiny-cap/corrupt/foreign state is refused with
an opaque error, never recovered. The directory must be canonical, owned and
exactly 0700; existing `demo.lock` and `state.db` must be owned regular 0600
single-link files. Descriptor no-follow/nonblocking opens and retained inode
anchors are checked before output. The existing lock is held shared,
nonblocking; an active cooperating demo's exclusive lock causes refusal.

Inspection never creates, migrates, repairs, chmods, removes or fsyncs state,
opens Store, starts a child or initializes the local provider. SQLite uses an
explicit percent-encoded `mode=ro&immutable=1` URI with READONLY/URI/NOFOLLOW,
query-only and untrusted-schema guards. Any existing WAL/SHM/rollback-journal
path, including a dangling symlink, is refused before and after reading. The
DB is bounded to 16 MiB; each reservation/operation record to 64 KiB with exactly
one row required. No untrusted strings, paths or raw database errors are emitted.

Inspection validates public cross-record consistency, not provenance, signatures
or authorization permissions. Recorded node IDs are bounded metadata shared
across records; stored fingerprints are neither recomputed nor authenticated,
and are not emitted. Fabricated consistent same-UID data is not an authenticated
source. Filesystem ownership/mode checks protect the read boundary, not authority
to execute or spend.

Advisory locks coordinate cooperating demos; this is not cross-process security
against an uncooperative same-UID writer or a filesystem/kernel latency bound.
It does not qualify denied network monitoring, Keychain or production security
paths; those independent gates remain unchanged.

## Browse model catalog metadata (default-available)

`m8s --help` lists the command groups and their gates. Catalog browsing needs no
feature flag, daemon, terminal, provider or credential. Supply your own explicit
absolute catalog path (replace `/absolute/workspace/m8s` below with your checkout):

```sh
m8s models --catalog /absolute/workspace/m8s/crates/m8s-models/models/examples.toml list
m8s models --catalog /absolute/workspace/m8s/crates/m8s-models/models/examples.toml show example-27b-q4km --json
m8s models --help
# Use a catalog ID in your configuration, validate all intent, then inspect a tier:
m8s config validate --config /absolute/workspace/m8s/crates/m8s-config/examples/m8s.toml --catalog /absolute/workspace/m8s/crates/m8s-models/models/examples.toml --json
m8s plan --local-development --config /absolute/workspace/m8s/crates/m8s-config/examples/m8s.toml --catalog /absolute/workspace/m8s/crates/m8s-models/models/examples.toml --tier small --json
```

Grammar: `models --catalog ABS list [--json]` or
`models --catalog ABS show ID [--json]`. The two flags may appear anywhere after
`models`, once each; command and optional ID remain in that positional order.
`--catalog` is mandatory, with no path/env discovery. Standalone `--help` is
filesystem-effect-free; combining it with other arguments fails. IDs use the
catalog's bounded lowercase ASCII identifier grammar. Unknown IDs, duplicates,
unknown flags, extra/missing values and non-UTF8 arguments fail opaquely.

List is sorted by ID, regardless of input order. Show returns one row in the
same projection. Text has a fixed explanatory header followed by formatted
JSON; `--json` emits only the `m8s.model-catalog/v1` projection. Both report exact
integer `params_billions`, `weights_bytes`, `runtime_overhead_bytes_per_replica`,
`kv_bytes_per_1024_tokens` and `max_context_tokens`, plus
`declared_license_class` and `declared_runtime_status`. Source URI, free-form
license and quantization text are deliberately omitted, including private or
terminal-control content they might contain.

The **entire catalog** is validated using the configuration validator's shared
metadata rules before list or lookup, including profiles unused by the selected
ID. Validation is not source verification, license permission, runtime
qualification or approval. Even a catalog-declared `certified` status remains
only a declaration: `catalog_metadata_only=true`, runtime verification
`not-performed`, deployment and approval `not-assessed`. There is no automatic
selection, fit recommendation, pricing, download, model execution or Ready claim.

Unix: the existing plan/catalog loader opens only a regular file with
final-component no-follow/nonblocking checks, at most 65536 bytes, and reads
at most cap+1 bytes before UTF-8/TOML parsing. Ancestors are trusted; concurrent
regular-file editing and filesystem/kernel latency are not bounded security
claims. Non-Unix catalog loading fails closed; help still works. No writes,
repair, Store, credential, source fetch or subprocess dispatch occurs.

Output is fully rendered and bounded to 256 KiB before stdout. JSON failures
use fixed opaque categories (`invalid-models-arguments`, `invalid-models-catalog`,
`unknown-model-id`, `models-output-unavailable`); text failures use the same
categories on stderr. Existing whole-argv non-UTF8 refusal remains the generic
`invalid arguments`, with no JSON output. Paths, argv, raw TOML/parser errors and
catalog free text are never echoed. Broken output is nonzero; a failed stream
can still receive partial bytes. Model metadata does not change plan's existing
intent-only, non-executable contract or any runtime/security gates.

## Validate the whole configuration (default-available)

```text
m8s config validate --config ABSOLUTE_CONFIG --catalog ABSOLUTE_CATALOG [--json]
m8s config --help
m8s config validate --help
```

No selected tier or feature flag is required. The command reuses the same bounded
catalog loader and actual configuration validation path as `plan`, with trusted
`ValidationLimits::default()`. Every tier, fallback/reference and catalog profile
is validated, including ones you do not plan to use. No file/default/env discovery.
`validate` is required once; `--config`, `--catalog` and optional `--json` may appear
anywhere after `config`, once each. Both paths must be explicit and absolute.
Only the two standalone help forms above bypass parsing/filesystem access.
Unknown/duplicate/missing arguments, `--tier`, execution/approval and socket flags
are rejected, never reinterpreted as another workflow.

`m8s.config-validation/v1` reports sorted tier IDs/model references, tier count,
whole-catalog model count and distinct referenced-model count. Configured budget
ceilings are descriptions, separately labeled from the default conservative
validation envelope actually used; neither is approval, available balance,
reservation or spend evidence. Exit 0 means **whole configuration intent validated,
not authorized or executable**. Local CPU/unmeasured still cannot satisfy the
configured GPU/provider requirements. Authorization remains `not_checked`, billing
`unknown`, readiness `not_assessed`, requirements `not_verified`, local execution
and inference serving false. No current runtime, fit, cost or Ready claim.

Text has a fixed intent-only header followed by the same JSON projection as
`--json`. No raw configuration, owner/schedule text, catalog source/license/quant,
paths or parser/error chains are returned. Both inputs retain existing Unix
regular-file, final-component no-follow/nonblocking and 65536-byte/cap+1 read
limits; ancestors and concurrent regular-file edits remain caller-trusted, not a
hostile-filesystem snapshot guarantee. Non-Unix loading fails closed; help works.
No writes, import, state/Store, provider/child/socket/terminal, credentials or model
download occurs.

Output is fully rendered and bounded to 65536 bytes before stdout. JSON failures
use fixed `invalid-config-arguments`, `invalid-config-catalog`,
`invalid-config-document` or `config-output-unavailable` categories; text failures
use those categories on stderr. Existing whole-argv non-UTF8 refusal remains
`invalid arguments` without JSON. Broken output is nonzero and may leave partial
bytes. This command does not change plan's selected-tier diagnostics, budget
validation rules, deployment gates or any existing runtime/security behavior.

## Offline model capacity calculator (default-available)

Supply hypothetical capacity explicitly; this does not discover local hardware
or search/evaluate a live provider offer. For example, using the shipped catalog:

```sh
m8s models fit --catalog /absolute/workspace/m8s/crates/m8s-models/models/examples.toml --model example-27b-q4km --gpu-count 2 --vram-bytes-per-gpu 80000000000 --layout a --context-tokens 1024 --minimum-sequences 1 --reserved-bytes-per-gpu 0 --json
m8s models fit --help
```

`fit` must immediately follow `models`. All eight value flags shown are required
exactly once, in any order after `fit`; `--json` is optional once. No default model,
context, layout, reservation or capacity is assumed. Catalog must be an explicit
absolute path, at most 4096 bytes, using the existing bounded read-only loader.
Model ID follows the catalog's bounded lowercase ASCII grammar, and the entire
catalog is validated before lookup, including unused profiles. Existing list/show
grammar and loader behavior are unchanged; standalone fit help opens no files.

Numbers are ASCII decimal integers only, at most 20 digits: no signs, whitespace,
fraction/exponent syntax or overflow. GPU count is positive u32; VRAM bytes,
context tokens and minimum sequences are positive u64; reserved bytes is u64 and
**must be supplied even when zero**. Minimum sequences means per replica. All
capacities/results are exact integer **bytes**, not ambiguous GB. Context above
the declared model maximum is invalid; reservation exhausting VRAM or insufficient
capacity is a no-fit refusal. Arithmetic overflow refuses, never wraps/clamps.

The actual model-library planner computes these alternative layouts:
- `a`: as many independent replicas per GPU as fit the requested minimum; remaining
  capacity is divided equally among replicas on each GPU.
- `b`: one independent replica per GPU, without sharing weights between GPUs.
- `c`: one replica balanced-sharded across all GPUs on **one host**, never across
  hosts. Balanced memory division and actual runtime sharding support remain
  assumptions to verify, not capabilities established by the calculator.

Text has a fixed offline-only header followed by the same formatted JSON
projection as `--json`. Schema `m8s.model-fit/v1` reports the selected model ID,
all explicit sizing inputs/layout, the typed FitPlan's replica/GPU counts,
weights-plus-overhead bytes, KV bytes per sequence, KV headroom and concurrent
sequence counts, plus a fixed layout assumption. Sequence counts are arithmetic
capacity estimates, **not throughput or measured runtime performance**.

Output explicitly says `capacity-estimate-only`, `model-unmeasured`,
`runtime-not-verified`, `provider-policy-not-assessed`, `not-authorized` and
`not-executable`. A catalog-declared certified status cannot change those labels.
The internal capacity adapter is deliberately synthetic with unknown country and
region and no price/network/reliability evidence; it is never a live offer or
policy-admissible selection. No provider admission, automatic model selection,
fit recommendation, cost/download estimate, Ready/current-runtime claim, execution,
credential discovery, socket, child, Store or source fetch occurs. A successful
estimate neither authorizes deployment nor changes config/plan/runtime gates.

Both formats render fully and check a 16 KiB bound before stdout. Source URI,
license/quant text, raw argv/paths/parser diagnostics and the internal synthetic
offer are omitted. JSON/text failures use fixed categories:
`invalid-model-fit-arguments`, `invalid-model-fit-catalog`, `unknown-model-fit-id`,
`model-does-not-fit`, `model-fit-overflow`, `model-fit-output-unavailable`.
Whole-argv non-UTF8 still fails generically without JSON. Broken output is nonzero
and may leave partial bytes. Loader ancestor/concurrent-edit/platform boundaries
are identical to catalog browsing; no security/default expansion is implied.

## Apply tier intent to the opt-in MOCK daemon

After `config validate`, the daemon can enforce configured membership/replica
bounds without deploying models or selecting a real provider:

```sh
m8sd /private/tmp/my-private-mock-state synthetic-mock-owner --config /absolute/workspace/m8s/crates/m8s-config/examples/m8s.toml --catalog /absolute/workspace/m8s/crates/m8s-models/models/examples.toml
# Separate terminal:
m8s --socket /private/tmp/my-private-mock-state/control.sock up small
m8s --socket /private/tmp/my-private-mock-state/control.sock scale small 2
m8s --socket /private/tmp/my-private-mock-state/control.sock status --json
m8s --socket /private/tmp/my-private-mock-state/control.sock down
```

Use your own private state path. Up selects max(1,min); positive scale must be
within configured bounds; known-tier scale0/down remain explicit stops even when
min>0. No startup scale is inferred. Desired state survives restart; an incompatible
new configuration refuses without clamping/resetting state or replacing the old
socket/opening Mock. Configuration is a startup snapshot, not live reload. See
[daemon boundaries](../m8sd/README.md#opt-in-configuration-backed-mock-control),
including disabled tiers, Store-compatible IDs and existing initialization rules.
The provider remains hardwired Mock, mode mock-only, bootstrap unavailable: no
Ready/model execution, scheduling, approval, live budget or security proof.

### Stop the Mock daemon

```sh
m8s --socket /absolute/private/state/control.sock shutdown --json
```

Unlike `down` (which keeps the daemon running), `shutdown` commits desired zero,
then exits normally only after bounded ordinary reconciliation confirms exact-owner
Mock inventory and retained node rows empty. A refusal keeps the daemon running; committed stop intent remains durable
(a failed database write cannot establish it). A lost response does not undo that intent; restart with
compatible configuration preserves zero. The socket path may remain. This is
hardwired Mock lifecycle control, not real-provider absence, settlement, Ready or
authorization. Existing socket trust/permission assumptions are unchanged.
