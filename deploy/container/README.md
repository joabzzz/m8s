# Full-node source image (not runtime-qualified)

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
