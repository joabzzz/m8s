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

`KeyringStore` stores one bounded internal binary envelope per driver item, so
names and expiry cannot become inconsistent with separate value items. This is
not the public TOML file contract. Driver errors remain opaque, never absence,
rollback guarantees or file-fallback signals. A failed save may already have
committed; the next explicit successful load supplies the backend evidence.

### Explicit exclusive-writer coordination (not OS/default qualification)

Identified backends use these library-only APIs:

1. `KeyringIdentity::new(service, account)` supplies **non-secret** exact backend
   identity metadata (validated service/account names, each at most 128 bytes).
2. `KeyringLockDomain::acknowledge_exclusive_writers(directory, identity)` is an
   **explicit security-contract acknowledgement**, not an automatically verified
   assertion. The owner must provision one trusted private lock directory and
   ensure **every** daemon/CLI/other writer for that service/account uses it.
3. `KeyringStore::coordinated(driver, domain)` rejects missing/mismatched driver
   identity before backend load/save. Identity is rechecked on each operation.

The lock directory follows FileStore's descriptor-relative path walk: explicit
absolute components, owner-0700 private directory, no symlinks/Git ancestry or
extended ACLs, trusted ancestors. The empty stable lock name is fixed by SHA256
of version-tagged, length-delimited service/account identity (not secret values).
It is owner-0600, regular, singly linked, no-follow/nonblocking/close-on-exec,
ACL checked, synced with its parent on construction and never replaced/deleted.
No secrets are written to lock files; this is not a new private keystore.

A shared process-wide identity registry rejects concurrent live conflicting
lock domains and shares one mutex for matching pinned directory/lock inodes.
Every get/list/put/delete holds that shared mutex **and an exclusive flock**
across the ENTIRE load/decode/read-or-mutate/encode/save operation. Each operation
opens a fresh lock descriptor, rewalks ancestry and pins inodes; checks repeat
after waiting for flock. Poison or validation failure fails closed before any
backend effect. File drop/process exit releases flock. No stale-lock deletion,
permission repair, implicit path discovery, lock stealing or fallback.
Contention has a monotonic 5-second default budget shared by process-mutex and
flock acquisition; `acknowledge_exclusive_writers_with_timeout` explicitly sets
0..=30 seconds (zero is immediate try-only). Timeout is an opaque `LockTimeout`
before backend access, with no automatic retry or unlock/poison recovery.
This budget bounds contention polling, not filesystem I/O or an already running
synchronous driver call; backend-operation deadlines require separate driver/OS
qualification. Calls are blocking/non-reentrant: use `spawn_blocking` from async code. Do not
fork live stores/mutexes; independently exec and construct each client.

**Supported guarantee is qualified cooperative exclusive deployment only.**
No API can prove third-party/raw/legacy OS Keychain clients honor this protocol,
or discover a globally canonical directory safely. All writers must agree on
one identity/domain and a filesystem with qualified local flock semantics.
Do not change/recreate/move a domain while ANY process can use the old one.
After all handles have dropped, creating a different domain requires a fresh
explicit acknowledgement and operational exclusion of all old/foreign clients;
there is no persistent global authority proving that exclusion. Malicious
same-UID code/root is outside this model, as for FileStore. Admin deployment/
security qualification is tracked in Needs Admin card
`01a15e95-8a36-77b3-97ea-8fbd08f21b49`. CLI Keychain/default remains **unavailable**;
synthetic locking tests do not qualify actual Keychain or cross-application use.

`KeyringDriver::identity()` must be pure and immutable, returning `Some` for any
backend with independent clients. `None` and `KeyringStore::injected(driver)`
remain ONLY for isolated synthetic/in-memory drivers; they do not provide a
multi-writer guarantee. An identified driver (including the OS adapter) cannot
perform backend operations through `injected`: it fails unavailable before
load/save. Custom drivers must uphold the identity/exclusivity contract; raw
driver calls bypass store coordination and are unsupported concurrent writers.

