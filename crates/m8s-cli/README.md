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
