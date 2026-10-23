# m8s

Owner-controlled model workers using the actual **mesh-llm** node. The Rust
workspace contains lifecycle policy, durable accounting, provider adapters,
secure bootstrap/control, node supervision, a CLI, and a terminal dashboard.

## Proven prototype

On **2026-10-23**, the CLI deployed Vast instance **57330173**, delivered a
canonical node certificate over pinned direct iroh TLS, observed the model
through the owner client, obtained a real completion, destroyed the instance,
and independently confirmed its absence. The command exited successfully.

> A mesh network is a local network structure where devices are connected through
> multiple direct or indirect connections, allowing for decentralized and
> self-sustaining communication.

Evidence:

- [Complete demo receipt](deploy/container/demo-proof.json)
- [Independent inventory and cost observation](deploy/container/final-account-evidence.json)
- [Native model qualification](deploy/container/native-model-inference-evidence.json)
- [Published runtime provenance](deploy/container/listener-agent-evidence.json)

The tested model is **Qwen2.5-0.5B-Instruct Q4_K_M**, using the packaged CPU
runtime. Its API identity is the GGUF SHA-256 identity, not its filename.
This does not qualify 27B/70B models or GPU performance. Final billing remains
unsettled; the accounting reservations are retained. Provider inventories were
empty after the demo.

## Build and run

Validated with **Rust 1.98**, macOS ARM64 for the owner, and Linux AMD64 for the
node image. Use a C compiler for bundled SQLite and at most two compiler jobs.

```sh
cargo build --workspace --locked -j 2
cargo build --locked -j 2 -p m8s-cli --features prototype-vast
```

The normal daemon remains a mock/development controller. Live Vast execution is
an explicit, default-off prototype command:

```sh
target/debug/m8s prototype launch --plan /absolute/private/new-attempt.json
```

The plan selects the owner binary, immutable image, credential **file path**,
account, exact ownership label, fresh run directory, and bounded budget. It must
account for previous spending and retained liabilities. See the
[CLI contract](crates/m8s-cli/README.md#one-attempt-vast-demo-default-off) and
[container documentation](deploy/container/README.md). A consumed attempt is
never reset or silently rented again.

The published image already contains the native node, CPU runtime, model,
restricted provisioning service, and control agent. A rented host does not
compile Rust or fetch model weights separately. The owner's native client needs
the certified-identity fix at revision `7a101840af79c7509789b9e3152ca6c4605186e8`;
[the source integration record](deploy/container/native-source.json) identifies
its companion repository and the node image's source revision.

## How the live path works

1. Start a fresh certificate-enforcing owner client with no local model.
2. Read the selected account, complete inventories, and fresh offer pricing.
3. Persist budget reservation and create intent; arm independent exact-resource
   cleanup before the one-use provider create call.
4. Pin the SSH host key obtained from that exact instance's authenticated log
   request. Send nonce/node material through SSH stdin, never provider metadata.
5. Wait for the node's fixed listener acknowledgment, then authenticate direct
   iroh TLS, consume the nonce, deliver the certificate, and maintain heartbeats.
6. Discover the exact GGUF identity through the owner client and send one prompt.
7. Destroy the exact owned resource and independently confirm absence before
   returning the completion. Stop local processes and remove private run keys.

## Workspace

| Component | Responsibility |
| --- | --- |
| `m8s-core`, `m8s-config` | Pure policy, capacity planning, configuration |
| `m8sd` | SQLite intent/nonce/accounting journals and operation coordinators |
| `m8s-provider-vast` | Explicit bounded Vast transports, observation, cleanup |
| `m8s-provider-local` | Local development execution and recovery |
| `m8s-bootstrap` | Canonical identity bridge, direct TLS delivery and control |
| `m8s-agent` | Required-certificate native launch, supervision and dead-man switch |
| `m8s-cli`, `m8s-top` | Operator commands and terminal UI |
| `m8s-secrets` | Explicit secret stores; no ambient discovery or silent fallback |

Core has no provider dependency. Money uses micro-USD; capacities use bytes;
network rates use Mbps. Policy time is injected. Secret values are excluded from
Debug/Serialize and never placed in command arguments or logs. Only public node
certificates and node keys reach workers; owner signing keys stay with the owner.

## Checks

```sh
scripts/check --no-pty --no-acl-mutation
python3 -B -m unittest discover -s deploy/container/tests -v
```

The latest Rust run passed **732 tests**, formatting, and strict Clippy. The
selected profile excludes four synthetic PTY tests and three ACL-mutation tests;
it is not an unfiltered OS qualification. Existing operator endpoint evidence is
retained and was not rerun. `scripts/check` without flags retains the normal
selection. See `scripts/check --help` and the crate READMEs for exact boundaries.

## Development controller

```sh
target/debug/m8sd /private/tmp/m8s-example owner-example
# Another terminal:
target/debug/m8s --socket /private/tmp/m8s-example/control.sock up qwen27 --json
target/debug/m8s --socket /private/tmp/m8s-example/control.sock status --json
target/debug/m8s --socket /private/tmp/m8s-example/control.sock down --json
```

This path reports mock/development state. A mock Booting node is not a live Ready
node. Stopped is not destroyed, destroy acknowledgment is not absence, and an
uncertain provider response never authorizes deleting a resource row or releasing
its liability. The live prototype is not yet a production fleet scheduler.

## Architecture and next work

The [decision index](docs/decisions/README.md) records the implemented prototype
choices and their evidence. The [production roadmap](docs/ROADMAP.md) separates
completed acceptance from remaining integration and operational qualification.

## Agent demo skill

Ask an agent: **“Use $m8s-demo to explain the proof and help me run a fresh bounded demo.”**
The repo-local [skill](.agents/skills/m8s-demo/SKILL.md) includes the artifact pins,
[runbook](.agents/skills/m8s-demo/references/run-demo.md), accounting constraints,
and failure/cleanup workflow. If the agent does not discover repo-local skills,
ask it to read that file explicitly. A new rental needs a current budget; reading
the proof does not make provider calls.
