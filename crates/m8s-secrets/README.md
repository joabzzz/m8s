# m8s-secrets — bounded ADR-0012 backend slice

All stores implement the existing synchronous API, now explicitly Send + Sync.
Use spawn_blocking from async code. There is no implicit backend selection or
fallback and no ambient environment discovery. EnvStore::injected remains
read-only. SecretValue has no Debug/Display/Clone/Serialize and zeroizes on drop.
Expiry uses caller-injected seconds; at exact expiry, get/list hide an entry.
Delete can still remove expired entries. List returns metadata only.

## Explicit TOML file fallback (latest ADR-0012)

The accepted fallback is bounded, versioned TOML (`secrets.toml`), not the
experimental binary format. Keychain remains the intended macOS default, but
this library does not select a default backend/path or silently fall back.
The CLI integration owner must choose the backend explicitly. No legacy-file
migration, binary autodetection, or alternate-format parser fallback exists.

FileStore::open takes an explicit absolute path on Unix. Supply an existing,
owned, exactly 0700 parent outside all Git worktrees. No directory creation,
permission repair, tilde expansion, or default-path discovery happens. Every
component must be a real directory: use /private/tmp, not the /tmp symlink, on
macOS. Relative/dot/parent components are rejected. Ancestors must be root or
current-user owned and not group/world writable, except root-owned sticky
temporary ancestors. Any .git marker in the ancestry is rejected; ancestry is
rechecked per operation. A changed parent inode fails closed.

Data and stable sibling lock files must be owned, regular, singly linked and
exactly 0600. Extended ACLs are rejected on macOS/Linux; other Unix ACL paths
are unimplemented and fail closed. Files are opened relative to pinned directory
descriptors, with no-follow, close-on-exec and nonblocking flags. Special files
cannot block a read. Restrictive creation never truncates existing paths.
The sibling <filename>.lock is held across read/modify/write by every operation,
including reads. Never remove/replace the lock while a store is in use;
independent cooperating processes must use that same lock.

Writes encode into a zeroizing buffer, create an exclusive 0600 sibling temp
file, sync it, atomically rename it and sync the parent directory. Temp files
are removed on ordinary failure. A crash before rename can leave a private temp
file; there is intentionally no automatic orphan sweep. A failure after rename
(e.g. directory fsync) can mean the update took effect: an error does not promise
rollback. Advisory locking requires cooperating writers. Malicious same-UID
code/root is not defended against.

### Version 1 schema

The encoder emits valid TOML using a deliberately restricted, line-oriented
schema (not arbitrary TOML). The parser accepts this subset only:

```toml
version = 1

[[secrets]]
name = "test/session"
value_hex = "007fff"
expires_at = 10
```

This example is synthetic. `value_hex` is lowercase hex encoding of arbitrary
bytes, **not encryption**. The exact header `version = 1` must be first, after
optional blank/full-line-comment lines. A record has exactly one `name` and
`value_hex`, in either order, plus optional canonical signed-i64 `expires_at`
(injected Unix seconds). There is no null/never-expire field: omit expiry.
Names follow SecretName validation. Empty values are supported by the store;
the interactive UX rejects empty input. Spaces/tabs around record keys and `=`
are accepted. LF/CRLF, blank lines and full-line `#` comments are accepted.
Unknown fields/tables, repeated fields/names, alternate versions, string escapes,
multiline/literal strings, inline comments, alternate integer forms and other
TOML features are rejected. This intentionally small grammar avoids generic
value-bearing serde/Debug allocations and parser diagnostics containing input.

Limits: 1 MiB encoded file, 1024 records, 128-byte names, 64 KiB decoded value.
Hex doubles value size, so the total-file limit may bind first. The parser checks
bounds before allocating values; the encoder sizes its zeroizing buffer before
writing. All parse errors are opaque. On-disk values are **not encrypted**;
disk encryption, backups and provisioning remain operator concerns.

## Keyring backend

KeyringStore::injected(driver) stores one bounded envelope per driver item, so
names and expiry cannot become inconsistent with separate value items. The
driver owns its namespace and must atomically replace its item. Driver errors
are opaque, never treated as absence or file-fallback signals. The store mutex
serializes its operations, **not independent objects or processes**. Use one
host-owned store or external serialization; multi-writer keyring coordination
is not implemented in this slice.

The optional os-keyring feature compiles MacKeyringDriver on macOS using exactly
keyring 3.6.3, apple-native, no default features. Downloaded local source was
inspected for its byte API and credential Debug/log representation (service,
account, domain only). Constructor takes an explicit validated namespace and
makes no OS calls. Explicit load/save operations use service m8s-secret-store-v1
and that namespace only. This adapter is excluded from unit test builds. Tests
use only injected mock drivers and **never call keyring**. No Linux Secret
Service adapter or implicit macOS default is wired yet.

Delete removes a named record; the empty envelope/item can remain. This is not
a physical-erasure guarantee. Keychain approval/access controls, binary signing,
prompts and size limits remain unverified: compilation is not live acceptance.
Never use the mesh owner keystore service/account.

## Secret UX library / integration contract

`m8s_secrets::ux` is ready for CLI integration; **no existing CLI main is wired**.
`parse(&[&str]) -> Command`, `execute(store, command, input, now) -> Output`:

- `set NAME [--stdin] [--expires-at SECONDS]`: no argv value option; rejects
  expired/non-future expiry before input. Default is explicit terminal entry.
- `list`: active names/expiry only; never reads values via get.
- `rm NAME`: deletes even an expired entry; returns whether it existed.
- `check`: validates local backend readability/schema and reports active count;
  it does **not** validate provider scope, credentials or connectivity.
- No `get` or `show`. Errors never interpolate rejected arguments.

The caller injects a selected `SecretStore` and `Input` implementation. Output
contains metadata only. `SystemInput` performs no access until read is requested.
Default terminal entry first refuses non-TTY stdin, opens `/dev/tty`, disables
echo/canonical mode/signals,
handles newline, backspace and Ctrl-C/Ctrl-D/Ctrl-Z cancellation locally, and
restores terminal attributes on ordinary success/error/cancellation. It fails
closed if unavailable; it never substitutes redirected stdin. No value/prompt is
printed by this library: the CLI may print a fixed non-secret prompt to stderr.
Abrupt process kill/host failure can bypass restoration; operators may need
`stty sane`. Non-Unix terminal input is unavailable, not plaintext fallback.
Explicit `--stdin` reads exact bytes through EOF (including a final newline).
Both paths bound input at 64 KiB and keep temporary buffers zeroizing. The caller
must not log raw argv/input or automatically render input errors with context.

Tests use synthetic dedicated temp files, injected keyring mocks and a dedicated
synthetic pseudo-terminal (no OS Keychain calls, ambient secret environment or
owner keystore). PTY checks verify no echo and restoration after success and
cancellation; actual Keychain qualification remains unverified. Dependencies and
root Cargo files are unchanged.

## Deferred

Default-path/backend setup, actual CLI main integration, lifecycle expiry sweeps,
owner-keystore auditing, provider key scope checks and real credential/provider/
keychain testing remain out of scope. No owner keystore, credential environment,
or provider access is needed for tests. Expiry is enforced on get/list/check;
physical erasure and automatic expiry deletion are not claimed.