Normal tests inject memory/file fixture drivers only: independent objects, six actual
subprocess writers, forced whole-operation race windows, crash/restart releasing
flock, mutex/flock contention timeouts (including a real-process timeout),
ambiguity-after-save, read failure/corruption, shared process-lock poison,
lock replacement/modes/links/Git/ACL/path swaps, expiry and deletion. Subprocess
environments are cleared and contain only dedicated non-secret synthetic fixture
parameters. No real OS keyring item, credential env or owner keystore is accessed.

The optional os-keyring feature compiles MacKeyringDriver on macOS using exactly
keyring 3.6.3, apple-native, no default features. Downloaded local source was
inspected for its byte API and credential Debug/log representation (service,
account, domain only). Constructor takes an explicit validated namespace and
makes no OS calls. Its pure identity reports service/account; use it only via a
qualified coordinated store, not injected/raw driver calls. Explicit load/save operations use service m8s-secret-store-v1
and that namespace only. This adapter is excluded from unit test builds. Normal tests
use injected mock drivers and **never call keyring**; the explicitly ignored
synthetic OS integration harness below is a separate opt-in operational step. No Linux Secret
Service adapter or implicit macOS default is wired yet.

Delete removes a named record; the empty envelope/item can remain. This is not
a physical-erasure guarantee. Keychain approval/access controls, binary signing,
prompts and size limits remain unverified: compilation is not live acceptance.
Never use the mesh owner keystore service/account.

## Secret UX library / integration contract

`m8s_secrets::ux` supplies the library command/input boundary; see the
`m8s-cli` README for actual explicit-file CLI wiring.
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
Terminal set reads twice and rejects mismatched confirmation before storing.
Default terminal entry first refuses non-TTY stdin, opens `/dev/tty`, disables
echo/canonical mode/signals,
handles newline, backspace and Ctrl-C/Ctrl-D/Ctrl-Z cancellation locally, and
restores terminal attributes on ordinary success/error/cancellation. It fails
closed if unavailable; it never substitutes redirected stdin. No value/prompt is
printed automatically by this library: `SystemInput::read_with_prompt` invokes
a fixed-prompt callback only after no-echo succeeds and restoration is guarded.
The CLI writes its fixed stderr prompts inside that callback, never beforehand.
Callback errors restore terminal attributes without reading input.
Abrupt process kill/host failure can bypass restoration; operators may need
`stty sane`. Non-Unix terminal input is unavailable, not plaintext fallback.
Explicit `--stdin` reads exact bytes through EOF (including a final newline).
Both paths bound input at 64 KiB and keep temporary buffers zeroizing. The caller
must not log raw argv/input or automatically render input errors with context.

Tests use synthetic dedicated temp files, injected keyring mocks and a dedicated
synthetic pseudo-terminal (no OS Keychain calls, ambient secret environment or
owner keystore). PTY checks verify no echo and restoration after success and
cancellation; actual Keychain qualification remains unverified. The UX library adds
no dependencies.

## Deferred

Default-path/backend setup, lifecycle expiry sweeps,
owner-keystore auditing, provider key scope checks and real-credential/provider
testing remain out of scope. Synthetic OS qualification
is a separate explicit gate below. No owner keystore, credential environment,
or provider access is needed for tests. Expiry is enforced on get/list/check;
physical erasure and automatic expiry deletion are not claimed.

## Synthetic macOS qualification — Phase A source freeze

Gate 1 is ready for technical qualification (ADR-0012 v8); Admin attendance is
not required. This harness is **not yet OS execution evidence** and does not
change default selection. Review its frozen source before explicitly running it.
Only `tests/os_keyring_qualification.rs` and this qualification documentation are
in scope; no production APIs, CLI, dependencies or lock schema are changed.

### Source writer/domain inventory at `cc07dd8`

The audit is limited to registered m8s workspace source/manifests, not a host,
home-directory, Keychain or process scan:

