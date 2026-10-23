---
name: m8s-demo
description: Explain or run this repository's proven Vast mesh-llm demo, inspect saved evidence, and diagnose or clean up an owned demo attempt. Use for m8s demo requests and secure launch/inference workflows.
---

# m8s demo

Resolve paths from this repository root. Read `AGENTS.md` and
`deploy/container/demo-proof.json` first. The historical acceptance is complete:
instance 57330173 securely bootstrapped, served Qwen through the owner client,
and was destroyed with independent absence confirmation. Reuse this evidence;
do not rent merely to explain or verify the existing result.

## Choose the requested task

- **Explain/show proof:** read `README.md`, the demo receipt, and
  `deploy/container/final-account-evidence.json`. Label billing observations by
  their timestamp; they are not current account balances or settled invoices.
- **Run a new paid demo:** read [the runbook](references/run-demo.md), then the
  CLI's “One-attempt Vast demo” documentation in `crates/m8s-cli/README.md`.
  Existing image/native qualification need not be repeated for unchanged pins.
- **Investigate a failed attempt:** start with saved receipts, watchdog events,
  and `m8s prototype audit`. A consumed claim cannot be reset. Preserve its
  directory and liability. Use exact-resource recovery contracts in
  `crates/m8s-provider-vast/README.md`; never destroy by a broad account sweep.
- **Continue implementation:** read `docs/ROADMAP.md` and `docs/decisions/`.
  The opt-in prototype is proven; the normal daemon is still a development
  controller. Do not present its mock state as live readiness.

## Non-obvious operating constraints

The runtime image already has the native node, model and CPU dependencies baked
in. Do not rebuild native Rust or fetch weights on a rental. The Mac owner
client must include the certified-key fix in `deploy/container/native-source.json`.
The tested binaries are on the private `prototype-proof-2026-10-23` release.

The model filename is not its API identity. Use the exact `local-gguf/sha256-…`
identity recorded in the receipt. Only a verified certificate, discovered model,
and completion through the model-free owner client constitute acceptance.

Direct UDP and pinned SSH TCP mappings are required. A listening acknowledgment
only reports provisioning progress. Keep canonical iroh TLS and authenticated
heartbeats; no plaintext or relay-policy workaround for a denied/failed path.

Keep the selected credential file outside the repository, build contexts and
archives. Pass its path, never its value, and never source it as shell code.
A new run needs current user authorization and bounded available budget; old
approval IDs and the historical top-up are not renewable demo credits. Honor
existing session authorization without asking again when it covers the action.
