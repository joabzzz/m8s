# Full-node source image (not runtime-qualified)

`Dockerfile.agent` refreshes only the Rust m8s agent in an existing full-node
image. Supply the reviewed runtime image by immutable digest and a clean,
SHA256-pinned `m8s-source.tar`. It does not compile mesh-llm, llama.cpp or the UI,
and retains the runtime image's baked model and provisioning entrypoint.
The registry and target caches are separate from the native mesh build caches;
compilation runs with networking disabled and at most two jobs.

## Current checkpoint (2026-10-22)

The Linux AMD64 CPU image **has built successfully**, including the actual mesh
node and `m8s-agent --vast-node` entrypoint. Local tag:
`m8s-prototype-cpu-amd64:a6e5a23`; image index digest:
`sha256:776b2fdc8ae1f04b2aa127eab2abd0ed73bd30e95c2fce7edb822e3d3ced17ca`.
Native source is `f2cbf8fd8159a16401040a3536550d49a3da8ce2`; image m8s source is
`a6e5a235c1eb26475437234ff3c53805b16270ff` (later owner-side changes are not
included in that image). Build log: `/private/tmp/m8s-amd64-build-2.log`.
The final-container ELF, required-ownership-option, and model-contract checks
passed. No model request or secure bootstrap was run by those build checks.

The user authorized publishing publicly at `ghcr.io/joabzzz/m8s-prototype`.
After the operator completed GitHub package authorization and made the package
public, the push succeeded. Public tag: `cpu-amd64-a6e5a23`. An anonymous Docker
pull with an empty login configuration succeeded for the exact index digest above
(the local layers were cached; this was an access/manifest check, not a fresh
full-network download). Receipt: `publication-evidence.json`. Earlier registry
denials are historical; no permission settings were changed by the agent.

The subsequent `vast-qwen-cpu-63e3645` image also contains the pinned Qwen
0.5B GGUF and restricted SSH provisioning service. Its public digest and prior
anonymous-pull evidence are recorded in `vast-publication-evidence.json`.
The isolated SSH qualification used synthetic keys and no provider network.
This image still contains the original node-initiated bootstrap binary; the new
owner-initiated bootstrap source must be qualified and packaged before use.
That transport qualification has since passed as recorded below. A local
agent-only refresh is now built as `m8s-prototype-vast:8388ce4`; its source,
digest, 21-second warm-cache Rust build and unchanged base-layer evidence are recorded in
`agent-refresh-evidence.json`. It has not been published or deployed.
The Mac owner host-only build completed successfully; log:
`/private/tmp/m8s-owner-host-build-3.log`. That is compilation evidence only.

### Owner-initiated transport check: operator-reported PASS

The owner can now initiate bootstrap to the provider-observed node UDP address,
so the owner's Mac does not need an inbound port mapping. The node authenticates
the pinned owner before opening its redemption stream. The owner still checks
the authenticated node's direct IP against the provider binding, burns the nonce
and delivers on that original stream. Relay-only bootstrap remains refused.

The new ignored transport test covers the changed direction, wrong-owner
rejection before nonce disclosure, exact resource receipt, and nonce replay.
The operator ran it in ordinary Terminal and supplied the result: **1 passed,
0 failed, 1 filtered out, 0.10 seconds**. Evidence is recorded in
`owner-initiated-bootstrap-evidence.json`; this is operator-reported output,
not an independently captured log. Historical E PASS remains separate and was
not rerun. The command below records what was run; do not repeat this completed
qualification unchanged:

```sh
cd /private/tmp/m8s-root-integration.VSK0VT
env PATH="/private/tmp/mp001-rustup/toolchains/1.98.0-aarch64-apple-darwin/bin:$PATH" \
  CARGO_HOME=/private/tmp/mp017-primary-cargo RUSTC_WRAPPER= CARGO_NET_OFFLINE=true \
  cargo test --locked --offline -j 2 -p m8s-bootstrap \
  --features iroh-transport --test iroh_loopback \
  owner_initiated_bootstrap_preserves_identity_and_single_use \
  -- --ignored --exact
```

Required capabilities: local UDP socket binding and normal iroh/macOS network
monitor access (BSD AF_ROUTE). Synthetic keys only; no provider requests,
credentials, spend or security-setting changes. The test runtime is capped at
25 seconds after compilation. A permission failure must be reported, not bypassed.

