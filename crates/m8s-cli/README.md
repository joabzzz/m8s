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
