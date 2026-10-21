# m8s-secrets — bounded ADR-0012 backend slice

All stores implement the existing synchronous API, now explicitly Send + Sync.
Use spawn_blocking from async code. There is no implicit backend selection or
fallback and no ambient environment discovery. EnvStore::injected remains
read-only. SecretValue has no Debug/Display/Clone/Serialize and zeroizes on drop.
Expiry uses caller-injected seconds; at exact expiry, get/list hide an entry.
Delete can still remove expired entries. List returns metadata only.

## EXPERIMENTAL file backend — Needs Admin

**This is not the accepted ADR-0012 TOML fallback.** The format/path contract
requires an Admin decision before any default or CLI selects it. Concrete card:
**Secret fallback format: retain TOML contract or accept bounded binary?**
(card 01a15cbc-5bc1-7026-8894-8792bd61053d, Needs Admin on the m8s board).
The recommendation is to accept the bounded binary format with an ADR amendment;
retaining TOML with equivalent safe parsing is the alternative. No approval is
implied by integration of this explicitly experimental library.


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

**Format decision pending Admin:** this uses a bounded, versioned binary
envelope, not TOML/.env. There was no existing file format to migrate. Choose an
honest filename such as secrets.bin, not the ADR's proposed secrets.toml. There
is no migration/parser fallback. Limits: 1 MiB total, 1024 records, 64 KiB/value,
and the existing 128-byte name limit. Invalid versions, duplicate names, malformed
lengths/expiry tags and trailing bytes fail closed. Values on disk are **not
encrypted**. Disk encryption, backups and provisioning are operator concerns.

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

## Deferred

CLI set/list/rm/check, default selection, terminal entry, lifecycle expiry sweeps,
owner-keystore auditing, provider key scope checks and real credential/provider/
keychain testing are out of scope. No owner keystore, credential environment,
or provider access is needed for tests. There is no get/show CLI.