No Vast instance has been rented by these steps. Secure owner-side LAUNCH,
independent cleanup process qualification, owner-routed model inference, and
provider-confirmed removal remain unfinished. No end-to-end success is claimed.

The cleanup worker's production session-detachment function has passed a local
parent-loss test: a synthetic launcher was placed in its own process group,
the worker detached and acknowledged, that exact launcher group was killed,
and the worker acknowledged survival after observing its parent change.
Log: `/private/tmp/m8s-watchdog-parent-loss.log`. No provider client or credentials
were used. This qualifies process survival only; it does not prove real provider
reconciliation, destruction, billing containment, or survival of host loss.
The subsequent repository check passed with the documented PTY/ACL exclusions:
`/private/tmp/m8s-watchdog-survival-check.log`.

The sections below include historical source-only and cutover checkpoints; their
statements that no image exists or the node entrypoint is unwired are superseded
by this checkpoint. Their security requirements still apply.

`Dockerfile.smoke` prepares a local CPU build from pinned Rust 1.98 and Node
24.20.0 base manifests. Its dependency-preparation steps use public package
registries and the native owner's pinned patch queue; product compilation then
runs with networking disabled and at most two compiler jobs. It takes the same
clean source archives, SHA256 arguments, checker, and model contract as the
offline source recipe. No credentials or model weights enter the build context.
This is local build preparation, with no provider deployment or Ready claim.

`Dockerfile.source` builds the **complete Linux native product**, not the
upstream `Dockerfile.client` (which explicitly cannot serve as a worker). It
calls the mesh owner's `just bundle` for CPU, or `just release-build-cuda` followed
by `just release-bundle` for CUDA, preserving embedded runtime compilation,
host dependency/ABI composition checks, console, manifest and licenses. It does
not introduce a separate llama-server or hand-assemble native libraries.

**Current state: source recipe only, not a built image or working inference demo.**
The required-certificate mesh source is committed at
`f2cbf8fd8159a16401040a3536550d49a3da8ce2`; the operator completed the native
`no-console-print` check. The earlier permission denial is historical, not a
current failing check. Public artifact pins are in `artifact-sources.json`.
The selected Qwen2.5-0.5B-Instruct Q4_K_M GGUF is 491,400,032 bytes, and its
downloaded bytes passed `check-model` against `model-artifact.json`. The CUDA
base manifests are pinned for linux/amd64 but have not been pulled or prepared.
The agent's secure executable route remains unwired, so the
image deliberately retains its existing fail-closed entrypoint. Do not change it
to `--local`, `--local-model-only` or unsigned raw `serve` to make a demo appear
functional. Existing BootGate, fresh DMS, actual-container-bound Store burn,
certificate verification and native node-key binding remain mandatory.

## Inputs and preparation contract

Use a known-permitted Linux builder for the selected architecture, with explicit
immutable `repository@sha256:…` builder/runtime bases. No default base is provided.
The builder must already contain the native owner's supported Rust toolchain,
just, sccache, C/C++/CMake/Ninja, Node/pnpm, Python3, all locked Cargo/UI caches and
the pinned patched native runtime source inputs. Runtime must contain Python3,
CA certificates and all native product dynamic dependencies. These recipes run
with networking disabled; missing tools/caches/dependencies **fail**, not fetch
or silently substitute. `NATIVE_BACKEND=cpu` is the default; select `cuda` only
with a prepared linux/amd64 CUDA builder and compatible runtime. The pinned
NVIDIA bases alone lack required build tools/caches and runtime Python3; they
are ingredients for prepared images, not valid ready-to-build substitutes.
No GPU compatibility or inference qualification is claimed.

Prepare an empty private build context from **accepted, clean, explicitly pinned
commits** of both repositories. Record their full commit IDs, archive SHA256s,
base digests and toolchain inventory in the operator receipt. Use ordinary
read-only `git status --porcelain --untracked-files=all`, require empty output,
require `git rev-parse HEAD` equal the accepted commit, then `git archive --format=tar
<exact-commit> > <fresh-context>/mesh-source.tar` (and `m8s-source.tar`). Never
include a checkout, `.git`, credentials, local caches, private keys or weights in
the context. The archive SHA256 build arguments pin the actual input bytes;
they do **not** establish review/acceptance on their own. The included extractor
rejects links, traversal, special files, excessive count/size and existing output.

