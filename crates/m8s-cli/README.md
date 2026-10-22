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
socket/opening Mock. Configuration stays frozen until explicit validated reload (never automatic). See
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

### Page retained local history

```sh
m8s logs --help
m8s --socket /absolute/private/state/control.sock logs --after 0 --limit 50 --json
# Use next_cursor from a successful response for the next explicit request.
```

Paging flags are both mandatory, once each, in either order **after logs**;
`--json` may appear once among them. Cursor is ASCII decimal 0..i64::MAX, limit
1..200. Legacy `logs [--json]` still returns its existing tail. Pages use a
separate typed response with ascending `seq`, recorded/injected `at`, `message`,
`next_cursor` and `has_more` (one SQLite limit+1 query snapshot). Text and JSON use
the same escaped ASCII JSON projection: event text is never raw terminal control
output. Invalid/oversized pages refuse without truncating/advancing a cursor.
Frames, including newline, are capped at 65536 bytes; rendering completes before
stdout, but broken stdout can partially write and returns nonzero.

This is one-shot read-only paging, not follow/poll, inference or secret transport.
This is not immutable filesystem inspection: ordinary SQLite WAL reader marks
may change, and the daemon timer can reconcile independently. Cursors apply only to this
state's retained history, not authenticated cross-DB/global continuity, current
clock, real-time delivery or proof of no lost events. `ok=false` empty records are
**not** evidence of no history; an empty successful page only describes that
query snapshot. Model/runtime/readiness/authorization are not assessed.

### Reload an explicitly configured Mock daemon

```sh
m8s --socket /absolute/private/state/control.sock reload-config --json
```

Only paired-config startup supports this path-free request. It rereads the exact
startup-selected config/catalog files, validates both completely and checks all
retained tiers/current positive desired counts against new bounds before one
policy assignment. File edits alone do nothing. Invalid/incompatible reload
refuses with old policy active and desired/history unchanged: no clamp/reset/down.
Explicit down/known-tier scale0 can make new bounds compatible, but a retained
zero tier still cannot be removed. Success changes subsequent up/scale admission;
compatible restart reads the files normally. Legacy no-config mode refuses, with
no file discovery. The independent daemon timer still reconciles normally.
This is hardwired Mock control only, not model activation, provider selection,
scheduling, Ready, budget/security qualification or authorization.

### Show active Mock admission bounds

```sh
m8s --socket /absolute/private/state/control.sock config-status --json
```

Text and JSON use the same safe deterministic projection: configured flag and
sorted tier ID/min/max, fixed mock-only/admission-only labels, ok/error. This
queries only the daemon's actual memory, never files or Store: file edits alone
leave it unchanged, successful reload updates it, refused reload preserves it.
Use `config validate` for file contents, `status` for desired state. Legacy
configured=false/empty tiers does not mean desired empty or verified default cap;
failed empty responses do not establish no policy. No paths/models/owners/free
config text/fingerprints/credentials or global version appear. No provenance,
authorization/readiness/live-policy guarantee; the snapshot may change after the
query or restart. Output is bounded before stdout; broken writes return nonzero
and may be partial. No file discovery, provider/model activation or inference.

`m8s top --socket PATH --show-config --once` (or `--json`) optionally adds active
Mock bounds beside reported desired. Default dashboard and JSON remain unchanged;
standalone `m8s-top` supports the same flag. Configured, legacy UNCONFIGURED and
config UNKNOWN are distinct, not fleet-empty/approval/Ready claims. Text bounds
are viewport-limited; opt-in JSON includes all 32 validated tiers. These are two
**non-atomic** observations sharing one total timeout and each full socket trust
check. Failed config read preserves status with config UNKNOWN, not file fallback;
failed whole refresh marks retained config UNKNOWN with the existing stale banner.
No files/reload/mutation or TTY initialization for redirected one-shot/JSON use.

## Offline queue replay

```sh
m8s queue replay --config /absolute/m8s.toml --catalog /absolute/catalog.toml \
  --tier small --samples /absolute/samples.json \
  --freshness-seconds 2 --max-gap-seconds 2 --json
```

All six flags are required once; flag order after `replay` is arbitrary. Seconds
are positive ASCII decimal integers <= i64MAX. `queue replay --help` needs no files.
The entire config/catalog is validated with existing default validation limits;
selected-tier queue thresholds, durations and replica bounds feed the actual core
queue controller. Missing queue rules or bounds incompatible with its max16 cap
refuse; freshness/gap are explicit engineering inputs, not approved live policy.

Samples use a strict positional JSON schema (no object/free-text fields):

```json
["m8s.queue-samples/v1", [
  [0, 0, "enabled", [0, 2001, 0]],
  [2, 0, "paused", null]
]]
```

