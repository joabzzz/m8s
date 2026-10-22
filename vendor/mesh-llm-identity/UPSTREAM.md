# Canonical MP-001 package import

Accepted Mesh-LLM commit: c2190fdd6bc4f9100ac5029621622341c3e8c8e6
Source checkout inspected: /private/tmp/mp014-primary-qualification.HCUsrP (provenance only, never a build path).
Package path: mesh/crates/mesh-llm-identity.
m8s baseline: b614e7ae2bf6443aa4adcff02f3ce6d6207db52c.
Design: agor://kb/document/01a160e3-192c-70b6-a80f-8b952aad7dad

## Inventory / authority
Eleven complete src/*.rs files, public certificate test, package README, and
external root LICENSE are byte-identical to the accepted Git objects.
This is the full identity package, NOT a verification-only extraction: existing
in-memory signing, envelope and key APIs are included but not wired by m8s.
host-io stays default-OFF; m8s never forwards it. Upstream README wording about
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