| Actual source | Constructor / effect | Coordination / status |
| --- | --- | --- |
| `src/keychain.rs::MacKeyringDriver` | Explicit namespace; fixed service `m8s-secret-store-v1`; `load` exact `get_secret`, `save` exact `set_secret` | Constructor has no OS effect. Only `NoEntry` maps to absence; all other OS failures are opaque `Backend`. No item enumeration/deletion/default constructor. |
| `src/keychain.rs::KeyringStore` | `coordinated(driver, domain)`; every get/list/put/delete transaction | Identity matched before access and per operation; process mutex + stable flock cover entire envelope load/modify/save. Identified `injected` rejects before driver access. Direct trait calls could bypass this: the trait is public, not an access-control boundary. |
| `src/keyring_lock.rs::KeyringLockDomain` | Explicit directory and exact identity; optional explicit wait budget | No production path/default provisioning. SHA256 version/length-delimited identity-derived empty stable owner-0600 lock; pinned private owner-0700 directory, revalidated ancestry/ACL/inodes. No auto-discovery or migration. |
| `m8s-cli/src/main.rs`, `src/secrets.rs::run_inner` | `m8s secrets` requires explicit `--file`, constructs `FileStore` | No production `MacKeyringDriver`, `KeyringStore` or lock-domain constructor/call site. Missing file choice returns `Unavailable`; no fallback. CLI defaults remain OFF. |
| Other workspace production callers | Vast readonly adapter uses the `SecretValue` type only; daemon has no Keyring writer | No additional OS-driver writer/constructor found by source search. No provider invocation is part of this audit. |
| Existing secret/CLI tests | `KeyringStore::injected` memory mocks; coordinated memory/synthetic-file fixtures in `keyring_lock_tests.rs` | Synthetic source cooperation only, not actual OS/deployment qualification. |
| New ignored integration harness | Coordinated wrapper delegates Mac driver load/save only while store transaction holds canonical domain | Exact single synthetic service/account shared by all independent thread/process clients; wrapper rejects a pre-existing envelope on initial load. |

Source search covered `KeyringStore`, `MacKeyringDriver`, `KeyringLockDomain`,
`KeyringDriver`, `keyring::`, `FileStore`, and secret command wiring in registered
workspace Rust/manifests. No source evidence proves there are no external,
legacy, third-party or deployment writers. Acknowledging exclusivity is an
operational contract, not discovery or verification of those writers.

The harness provisions **only a TEST domain** under `/private/tmp`, never the
production canonical directory. The ADR candidate `~/.config/m8s` still requires
future explicit permitted provisioning/validation and a deployment-wide writer
inventory/routing check. This audit has not accessed/provisioned that directory,
verified its permissions, or inspected user private stores. No bypass is allowed
if a future candidate is unwritable. Default enablement additionally needs
actual OS roundtrip/concurrency qualification, approval/prompt/signing/access
policy and deployment-client verification; a passing synthetic namespace alone
cannot discharge those obligations.

### Reviewed execution contract and bounded cleanup proposal

The integration crate is gated by **macOS AND `os-keyring`**, and its OS test is
explicitly **ignored**. Six ordinary tests cover injected watchdog decisions,
strict metadata parsing, descriptor checks and actual no-OS exec handshakes;
none can call a scenario/store, regardless of environment or received mode.
Normal workspace tests compile the OS path but never execute Keychain calls. The integration library (unlike the unit-test library) includes the Mac
adapter. `--list`/normal selection are safe; do not use blanket `--ignored`.
After primary source review, the exact permitted operational invocation is:

```sh
cargo test -p m8s-secrets --features os-keyring --locked -j 1 \
  --test os_keyring_qualification -- --ignored --exact os_keyring_qualification \
  --test-threads=1 --nocapture
```

Use the same isolated build/cache paths as the frozen normal checks. The parent
prints **only non-secret account/domain identifiers**. It generates 128 bits of
OS randomness for a clearly synthetic account, exclusively creates its 0700
same-named directory under `/private/tmp`, and validates the canonical public
lock domain before starting a scenario process. Existing paths abort without
repair or retry. No secret input comes from argv/env/fixtures/repository; fixed
synthetic bytes are generated in memory, never formatted, compared only as bytes
without value-bearing assertions, and stored only in the exact synthetic item.
No real credentials, owner-key stores, Keychain enumeration/search, provider,
network or home-directory access is performed by the harness.