Each record is `[now, desired, intent, sample]`; sample is `null` or
`[observed_at, queue_wait_ms, inflight]`. Intent is exactly `enabled` or `paused`.
Times are nonnegative integer Unix seconds <= i64MAX; metrics are unsigned u64,
desired unsigned u32. At most 256 records (at least one), 65536 input bytes;
explicit regular files only, no final symlink/special-file following on Unix.
Non-Unix loaders refuse. Unknown/stale samples reset windows, not idle evidence;
future/replayed timestamps, non-increasing now or invalid desired refuse the
whole replay. Gap/freshness equality is accepted; strict pressure thresholds,
sample-time durations and all proposal decisions are the core controller's rules.

Output is deterministic compact JSON or the same projection as pretty JSON,
bounded to 256KiB and rendered completely before stdout. Includes only safe policy
fields and indexed proposals/reasons, never raw samples/paths/catalog metadata.
Any validation error refuses the entire command without partial results. Broken
stdout returns nonzero; partial output on an output-device failure is possible.

This is **authored counterfactual simulation with no feedback**: each record's
desired is supplied by the author, NOT changed by prior proposals. There is no
clock, live metrics reader, daemon/socket/Store access, provider or effect.
Enabled is not authorization; metrics authenticity, live currentness and readiness
are not assessed. A future coordinator must enforce durable stop, current desired,
authorization, clock, claims, rates and budgets atomically before effects.

### Check a prospective Mock reload without applying

```sh
m8s --socket /absolute/state/control.sock reload-config --check --json
m8s reload-config --help
```

The configured daemon validates its startup-selected files and retained desired
using the same checks as actual reload; no new paths/overrides/force are accepted.
Success returns sorted actual active/prospective candidate tier bounds, NOT the
config/catalog contents. `bounds_changed` compares ONLY membership/min/max,
not whole-config equality. `check_only=true`, `applied=false` and
`later_reload_guaranteed=false`: a later reload can fail or observe changed files
or desired. Invalid inputs/incompatibility and legacy unconfigured daemons refuse
opaquely, keeping the current policy. `--check` and optional `--json` appear once
in either order after `reload-config`; standalone help performs no file/socket IO.
Text and JSON use the same bounded, fully rendered safe projection. Broken output
is nonzero (partial bytes possible on device failure). No authorization/readiness
or immutable-filesystem claim: the daemon reads selected files/SQLite state,
SQLite bookkeeping and independent timer reconciliation are not globally frozen.

### Explicit log follow (read-only polling)

```sh
m8s --socket /absolute/state/control.sock logs --follow --after 0 --limit 100 \
  --poll-ms 250 --max-pages 10 --json
m8s logs --follow --help
```

`--follow`, `--after` and `--limit` are mandatory once; optional flags also once,
in any order after `logs`. Cursor is ASCII decimal 0..i64MAX, limit1..200.
`--poll-ms` is 50..60000 (default250ms), solely a user-opted read-only UI polling
interval, NOT an automated scaling policy. `--max-pages` is 1..10000 and counts
EVERY successful emitted page, including empty pages; omitted means follow until
externally stopped. No terminal/signal handling is installed. At max-pages, stop
without another request or sleep. Each fetch has its own existing absolute3s
API deadline, NOT a whole-session deadline or follow-duration cap.

Each successful fetch emits one complete ASCII-escaped NDJSON page in BOTH text
and `--json` modes, including empty checkpoints with stable cursor. The shared
one-shot encoder caps each newline-inclusive page at65536 bytes and escapes
controls/nonASCII/bidi, not raw terminal messages. Cursor advances only after
complete output AND flush. `has_more=true` drains immediately; at end/empty wait
poll interval. First transport/refusal/malformed/output failure stops nonzero with
fixed stderr; no retry/fallback or further fetch. Earlier emitted pages remain;
partial output-device writes are possible on failure.

Local-history cursors are not authentication, currentness, cross-database history
generations, globally exactly-once delivery or no-event-loss proof. Restart/reset
is not magically detected. Follow performs only existing read-only log-page calls;
legacy tail and one-shot paging are unchanged. Independent daemon reconciliation
continues; no provider/inference/approval/readiness claim.

### Offline owner-calendar event preview

```sh
m8s schedule preview --config /absolute/m8s.toml --catalog /absolute/catalog.toml \
  --tier small --from 1709553600 --hours 24 --limit 256 --json
m8s schedule preview --help
```

All six flags mandatory once, any order after `preview`; optional `--json` once.
`--from` is explicit nonnegative ASCII integer UTC Unix seconds <=i64MAX;
`--hours` 1..168, `--limit` 1..256. No real clock, timezone discovery or live cadence.
Whole config/catalog uses actual default-limit validation and existing bounded
loaders. Selected tier requires owner IANA timezone and at least one schedule.

