# Canonical MP-001 package import

Accepted Mesh-LLM commit: f2cbf8fd8159a16401040a3536550d49a3da8ce2
Source checkout inspected: /Users/sandro/.agor/worktrees/local/mesh-mp001-reviewed/prototype-keyless-main-integration (provenance only, never a build path).
Package path: mesh/crates/mesh-llm-identity.
Original m8s import baseline: 14ff55348bf5b5559e6214f09c2d26a34736c58b.
Design: agor://kb/document/01a16108-e37d-75cf-8223-ea95551f59bc

## Inventory / authority
Fourteen complete src Rust files, public certificate test, package README, and
external root LICENSE are byte-identical to the accepted Git objects.
This is the full identity package, NOT a verification-only extraction: existing
in-memory signing, envelope and key APIs are included. The default-off m8s
owner-signing adapter consumes the pure exact-signing API without host-io.
host-io stays default-OFF; production m8s never forwards it. Upstream README wording about
host-io is retained verbatim; canonical public verification is pure.
Declared license remains MIT OR Apache-2.0; supplied LICENSE is the upstream
Apache-2.0 text. No additional license claim is made.
No lock, .git, cache, target, configuration or host artifacts were copied.

Original manifest SHA256: c5c3042e0f282a382dad92b0381de62ccf968fcb82bdd5187750ae257597172d
Normalized manifest SHA256: 583a96b998cb30d5661f9122dd11809a1bbe743c620050ef717fc7edec7c26fb

## Exact manifest normalization
Only workspace metadata/dependency inheritance is expanded; all other entries
are unchanged. sha2 is upstream 0.11, not m8s workspace 0.10.

```diff
--- accepted/Cargo.toml
+++ vendor/Cargo.toml
@@ -1,8 +1,8 @@
 [package]
 name = "mesh-llm-identity"
-edition.workspace = true
-license.workspace = true
-version.workspace = true
+edition = "2024"
+license = "MIT OR Apache-2.0"
+version = "0.78.0"
 description = "Shared owner identity, signing, and envelope crypto for Mesh LLM crates"
 repository = "https://github.com/Mesh-LLM/mesh-llm"
 homepage = "https://github.com/Mesh-LLM/mesh-llm"
@@ -23,9 +23,9 @@
 hex = "0.4"
 keyring = { version = "4.2", optional = true }
 rand = "0.10"
-serde.workspace = true
-serde_json.workspace = true
-sha2.workspace = true
+serde = { version = "1", features = ["derive"] }
+serde_json = "1"
+sha2 = "0.11"
 thiserror = "2"
 zeroize = { version = "1", features = ["derive"] }

```

SHA256SUMS covers all imported files including the normalized manifest (not
this provenance document or the checksum manifest itself). Verify from here:
`shasum -a 256 -c SHA256SUMS`.

## Update procedure
Require a separately accepted canonical commit and explicit primary import scope.
Compare every allowlisted file to that commit's Git object and freeze hashes.
Never rewrite/prune source/tests, copy an encoder, or follow a mutable checkout.
Review manifest-only inheritance expansion, dependency inventory and licenses.
Keep workspace exclusion and defaultfalse/no-host-io graph evidence.
Primary reviews isolated root lock patch/new-package inventory before changing
the primary-owned root lock. Re-run relocated focused checks and independent
review, then primary full scripts/check before commit/bundle handoff.

## Current exact-signing import

The primary integration owner imported the complete package directly from Git
objects at f2cbf8fd8159a16401040a3536550d49a3da8ce2 after verifying a clean
canonical worktree. No canonical source or test bytes were rewritten. The six
manifest expansions above remain byte-identical to the previous import; there
are no added dependencies or manifest/lock changes. SHA256SUMS was regenerated
for all 18 imported objects, including the normalized manifest and LICENSE.
The new pure exact signer and required-certificate validator are available
without enabling host-io. The operator reported native `just no-console-print`
PASS, then committed the previously reviewed 16-file candidate. Existing E PASS
is historical qualification evidence for its original source, not runtime proof
of this refreshed import or a reason to rerun that completed qualification.

## Historical composed qualification import (Stage A only)

Explicit ten-file primary grant imports the accepted exact-request module, not
an m8s source fork. All fourteen non-manifest package objects (README, twelve
source files, public test) and external root LICENSE match cecee Git blobs.
The manifest is unchanged from the preceding import: exactly six inheritance
expansions, sha2 0.11, unchanged features/dependencies/license declaration.
No upstream tests or legacy signing functions were edited.

Only the separately workspace-excluded qualification/bootstrap-mp001 package
unifies host-io to compile the canonical in-memory exact signer. Its own reviewed
lock is independently generated; root Cargo.lock remains unchanged. Production
bootstrap/default/all-feature graphs must keep host-io inactive. Compiling the
host package does not execute keystore, Keychain, CLI or legacy signer paths.
The harness uses fixed inline synthetic keys and explicit TEST policy values;
no production numerical policy or automated invoker authorization is implied.
Stage A never executes its single ignored endpoint body. Stage B requires a
separate explicit primary instruction after frozen source acceptance.

Accepted source evidence: agor://kb/document/01a160ff-0f43-7704-83cd-08d1fbd4184a
Independent source review: agor://kb/document/01a16101-1e2f-7271-91c3-91888e1d0c07