1. The scenario's first coordinated read requires actual `NoEntry` from the Mac
   driver. **This is not an existence-only API:** Mac `get_secret` can fetch
   existing bytes into a `SecretValue` before returning `Some`. On collision,
   even with a valid empty envelope, the wrapper rejects and drops/zeroizes those
   bytes **before decode/save/delete**; it never exposes/logs them. The fresh
   128-bit namespace reduces accidental collisions but does not prove impossibility.
   An error is not absence. The unpredictable namespace plus
   initial absence and first ownership-record save establish this run's bounded
   synthetic scope under the cooperative-writer contract; no API can exclude a
   malicious same-UID raw writer racing that scope.
2. Two independent thread stores begin together; two actual independently exec'd
   process stores announce readiness, then start on one non-secret gate. All
   clients reconstruct the **same** service/account/domain; none uses an alternate
   lock or uncoordinated writer. Each inserts four distinct records; an in-lock
   delay widens envelope contention. All 16 records plus ownership must survive,
   with byte equality after clients exit and a newly exec'd read-only restart client
   reconstructs the store. This tests
   concurrent whole-envelope preservation, not a scheduler-independent proof
   that both processes reached the OS at the same instant.
3. Verify injected expiry at 9/10, exact metadata counts, deletion including an
   expired record, repeated deletion false, and empty listing after reconstruction.
   Threads are joined and child processes reaped **before** any cleanup writes.
   Cleanup deletes only this run's known names through the same coordinated
   `SecretStore`; it never deletes arbitrary names obtained from a listing.
4. **Proposed safe artifact policy:** leave the empty synthetic OS envelope and
   the private stable lock/domain plus empty coordination/stage markers. Do not add a
   production item-removal API or call raw `delete_credential`. This is bounded
   to one synthetic OS item per explicit run, **not zero OS items**, physical
   erasure or an absence claim. The parent reports its exact identifiers. Failed
   or interrupted runs can retain some/all synthetic records; do not auto-clean,
   retry into a different account, enumerate, sweep or remove the stable lock.
   Any later physical cleanup needs a separately reviewed exact-item procedure
   after every writer has exited, coordinated in this same domain.

### v3 current-parent IPC confinement (no malicious same-UID auth claim)

Every entry rejects the presence of ANY legacy `M8S_SYNTHETIC_MODE`,
`M8S_SYNTHETIC_ACCOUNT` or `M8S_SYNTHETIC_DOMAIN` variable, including non-Unicode
values, before driver construction. Unrecognized control selectors also fail.
Without a selector the ignored OS test can only allocate a new root session;
there is no old-session discovery/reuse or arbitrary namespace constructor.

Each exec receives a newly created anonymous `UnixStream::pair` endpoint as stdin;
no named listener/path is used. The parent retains the other endpoint and owns
its `Child` in RAII **before** starting the handshake. Failure kills/reaps only
that direct child, never a saved numeric process group; descendant/OS effect
uncertainty is retained. All socket endpoints are close-on-exec except deliberate
child stdin. The receiver duplicates stdin with `F_DUPFD_CLOEXEC`, takes ownership
only of that successful new fd, then checks fstat socket type/current UID,
SO_TYPE stream, AF_UNIX family, same-UID peer and kernel `LOCAL_PEERPID` equal to
its immediate parent, **before** converting the owned duplicate to `UnixStream`.
Regular/null/closed/wrong-peer descriptors fail without a driver. No double-close
or adoption of the original stdin descriptor occurs.

The child sends a fresh 128-bit non-secret OS-random challenge. A live parent
replies with a fixed-size v3 frame: magic, echoed challenge, allowed mode, current
root-session ID, exact account/path and pinned directory device/inode. The frame
is statically bounded below 1 KiB, read into a fixed array before bounded string
allocations; there are no peer-controlled lengths. EOF, truncation, trailing
bytes, stale challenge, unknown mode, mismatched session/account, malformed suffix
or aliased path fails closed. Response ends with a write-half close; child ACK
occurs only after full validation. One monotonic **two-second whole-handshake**
budget recalculates remaining socket timeouts for every partial read/write; it
is not reset by a slow trickle. Filesystem/kernel stalls, entropy, process startup
and kill/reap still have the honest synchronous/OS limits described below.

