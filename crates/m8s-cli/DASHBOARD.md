# m8s-top — read-only local dashboard

ADR-0015 terminal dashboard slice, shared by standalone `m8s-top` and the
in-process `m8s top` command. Only the existing newline-JSON `Request::Status {}` is sent to the
explicit Unix socket. There are no launch, scale, destroy, credential or provider
operations in this crate.

```sh
m8s --socket /path/to/control.sock top
m8s top --socket /path/to/control.sock --once
m8s-top --socket /path/to/control.sock
m8s-top --socket /path/to/control.sock --once
m8s-top --socket /path/to/control.sock --json --timeout-ms 1000
```

Interactive mode requires **both** stdin and stdout to be TTYs. Otherwise it
performs one text snapshot automatically, without raw mode, cursor changes or
an alternate screen. `--once` and `--json` always avoid terminal initialization.
The executable supports q, Esc, Ctrl-C to exit; r refreshes; j/k or arrows scroll.
Below 100x14 it shows a compact unqualified-status/resize notice, not a clipped
node table whose readiness warnings could disappear. Terminal-reported dimensions
are capped at 300x120 before buffer allocation, including resize events; pure
text snapshot dimensions are bounded too.
It refreshes once a second; errors retain the last snapshot with a conspicuous
**STALE / UNKNOWN** banner. No usable snapshot means unknown fleet, not empty.

## Honest observations

The API v1 available to this slice exposes node UID, tier/pool, owner, instance
ID, lifecycle phase and creation timestamp; pools expose desired replicas.
It does **not** expose last provider observation, heartbeat age, billing rate,
certificate/model/canary evidence or model readiness. Those fields remain
`unknown` in the UI and JSON `null`, not zero or false. Creation time is never
presented as observation time. `Ready` is labelled **reported Ready (unverified)**, not
independently verified. A running local subprocess is not readiness evidence.

`Destroying`/`Draining` indicate pending teardown, never confirmed absence.
`Orphaned` shows cleanup unknown; even a reported `Gone` or an empty successful
list is not independent confirmation that historical resources/storage are gone.
The view does not claim secure bootstrap, stopped billing or provider absence.

`--json` exports a dashboard projection (local schema_version 1), **not** the raw
API envelope: reported mode, pools, nodes and nullable evidence/metrics. Raw server
errors and event strings are deliberately omitted from display/export. Non-success
requests exit 1 with a fixed error category on stderr; they do not print raw
payloads, parser errors, socket paths or server error values. API fields must
remain secret-free; this dashboard is not a general secret-redaction system.

## Bounded protocol handling

- Timeout defaults to 1 second, configurable from 1ms to 10 seconds. Socket2
  bounds Unix connect; partial writes/reads use remaining time from one absolute
  deadline so slow-drip responses cannot renew a per-read timeout indefinitely.
- Responses are bounded by existing `m8s_api::MAX_FRAME` (64KiB including newline)
  and must be exactly one newline-terminated JSON frame. Premature EOF,
  malformed JSON, unknown/duplicate/missing fields, unknown core `Phase` enum
  values and inconsistent success/error envelopes fail closed.
- Maximum 256 nodes/pools, bounded identifiers and error/event fields; duplicate
  node IDs/pool names are rejected. JSON parser recursion limits also remain on.
- Existing v1 fields are structurally mirrored for stricter validation, then
  projected. This is not a new daemon wire protocol. Future API schema additions
  intentionally require review rather than silently changing meaning.
- Untrusted identifier/mode text is rendered as printable ASCII or visible Unicode
  escapes; control/ANSI/bidi characters cannot become terminal commands. Invalid
  responses/errors never become an empty-fleet or confirmed-absence observation.

The caller selects the status socket. Before connecting, the client rejects
symlinks in every path component (including ancestor symlinks), `..` traversal,
non-sockets, foreign effective-UID ownership, a non-0700 immediate parent or
non-0600 socket (including special permission bits). User input is **not**
canonicalized through symlinks: on macOS use real `/private/tmp` or
`/private/var/...` paths rather than `/tmp` or `/var` aliases. Relative paths are
resolved lexically against the current directory, with the same checks.
Ancestor directories must be owned by root or the current effective UID and not
be group/other-writable unless sticky (allowing protected roots like `/tmp`).
A non-sticky writable ancestor is rejected even when the immediate parent is
private, since it could otherwise allow another user to swap path entries.

After connecting, kernel peer credentials must match the reader's effective
UID (Linux/Android and Apple/FreeBSD/DragonFly credential paths; unsupported
platforms fail closed). The entire path is revalidated and the socket/parent
inode, device, UID and mode must still match before any Status bytes are sent.
Path metadata failures, replacements and peer credential errors never turn into
an empty snapshot. This is local same-user IPC validation, **not** cryptographic
daemon identity or secure node admission. It does not defend against a malicious
same-UID process, privileged filesystem/kernel attacker or all filesystem race
models. Private socket creation/permissions also remain the daemon's responsibility.
Filesystem metadata queries and kernel stalls cannot be preempted by the socket
I/O deadline; elapsed deadline is rechecked before connect/request effects.

## Terminal lifetime and qualification