Copy this directory's `artifacts.py` and `Dockerfile.source` into that context.
Copy the selected public `model-artifact.json` too. Its exact schema is:

```json
{"schema":"m8s.local-model/v1","model":"Qwen/Qwen2.5-0.5B-Instruct-GGUF","format":"gguf","bytes":491400032,"sha256":"74a4da8c9fdbcd15bd1f6d01d621410d31c6fc00986f5eb687824e7b93d7a9db"}
```

The selected model is Apache-2.0 licensed. Inputs must be GGUF version 2/3, between
24 bytes and **2 GiB**. Pin its actual exact length and SHA256; no mutable URL or
automatic model download. This bound targets a small CPU smoke model and does
not prove inference fit or full GGUF validity. Before any qualified start:

For launch and inference, use the exact `served_model_id` in
`artifact-sources.json`: `qwen2.5-0.5b-instruct-q4_k_m`. The model contract's
repository name identifies the artifact, not the HTTP model name. Native raw
GGUF serving derives its name from the filename; the container adapter mounts
the file at `/models/<admitted-model-id>.gguf` and requires a safe single filename.

```sh
python3 artifacts.py check-model /explicit/selected/model.gguf model-artifact.json
```

The checker streams a bounded regular single-link file, rejects symlinks,
unsafe write permissions and changed file identity/metadata. It does not replace
the agent's descriptor/ACL checks or the native loader. A writable ancestor or
privileged concurrent writer remains outside this helper's protection; selected
model must stay an immutable readonly mount throughout the run.

## Future admitted build command — NOT executed or authorized by this document

```sh
python3 artifacts.py check-base "$PINNED_BUILDER_DIGEST"
python3 artifacts.py check-base "$PINNED_RUNTIME_DIGEST"
docker build --network=none --pull=false -f Dockerfile.source \
  --build-arg BUILDER_BASE="$PINNED_BUILDER_DIGEST" \
  --build-arg RUNTIME_BASE="$PINNED_RUNTIME_DIGEST" \
  --build-arg MESH_SOURCE_SHA256="$RECORDED_MESH_ARCHIVE_SHA256" \
  --build-arg M8S_SOURCE_SHA256="$RECORDED_M8S_ARCHIVE_SHA256" \
  --build-arg NATIVE_BACKEND=cpu \
  -t m8s-prototype-source .
```

Validate base arguments BEFORE invoking Docker: Docker resolves FROM before RUN,
so its later in-build check cannot prevent resolution of an invalid base input.
The operator must explicitly have the selected images provisioned; this source
packet does not authorize registry pulls.

Both native build output and final runtime must successfully answer bounded
`--help-advanced` with the exact `--node-ownership-required` option. Unsupported
binaries refuse; no flag-removal fallback. This compatibility check does not
prove security enforcement, start an endpoint, mint admission or advertise Ready.
The runtime model check is explicit, **not yet wired into a secure entrypoint**;
the image's public metadata alone cannot authorize launch. The container adapter
continues to require its external qualified gate and exact-ID teardown. No paid
instance may wait for unfinished source integration or an unqualified image.

## Safe source checks

```sh
python3 -B -m unittest discover -s deploy/container/tests -v
```

Tests use only tiny synthetic files/archives and owned fake native programs.
They do not run Docker, a mesh binary, model inference, endpoints or providers.

### Historical source prerequisite checkpoint

The following records the handoff state. The committed source pin and completed
operator check above supersede its uncommitted-candidate restriction.

The inspected canonical tree is based on main `9d97fbfa58c428fb95263a0de719bed959f2510a`
through the reviewed prerequisites `6b4dd72fd`, `cc08962a3`, `816b333db`, and
`a8ed6bf781d3674ebf097f01a08b2849409cb72d`. Its proposed required-mode/pure-signer
16-file diff SHA256 is
`2a23caae12726a8f228035e4ca3ff047cdb46bedf4175f9beace398473f81a67`.
That diff is **not a commit/archive pin and must not be substituted for one**.
The consumer baseline is `4bb0a0c2822ca250bdc67f2741bdaf6dab779fd6`, with
uncommitted required-flag/one-shot-lifecycle and these recipe additions. Neither
baseline alone contains all prerequisites. Before build, both integrated inputs
need actual accepted commit IDs and clean committed archives; no dirty candidate
import or direct original/main edit is part of this recipe.