Account must be **exactly** `m8s-synthetic-os-qualification-` plus 32 lowercase hex
digits derived from this root session ID. Directory bytes must be the literal
`/private/tmp/ACCOUNT`, with no symlink-root alias, dot/parent/doubled separator or
trailing slash. Session directory owner0700/device/inode is revalidated before
constructing the public lock domain and again inside coordinated driver calls;
the existing `KeyringLockDomain` independently pins/revalidates its directory,
ancestry, ACLs and stable lock before each actual operation. No alternate lock.

Private `RootSession` issues scenario/preflight clients only. A private
`CreatedSession` is constructed only after exact initial NoEntry and successful
ownership-record save; only it issues writer/verify clients. All use fresh
per-child control pairs carrying the same current root scope. Threads share
that validated scope. No secret values travel over IPC. A deliberately cooperating
foreign/malicious same-UID parent could emulate this protocol or invoke the public
OS driver: this is current-flow confinement against accidental ambient/stale
entry, not a new authentication/access-control boundary against such code.

Normal no-OS exec tests use a separate **Probe-only** test function with no
scenario/store call path, even if a forged OS mode arrives. They verify an actual
successful handshake and actual exec rejection of stale challenge/session,
malformed header/account/path/inode, wrong mode, EOF and selector with null stdin.
Additional pure tests cover malformed suffixes/exact paths/legacy selectors and
regular/wrong-peer fd rejection. These control-only fixtures create fresh empty
private directories, never a lock/store/OS item, and remove only their exact
empty fixture directory after all direct probe clients are reaped. The actual
OS namespace/lock/artifact retention policy above is unchanged.

Driver errors signal a non-secret stop marker under that private domain; later
coordinated loads/saves fail without another OS call. No error retry, fallback,
policy modification or prompt automation is used. A failure stops qualification,
with no automatic cleanup writes. Already-running OS operations can have taken
effect despite an error or process kill; the result is uncertainty, not absence.

Operation count is fixed (four inserts/client and bounded verification/deletion),
lock contention uses the library's five-second budget, rendezvous waits cap at
10 seconds, writer completion waits at 30 seconds, fresh restart verification at 15 seconds,
and the outer parent caps the
scenario at 90 seconds. Only a true timeout immediately after the single-child
poll returns `None` signals the dedicated process group, **before any final
wait/reap**. The leader remains owned and unreaped between that observation and
signal, preventing numeric PID/PGID reuse. A nonzero exit may already have been
reaped by `try_wait`; it and polling I/O errors **never signal the old group**.
Those errors receive direct-child RAII kill/wait only, with no PID adoption or
claim that uncertain descendants were terminated. Even a successful group signal
is not proof that every descendant exited; the parent cannot reap grandchildren.
Deterministic no-OS tests cover reaped success/nonzero exits, polling error,
unreaped timeout signal order, and pending-then-exited behavior. Child invocations
run this exact ignored test with a cleared environment containing only a
non-secret `M8S_SYNTHETIC_CHILD_CONTROL=v3` selector; no mode/account/domain/session
is supplied in argv/env. The selector alone grants nothing. No nested Cargo/full
builds or agents. OS API prompts,
filesystem stalls, kill/reap latency, kernel/OS cancellation and the compiler are
**not bounded by the lock/operation budget**. The outer process watchdog limits
ordinary scenario runtime but is not a hard real-time or OS rollback guarantee.
If denied, unavailable, stalled or otherwise failing, stop and report opaque
failure/uncertainty; no sandbox bypass, security-policy change or retry masking.

### Evidence separation

Phase A normal test/build/fmt/Clippy results prove only source compatibility and
ignored selection. Phase B must separately record the exact reviewed hashes,
runtime/toolchain, non-secret synthetic identifiers, outcome and retained
artifacts. Neither phase proves external deployment writers cooperate, provisions
a production home directory, qualifies every binary/signing/prompt configuration,
or authorizes enabling the default. No Phase B result is claimed here.
