# ADR-0012 — SecretStore trait; macOS Keychain by default, 0600 file fallback


## Gate 1 OS qualification — incomplete after synthetic attempt

Gate 1 qualification attempt (2026-10-21): Development reports independently reviewed v3 harness source findings resolved, normal checks 426+16 pass/3 ignored plus Clippy/fmt. One explicit synthetic OS run in current coding sandbox FAILED: exit 101, 0 passed/1 failed, opaque Backend, 0.42s. Existing log read-only verified at /private/tmp/m8s-keychain-v3-os-qualification.log, SHA256 05482f412dd6b5c2ca73df90f6aaf6a4ffd5c187cc6eaa0722ff173a3ae6e772. Metadata /private/tmp/m8s-keychain-v3-runtime-metadata.log records checkout 2ab213f99e79a30303f0e99e05c4576f3b892e2e, rustc1.92.0, macOS26.6.1/build25G76. Synthetic account/domain m8s-synthetic-os-qualification-c65a20810681ec2508cd6f90c3dddddf retained. Development reports initial NoEntry marker passed, failure before stage-created; actual read/save cause unknown and possible OS effects uncertain. Do not infer permission denial or item absence. No cleanup, retries, permission changes, automated prompts, real credentials or default enablement. Attendance prerequisite remains removed; Admin authorization resolved. OS qualification technically incomplete pending a permitted execution context completing synthetic write/read/concurrency, not a new consent request. Gate 2 remains endpoint PASS; relay gate unchanged.



## Gate 1 clarification — ready for qualification (2026-10-21)