`check-native` also rejects non-64-bit/non-little-endian/non-Linux-ELF-format
binaries (supported machine types: amd64/arm64); this header check is not a
signature/provenance check or a full dependency/architecture compatibility proof.
The native owner remains responsible for composition and ABI verification.

## 5 PM demo cutover — actual executable surface

Optional scope is frozen; all uncommitted source above is preserved. There is
**no qualified end-to-end launch/request/stop demo yet**. In particular, no qualified
`m8s prototype launch` command exists. The later default-off `prototype stop`
source increment below provides owned-run cleanup, not launch. Do not substitute
raw unsigned Docker/native startup. The owned-container
library implements stopped create, actual-ID binding, gated start and one-shot
request/removal, but no secure executable coordinator currently calls it.

The implemented request-only command, for an **already running, independently
qualified** loopback inference service with an explicitly selected served model,
is exactly:

```sh
m8s prototype request --local-qualification \
  --endpoint http://127.0.0.1:9337 \
  --model "$ACTUAL_SERVED_MODEL_ID" \
  --prompt-file "$ABSOLUTE_PROMPT_FILE" \
  --max-tokens 16 --timeout-ms 30000 --json
```

This command is not a launch instruction and has **not** produced real inference
in this project. Prompt must be bounded UTF-8 (at most 8192 bytes). There is one
request and no retry; timeout leaves computation outcome unknown. A completion
alone does not prove mesh identity, owner routing or Ready. The selected service
must be stopped through its already-qualified exact-resource owner; an arbitrary
`docker rm`/name lookup is not a substitute for verified teardown.

Proven runtime is limited to the human-operated guarded **synthetic** composed
bootstrap test (TLS, Store burn, synthetic canonical signing and original-stream
delivery). Container lifecycle/required-flag checks use fake executables; source
artifact tests use synthetic files. No actual image build, model load, mesh
inference, paid instance or real-container teardown has been established.

Remaining necessary integration, in dependency order:

1. Completed: the operator ran the native check and committed the canonical
   source at `f2cbf8fd8159a16401040a3536550d49a3da8ce2`. Reuse existing E PASS;
   do not rerun completed endpoint qualification.
2. Integrate accepted source pins and a secure executable coordinator using the
   existing actual-container-ID-bound burn, original TLS delivery, concrete
   verifier, native key binding, BootGate/DMS and exact-ID cleanup. No new public
   approval boolean, fake capability, unsigned fallback or owner key on a node.
3. Provision explicitly approved compatible Linux bases/caches and one pinned
   model, then qualify the complete image and actual request/removal in a
   permitted context. The build recipe alone is not this qualification.
4. A paid run additionally needs established secure custody, verified pricing and
   liability/reserve, independently surviving destruction and billing/resource
   disposition within **USD6 cumulative total**. No paid launch is eligible now.

Agent-originated demo compute/provider spend to date: **USD0**; no container or
paid resource was created by these implementation steps. No account-wide balance,
external resource inventory or billing claim is made. Unknown Keychain item
state from the earlier failed experiment is unchanged, not probed or cleaned up.

### Later cutover source increment: real owned-run STOP CLI

The default-off `prototype-container` feature now wires the cleanup command above
through the owned-container library. See `crates/m8s-cli/README.md` for exact
`prototype stop` syntax. This is implemented/tested source, not a real Docker
execution result. New v2 run intents persist the original selected transport
identity before create; older v1 intents cannot be safely resumed by this path.
The secure LAUNCH executable coordinator is still not implemented.

### Agent process initialization

The images run `/usr/bin/tini -- /usr/local/bin/m8s-agent`; the agent must not be
PID 1. The online smoke recipe installs Debian Tini. Offline prepared runtime
bases must already contain `/usr/bin/tini`, and packaging refuses if it is absent.
Both source recipes enable `vast-node`, whose explicit private input contract is
documented in the agent README. This does not provide owner-side LAUNCH or Ready.