The read client, projection and renderer remain effect-free apart from explicit
client requests. `app::Options::parse(&[OsString])` validates explicit arguments
without I/O; its fields are private. Imports/constructors never initialize a
terminal, install handlers or connect a socket. The deliberately effectful
`app::run(Options)` is the shared opt-in executable adapter: both binaries call
it directly in-process, without PATH/sibling discovery, a shell, subprocess
dispatch or global argv mutation. Call it serially from an explicit command, not
concurrently with another terminal/signal owner. The runner
checks TTYs first, installs signal-hook atomic-flag callbacks for SIGTERM/SIGINT
only in interactive mode, and owns a cleanup guard before entering raw/alternate
screen mode. Normal exit, propagated errors, partial terminal setup failures,
Ctrl-C key and termination signals leave the alternate screen, show the cursor
and attempt raw-mode restoration. Cleanup operations are all attempted even if
an earlier output operation fails. Callbacks are removed at scope exit; this is
not a library facility promising restoration of arbitrary signal dispositions.

Signal-driven exit can wait for the bounded in-flight status request. SIGKILL,
abort, OS failure/uninterruptible I/O, terminal disappearance or failed restoration
writes can defeat best-effort cleanup; no unconditional cleanup guarantee is made.

Tests use explicitly private (0700/0600) synthetic Unix sockets, in-memory ratatui TestBackend snapshots and
fresh synthetic PTYs only (not the user's controlling terminal). PTY tests check
q/Ctrl-C keys and real SIGINT/SIGTERM restore terminal attributes, cursor and
alternate screen. Guard fault injection checks partial/error cleanup. No tests
access credentials, real provider resources, secure admission or private state.

## Argument compatibility

Both binaries share the standalone parser. An explicit `--socket PATH` is
required; `--once`/`--json` are each allowed once and may be combined (JSON wins).
`--timeout-ms` accepts 1..10000 inclusive; repeated timeout uses the last value,
preserving standalone behavior. Unknown/duplicate flags or missing values fail
without echoing their contents. Filesystem/socket validation happens only on an
explicit run request, not while parsing. Standalone OS-byte socket paths remain
opaque; `m8s` preserves its existing whole-argv UTF-8 requirement and opaque
`invalid arguments` failure, including top paths. Existing non-top grammar and
raw API JSON envelopes are unchanged. Top JSON is still the distinct projection
described above.

Synthetic PTY cleanup helpers are shared by standalone and CLI alias tests,
including q and real SIGINT/SIGTERM; no user controlling terminal or /dev/tty
bypass is used. Tests prove local command/terminal behavior only, not
cryptographic daemon identity, secure admission, provider absence or readiness.

### Optional active configuration observation

`m8s-top --socket PATH --show-config --once` (or `--json`) adds the actual active
Mock admission bounds. `m8s top` accepts the same flag in-process. Default output,
JSON and single Status request are unchanged; duplicate --show-config refuses.
No file reads, reload or mutation is requested. JSON exposes all validated tiers
(up to 32), while the bounded text/TUI viewport labels its bounds summary as limited.

Configured, legacy UNCONFIGURED and config UNKNOWN are distinct. Unconfigured is
not empty desired/fleet or verified default cap; unknown never substitutes an empty
configured policy. Retained config becomes UNKNOWN on whole-status refresh failure,
while the existing stale banner continues to qualify retained status rows.

Status and config are **two non-atomic observations**, with no cross-request
consistency/provenance/version/approval/Ready claim. Every connection repeats the
existing path/inode/UID/mode/peer checks. Both share ONE absolute total timeout;
config refusal/unsupported/malformed/timeout preserves usable status as config
UNKNOWN, never retries or falls back to files. JSON marks atomic_with_status=false.
No terminal initialization occurs for redirected one-shot/JSON use.

### Redacted status I/O diagnostics

Non-timeout I/O errors report a fixed typed stage (`working-directory`,
`path-metadata`, `socket-create`, `address`, `connect`, `nonblocking`, `poll`,
`write`, `read`; legacy `write-option`/`read-option` labels remain available), a
fixed kind label (or `other`), and optional signed numeric
OS error code (`none` if absent). No paths, addresses, argv/environment, raw error
text or error chains are retained or formatted. Codes are platform-specific
observations, not a diagnosis of daemon state or permission/security readiness.
Deadline expiry remains Timeout; WouldBlock waits for readiness as described
below. Trust/schema/refusal errors are unchanged.
Optional configuration failures still show UNKNOWN without erasing usable status.
There are no request retries, fallback, new timeout budgets or trust-policy changes.

Status exchange now sets its owned descriptor nonblocking once after the existing
bounded connect/trust checks. Safe `nix::poll` readiness waits use the same original
absolute deadline for partial writes, reads and the optional second observation;
no per-read/write SO_RCVTIMEO/SO_SNDTIMEO changes occur. WouldBlock waits again,
EINTR recomputes remaining time, and checked ceiling-millisecond poll conversion
is followed by a deadline recheck. Request offsets prevent reissuing sent bytes.
HUP/ERR readiness only permits a bounded nonblocking I/O attempt, never proves
success or a complete frame. Buffered complete frames followed by immediate peer
close can decode; partial EOF, malformed/oversized frames and invalid poll events
refuse. Setup/poll failures use redacted typed `nonblocking`/`poll` stages. There
is no application request retry/reconnect/fallback, transport authorization change
or new time budget.
