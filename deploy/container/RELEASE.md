# Reproducible runtime release pipeline

`release.py` is offline and credential-free. It builds, pushes and signs nothing
and records no timestamps, so identical pins give byte-identical records.

## Layers and rebuild cost

| Layer | Inputs (hashed into `cache_key`) | Dockerfile | Rebuild cost |
|-------|----------------------------------|------------|--------------|
| native | `native-source.json`, `artifact-sources.json` (CUDA base digests, mesh commit), client patch, `Dockerfile.source`/`.flint`, `normalize_cuda_arch.py` | `Dockerfile.source`, `Dockerfile.flint` | hours, GPU toolchain |
| model | `*model-artifact.json` contracts (weights stay a readonly mount or pinned HF revision) | n/a | none in the image |
| agent | `m8s-source.tar` sha256 + revision, `Dockerfile.agent*`, `artifacts.py` | `Dockerfile.agent-update` / `Dockerfile.agent` | minutes, `FROM` an immutable runtime digest |

An agent-only change alters only the `agent` key (tested), so it is built with
`Dockerfile.agent-update` on top of the already-qualified native/model image
digest. `release.py audit --dockerfiles` enforces that those two files never
mention `just`, `mesh-source`, llama, GGUF or CUDA compiler steps, and that every
`FROM` is a digest or a build-arg.

## Procedure

```sh
CTX=$(mktemp -d)                       # isolated build context
python3 release.py source --rev <commit> --output "$CTX/m8s-source.tar"   # prints sha256
cp Dockerfile.agent-update artifacts.py "$CTX"/   # plus any other file the chosen Dockerfile COPYs
python3 release.py audit --context "$CTX" --dockerfiles .   # no .env/.git/keys/weights/secret-like content, also inside tars
SOURCE_DATE_EPOCH=$(git show -s --format=%ct <commit>) \
docker buildx build --platform linux/amd64 --provenance=false --no-cache \
  --build-arg M8S_SOURCE_SHA256=<sha> \
  --output type=oci,dest=build1.tar,rewrite-timestamp=true "$CTX"
# repeat for build2.tar; record each index digest, then:
python3 release.py manifest --source-tar "$CTX/m8s-source.tar" \
  --image agent=sha256:<digest> --mac-artifact m8s-macos-arm64=<path> --output rel1
python3 release.py verify rel1 --search "$CTX" --search <dir with mac binary>
python3 release.py compare rel1 rel2 [--documented images.]
```

`manifest` writes `release-manifest.json`, `provenance.json` (an unsigned
in-toto statement: subjects are image digests, Mac binaries and the source
archive; materials are the source revision, base digests and layer keys) and
`SHA256SUMS`. Publish those three plus the artifacts they list. `compare` exits 1
on any diff not covered by a `--documented` prefix; documented diffs are printed
so the reason can be recorded beside the release.

Secrets: contexts carry only pinned sources, Dockerfiles and public contracts.
No `--secret`, remote `ADD`, or secret-named `ARG`/`ENV` is permitted by the lint,
and the VAST/HF/GitHub credentials are never inputs.

## Known gaps (honest status)

- **No full image build was run for this change.** The runtime images need the
  private native source, CUDA bases and hours of GPU-toolchain compile; only
  script tests, the source-archive determinism check and Dockerfile/context
  linting were run. The digest-match acceptance for images is therefore
  *unproven*; the `compare` tool is how it should be recorded.
- Image digests are inputs to `manifest`; this tool does not build or inspect
  images. `rewrite-timestamp` needs a recent BuildKit; `apt-get install` in the
  `agent-build` stage is unpinned and `cargo fetch` needs the network
  (Cargo.lock-checksummed); the unpinned apt packages can change the compiled
  agent binary and are the likeliest source of documented diffs.
- Provenance is unsigned and operator-asserted; no signing or registry
  attestation. Mac artifact builds are not performed here; only their hashes are
  recorded.