Admin asked to unblock qualification and notify Development in [Metal-0 Unblock conversation](http://localhost:3030/ui/s/01a160a421a176879f4a042c/). Mandatory Admin attendance is not a prerequisite: the accepted recommendation at 20:09 UTC required writer-domain verification and synthetic OS tests, not attendance; version 7 added “with Admin present” without explicit support in that exchange. The preceding section is retained as historical wording, corrected here. Proceed with canonical writer/lock-domain provisioning, verification of every m8s writer, and synthetic-only OS Keychain roundtrip/concurrency tests in a permitted runtime without waiting for attendance. Actual runtime access remains unverified, not a proven denial. Keep the Keychain default unavailable until those technical checks pass. No real credential access, silent fallback, or security weakening authorized.

Status: **accepted; amended 2026-10-21 (TOML file contract retained; latest Admin direction supersedes binary approval)** · Decided by: Admin · Recorded by: Metal-0 · Date: 2026-10-20 · Research: [S3 §8](agor://kb/document/01a15b4c-9887-7452-a281-858ce1d16477) · Related: [ADR-0006](agor://kb/document/01a15b4d-b4ae-765a-ba27-4491ae240bf9), [ADR-0008](agor://kb/document/01a15b4d-b4f9-7254-adfb-37d41a4297e1), [ADR-0013](agor://kb/document/01a15b4e-cb32-77ff-afa6-bb74960a1048), [MP-001](agor://kb/document/01a15c1c-ac26-7000-970b-de909358ba57)

## Context

Admin asked for an open, adaptable trait, and said a `.env` file was acceptable for the prototype "unless that's horrible for security".

| Secret | What it unlocks | Lives on | Must never go |
|---|---|---|---|
| **Mesh owner key** | Admits any node; owner-control of the whole mesh | Mac mini, inside **mesh-llm's own keystore**. m8s never reads it | Rented containers (env, image or RAM), k8s Secrets or CRDs, any repo or worktree, logs or OTel, Agor or chat |
| ~~Fleet owner key~~ | *Dropped.* Accepted [ADR-0006](agor://kb/document/01a15b4d-b4ae-765a-ba27-4491ae240bf9) closes the admission gap with [MP-001](agor://kb/document/01a15c1c-ac26-7000-970b-de909358ba57): short-lived per-node certificates signed on the Mac. **No owner-class key reaches a rented box.** | — | — |
| Bootstrap nonces | One boot redemption (15 min, single use) | Bootstrap nonce ledger on the Mac | Anywhere beyond the provider env of its own instance |
| **Vast API key** | Rent, destroy and inspect instances on the account | Keychain on the Mac, read by the host-side provider adapter | Any rented container, images, repos, logs, shell history or argv, Agor or chat |
| **Wallet material** (later) | Moving money | **Outside m8s** (hardware wallet or a separate signer) | m8s, the file fallback, and everywhere above |

## Is `.env` horrible?

No, not for a single-user Mac with FileVault, as long as the file is 0600 and sits outside every repo. The real risks, most serious first:

1. **Agents.** Coding agents on the Mac mini run as the same macOS user. A file under `~` is one `cat` or `grep` away from a transcript and a model provider's logs. Keychain's per-binary access list stops these accidental reads.
2. **Accidental copies.** Docker build contexts, worktree copies, and unencrypted backups.
3. **Blast radius.** A leaked Vast key costs money, bounded by the account balance. A leaked owner key compromises the mesh.

Keychain is **not** a defence against malicious code running as the user. That code can drive an allowed binary, or phish the access prompt. Keychain does stop accidental reads and leaks, which are the realistic threat here.

## Decision

1. **A `SecretStore` trait with pluggable backends.**
   - `keychain` (via `security-framework`) is the default on macOS.
   - `file` is the fallback: bounded, versioned TOML at `~/.config/m8s/secrets.toml`, mode 0600, in a private 0700 directory. Latest Admin direction retains the TOML contract and supersedes the earlier binary-format approval (2026-10-21). The existing binary implementation remains experimental, not the accepted fallback. Refuse unsafe ownership, permissions, extended ACLs, symlinks, hardlinks, non-regular files and Git-worktree locations. Preserve descriptor-relative no-follow access, serialized updates, atomic replacement with fsync, expiry enforcement, opaque errors and zeroization. Do not silently fall back from Keychain or migrate/read an old TOML file.
   - `env` is read-only, for CI and tests.
   - Later backends (`op`, `sops`, `vault`) plug in behind the same trait.
2. **m8s never holds the mesh owner key.**
   - mesh-llm keeps it in its own keystore. m8s stores only the owner's *public* endpoint id.
   - When m8s needs owner authority, such as signing MP-001 per-node certificates during the sealed bootstrap, it asks mesh-llm to sign through `auth sign-node` or the loopback API, and never reads the key bytes.
   - Audit reports are signed with a separate m8s key.
3. **Session-scoped secrets live in Keychain with an expiry.**
   - Admin agreed to this for the fleet key; with ADR-0006 accepted, the fleet key no longer exists, so the answer becomes the general rule.
   - These secrets are created at `m8s up` and deleted at `m8s down`, and an expiry sweep catches anything missed.
   - They survive an operator restart.
   - If Keychain prompts on dev builds cause too much friction, dev builds may keep them in memory only.
4. **Secret-holding components run on the host, not in minikube.**
   - The provider adapter and the bootstrap/control server run as host processes (a launchd user agent).
   - No k8s Secrets are used for these, because minikube etcd is unencrypted.
   - Graduating to real Kubernetes is outside this framework; it would need its own ADR.
5. **A full-account Vast key is acceptable for the prototype.**
   - The key controls instances. Adding funds is a separate step, so exposure is bounded by the balance, about $20.
   - Conditions:
     - auto top-up/auto-billing stays **off**, otherwise the bound disappears;
     - the key never reaches a rented box.
   - Prefer a restricted or expiring key if Vast offers one (not yet verified).
   - Accepted residual risks: an attacker could destroy our instances or launch abusive ones on the account.
6. **Wallet material stays outside m8s for now**, consistent with [ADR-0013](agor://kb/document/01a15b4e-cb32-77ff-afa6-bb74960a1048).

## Historical amendment — bounded binary fallback accepted (2026-10-21; superseded below)

**Decision by Admin:** “accept bounded binary and amend ADR-0012 (recommendation).” Recorded in [Sprint 1 session](http://localhost:3030/ui/s/01a15c9c1eb37715800322f7/), resolving decision card `01a15cbc-5bc1-7026-8894-8792bd61053d`.

Options considered: retain TOML with equivalent hardened parsing, or accept the implemented bounded versioned binary envelope. **Bounded binary is accepted.** Its explicit limits/versioning and zeroizing byte codec avoid requiring general-purpose value-bearing serde types. This is an encoding decision, **not encryption at rest** and not a relaxation of any secret handling or filesystem security requirement. Keychain remains the intended default; file fallback remains explicit, never automatic.

Current envelope version `M8SSEC01` is bounded to 1 MiB total, 1024 records, 128-byte validated names and 64 KiB per value. Unknown versions, malformed/truncated data, duplicate names, trailing bytes and exceeded limits fail closed. Values remain non-Debug/non-Display/non-Serialize and zeroized on drop. Mesh owner keys and wallet material remain outside m8s.

Format approval resolves the prior experimental-format gate only. It does not claim CLI/default selection, expiry sweeps, migration, real OS Keychain qualification or live provider readiness are implemented. Sprint 1 source docs at commit `2ea7c3e` retain historical pending-decision wording; reconcile those in the next repository documentation update. See [current handoff](agor://kb/document/01a1575c-3c1f-7036-84be-7c5f7b6deb12).


## Latest amendment — retain TOML contract (2026-10-21)

In the parent Metal-0 conversation, Metal-0 recommended keeping the accepted TOML secret-file contract instead of adopting the experimental binary format. Admin replied: “sure just update the ADRs.” This latest direction supersedes the preceding binary-format approval; history is retained above.

The next implementation must provide bounded, versioned TOML with all existing filesystem, secret-value, expiry and fail-closed requirements preserved. Parsing/encoding must avoid leaking values through errors, Debug, logging or lingering temporary allocations. Duplicate entries, oversized input and unknown versions must fail closed. Do not enable the binary backend as the default or silently migrate existing files. Keychain remains the intended default. Format approval is not implementation or OS qualification; binary code in Sprint 1 remains experimental until replaced or separately reapproved.

## Trait sketch

```rust
/// Validated name: [a-z0-9-/], e.g. "vast/api-key", "m8s/audit-signing-key".
pub struct SecretName(String);
/// Zeroized on drop; no Debug/Display/Serialize, so values can't reach logs or CRDs.
pub struct SecretValue(secrecy::SecretBox<[u8]>);

pub trait SecretStore: Send + Sync {
    fn backend(&self) -> &'static str;
    fn get(&self, name: &SecretName) -> Result<Option<SecretValue>>;
    fn put(&self, name: &SecretName, value: SecretValue, meta: SecretMeta) -> Result<()>; // meta: kind, expires
    fn delete(&self, name: &SecretName) -> Result<bool>;
    fn list(&self) -> Result<Vec<SecretInfo>>; // metadata only, never values
}
```

The trait is synchronous, and async callers use `spawn_blocking`. Keychain has no native expiry, so expiry is stored as item metadata and enforced by m8s. A later `Signer` extension (sign only, no `get`) can keep keys in hardware.

## UX: entering secrets

```
$ m8s secrets set vast/api-key        # local terminal, no echo, confirm prompt
✔ stored · backend keychain · fingerprint 3f1a9c02 (sha256 prefix, not the value)
$ op read op://m8s/vast | m8s secrets set vast/api-key --stdin   # explicit opt-in
$ m8s secrets list                    # names, backend, created, expiry
$ m8s secrets rm vast/api-key
$ m8s secrets check                   # backend reachable, perms, no file in a worktree, no secrets in env, dev-binary signing
```

Rules:

- There is no `get` or `show` command.
- Values are never accepted as argv.
- A non-TTY stdin is refused without `--stdin`.
- An error for a missing secret tells the user to run `m8s secrets set <name>` in a local terminal.
- Secrets are never pasted into chat or Agor.

## Follow-up findings (Metal-0, 2026-10-21)

**1. Does mesh-llm encrypt its owner keystore at rest?** Only if a passphrase is set.
- `mesh-llm-identity/src/keystore.rs`: with a passphrase, the secret keys are sealed with **Argon2id (19 MiB, t=2) + ChaCha20-Poly1305**. Without one, the keystore is written **in plaintext**.
- `mesh-llm-identity/src/keychain.rs`: mesh-llm can already keep the keystore **unlock secret in the OS keychain**. It uses the `keyring` crate under service `mesh-llm`, account `owner-keystore`, through `save_keystore_with_keychain` and `load_owner_keypair_from_keychain`. `MESH_LLM_OWNER_PASSPHRASE` also exists.
- **Action:** the m8s setup docs and `m8s secrets check` should require the owner keystore to be **passphrase-encrypted with the unlock secret in Keychain**, and warn if it is plaintext.
- **Implementation note:** m8s's own Keychain backend should use the same `keyring` crate, for consistency and cross-platform support (Secret Service on Linux), rather than a macOS-only crate.

**2. Does Vast offer scoped or expiring keys?** Scoped yes, expiring no ([Vast permissions](https://docs.vast.ai/api-reference/permissions), [API keys](https://docs.vast.ai/guides/reference/api-keys)).
- Keys can be limited to permission categories: `instance_read/write`, `user_read/write`, `billing_read/write`, `machine_read/write`, `misc`, `team_read/write`. Create one with `vastai create api-key --permission_file perms.json` or in the console.
- Keys **do not expire**, but they can be revoked at any time.
- **Recommendation:** Admin may use a full-account key for the prototype, as decided. But a scoped key costs nothing extra, so prefer one with **`instance_read` + `instance_write`** (plus `misc` if the offer search needs it; confirm in P0). Use no `billing_*`, `user_*` or `team_*`.
- `m8s secrets check` warns on a full-access key. Rotate it manually each quarter.
- Inside containers, only Vast's own per-instance `CONTAINER_API_KEY` is ever present ([ADR-0007](agor://kb/document/01a15b4e-5bfc-72c9-a718-77a74f349922)).

## Consequences

- There is a small amount of macOS-specific code. Linux owners use the file fallback until a Secret Service backend exists.
- Dev builds need a stable code-signing identity, so that `cargo` rebuilds don't trigger Keychain access prompts that block headless agents. Don't click "Always Allow" for Terminal or `/usr/bin/security`.
- **ADR-0008:** the operator can stay in minikube for non-secret reconcile state, but the secret-holding parts run on the host.
- **Architecture v0:** the diagram should say "owner identity (mesh-llm keystore)", not Keychain.
- **Follow-ups:**
  - Check whether mesh-llm encrypts its owner keystore at rest; if not, log it as an upstream gap.
  - Check whether Vast offers scoped or expiring keys, and confirm auto top-up is off.



## Admin gate decision (2026-10-21)

Admin accepted the recommendation to keep the Keychain default unavailable until one canonical exclusive writer/lock domain is deployment-qualified, all m8s writers are verified cooperative, and synthetic OS Keychain roundtrip/concurrency tests pass with Admin present. This does not authorize CLI/default enablement or real credential access.