Preview scans UTC-minute instants in **(from, from+hours*3600]** (exclusive from,
inclusive checked horizon), converts via chrono-tz, and returns only matching
open/close rule events. Maximum10080 UTC minutes x32 open/close expressions;
checked UTC/local representability errors refuse the whole preview. Lists union;
other calendar fields intersect. Sunday=0; wildcard steps anchor to field minimum,
range steps to range minimum. To avoid inventing DOM/DOW semantics, each expression
must have day-of-month OR day-of-week **literal `*`** (or both). Otherwise the entire
preview refuses, even if `*/1` is equivalent; a valid config may be unpreviewable.

Events sort by UTC instant, zero-based rule index, then open before close;
overlaps/simultaneous events remain separate. Spring nonexistent local minutes
produce no matches, repeated autumn minutes produce two distinct UTC events;
this does NOT prescribe double firing by a live scheduler. Local timestamps include
numeric offsets (historical offsets may include seconds); the grid is UTC minutes,
not a claim that all historical local offsets are whole minutes. `has_more` and
`truncated` explicitly indicate limit truncation; no unbounded search.

Compact JSON and pretty-JSON text have the same safe projection, fully rendered
and bounded to256KiB before stdout. No raw config/schedule/path/model/source/error
strings. Broken stdout is nonzero (partial device bytes possible). These are NOT
current open/closed state, paired intervals, conflict resolution, desired replica
targets, execution, authorization or live firing policy. No daemon/Store/provider,
realclock, filesystem writes or schedule activation occurs.

### Create a fresh Mock starter workspace (Unix)

```sh
m8s init --mock --directory /absolute/existing/private-parent/new-workspace --json
```

`--mock` and an explicit absolute `--directory` are mandatory once; `--json` optional
once, any flag order. `m8s init --help` performs no filesystem work. No recursive
parent creation, default discovery, overwrite/adoption, force/reset, chmod,
rollback/deletion or automatic retry. ANY existing final entry refuses, even an
empty directory. Parent must exist, be canonical/current-user-owned and not
group/other writable; ancestors (max128) must be root/current-user owned, non-writable
except root-owned sticky temporary ancestors. No dot/dotdot/symlink aliases.
Descriptor/inode checks detect replacement but are NOT hostile-same-UID confinement.

Creates0700 final directory,0600 `m8s.toml`, `catalog.toml`, static `README.md`, and
EMPTY0700 `state/`. Config/catalog bytes are exact shipped public examples, validated
with actual default limits before writes. Generated README gives manual launch
and control commands from workspace cwd using quoted `"$PWD"` paths, never
interpolated user paths. No Store/journal/schema/lock/daemon/credential/model/provider
creation. Mock-only simulation, no inference/Ready/approval/GPU qualification;
schedule/queue previews are not automation; local-demo remains separate/default OFF.

Text/JSON print the same fixed bounded relative inventory, never the selected path.
Creation is not transactional: errors may leave owned partial artifacts. Broken
stdout can occur AFTER complete creation; nonzero does not mean no files/effects.
Inspect deliberately rather than blindly retrying, adopting or deleting state.

### Atomic manual multi-tier desired update

```sh
m8s --socket /absolute/state/control.sock scale-batch small=2 large=0 --json
m8s scale-batch --help
```

1..32 UNIQUE ASCII alnum/underscore/hyphen IDs<=64, explicit decimal counts0..16.
`--json` once anywhere after the command; no implicit targets/defaults/--all.
Unmentioned tiers unchanged. Configured Mock checks ALL targets against active
membership/min/max before a single Store transaction; zero is an explicit stop
for a known tier even when min>0. Positive batch after down is a MANUAL request,
not automated resume. Legacy Mock retains Store bounds.

Atomicity covers selected desired upserts and existing explicit-zero nonce
cancellations in ONE IMMEDIATE transaction, NOT provider effects, allocation,
destruction, convergence or Ready. Asynchronous reconciliation still follows.
Reply includes only checked Mock mode, sorted requested targets and bounded safe
desired snapshot (<=256 tiers), with requested counts matched before printing.
Text/JSON share one bounded fully rendered projection; node/log/error payloads
are omitted. A refused/invalid/lost reply, timeout, snapshot or output failure MAY
follow a committed mutation. Nonzero NEVER proves rollback/no effects; do not
blindly retry. No provider approval/authentication or execution authority implied.

Init also rejects extended ACL metadata on every retained ancestor, root, child
file and state-directory descriptor, before/after identity checks; Linux additionally
checks default/inheritable ACL presence. Any ACL (including empty/deny-only),
unexpected metadata failure or unsupported checker refuses conservatively. No
ACL interpretation/repair/removal is attempted. Parent refusal precedes mkdir;
new-file ACL refusal precedes writing example bytes (an empty partial file may
remain). This ACL-only borrowed-FD check is not an ownership/mode/identity,
authentication or complete private-access certificate; existing checks and
same-UID/nontransactional limitations still apply.
